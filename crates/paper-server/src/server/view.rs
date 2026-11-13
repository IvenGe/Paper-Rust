//! What each player sees: chunk streaming, other players, light updates, keep-alives.

use super::join::info_entry;
use super::player::{ChunkView, Location, Player, OFFHAND_SLOT};
use super::Server;
use crate::world::chunk::ChunkPos;
use crate::world::light::{ChangedSection, LightKind};
use crate::world::BlockChange;
use bytes::Bytes;
use paper_protocol::codec::{RemainingBytes, VarInt};
use paper_protocol::metadata::{flags, index, pose, Metadata};
use paper_protocol::packets::{encode_packet, play};
use paper_protocol::text::Component;
use paper_protocol::types::{Angle, LightData, LpVec3, Slot};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// A client that has not answered a keep-alive within this time is dropped.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// Ticks between forced absolute position updates for tracked players.
const POSITION_SYNC_INTERVAL: u32 = 400;

/// Fixed-point position units used by relative move packets.
fn fixed(v: f64) -> i64 {
    (v * 4096.0).round() as i64
}

fn angle_byte(degrees: f32) -> u8 {
    (degrees.rem_euclid(360.0) * 256.0 / 360.0) as i32 as u8
}

/// The metadata other players need to render `player`.
fn player_metadata(player: &Player) -> Vec<u8> {
    let mut bits = 0;
    if player.sneaking {
        bits |= flags::SNEAKING;
    }
    if player.sprinting {
        bits |= flags::SPRINTING;
    }
    Metadata::new()
        .byte(index::FLAGS, bits)
        .pose(index::POSE, if player.sneaking { pose::SNEAKING } else { pose::STANDING })
        .byte(index::SKIN_PARTS, player.settings.skin_parts as i8)
        .finish()
}

/// What `player` holds and wears.
fn player_equipment(player: &Player) -> Vec<(u8, Slot)> {
    use play::equipment_slot::*;
    vec![
        (MAIN_HAND, player.held_item().clone()),
        (OFF_HAND, player.inventory[OFFHAND_SLOT].clone()),
        (HELMET, player.inventory[5].clone()),
        (CHESTPLATE, player.inventory[6].clone()),
        (LEGGINGS, player.inventory[7].clone()),
        (BOOTS, player.inventory[8].clone()),
    ]
}

impl Server {
    // --- chunks ----------------------------------------------------------------------------

    /// Recomputes which chunks a player should have after it moved or its view distance
    /// changed: releases chunks that left the view, requests the ones that entered it.
    pub(super) fn refresh_chunk_view(&mut self, entity_id: i32) {
        let Some(player) = self.players.get_mut(&entity_id) else { return };
        let Some(world) = self.worlds.get_mut(player.world) else { return };
        let center = player.loc.chunk();
        let radius = player.view.radius;
        let recentered = center != player.view.center;
        if recentered {
            player.view.center = center;
            player.conn.send(&play::SetChunkCacheCenter { x: VarInt(center.x), z: VarInt(center.z) });
        }

        let wanted: HashSet<ChunkPos> = ChunkView::area(center, radius).collect();
        let leaving: Vec<ChunkPos> = player.view.tracked.iter().filter(|p| !wanted.contains(p)).copied().collect();
        for pos in &leaving {
            player.view.tracked.remove(pos);
            world.remove_ticket(*pos);
            if player.view.sent.remove(pos) {
                player.conn.send(&play::ForgetLevelChunk { x: pos.x, z: pos.z });
            }
        }
        let mut entered = false;
        for pos in &wanted {
            if player.view.tracked.insert(*pos) {
                world.add_ticket(*pos);
                entered = true;
            }
        }
        if entered || recentered || !leaving.is_empty() {
            let view = &mut player.view;
            view.pending = view.tracked.iter().filter(|p| !view.sent.contains(p)).copied().collect();
            // Farthest first, so the nearest chunk is popped off the end.
            view.pending.sort_unstable_by_key(|p| {
                let (dx, dz) = ((p.x - center.x) as i64, (p.z - center.z) as i64);
                std::cmp::Reverse(dx * dx + dz * dz)
            });
        }
    }

    /// Sends each player the next batch of chunks, honouring the client's flow control.
    pub(super) fn tick_chunk_views(&mut self) {
        let Server { players, worlds, workers, player_order, .. } = self;
        for id in player_order.iter() {
            let Some(player) = players.get_mut(id) else { continue };
            let Some(world) = worlds.get_mut(player.world) else { continue };
            let view = &mut player.view;
            if view.unacknowledged_batches >= view.max_unacknowledged_batches {
                continue;
            }
            view.batch_quota = (view.batch_quota + view.desired_per_tick).min(view.desired_per_tick.max(1.0));
            if view.batch_quota < 1.0 || view.pending.is_empty() {
                continue;
            }
            let max = view.batch_quota.floor() as usize;
            let mut batch = Vec::new();
            let mut i = view.pending.len();
            while i > 0 && batch.len() < max {
                i -= 1;
                let pos = view.pending[i];
                if let Some(frame) = world.chunk_packet(pos, workers) {
                    view.pending.remove(i);
                    batch.push((pos, frame));
                }
            }
            if batch.is_empty() {
                continue;
            }
            view.unacknowledged_batches += 1;
            view.batch_quota -= batch.len() as f32;
            player.conn.send(&play::ChunkBatchStart {});
            let count = batch.len() as i32;
            for (pos, frame) in batch {
                view.sent.insert(pos);
                player.conn.send_framed(frame);
            }
            player.conn.send(&play::ChunkBatchFinished { batch_size: VarInt(count) });
        }
    }

    /// The client reports how fast it processed the last chunk batch.
    pub(super) fn handle_chunk_batch_received(&mut self, entity_id: i32, chunks_per_tick: f32) {
        let Some(player) = self.players.get_mut(&entity_id) else { return };
        let view = &mut player.view;
        view.unacknowledged_batches = view.unacknowledged_batches.saturating_sub(1);
        view.desired_per_tick = if chunks_per_tick.is_nan() { 0.01 } else { chunks_per_tick.clamp(0.01, 64.0) };
        if view.unacknowledged_batches == 0 {
            view.batch_quota = 1.0;
        }
        view.max_unacknowledged_batches = 10;
    }

    /// Changes a player's effective view distance.
    pub(super) fn update_view_distance(&mut self, entity_id: i32) {
        let server_view = self.config.properties.view_distance;
        let Some(player) = self.players.get_mut(&entity_id) else { return };
        let radius = player.view_distance(server_view);
        if radius != player.view.radius {
            player.view.radius = radius;
            player.conn.send(&play::SetChunkCacheRadius { view_distance: VarInt(radius) });
            self.refresh_chunk_view(entity_id);
        }
    }

    // --- block changes ---------------------------------------------------------------------

    /// Tells the affected clients about a block change and the light changes it caused.
    pub fn publish_block_change(&mut self, world: usize, change: &BlockChange) {
        let chunk = ChunkPos::of_block(change.pos.x, change.pos.z);
        self.broadcast_to_chunk_viewers(world, chunk, &play::BlockUpdate { pos: change.pos, state: VarInt(change.new as i32) });
        self.publish_light_changes(world, &change.light);
    }

    fn publish_light_changes(&mut self, world: usize, changed: &HashSet<ChangedSection>) {
        if changed.is_empty() {
            return;
        }
        let mut by_chunk: HashMap<ChunkPos, Vec<(usize, LightKind)>> = HashMap::new();
        for (pos, section, kind) in changed {
            by_chunk.entry(*pos).or_default().push((*section, *kind));
        }
        let Some(w) = self.worlds.get(world) else { return };
        for (pos, mut sections) in by_chunk {
            let Some(chunk) = w.chunks.get(&pos) else { continue };
            sections.sort_unstable_by_key(|(section, _)| *section);
            let mut data = LightData::default();
            let set = |mask: &mut Vec<u64>, i: usize| {
                if mask.len() <= i / 64 {
                    mask.resize(i / 64 + 1, 0);
                }
                mask[i / 64] |= 1 << (i % 64);
            };
            for (section, kind) in sections {
                let (source, mask, empty_mask, arrays) = match kind {
                    LightKind::Sky => (&chunk.sky_light, &mut data.sky_mask, &mut data.empty_sky_mask, &mut data.sky_arrays),
                    LightKind::Block => (&chunk.block_light, &mut data.block_mask, &mut data.empty_block_mask, &mut data.block_arrays),
                };
                let Some(light) = source.get(section) else { continue };
                if matches!(**light, crate::world::chunk::LightSection::Uniform(0)) {
                    set(empty_mask, section);
                } else {
                    set(mask, section);
                    arrays.push(light.to_bytes());
                }
            }
            self.broadcast_to_chunk_viewers(world, pos, &play::LightUpdate { x: VarInt(pos.x), z: VarInt(pos.z), light: data });
        }
    }

    // --- other players ---------------------------------------------------------------------

    /// Updates which players see each other and broadcasts movement, metadata and equipment.
    pub(super) fn tick_tracker(&mut self) {
        let ids = self.player_order.clone();

        // Visibility.
        for &viewer_id in &ids {
            let Some(viewer) = self.players.get(&viewer_id) else { continue };
            let (world, center, radius) = (viewer.world, viewer.view.center, viewer.view.radius);
            let now_visible: HashSet<i32> = ids
                .iter()
                .copied()
                .filter(|id| *id != viewer_id)
                .filter(|id| self.players.get(id).is_some_and(|t| t.world == world && ChunkView::contains(center, radius, t.loc.chunk())))
                .collect();
            let appeared: Vec<i32> = now_visible.difference(&viewer.visible).copied().collect();
            let vanished: Vec<i32> = viewer.visible.difference(&now_visible).copied().collect();
            if appeared.is_empty() && vanished.is_empty() {
                continue;
            }
            for target_id in appeared {
                let Some(target) = self.players.get(&target_id) else { continue };
                let viewer = &self.players[&viewer_id];
                let loc = target.tracked_loc;
                viewer.conn.send(&play::AddEntity {
                    entity_id: VarInt(target_id),
                    uuid: target.uuid(),
                    entity_type: VarInt(player_entity_type()),
                    x: loc.x,
                    y: loc.y,
                    z: loc.z,
                    velocity: LpVec3(0.0, 0.0, 0.0),
                    pitch: Angle(loc.pitch),
                    yaw: Angle(loc.yaw),
                    head_yaw: Angle(loc.yaw),
                    data: VarInt(0),
                });
                viewer.conn.send(&play::SetEntityData { entity_id: VarInt(target_id), metadata: RemainingBytes(player_metadata(target)) });
                let equipment = player_equipment(target);
                if equipment.iter().any(|(_, item)| !item.is_empty()) {
                    viewer.conn.send(&play::SetEquipment { entity_id: target_id, equipment });
                }
                self.players.get_mut(&viewer_id).unwrap().visible.insert(target_id);
                self.players.get_mut(&target_id).unwrap().seen_by.insert(viewer_id);
            }
            if !vanished.is_empty() {
                let viewer = self.players.get_mut(&viewer_id).unwrap();
                viewer.conn.send(&play::RemoveEntities { entity_ids: vanished.iter().map(|id| VarInt(*id)).collect() });
                for id in &vanished {
                    viewer.visible.remove(id);
                }
                for id in vanished {
                    if let Some(target) = self.players.get_mut(&id) {
                        target.seen_by.remove(&viewer_id);
                    }
                }
            }
        }

        // Movement, metadata and equipment of each player to those who see it.
        for &id in &ids {
            let Some(player) = self.players.get_mut(&id) else { continue };
            let mut payloads: Vec<Bytes> = Vec::new();
            let (cur, prev) = (player.loc, player.tracked_loc);
            let (dx, dy, dz) = (fixed(cur.x) - fixed(prev.x), fixed(cur.y) - fixed(prev.y), fixed(cur.z) - fixed(prev.z));
            let moved = dx != 0 || dy != 0 || dz != 0;
            let rotated = angle_byte(cur.yaw) != angle_byte(prev.yaw) || angle_byte(cur.pitch) != angle_byte(prev.pitch);
            let fits = |d: i64| (i16::MIN as i64..=i16::MAX as i64).contains(&d);
            player.ticks_since_position_sync += 1;
            // An absolute update when the delta does not fit a packet, when the on-ground
            // state flips, after a teleport, and periodically to correct any drift.
            let sync = player.force_position_sync
                || player.ticks_since_position_sync > POSITION_SYNC_INTERVAL
                || player.on_ground != player.tracked_on_ground
                || !(fits(dx) && fits(dy) && fits(dz));
            if sync {
                payloads.push(encode_packet(&play::EntityPositionSync { entity_id: id, x: cur.x, y: cur.y, z: cur.z, yaw: cur.yaw, pitch: cur.pitch, on_ground: player.on_ground }).into());
                player.ticks_since_position_sync = 0;
                player.force_position_sync = false;
            } else if moved && rotated {
                payloads.push(encode_packet(&play::MoveEntityPosRot { entity_id: id, dx: dx as i16, dy: dy as i16, dz: dz as i16, yaw: cur.yaw, pitch: cur.pitch, on_ground: player.on_ground }).into());
            } else if moved {
                payloads.push(encode_packet(&play::MoveEntityPos { entity_id: id, dx: dx as i16, dy: dy as i16, dz: dz as i16, on_ground: player.on_ground }).into());
            } else if rotated {
                payloads.push(encode_packet(&play::MoveEntityRot { entity_id: VarInt(id), on_ground: player.on_ground, yaw: Angle(cur.yaw), pitch: Angle(cur.pitch) }).into());
            }
            if rotated || sync {
                payloads.push(encode_packet(&play::RotateHead { entity_id: VarInt(id), head_yaw: Angle(cur.yaw) }).into());
            }
            player.tracked_loc = cur;
            player.tracked_on_ground = player.on_ground;

            if player.metadata_dirty {
                player.metadata_dirty = false;
                payloads.push(encode_packet(&play::SetEntityData { entity_id: VarInt(id), metadata: RemainingBytes(player_metadata(player)) }).into());
            }
            if player.equipment_dirty {
                player.equipment_dirty = false;
                payloads.push(encode_packet(&play::SetEquipment { entity_id: id, equipment: player_equipment(player) }).into());
            }
            if payloads.is_empty() || player.seen_by.is_empty() {
                continue;
            }
            let viewers: Vec<i32> = player.seen_by.iter().copied().collect();
            for viewer in viewers {
                if let Some(v) = self.players.get(&viewer) {
                    for payload in &payloads {
                        v.conn.send_payload(payload.clone());
                    }
                }
            }
        }
    }

    /// Sends a packet to everyone who currently sees `entity_id`.
    pub fn broadcast_to_viewers<P: paper_protocol::packets::Packet + paper_protocol::codec::Encode>(&self, entity_id: i32, packet: &P) {
        let Some(player) = self.players.get(&entity_id) else { return };
        let payload = Bytes::from(encode_packet(packet));
        for viewer in &player.seen_by {
            if let Some(v) = self.players.get(viewer) {
                v.conn.send_payload(payload.clone());
            }
        }
    }

    // --- teleports -------------------------------------------------------------------------

    /// Moves a player and tells its client; the client must confirm before its own
    /// movement packets are accepted again.
    pub fn teleport(&mut self, entity_id: i32, target: Location) {
        let Some(player) = self.players.get_mut(&entity_id) else { return };
        let id = player.next_teleport_id;
        player.next_teleport_id = player.next_teleport_id.wrapping_add(1).max(1);
        player.pending_teleport = Some((id, target));
        player.loc = target;
        player.force_position_sync = true;
        player.conn.send(&play::PlayerPosition {
            teleport_id: VarInt(id),
            x: target.x,
            y: target.y,
            z: target.z,
            velocity_x: 0.0,
            velocity_y: 0.0,
            velocity_z: 0.0,
            yaw: target.yaw,
            pitch: target.pitch,
            flags: 0,
        });
        self.refresh_chunk_view(entity_id);
    }

    // --- liveness --------------------------------------------------------------------------

    pub(super) fn tick_keep_alive(&mut self) {
        let now = Instant::now();
        let mut timed_out = Vec::new();
        for player in self.players.values_mut() {
            let keep_alive = &mut player.keep_alive;
            if now.duration_since(keep_alive.last_sent) < KEEP_ALIVE_INTERVAL {
                continue;
            }
            if keep_alive.pending.is_some() {
                timed_out.push(player.entity_id);
                continue;
            }
            let id = self.start_time.elapsed().as_millis() as i64;
            keep_alive.pending = Some(id);
            keep_alive.last_sent = now;
            player.conn.send(&play::KeepAlive { id });
        }
        for id in timed_out {
            self.kick(id, &Component::translatable("disconnect.timeout", vec![]));
        }
    }

    pub(super) fn handle_keep_alive(&mut self, entity_id: i32, id: i64) {
        let Some(player) = self.players.get_mut(&entity_id) else { return };
        if player.keep_alive.pending == Some(id) {
            let sample = player.keep_alive.last_sent.elapsed().as_millis() as i32;
            player.keep_alive.latency = (player.keep_alive.latency * 3 + sample) / 4;
            player.keep_alive.pending = None;
        }
    }

    /// Refreshes the ping column of everyone's player list.
    pub(super) fn broadcast_latency(&mut self) {
        if self.players.is_empty() {
            return;
        }
        let entries = self.online_players().map(info_entry).collect();
        self.broadcast(&play::PlayerInfoUpdate { actions: play::player_info::UPDATE_LATENCY, entries });
    }
}

/// Protocol id of the player entity type.
fn player_entity_type() -> i32 {
    static ID: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| paper_data::registries::id("minecraft:entity_type", "minecraft:player").expect("player entity type") as i32)
}
