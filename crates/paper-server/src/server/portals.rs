//! Travelling between dimensions: Nether portals (lit with flint and steel, found or built
//! on the other side like vanilla's `PortalForcer`), End portals (frames filled with eyes of
//! ender, the obsidian platform at the End spawn, leaving the End through the exit portal
//! with the credits), and moving a player from one world to another. Also buckets, which
//! are how players move water and lava around.

use super::player::{ChunkView, Location, HOTBAR_START, OFFHAND_SLOT};
use super::survival::{self, category, Fluid};
use super::Server;
use crate::config::GameMode;
use crate::world::block_updates::fire;
use crate::world::chunk::ChunkPos;
use crate::world::entity::{Aabb, Vec3};
use crate::world::portal::{self, Axis, PortalShape};
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

/// How a player is travelling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TravelKind {
    /// Through a Nether portal of this axis.
    Nether(Axis),
    /// Into the End.
    End,
    /// Out of the End, back to the respawn point (the credits roll).
    EndExit,
}

/// A trip waiting for the destination chunks to load.
#[derive(Clone, Debug)]
pub struct Travel {
    pub kind: TravelKind,
    pub to: usize,
    /// Where the player should come out, before searching for a portal.
    pub approx: BlockPos,
    pub chunks: Vec<ChunkPos>,
    pub started: u64,
}

/// World border of vanilla's default size.
const BORDER: i32 = 29_999_984;

impl Server {
    // --- standing in portals -----------------------------------------------------------------

    /// Portal blocks around a player: the Nether portal wait, End portals at once.
    pub(super) fn tick_player_portal(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else { return };
        if p.dead || p.travel.is_some() || p.won_game {
            return;
        }
        let world = p.world;
        let area = Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8).inflate(-1.0e-5, -1.0e-5, -1.0e-5);
        let w = &self.worlds[world];
        let mut nether: Option<Axis> = None;
        let mut end = false;
        let mut gateway: Option<BlockPos> = None;
        for y in area.min.y.floor() as i32..=area.max.y.floor() as i32 {
            for z in area.min.z.floor() as i32..=area.max.z.floor() as i32 {
                for x in area.min.x.floor() as i32..=area.max.x.floor() as i32 {
                    let Some(state) = w.block_at(x, y, z) else { continue };
                    match blocks::block_of_state(state).name {
                        "minecraft:nether_portal" => nether = Some(Axis::of_state(state)),
                        // The End portal is a slab-height block.
                        "minecraft:end_portal" if area.min.y < y as f64 + 0.75 => end = true,
                        "minecraft:end_gateway" => gateway = Some(BlockPos::new(x, y, z)),
                        _ => {}
                    }
                }
            }
        }
        let creative = p.game_mode == GameMode::Creative;
        let rules = &w.rules;
        let nether_allowed = rules.bool("allow_entering_nether_using_portals");
        let delay = if creative { rules.int("players_nether_portal_creative_delay") } else { rules.int("players_nether_portal_default_delay") }.max(1);
        let p = self.players.get_mut(&id).unwrap();
        // After arriving the player has to step out of the portal first.
        if p.portal_cooldown > 0 {
            if nether.is_some() || end || gateway.is_some() {
                p.portal_cooldown = 10;
            } else {
                p.portal_cooldown -= 1;
            }
            return;
        }
        if end {
            self.start_travel(id, TravelKind::End);
            return;
        }
        if let Some(pos) = gateway {
            self.use_end_gateway(id, world, pos);
            return;
        }
        match nether {
            Some(axis) if nether_allowed => {
                p.portal_time += 1;
                if p.portal_time >= delay {
                    p.portal_time = 0;
                    self.start_travel(id, TravelKind::Nether(axis));
                }
            }
            _ => p.portal_time = (p.portal_time - 4).max(0),
        }
    }

    /// `TheEndGatewayBlockEntity.teleportEntity`: a gateway that knows its exit (the return
    /// gateways on the outer islands lead to the main island) sends the player there,
    /// exactly or onto the tallest block beside it, then shows its beam and rests for two
    /// seconds. Gateways without an exit (the dragon fight's) do nothing yet.
    fn use_end_gateway(&mut self, id: i32, world: usize, pos: BlockPos) {
        if self.gateway_cooldowns.get(&(world, pos)).is_some_and(|used| self.tick < used + 40) {
            return;
        }
        let w = &self.worlds[world];
        let Some(chunk) = w.chunks.get(&ChunkPos::of_block(pos.x, pos.z)) else { return };
        let Some(entity) = chunk.block_entities.get(&(pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8)) else { return };
        let exit = match entity.get("exit_portal") {
            Some(paper_nbt::Tag::IntArray(v)) if v.len() == 3 => BlockPos::new(v[0], v[1], v[2]),
            Some(paper_nbt::Tag::Compound(c)) => match (c.i32("X"), c.i32("Y"), c.i32("Z")) {
                (Some(x), Some(y), Some(z)) => BlockPos::new(x, y, z),
                _ => return,
            },
            _ => return,
        };
        let target = if entity.bool("ExactTeleport").unwrap_or(false) {
            Vec3::new(exit.x as f64 + 0.5, exit.y as f64, exit.z as f64 + 0.5)
        } else {
            // `findExitPosition`: on top of the tallest full block within five of the exit.
            let top = exit.offset(0, 2, 0);
            let mut tallest: Option<BlockPos> = None;
            for dx in -5..=5 {
                for dz in -5..=5 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    let floor = tallest.map_or(w.min_y, |t| t.y);
                    let mut y = top.y;
                    while y > floor {
                        let state = w.block_at(top.x + dx, y, top.z + dz).unwrap_or(blocks::AIR);
                        if blocks::has_collision(state) && blocks::block_of_state(state).name != "minecraft:bedrock" {
                            tallest = Some(BlockPos::new(top.x + dx, y, top.z + dz));
                            break;
                        }
                        y -= 1;
                    }
                }
            }
            let at = tallest.unwrap_or(top);
            Vec3::new(at.x as f64 + 0.5, (at.y + 1) as f64, at.z as f64 + 0.5)
        };
        self.gateway_cooldowns.insert((world, pos), self.tick);
        self.gateway_cooldowns.retain(|_, used| self.tick < *used + 40);
        if let Some(block) = blocks::by_name("minecraft:end_gateway") {
            let block = blocks::block_id_of_state(block.default_state) as i32;
            self.send_near(world, pos, 64.0, None, &play::BlockEvent { pos, action: 1, param: 0, block: VarInt(block) });
        }
        let Some(p) = self.players.get_mut(&id) else { return };
        p.portal_cooldown = 10;
        let (yaw, pitch) = (p.loc.yaw, p.loc.pitch);
        self.teleport(id, Location { x: target.x, y: target.y, z: target.z, yaw, pitch });
    }

    /// Picks the destination and starts loading its chunks.
    fn start_travel(&mut self, id: i32, kind: TravelKind) {
        let Some(p) = self.players.get(&id) else { return };
        let from = p.world;
        let from_dim = self.worlds[from].dimension.clone();
        let overworld = self.world_of_dimension("minecraft:overworld").unwrap_or(0);
        let (to, approx, kind) = match kind {
            TravelKind::Nether(axis) => {
                let to = match from_dim.as_str() {
                    "minecraft:overworld" => self.world_of_dimension("minecraft:the_nether"),
                    "minecraft:the_nether" => Some(overworld),
                    _ => None,
                };
                let Some(to) = to else { return };
                let scale = |w: usize| paper_data::synced::dimension_type(&self.worlds[w].dimension).map_or(1.0, |d| d.coordinate_scale);
                let factor = scale(from) / scale(to);
                let dest = &self.worlds[to];
                let top = dest.min_y + paper_data::synced::dimension_type(&dest.dimension).map_or(dest.height, |d| d.logical_height) - 1;
                let x = ((p.loc.x * factor).floor() as i32).clamp(-BORDER, BORDER);
                let z = ((p.loc.z * factor).floor() as i32).clamp(-BORDER, BORDER);
                let y = (p.loc.y.floor() as i32).clamp(dest.min_y + 1, top - 1);
                (to, BlockPos::new(x, y, z), TravelKind::Nether(axis))
            }
            TravelKind::End | TravelKind::EndExit => {
                if from_dim == "minecraft:the_end" {
                    let (w, pos) = p.respawn.filter(|(w, _)| *w < self.worlds.len()).unwrap_or((overworld, self.worlds[overworld].spawn));
                    (w, pos, TravelKind::EndExit)
                } else {
                    let Some(end) = self.world_of_dimension("minecraft:the_end") else { return };
                    (end, BlockPos::new(100, 49, 0), TravelKind::End)
                }
            }
        };
        if kind == TravelKind::EndExit {
            self.leave_the_end(id);
            return;
        }
        let radius = if matches!(kind, TravelKind::Nether(_)) { 2 } else { 1 };
        let center = ChunkPos::of_block(approx.x, approx.z);
        let mut chunks = Vec::new();
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let c = ChunkPos::new(center.x + dx, center.z + dz);
                self.worlds[to].add_ticket(c);
                chunks.push(c);
            }
        }
        let started = self.tick;
        if let Some(p) = self.players.get_mut(&id) {
            p.travel = Some(Travel { kind, to, approx, chunks, started });
        }
    }

    /// Finishes trips whose destination is loaded.
    pub(super) fn tick_travels(&mut self) {
        let travelling: Vec<i32> = self.players.values().filter(|p| p.travel.is_some()).map(|p| p.entity_id).collect();
        for id in travelling {
            let Some(travel) = self.players.get(&id).and_then(|p| p.travel.clone()) else { continue };
            let ready = travel.chunks.iter().all(|c| self.worlds[travel.to].is_ready(*c));
            let expired = self.tick.saturating_sub(travel.started) > 600;
            if !ready && !expired {
                continue;
            }
            if let Some(p) = self.players.get_mut(&id) {
                p.travel = None;
            }
            if ready {
                if let Some(target) = self.arrival(id, &travel) {
                    self.change_dimension(id, travel.to, target);
                    if let Some(p) = self.players.get_mut(&id) {
                        p.portal_cooldown = 10;
                    }
                }
            } else {
                log::warn!("Gave up waiting for the destination of a portal ({},{} in {})", travel.approx.x, travel.approx.z, self.worlds[travel.to].name);
            }
            for c in travel.chunks {
                self.worlds[travel.to].remove_ticket(c);
            }
        }
    }

    /// Where a trip ends: an existing portal, a new one, or the End platform.
    fn arrival(&mut self, id: i32, travel: &Travel) -> Option<Location> {
        let yaw = self.players.get(&id).map_or(0.0, |p| p.loc.yaw);
        match travel.kind {
            TravelKind::End => {
                self.make_end_platform(travel.to, BlockPos::new(100, 49, 0));
                Some(Location { x: 100.5, y: 49.0, z: 0.5, yaw: 90.0, pitch: 0.0 })
            }
            TravelKind::Nether(axis) => {
                let radius = if self.worlds[travel.to].dimension == "minecraft:the_nether" { 16 } else { 128 };
                let shape = match self.find_nether_portal(travel.to, travel.approx, radius) {
                    Some(found) => PortalShape::find_any(&self.worlds[travel.to], found, Axis::of_state(self.worlds[travel.to].block_at(found.x, found.y, found.z).unwrap_or(0))),
                    None => self.create_nether_portal(travel.to, travel.approx, axis)?,
                };
                let (x, y, z) = shape.arrival();
                Some(Location { x, y, z, yaw, pitch: 0.0 })
            }
            TravelKind::EndExit => None,
        }
    }

    /// The nearest portal block within `radius` blocks of `near` in loaded chunks (vanilla
    /// asks its point-of-interest index; here the loaded sections are searched).
    fn find_nether_portal(&self, world: usize, near: BlockPos, radius: i32) -> Option<BlockPos> {
        let w = &self.worlds[world];
        let portal_states: Vec<u16> = blocks::by_name("minecraft:nether_portal").map(|b| (b.min_state..b.min_state + b.state_count).map(|s| s as u16).collect()).unwrap_or_default();
        let top = w.min_y + paper_data::synced::dimension_type(&w.dimension).map_or(w.height, |d| d.logical_height);
        let (c0, c1) = (ChunkPos::of_block(near.x - radius, near.z - radius), ChunkPos::of_block(near.x + radius, near.z + radius));
        let mut best: Option<(i64, i32, BlockPos)> = None;
        for cz in c0.z..=c1.z {
            for cx in c0.x..=c1.x {
                let Some(chunk) = w.chunks.get(&ChunkPos::new(cx, cz)) else { continue };
                for (si, section) in chunk.sections.iter().enumerate() {
                    let base_y = w.min_y + si as i32 * 16;
                    if base_y >= top || !section.contains_any(&portal_states) {
                        continue;
                    }
                    for i in 0..4096usize {
                        let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
                        if !portal_states.contains(&(section.get(x, y, z) as u16)) {
                            continue;
                        }
                        let pos = BlockPos::new(cx * 16 + x as i32, base_y + y as i32, cz * 16 + z as i32);
                        if (pos.x - near.x).abs() > radius || (pos.z - near.z).abs() > radius {
                            continue;
                        }
                        let (dx, dy, dz) = ((pos.x - near.x) as i64, (pos.y - near.y) as i64, (pos.z - near.z) as i64);
                        let d = dx * dx + dy * dy + dz * dz;
                        if best.is_none_or(|(bd, by, _)| d < bd || d == bd && pos.y < by) {
                            best = Some((d, pos.y, pos));
                        }
                    }
                }
            }
        }
        best.map(|(_, _, p)| p)
    }

    /// Builds a 4x5 obsidian frame with a lit portal near `near` (`PortalForcer.createPortal`).
    fn create_nether_portal(&mut self, world: usize, near: BlockPos, axis: Axis) -> Option<PortalShape> {
        let w = &self.worlds[world];
        let dir = match axis {
            Axis::X => (1, 0),
            Axis::Z => (0, 1),
        };
        let side = (-dir.1, dir.0);
        let top = (w.min_y + paper_data::synced::dimension_type(&w.dimension).map_or(w.height, |d| d.logical_height) - 1).min(w.max_y() - 1);
        let state = |p: BlockPos| w.block_at(p.x, p.y, p.z).unwrap_or(blocks::AIR);
        let replaceable = |p: BlockPos| {
            let s = state(p);
            blocks::is_replaceable(s) && survival::fluid_of(s).is_none()
        };
        let can_host = |origin: BlockPos, offset: i32| {
            for i in -1..3 {
                for j in -1..4 {
                    let p = origin.offset(dir.0 * i + side.0 * offset, j, dir.1 * i + side.1 * offset);
                    if j < 0 && !paper_data::physics::is_solid(state(p)) {
                        return false;
                    }
                    if j >= 0 && !replaceable(p) {
                        return false;
                    }
                }
            }
            true
        };
        // Search a 33x33 area for a place with room, from the surface down.
        let (mut best, mut best_d) = (None, f64::MAX);
        let (mut fallback, mut fallback_d) = (None, f64::MAX);
        for dz in -16..=16 {
            for dx in -16..=16 {
                let (x, z) = (near.x + dx, near.z + dz);
                let Some(column_top) = w.top_y(x, z) else { continue };
                let mut y = column_top.min(top);
                while y >= w.min_y {
                    let p = BlockPos::new(x, y, z);
                    if replaceable(p) {
                        let start = y;
                        while y > w.min_y && replaceable(BlockPos::new(x, y - 1, z)) {
                            y -= 1;
                        }
                        let gap = start - y;
                        if y + 4 <= top && (gap <= 0 || gap >= 3) {
                            let origin = BlockPos::new(x, y, z);
                            if can_host(origin, 0) {
                                let d = ((origin.x - near.x).pow(2) + (origin.y - near.y).pow(2) + (origin.z - near.z).pow(2)) as f64;
                                if can_host(origin, -1) && can_host(origin, 1) && d < best_d {
                                    best_d = d;
                                    best = Some(origin);
                                }
                                if d < fallback_d {
                                    fallback_d = d;
                                    fallback = Some(origin);
                                }
                            }
                        }
                    }
                    y -= 1;
                }
            }
        }
        let mut blocks_to_set: Vec<(BlockPos, u32)> = Vec::new();
        let obsidian = blocks::default_state("minecraft:obsidian")?;
        let origin = match best.or(fallback) {
            Some(o) => o,
            None => {
                // Nowhere suitable: build a platform in the air.
                let low = (w.min_y + 1).max(70);
                let high = top - 9;
                if high < low {
                    return None;
                }
                let origin = BlockPos::new(near.x - dir.0, near.y.clamp(low, high), near.z - dir.1);
                for l in -1..2 {
                    for m in 0..2 {
                        for n in -1..3 {
                            let p = origin.offset(m * dir.0 + l * side.0, n, m * dir.1 + l * side.1);
                            blocks_to_set.push((p, if n < 0 { obsidian } else { blocks::AIR }));
                        }
                    }
                }
                origin
            }
        };
        for o in -1..3 {
            for p in -1..4 {
                if o == -1 || o == 2 || p == -1 || p == 3 {
                    blocks_to_set.push((origin.offset(o * dir.0, p, o * dir.1), obsidian));
                }
            }
        }
        for (pos, state) in blocks_to_set {
            self.set_block_and_update(world, pos, state);
        }
        let portal_state = blocks::with_property(blocks::default_state("minecraft:nether_portal")?, "axis", axis.name())?;
        for i in 0..2 {
            for j in 0..3 {
                let pos = origin.offset(i * dir.0, j, i * dir.1);
                self.set_block_with_flags(world, pos, portal_state, crate::world::block_updates::flags::CLIENTS | crate::world::block_updates::flags::KNOWN_SHAPE);
            }
        }
        let shape = PortalShape::find_any(&self.worlds[world], origin, axis);
        shape.is_valid().then_some(shape)
    }

    /// The obsidian platform players arrive on in the End (`EndPlatformFeature`).
    fn make_end_platform(&mut self, world: usize, at: BlockPos) {
        let Some(obsidian) = blocks::default_state("minecraft:obsidian") else { return };
        for dx in -2..=2 {
            for dz in -2..=2 {
                for dy in -1..3 {
                    let pos = at.offset(dx, dy, dz);
                    let state = if dy == -1 { obsidian } else { blocks::AIR };
                    if self.worlds[world].block_at(pos.x, pos.y, pos.z) != Some(state) {
                        self.set_block_and_update(world, pos, state);
                    }
                }
            }
        }
    }

    /// Leaving the End: the player vanishes from the End, the client shows the credits the
    /// first time, then asks to respawn (handled in `respawn_player`).
    fn leave_the_end(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        p.won_game = true;
        let first = !p.seen_credits;
        p.seen_credits = true;
        p.conn.send(&play::GameEvent { event: play::game_event::WIN_GAME, value: if first { 1.0 } else { 0.0 } });
        let viewers: Vec<i32> = p.seen_by.drain().collect();
        for v in viewers {
            if let Some(viewer) = self.players.get_mut(&v) {
                viewer.conn.send(&play::RemoveEntities { entity_ids: vec![VarInt(id)] });
                viewer.visible.remove(&id);
            }
        }
    }

    // --- changing worlds ---------------------------------------------------------------------

    /// Forgets the player's presence in its world: chunk tickets, who sees whom, open
    /// windows. Used before moving to another world.
    pub(super) fn leave_world(&mut self, id: i32) {
        self.close_menu(id, true);
        let Some(p) = self.players.get_mut(&id) else { return };
        let world = p.world;
        let tracked: Vec<ChunkPos> = p.view.tracked.drain().collect();
        let viewers: Vec<i32> = p.seen_by.drain().collect();
        let visible: Vec<i32> = p.visible.drain().collect();
        p.view.sent.clear();
        p.view.pending.clear();
        p.using_item = None;
        for pos in tracked {
            self.worlds[world].remove_ticket(pos);
        }
        for v in viewers {
            if let Some(viewer) = self.players.get_mut(&v) {
                viewer.conn.send(&play::RemoveEntities { entity_ids: vec![VarInt(id)] });
                viewer.visible.remove(&id);
            }
        }
        for t in visible {
            if let Some(target) = self.players.get_mut(&t) {
                target.seen_by.remove(&id);
            }
        }
        for e in self.worlds[world].entities.values_mut() {
            e.viewers.remove(&id);
        }
    }

    /// Moves a player to another world (`ServerPlayer.teleport` across dimensions): the
    /// client gets a Respawn packet for the new dimension and everything it needs again.
    pub fn change_dimension(&mut self, id: i32, to: usize, target: Location) {
        if to >= self.worlds.len() {
            return;
        }
        let Some(from) = self.players.get(&id).map(|p| p.world) else { return };
        if from == to {
            self.teleport(id, target);
            return;
        }
        self.leave_world(id);
        let server_view = self.config.properties.view_distance;
        let spawn_info = {
            let p = &self.players[&id];
            self.spawn_info(to, p.game_mode, p.previous_game_mode)
        };
        let overworld = self.world_of_dimension("minecraft:overworld").unwrap_or(0);
        let (spawn_dimension, spawn, spawn_yaw) = {
            let w = &self.worlds[overworld];
            (w.dimension.clone(), w.spawn, w.spawn_yaw)
        };
        let time = self.time_payload(to);
        let difficulty = self.difficulty.id();
        let Some(p) = self.players.get_mut(&id) else { return };
        p.world = to;
        p.loc = target;
        p.view = ChunkView::new(target.chunk(), p.view_distance(server_view));
        p.fall_distance = 0.0;
        p.portal_time = 0;
        p.sent_health = None;
        p.metadata_dirty = true;
        p.equipment_dirty = true;
        // Keep attributes and entity data, as for a portal trip.
        p.conn.send(&play::Respawn { spawn: spawn_info, data_kept: 3 });
        p.conn.send(&play::ChangeDifficulty { difficulty, locked: false });
        p.conn.send(&play::PlayerAbilities { flags: p.ability_flags(), flying_speed: 0.05, fov_modifier: 0.1 });
        p.conn.send(&play::SetHeldSlot { slot: VarInt(p.held_slot as i32) });
        p.conn.send(&play::SetDefaultSpawnPosition { dimension: spawn_dimension, pos: spawn, yaw: spawn_yaw, pitch: 0.0 });
        p.conn.send_payload(time);
        p.conn.send(&play::GameEvent { event: play::game_event::START_WAITING_FOR_CHUNKS, value: 0.0 });
        p.conn.send(&play::SetChunkCacheCenter { x: VarInt(p.view.center.x), z: VarInt(p.view.center.z) });
        // The portal travel sound.
        p.conn.send(&play::LevelEvent { event: 1032, pos: BlockPos::new(0, 0, 0), data: 0, global: false });
        self.send_weather(id);
        self.teleport(id, target);
        self.resync_inventory(id);
        self.send_experience(id);
        self.send_health_if_changed(id);
        self.refresh_chunk_view(id);
        crate::plugins::events::on_changed_world(self, id, from);
    }

    // --- items that light, fill and pour -------------------------------------------------------

    /// Flint and steel or a fire charge used on a block: TNT is primed, fire is lit (which
    /// lights a Nether portal inside an obsidian frame). Returns whether the click was used.
    pub(super) fn use_igniter(&mut self, id: i32, use_on: &play::UseItemOn) -> bool {
        let Some(p) = self.players.get(&id) else { return false };
        let slot = if use_on.hand.0 == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        let item = items::get(stack.item as u32).map_or("", |i| i.name);
        if !matches!(item, "minecraft:flint_and_steel" | "minecraft:fire_charge") || !matches!(p.game_mode, GameMode::Survival | GameMode::Creative) {
            return false;
        }
        let (world, creative) = (p.world, p.game_mode == GameMode::Creative);
        let Some(clicked) = self.worlds[world].block_at(use_on.pos.x, use_on.pos.y, use_on.pos.z) else { return false };
        let face = use_on.face.0 as usize;
        let mut used = false;
        let sound_at = Vec3::new(use_on.pos.x as f64 + 0.5, use_on.pos.y as f64 + 0.5, use_on.pos.z as f64 + 0.5);
        let clicked_name = blocks::block_of_state(clicked).name;
        if clicked_name == "minecraft:tnt" {
            // `TntBlock.useItemOn`.
            if self.worlds[world].rules.bool("tnt_explodes") {
                self.set_block_and_update(world, use_on.pos, blocks::AIR);
                self.prime_tnt(world, use_on.pos, 80, Some(id));
                used = true;
            }
        } else if (clicked_name.ends_with("campfire") || clicked_name.ends_with("candle") || clicked_name.ends_with("candle_cake")) && blocks::property(clicked, "lit") == Some("false") {
            if let Some(lit) = blocks::with_property(clicked, "lit", "true") {
                self.set_block_and_update(world, use_on.pos, lit);
                used = true;
            }
        } else if face < 6 {
            let (dx, dy, dz) = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)][face];
            let target = use_on.pos.offset(dx, dy, dz);
            let w = &self.worlds[world];
            let empty = w.block_at(target.x, target.y, target.z).is_some_and(blocks::is_air);
            let portal_frame = matches!(w.dimension.as_str(), "minecraft:overworld" | "minecraft:the_nether") && PortalShape::find_empty(w, target, Axis::X).is_some();
            let fire_state = fire::fire_state_for(w, target);
            let supported = {
                let below = w.block_at(target.x, target.y - 1, target.z).unwrap_or(blocks::AIR);
                paper_data::physics::is_face_sturdy(below, 1) || (0..6).any(|f| {
                    let (fx, fy, fz) = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)][f];
                    w.block_at(target.x + fx, target.y + fy, target.z + fz).is_some_and(|s| paper_data::physics::flags(s) & paper_data::physics::BURNABLE != 0)
                })
            };
            if empty && (supported || portal_frame) && crate::plugins::events::on_block_ignite(self, id, world, target) {
                self.set_block_and_update(world, target, fire_state);
                used = true;
            }
        }
        if !used {
            return false;
        }
        let pitch = rand::thread_rng().gen::<f32>() * 0.4 + 0.8;
        let sound = if item == "minecraft:fire_charge" { "minecraft:item.firecharge.use" } else { "minecraft:item.flintandsteel.use" };
        self.play_sound(world, sound_at, sound, category::BLOCKS, 1.0, pitch, Some(id));
        if !creative {
            let p = self.players.get_mut(&id).unwrap();
            if item == "minecraft:fire_charge" {
                p.inventory[slot] = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            } else if survival::hurt_item(&mut p.inventory[slot], 1, false) {
                self.play_sound(world, sound_at, "minecraft:entity.item.break", category::PLAYERS, 0.8, 1.0, None);
            }
            let item = self.players[&id].inventory[slot].clone();
            self.set_inventory_slot(id, slot, item);
        }
        true
    }

    /// An eye of ender thrown into the air (`EnderEyeItem.use`): it flies towards the nearest
    /// stronghold. Nothing happens where there is none (no vanilla generation, or
    /// `generate-structures` off).
    pub(super) fn throw_ender_eye(&mut self, id: i32, slot: usize) {
        use rand::Rng;
        let Some(p) = self.players.get(&id) else { return };
        let (world, loc, creative) = (p.world, p.loc, p.game_mode == GameMode::Creative);
        let Some(stack) = p.inventory[slot].0.clone() else { return };
        let found = self.worlds[world].generator.staged().and_then(|g| g.inner.locate("minecraft:stronghold", loc.x.floor() as i32, loc.z.floor() as i32, 100));
        let (Some(found), Some(info)) = (found, paper_data::entity_types::by_name("minecraft:eye_of_ender")) else { return };
        let start = Vec3::new(loc.x, loc.y + 0.9, loc.z);
        // `signalTo`: at most 12 blocks out and 8 up at a time.
        let (dx, dz) = (found.x as f64 - start.x, found.z as f64 - start.z);
        let distance = (dx * dx + dz * dz).sqrt();
        let target = if distance > 12.0 {
            Vec3::new(start.x + dx / distance * 12.0, start.y + 8.0, start.z + dz / distance * 12.0)
        } else {
            Vec3::new(found.x as f64, found.y as f64, found.z as f64)
        };
        let mut rng = rand::thread_rng();
        let kind = crate::world::entity::EntityKind::EyeOfEnder { target, life: 0, survive: rng.gen_range(0..5) > 0 };
        let e = crate::world::entity::Entity::new(self.next_entity_id(), info, start, kind);
        self.spawn_entity(world, e);
        let pitch = 0.33 + rng.gen::<f32>() * (0.5 - 0.33);
        self.play_sound(world, Vec3::new(loc.x, loc.y, loc.z), "minecraft:entity.ender_eye.launch", category::NEUTRAL, 1.0, pitch, None);
        if !creative {
            let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            self.set_inventory_slot(id, slot, rest);
        }
        let hand = if slot == OFFHAND_SLOT { 1 } else { 0 };
        self.broadcast_to_viewers(id, &play::SwingAnimation { entity_id: VarInt(id), hand: VarInt(hand), animation_type: VarInt(1), duration: VarInt(6) });
    }

    /// An eye of ender put into an End portal frame; a complete ring opens the portal.
    pub(super) fn use_ender_eye(&mut self, id: i32, use_on: &play::UseItemOn) -> bool {
        let Some(p) = self.players.get(&id) else { return false };
        let slot = if use_on.hand.0 == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        if items::get(stack.item as u32).is_none_or(|i| i.name != "minecraft:ender_eye") {
            return false;
        }
        let (world, creative) = (p.world, p.game_mode == GameMode::Creative);
        let Some(state) = self.worlds[world].block_at(use_on.pos.x, use_on.pos.y, use_on.pos.z) else { return false };
        if blocks::block_of_state(state).name != "minecraft:end_portal_frame" || blocks::property(state, "eye") != Some("false") {
            return false;
        }
        let Some(with_eye) = blocks::with_property(state, "eye", "true") else { return false };
        self.set_block_and_update(world, use_on.pos, with_eye);
        // Particles and sound of the eye going in.
        self.broadcast_level_event(world, 1503, use_on.pos, 0, false);
        if !creative {
            let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            self.set_inventory_slot(id, slot, rest);
        }
        if let Some(center) = portal::end_portal_center(&self.worlds[world], use_on.pos) {
            let Some(portal_block) = blocks::default_state("minecraft:end_portal") else { return true };
            for dx in -1..=1 {
                for dz in -1..=1 {
                    self.set_block_with_flags(world, center.offset(dx, 0, dz), portal_block, crate::world::block_updates::flags::CLIENTS);
                }
            }
            // Heard everywhere.
            self.broadcast_level_event(world, 1038, center.offset(1, 0, 1), 0, true);
        }
        true
    }

    /// Sends a level event (sound or particles) to the players of a world: near the position,
    /// or everyone when `global`.
    pub fn broadcast_level_event(&self, world: usize, event: i32, pos: BlockPos, data: i32, global: bool) {
        let packet = play::LevelEvent { event, pos, data, global };
        for p in self.players.values().filter(|p| p.world == world) {
            let (dx, dz) = (p.loc.x - pos.x as f64, p.loc.z - pos.z as f64);
            if global || dx * dx + dz * dz < 64.0 * 64.0 {
                p.conn.send(&packet);
            }
        }
    }

    /// Buckets (`BucketItem.use`): an empty bucket scoops up a fluid source; a full one pours
    /// its fluid where the player looks (water boils away in the Nether). Returns whether the
    /// bucket was used.
    pub(super) fn use_bucket(&mut self, id: i32, hand: i32, yaw: f32, pitch: f32) -> bool {
        let Some(p) = self.players.get(&id) else { return false };
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        let item = items::get(stack.item as u32).map_or("", |i| i.name);
        let content = match item {
            "minecraft:bucket" => None,
            "minecraft:water_bucket" => Some(Fluid::Water),
            "minecraft:lava_bucket" => Some(Fluid::Lava),
            _ => return false,
        };
        if !matches!(p.game_mode, GameMode::Survival | GameMode::Creative) {
            return false;
        }
        let (world, creative) = (p.world, p.game_mode == GameMode::Creative);
        let eye = Vec3::new(p.loc.x, p.loc.y + p.eye_height(), p.loc.z);
        let dir = Vec3::from_rotation(yaw, pitch);
        let Some((hit, face, fluid_source)) = raycast(&self.worlds[world], eye, dir, 4.5, content.is_none()) else { return false };
        match content {
            None => {
                // Scoop up a source block (or the water of a waterlogged block).
                let Some(state) = self.worlds[world].block_at(hit.x, hit.y, hit.z) else { return false };
                let Some(fluid) = fluid_source else { return false };
                let (new_state, filled) = match fluid {
                    Fluid::Water if blocks::property(state, "waterlogged") == Some("true") => (blocks::with_property(state, "waterlogged", "false").unwrap_or(state), "minecraft:water_bucket"),
                    Fluid::Water => (blocks::AIR, "minecraft:water_bucket"),
                    Fluid::Lava => (blocks::AIR, "minecraft:lava_bucket"),
                };
                self.set_block_and_update(world, hit, new_state);
                let sound = if fluid == Fluid::Lava { "minecraft:item.bucket.fill_lava" } else { "minecraft:item.bucket.fill" };
                self.play_sound(world, Vec3::new(hit.x as f64 + 0.5, hit.y as f64 + 0.5, hit.z as f64 + 0.5), sound, category::BLOCKS, 1.0, 1.0, None);
                let filled = ItemStack::new(items::id(filled).unwrap_or(0) as i32, 1);
                self.fill_container_item(id, slot, &stack, filled, creative);
                true
            }
            Some(fluid) => {
                let w = &self.worlds[world];
                let Some(clicked) = w.block_at(hit.x, hit.y, hit.z) else { return false };
                // Waterloggable blocks take the water themselves.
                let (target, state) = if fluid == Fluid::Water && blocks::property(clicked, "waterlogged") == Some("false") {
                    (hit, blocks::with_property(clicked, "waterlogged", "true").unwrap_or(clicked))
                } else {
                    let target = if blocks::is_replaceable(clicked) { hit } else { hit.offset(face.0, face.1, face.2) };
                    let existing = w.block_at(target.x, target.y, target.z).unwrap_or(blocks::AIR);
                    if !blocks::is_replaceable(existing) && !blocks::is_air(existing) {
                        return false;
                    }
                    let source = if fluid == Fluid::Water { "minecraft:water" } else { "minecraft:lava" };
                    (target, blocks::default_state(source).unwrap_or(blocks::AIR))
                };
                let at = Vec3::new(target.x as f64 + 0.5, target.y as f64 + 0.5, target.z as f64 + 0.5);
                let evaporates = fluid == Fluid::Water && paper_data::synced::dimension_type(&w.dimension).is_some_and(|d| d.water_evaporates);
                if evaporates {
                    let pitch = 2.6 + (rand::thread_rng().gen::<f32>() - rand::thread_rng().gen::<f32>()) * 0.8;
                    self.play_sound(world, at, "minecraft:block.fire.extinguish", category::BLOCKS, 0.5, pitch, None);
                } else {
                    let existing = self.worlds[world].block_at(target.x, target.y, target.z).unwrap_or(blocks::AIR);
                    if !blocks::is_air(existing) && survival::fluid_of(existing).is_none() && target != hit {
                        self.drop_block_loot(world, target, existing, None, None);
                    }
                    self.set_block_and_update(world, target, state);
                    let sound = if fluid == Fluid::Lava { "minecraft:item.bucket.empty_lava" } else { "minecraft:item.bucket.empty" };
                    self.play_sound(world, at, sound, category::BLOCKS, 1.0, 1.0, None);
                }
                if !creative {
                    let empty = ItemStack::new(items::id("minecraft:bucket").unwrap_or(0) as i32, 1);
                    self.set_inventory_slot(id, slot, Slot(Some(empty)));
                }
                true
            }
        }
    }

    /// Replaces one item of `stack` in `slot` with `filled` (`ItemUtils.createFilledResult`).
    fn fill_container_item(&mut self, id: i32, slot: usize, stack: &ItemStack, filled: ItemStack, creative: bool) {
        if creative {
            let has = self.players.get(&id).is_some_and(|p| p.inventory.iter().any(|s| s.0.as_ref().is_some_and(|s| s.item == filled.item)));
            if !has {
                self.add_to_inventory(id, &filled);
            }
            return;
        }
        if stack.count <= 1 {
            self.set_inventory_slot(id, slot, Slot(Some(filled)));
        } else {
            self.set_inventory_slot(id, slot, Slot(Some(stack.with_count(stack.count - 1))));
            if self.add_to_inventory(id, &filled) > 0 {
                self.player_drop(id, Slot(Some(filled)), false);
            }
        }
    }
}

/// Steps along a ray through blocks (`ClipContext`): returns the first block hit, the face
/// it was entered through as an offset, and the fluid if the block is a fluid source (when
/// `fluids` is set, sources stop the ray).
fn raycast(world: &crate::world::World, from: Vec3, dir: Vec3, range: f64, fluids: bool) -> Option<(BlockPos, (i32, i32, i32), Option<Fluid>)> {
    let to = from.add(dir.scale(range));
    let (mut x, mut y, mut z) = (from.x.floor() as i32, from.y.floor() as i32, from.z.floor() as i32);
    let step = |d: f64| if d > 0.0 { 1 } else if d < 0.0 { -1 } else { 0 };
    let (sx, sy, sz) = (step(dir.x), step(dir.y), step(dir.z));
    let next = |p: f64, s: i32| if s > 0 { p.floor() + 1.0 } else { p.floor() };
    let t_of = |p: f64, d: f64, s: i32| if s == 0 { f64::MAX } else { (next(p, s) - p) / d };
    let delta = |d: f64| if d == 0.0 { f64::MAX } else { 1.0 / d.abs() };
    let (mut tx, mut ty, mut tz) = (t_of(from.x, dir.x, sx), t_of(from.y, dir.y, sy), t_of(from.z, dir.z, sz));
    let (dtx, dty, dtz) = (delta(dir.x), delta(dir.y), delta(dir.z));
    let mut face = (0, 0, 0);
    let max_t = to.sub(from).length();
    let mut t = 0.0;
    while t <= max_t {
        let state = world.block_at(x, y, z)?;
        let source = survival::fluid_of(state).filter(|(_, amount)| *amount == 8).filter(|_| {
            // A source, not falling or flowing water.
            blocks::property(state, "level").is_none_or(|l| l == "0")
        });
        if fluids && source.is_some() {
            return Some((BlockPos::new(x, y, z), face, source.map(|(f, _)| f)));
        }
        let solid = !blocks::is_air(state) && survival::fluid_of(state).is_none_or(|_| blocks::property(state, "waterlogged").is_some() || !paper_data::physics::collision_boxes(state).is_empty());
        if solid {
            return Some((BlockPos::new(x, y, z), face, None));
        }
        if tx < ty && tx < tz {
            x += sx;
            t = tx;
            tx += dtx;
            face = (-sx, 0, 0);
        } else if ty < tz {
            y += sy;
            t = ty;
            ty += dty;
            face = (0, -sy, 0);
        } else {
            z += sz;
            t = tz;
            tz += dtz;
            face = (0, 0, -sz);
        }
    }
    None
}
