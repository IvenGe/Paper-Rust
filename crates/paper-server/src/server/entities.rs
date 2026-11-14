//! Running entities: spawning, ticking, item pickup and merging, and showing entities to
//! players (spawn, movement, metadata and removal packets).

use super::loot::LootContext;
use super::player::HOTBAR_START;
use super::Server;
use crate::config::GameMode;
use crate::world::chunk::ChunkPos;
use crate::world::entity::{Aabb, Entity, EntityKind, Vec3};
use bytes::Bytes;
use paper_data::{blocks, entity_types, items};
use paper_protocol::codec::{BlockPos, RemainingBytes, VarInt};
use paper_protocol::metadata::{flags, Metadata};
use paper_protocol::packets::{encode_packet, play};
use paper_protocol::types::{Angle, ItemStack, LpVec3, Slot};
use rand::Rng;
use std::collections::HashSet;
use std::sync::atomic::Ordering;

/// Item entities disappear after five minutes.
const ITEM_LIFETIME: i32 = 6000;
/// Ticks between absolute position updates.
const SYNC_INTERVAL: i32 = 60;

fn fixed(v: f64) -> i64 {
    (v * 4096.0).round() as i64
}

fn angle(d: f32) -> u8 {
    ((d.rem_euclid(360.0)) * 256.0 / 360.0) as i32 as u8
}

/// Whether two stacks are the same item with the same components (and so can stack).
pub fn same_item(a: &ItemStack, b: &ItemStack) -> bool {
    a.item == b.item && a.added == b.added && a.removed == b.removed
}

pub fn max_stack(stack: &ItemStack) -> i32 {
    items::get(stack.item as u32).map_or(64, |i| i.max_stack_size as i32).max(1)
}

impl Server {
    pub fn next_entity_id(&self) -> i32 {
        self.shared.next_entity_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Adds an entity to a world; it is shown to players on the next tick.
    pub fn spawn_entity(&mut self, world: usize, e: Entity) -> i32 {
        let id = e.id;
        self.worlds[world].add_entity(e);
        id
    }

    /// Brings back the entities saved with a chunk that was just loaded.
    pub(super) fn spawn_saved_entities(&mut self, world: usize, pos: ChunkPos) {
        let Some(saved) = self.worlds.get_mut(world).and_then(|w| w.chunks.get_mut(&pos)).map(|c| std::mem::take(&mut c.entity_nbt)) else { return };
        for nbt in saved {
            let id = self.next_entity_id();
            match crate::world::entity_nbt::entity_from_nbt(id, &nbt) {
                Some(e) => {
                    self.spawn_entity(world, e);
                }
                None => log::debug!("Dropping a saved entity that could not be read: {:?}", nbt.string("id")),
            }
        }
    }

    /// Spawns an item entity.
    pub fn spawn_item(&mut self, world: usize, pos: Vec3, stack: Slot, vel: Vec3, pickup_delay: i32) -> Option<i32> {
        if stack.0.as_ref().is_none_or(|s| s.count <= 0) {
            return None;
        }
        let info = entity_types::by_name("minecraft:item")?;
        let mut e = Entity::new(self.next_entity_id(), info, pos, EntityKind::Item { stack, pickup_delay, thrower: None });
        e.vel = vel;
        e.yaw = rand::thread_rng().gen_range(0.0..360.0);
        Some(self.spawn_entity(world, e))
    }

    /// Drops a stack out of a block the way vanilla's `Block.popResource` does.
    pub fn pop_resource(&mut self, world: usize, block: BlockPos, stack: Slot) {
        let mut rng = rand::thread_rng();
        let half = 0.125;
        let pos = Vec3::new(
            block.x as f64 + 0.5 + rng.gen_range(-0.25..0.25),
            block.y as f64 + 0.5 + rng.gen_range(-0.25..0.25) - half,
            block.z as f64 + 0.5 + rng.gen_range(-0.25..0.25),
        );
        let vel = Vec3::new(rng.gen_range(-0.1..0.1), 0.2, rng.gen_range(-0.1..0.1));
        self.spawn_item(world, pos, stack, vel, 10);
    }

    /// Drops what a block yields when broken (by a player with `tool`, or by an explosion).
    pub fn drop_block_loot(&mut self, world: usize, pos: BlockPos, state: u32, tool: Option<&Slot>, explosion_radius: Option<f32>) {
        let drops = {
            let block = blocks::block_of_state(state);
            let correct_tool = !paper_data::physics::requires_correct_tool(state) || tool.is_some_and(|t| self.tool_is_correct(t, state));
            if !correct_tool {
                Vec::new()
            } else if let Some(table) = paper_data::physics::loot_table(block.name) {
                if self.loot.available() {
                    self.loot.roll(table, &LootContext { tool, block_state: Some(state), explosion_radius, ..Default::default() })
                } else {
                    super::loot::fallback_block_drop(state)
                }
            } else {
                Vec::new()
            }
        };
        for stack in drops {
            self.pop_resource(world, pos, stack);
        }
    }

    /// Whether `tool` is the right kind (and tier) of tool to get drops from `state`.
    pub fn tool_is_correct(&self, tool: &Slot, state: u32) -> bool {
        let Some(stack) = tool.0.as_ref() else { return false };
        let Some(t) = paper_data::item_data::by_id(stack.item as u32).tool.as_ref() else { return false };
        for rule in &t.rules {
            if block_in(&rule.blocks, state) {
                if let Some(correct) = rule.correct_for_drops {
                    return correct;
                }
            }
        }
        false
    }

    /// The item a player throws (Q, or dropping from the inventory screen).
    pub fn player_drop(&mut self, id: i32, stack: Slot, thrown: bool) {
        let Some(player) = self.players.get(&id) else { return };
        let world = player.world;
        let eye = Vec3::new(player.loc.x, player.loc.y + player.eye_height() - 0.3, player.loc.z);
        let mut rng = rand::thread_rng();
        let vel = if thrown {
            let (yaw, pitch) = ((player.loc.yaw as f64).to_radians(), (player.loc.pitch as f64).to_radians());
            let (spread, angle_r) = (0.02 * rng.gen::<f64>(), rng.gen::<f64>() * std::f64::consts::TAU);
            Vec3::new(
                -yaw.sin() * pitch.cos() * 0.3 + angle_r.cos() * spread,
                -pitch.sin() * 0.3 + 0.1 + (rng.gen::<f64>() - rng.gen::<f64>()) * 0.1,
                yaw.cos() * pitch.cos() * 0.3 + angle_r.sin() * spread,
            )
        } else {
            Vec3::new(rng.gen_range(-0.25..0.25), 0.2, rng.gen_range(-0.25..0.25))
        };
        let uuid = player.uuid();
        if let Some(item) = self.spawn_item(world, eye, stack.clone(), vel, 40) {
            if let Some(EntityKind::Item { thrower, .. }) = self.worlds[world].entities.get_mut(&item).map(|e| &mut e.kind) {
                *thrower = Some(uuid);
            }
            // Plugin hook: PlayerDropItemEvent. A cancelled drop goes back to the player
            // (the held slot first, as CraftBukkit does), or stays dropped if it cannot.
            if !crate::plugins::events::on_drop_item(self, id, item, &stack) {
                let Some(e) = self.worlds[world].entities.get_mut(&item) else { return };
                let EntityKind::Item { stack: Slot(Some(back)), .. } = std::mem::replace(&mut e.kind, EntityKind::Other) else { return };
                e.removed = true;
                let Some(p) = self.players.get(&id) else { return };
                let held = HOTBAR_START + p.held_slot as usize;
                let room = match &p.inventory[held].0 {
                    None => back.count,
                    Some(s) if same_item(s, &back) => (max_stack(s) - s.count).min(back.count),
                    Some(_) => 0,
                };
                if room > 0 {
                    let current = p.inventory[held].0.as_ref().map_or(0, |s| s.count);
                    self.set_inventory_slot(id, held, Slot(Some(back.with_count(current + room))));
                }
                let left = self.add_to_inventory(id, &back.with_count(back.count - room));
                if left > 0 && back.count - room > 0 {
                    // No room: drop the rest without asking again.
                    self.spawn_item(world, eye, Slot(Some(back.with_count(left))), vel, 40);
                }
            }
        }
    }

    /// How many items of `stack` fit into a player's inventory.
    pub fn inventory_room(&self, id: i32, stack: &ItemStack) -> i32 {
        let Some(player) = self.players.get(&id) else { return 0 };
        let max = max_stack(stack);
        (HOTBAR_START..HOTBAR_START + 9)
            .chain(9..HOTBAR_START)
            .map(|slot| match &player.inventory[slot].0 {
                None => max,
                Some(existing) if same_item(existing, stack) => (max - existing.count).max(0),
                Some(_) => 0,
            })
            .sum::<i32>()
            + match &player.inventory[super::player::OFFHAND_SLOT].0 {
                Some(existing) if same_item(existing, stack) => (max - existing.count).max(0),
                _ => 0,
            }
    }

    /// Adds a stack to a player's inventory (stacking first, hotbar before the main
    /// inventory). Returns how many items did not fit.
    pub fn add_to_inventory(&mut self, id: i32, stack: &ItemStack) -> i32 {
        let mut count = stack.count;
        let max = max_stack(stack);
        let order: Vec<usize> = (HOTBAR_START..HOTBAR_START + 9).chain(9..HOTBAR_START).chain([super::player::OFFHAND_SLOT]).collect();
        for pass in 0..2 {
            for &slot in &order {
                if count <= 0 {
                    return 0;
                }
                let Some(player) = self.players.get(&id) else { return count };
                let updated = match (&player.inventory[slot].0, pass) {
                    (Some(existing), 0) if same_item(existing, stack) && existing.count < max => {
                        let add = (max - existing.count).min(count);
                        count -= add;
                        Some(Slot(Some(existing.with_count(existing.count + add))))
                    }
                    (None, 1) if slot != super::player::OFFHAND_SLOT => {
                        let add = max.min(count);
                        count -= add;
                        Some(Slot(Some(stack.with_count(add))))
                    }
                    _ => None,
                };
                if let Some(updated) = updated {
                    self.set_inventory_slot(id, slot, updated);
                }
            }
        }
        count
    }

    /// One tick of every entity near a player, then pickup, merging and client updates.
    pub(super) fn tick_entities(&mut self) {
        let simulation = self.config.properties.simulation_distance.max(2);
        for world in 0..self.worlds.len() {
            let player_chunks: Vec<ChunkPos> = self.players.values().filter(|p| p.world == world).map(|p| p.loc.chunk()).collect();
            if player_chunks.is_empty() && self.worlds[world].entities.is_empty() {
                continue;
            }
            let ticking: Vec<i32> = {
                let w = &self.worlds[world];
                let mut ids: Vec<i32> = w
                    .entities
                    .values()
                    .filter(|e| w.is_ready(e.chunk) && player_chunks.iter().any(|c| c.distance(e.chunk) <= simulation))
                    .map(|e| e.id)
                    .collect();
                ids.sort_unstable();
                ids
            };
            for id in ticking {
                self.tick_entity(world, id);
            }
            if self.tick % 2 == 0 {
                self.merge_items(world);
            }
            self.pickup_items(world);
            self.remove_dead_entities(world);
            self.track_entities(world);
        }
    }

    fn tick_entity(&mut self, world: usize, id: i32) {
        let Some(mut e) = self.worlds[world].entities.remove(&id) else { return };
        e.age += 1;
        match &mut e.kind {
            EntityKind::Item { pickup_delay, .. } => {
                if *pickup_delay > 0 && *pickup_delay != 32767 {
                    *pickup_delay -= 1;
                }
                let w = &self.worlds[world];
                let feet = w.block_at(e.pos.x.floor() as i32, e.pos.y.floor() as i32, e.pos.z.floor() as i32).unwrap_or(blocks::AIR);
                let in_water = blocks::block_of_state(feet).name == "minecraft:water" || blocks::property(feet, "waterlogged") == Some("true");
                let in_lava = blocks::block_of_state(feet).name == "minecraft:lava";
                if in_lava && !items_fire_resistant(&e) {
                    e.removed = true;
                }
                if in_water {
                    // Float up, as vanilla's setUnderwaterMovement does.
                    e.vel = Vec3::new(e.vel.x * 0.99, e.vel.y + if e.vel.y < 0.06 { 5.0e-4 } else { 0.0 }, e.vel.z * 0.99);
                } else if !e.no_gravity {
                    e.vel.y -= 0.04;
                }
                if e.vel.length_squared() > 1.0e-10 || !e.on_ground || e.age % 4 == 0 {
                    let d = e.vel;
                    e.move_by(w, d, 0.0);
                    let friction = if e.on_ground { crate::world::entity::friction_below(w, e.pos) * 0.98 } else { 0.98 };
                    e.vel = Vec3::new(e.vel.x * friction, e.vel.y * 0.98, e.vel.z * friction);
                    if e.on_ground && e.vel.y < 0.0 {
                        e.vel.y *= -0.5;
                    }
                }
                if e.age >= ITEM_LIFETIME || e.pos.y < (w.min_y - 64) as f64 {
                    e.removed = true;
                }
            }
            EntityKind::Mob(_) => {
                self.worlds[world].entities.insert(id, e);
                self.tick_mob(world, id);
                self.worlds[world].reindex_entity(id);
                return;
            }
            EntityKind::Arrow { .. } => {
                self.worlds[world].entities.insert(id, e);
                self.tick_arrow(world, id);
                self.worlds[world].reindex_entity(id);
                return;
            }
            EntityKind::FallingBlock { state, time } => {
                *time += 1;
                let (state, time) = (*state, *time);
                let w = &self.worlds[world];
                if !e.no_gravity {
                    e.vel.y -= 0.04;
                }
                let d = e.vel;
                e.move_by(w, d, 0.0);
                e.vel = e.vel.scale(0.98);
                if e.on_ground || time > 600 || e.pos.y < (w.min_y - 64) as f64 {
                    e.removed = true;
                    let pos = BlockPos::new(e.pos.x.floor() as i32, e.pos.y.round() as i32, e.pos.z.floor() as i32);
                    self.worlds[world].entities.insert(id, e);
                    self.land_falling_block(world, pos, state);
                    return;
                }
            }
            EntityKind::EyeOfEnder { target, life, survive } => {
                // `EyeOfEnder.tick`: steer towards the target, rising or sinking to its height.
                let next = e.pos.add(e.vel);
                let horizontal = (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt();
                let (dx, dz) = (target.x - next.x, target.z - next.z);
                let to_target = (dx * dx + dz * dz).sqrt() as f32;
                let angle = dz.atan2(dx);
                let mut speed = horizontal + 0.0025 * (to_target as f64 - horizontal);
                let mut vy = e.vel.y;
                if to_target < 1.0 {
                    speed *= 0.8;
                    vy *= 0.8;
                }
                let sign = if e.pos.y < target.y { 1.0 } else { -1.0 };
                e.vel = Vec3::new(angle.cos() * speed, vy + (sign - vy) * 0.015f32 as f64, angle.sin() * speed);
                e.pos = next;
                *life += 1;
                if *life > 80 {
                    e.removed = true;
                    let (at, survive) = (e.pos, *survive);
                    self.worlds[world].entities.insert(id, e);
                    self.play_sound(world, at, "minecraft:entity.ender_eye.death", super::survival::category::NEUTRAL, 1.0, 1.0, None);
                    let pos = BlockPos::new(at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32);
                    match items::id("minecraft:ender_eye").filter(|_| survive) {
                        Some(eye) => {
                            self.spawn_item(world, at, Slot::of(eye as i32, 1), Vec3::ZERO, 10);
                        }
                        None => self.broadcast_level_event(world, 2003, pos, 0, false),
                    }
                    return;
                }
            }
            EntityKind::Tnt { fuse } => {
                *fuse -= 1;
                let explode = *fuse <= 0;
                let w = &self.worlds[world];
                e.vel.y -= 0.04;
                let d = e.vel;
                e.move_by(w, d, 0.0);
                e.vel = e.vel.scale(0.98);
                if e.on_ground {
                    e.vel = Vec3::new(e.vel.x * 0.7, e.vel.y * -0.5, e.vel.z * 0.7);
                }
                if explode {
                    e.removed = true;
                    let center = Vec3::new(e.pos.x, e.pos.y + 0.0625, e.pos.z);
                    self.worlds[world].entities.insert(id, e);
                    self.explode(world, center, 4.0, true, None);
                    return;
                }
            }
            EntityKind::ExperienceOrb { .. } => {
                self.tick_experience_orb(world, &mut e);
                let w = &self.worlds[world];
                e.vel.y -= 0.03;
                let d = e.vel;
                e.move_by(w, d, 0.0);
                e.vel = e.vel.scale(0.98);
                if e.age >= ITEM_LIFETIME {
                    e.removed = true;
                }
            }
            EntityKind::Other => {
                // A lightning bolt flashes for a few ticks.
                if e.info.name == "minecraft:lightning_bolt" && e.age > 8 {
                    e.removed = true;
                }
            }
        }
        self.worlds[world].entities.insert(id, e);
        self.worlds[world].reindex_entity(id);
    }

    /// A falling block landed: it becomes a block again, or drops as an item.
    fn land_falling_block(&mut self, world: usize, pos: BlockPos, state: u32) {
        let here = self.worlds[world].block_at(pos.x, pos.y, pos.z);
        if here.is_some_and(blocks::is_replaceable) {
            if self.set_block_and_update(world, pos, state) {
                return;
            }
        }
        for stack in super::loot::fallback_block_drop(state) {
            self.pop_resource(world, pos, stack);
        }
    }

    /// Merges nearby identical item stacks, as vanilla does every few ticks.
    fn merge_items(&mut self, world: usize) {
        let ids: Vec<i32> = self.worlds[world].entities.values().filter(|e| matches!(e.kind, EntityKind::Item { .. }) && !e.removed).map(|e| e.id).collect();
        for id in ids {
            let w = &self.worlds[world];
            let Some(e) = w.entities.get(&id) else { continue };
            let EntityKind::Item { stack: Slot(Some(stack)), pickup_delay, .. } = &e.kind else { continue };
            if e.removed || *pickup_delay == 32767 || stack.count >= max_stack(stack) {
                continue;
            }
            let area = e.bounding_box().inflate(0.5, 0.0, 0.5);
            let stack = stack.clone();
            let partner = w.entities_in(&area).into_iter().find(|other| {
                *other != id
                    && w.entities.get(other).is_some_and(|o| {
                        !o.removed && matches!(&o.kind, EntityKind::Item { stack: Slot(Some(s)), .. } if same_item(s, &stack) && s.count + stack.count <= max_stack(&stack))
                    })
            });
            let Some(partner) = partner else { continue };
            // The larger stack absorbs the smaller one.
            let other = &w.entities[&partner];
            let (keep, absorb) = match &other.kind {
                EntityKind::Item { stack: Slot(Some(s)), .. } if s.count > stack.count => (partner, id),
                _ => (id, partner),
            };
            let absorbed = match &self.worlds[world].entities[&absorb].kind {
                EntityKind::Item { stack: Slot(Some(s)), .. } => s.count,
                _ => 0,
            };
            if let Some(EntityKind::Item { stack: Slot(Some(s)), pickup_delay, .. }) = self.worlds[world].entities.get_mut(&keep).map(|e| &mut e.kind) {
                s.count += absorbed;
                *pickup_delay = (*pickup_delay).max(0);
            }
            let absorbed_age = self.worlds[world].entities.get(&absorb).map_or(i32::MAX, |a| a.age);
            if let Some(e) = self.worlds[world].entities.get_mut(&keep) {
                e.metadata_dirty = true;
                e.age = e.age.min(absorbed_age);
            }
            if let Some(e) = self.worlds[world].entities.get_mut(&absorb) {
                e.removed = true;
            }
        }
    }

    /// Players pick up items they touch.
    fn pickup_items(&mut self, world: usize) {
        let collectors: Vec<(i32, Aabb)> = self
            .players
            .values()
            .filter(|p| p.world == world && p.game_mode != GameMode::Spectator && p.health > 0.0)
            .map(|p| (p.entity_id, Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8).inflate(1.0, 0.5, 1.0)))
            .collect();
        for (player, area) in collectors {
            let candidates = self.worlds[world].entities_in(&area);
            for item in candidates {
                let Some(e) = self.worlds[world].entities.get(&item) else { continue };
                let EntityKind::Item { stack: Slot(Some(stack)), pickup_delay, .. } = &e.kind else { continue };
                if *pickup_delay > 0 || e.removed {
                    continue;
                }
                let stack = stack.clone();
                if !crate::plugins::events::on_pickup_item(self, player, world, item, &stack) {
                    continue;
                }
                let left = self.add_to_inventory(player, &stack);
                let taken = stack.count - left;
                if taken <= 0 {
                    continue;
                }
                let packet = play::TakeItemEntity { collected_id: VarInt(item), collector_id: VarInt(player), count: VarInt(taken) };
                let mut viewers: HashSet<i32> = self.worlds[world].entities.get(&item).map(|e| e.viewers.clone()).unwrap_or_default();
                viewers.insert(player);
                let payload = Bytes::from(encode_packet(&packet));
                for v in viewers {
                    if let Some(p) = self.players.get(&v) {
                        p.conn.send_payload(payload.clone());
                    }
                }
                if let Some(e) = self.worlds[world].entities.get_mut(&item) {
                    if left > 0 {
                        if let EntityKind::Item { stack: Slot(Some(s)), .. } = &mut e.kind {
                            s.count = left;
                        }
                        e.metadata_dirty = true;
                    } else {
                        e.removed = true;
                    }
                }
            }
        }
    }

    /// Removes entities marked as removed and tells their viewers.
    fn remove_dead_entities(&mut self, world: usize) {
        let dead: Vec<i32> = self.worlds[world].entities.values().filter(|e| e.removed).map(|e| e.id).collect();
        for id in dead {
            if let Some(e) = self.worlds[world].remove_entity(id) {
                let packet = play::RemoveEntities { entity_ids: vec![VarInt(id)] };
                for v in &e.viewers {
                    if let Some(p) = self.players.get(v) {
                        p.conn.send(&packet);
                    }
                }
            }
        }
    }

    /// Spawns entities for players who should see them, removes them for those who should
    /// not, and sends movement and metadata updates.
    fn track_entities(&mut self, world: usize) {
        let watchers: Vec<(i32, ChunkPos, i32)> = self.players.values().filter(|p| p.world == world).map(|p| (p.entity_id, p.loc.chunk(), p.view.radius)).collect();
        let ids: Vec<i32> = self.worlds[world].entities.keys().copied().collect();
        for id in ids {
            let (wanted, appeared, vanished) = {
                let e = &self.worlds[world].entities[&id];
                let wanted: HashSet<i32> = watchers
                    .iter()
                    .filter(|(pid, center, radius)| {
                        center.distance(e.chunk) <= (*radius).min(e.info.tracking_range.max(1))
                            && self.players.get(pid).is_some_and(|p| p.view.sent.contains(&e.chunk))
                    })
                    .map(|(pid, _, _)| *pid)
                    .collect();
                let appeared: Vec<i32> = wanted.difference(&e.viewers).copied().collect();
                let vanished: Vec<i32> = e.viewers.difference(&wanted).copied().collect();
                (wanted, appeared, vanished)
            };
            if !appeared.is_empty() {
                let spawn = self.spawn_packets(world, id);
                for pid in &appeared {
                    if let Some(p) = self.players.get(pid) {
                        for payload in &spawn {
                            p.conn.send_payload(payload.clone());
                        }
                    }
                }
            }
            if !vanished.is_empty() {
                let packet = play::RemoveEntities { entity_ids: vec![VarInt(id)] };
                for pid in &vanished {
                    if let Some(p) = self.players.get(pid) {
                        p.conn.send(&packet);
                    }
                }
            }
            let updates = self.update_packets(world, id);
            let e = self.worlds[world].entities.get_mut(&id).unwrap();
            e.viewers = wanted;
            if updates.is_empty() {
                continue;
            }
            let viewers: Vec<i32> = e.viewers.iter().copied().filter(|v| !appeared.contains(v)).collect();
            for v in viewers {
                if let Some(p) = self.players.get(&v) {
                    for payload in &updates {
                        p.conn.send_payload(payload.clone());
                    }
                }
            }
        }
    }

    /// Everything a client needs to show an entity.
    fn spawn_packets(&self, world: usize, id: i32) -> Vec<Bytes> {
        let e = &self.worlds[world].entities[&id];
        let data = match &e.kind {
            EntityKind::FallingBlock { state, .. } => *state as i32,
            EntityKind::Arrow { owner, .. } => owner.unwrap_or(0),
            _ => 0,
        };
        let mut out = vec![Bytes::from(encode_packet(&play::AddEntity {
            entity_id: VarInt(id),
            uuid: e.uuid,
            entity_type: VarInt(e.info.id),
            x: e.pos.x,
            y: e.pos.y,
            z: e.pos.z,
            velocity: LpVec3(e.vel.x, e.vel.y, e.vel.z),
            pitch: Angle(e.pitch),
            yaw: Angle(e.yaw),
            head_yaw: Angle(e.head_yaw),
            data: VarInt(data),
        }))];
        let meta = entity_metadata(e, true);
        if meta.len() > 1 {
            out.push(Bytes::from(encode_packet(&play::SetEntityData { entity_id: VarInt(id), metadata: RemainingBytes(meta) })));
        }
        if let EntityKind::Mob(m) = &e.kind {
            let equipment: Vec<(u8, Slot)> = m.equipment.iter().enumerate().filter(|(_, s)| !s.is_empty()).map(|(i, s)| (i as u8, s.clone())).collect();
            if !equipment.is_empty() {
                out.push(Bytes::from(encode_packet(&play::SetEquipment { entity_id: id, equipment })));
            }
            // Changed attributes (a plugin's scale, say).
            if let Some(packet) = Server::mob_attributes_for_new_viewer(&m.attributes, &e.info.name, id) {
                out.push(Bytes::from(encode_packet(&packet)));
            }
        }
        out
    }

    /// Movement, rotation, velocity, metadata and equipment changes since the last send.
    fn update_packets(&mut self, world: usize, id: i32) -> Vec<Bytes> {
        let e = self.worlds[world].entities.get_mut(&id).unwrap();
        let mut out = Vec::new();
        e.ticks_since_sync += 1;
        let interval = e.info.update_interval.max(1);
        let is_mob = matches!(e.kind, EntityKind::Mob(_));
        if e.force_sync || e.ticks_since_sync % interval == 0 || e.on_ground != e.sent_on_ground {
            let (dx, dy, dz) = (fixed(e.pos.x) - fixed(e.sent_pos.x), fixed(e.pos.y) - fixed(e.sent_pos.y), fixed(e.pos.z) - fixed(e.sent_pos.z));
            let moved = dx != 0 || dy != 0 || dz != 0;
            let rotated = angle(e.yaw) != angle(e.sent_yaw) || angle(e.pitch) != angle(e.sent_pitch);
            let fits = |d: i64| (i16::MIN as i64..=i16::MAX as i64).contains(&d);
            if e.force_sync || e.ticks_since_sync > SYNC_INTERVAL * interval || !(fits(dx) && fits(dy) && fits(dz)) {
                out.push(Bytes::from(encode_packet(&play::EntityPositionSync { entity_id: id, x: e.pos.x, y: e.pos.y, z: e.pos.z, yaw: e.yaw, pitch: e.pitch, on_ground: e.on_ground })));
                e.ticks_since_sync = 0;
                e.force_sync = false;
            } else if moved && rotated {
                out.push(Bytes::from(encode_packet(&play::MoveEntityPosRot { entity_id: id, dx: dx as i16, dy: dy as i16, dz: dz as i16, yaw: e.yaw, pitch: e.pitch, on_ground: e.on_ground })));
            } else if moved {
                out.push(Bytes::from(encode_packet(&play::MoveEntityPos { entity_id: id, dx: dx as i16, dy: dy as i16, dz: dz as i16, on_ground: e.on_ground })));
            } else if rotated {
                out.push(Bytes::from(encode_packet(&play::MoveEntityRot { entity_id: VarInt(id), on_ground: e.on_ground, yaw: Angle(e.yaw), pitch: Angle(e.pitch) })));
            }
            e.sent_pos = e.pos;
            e.sent_yaw = e.yaw;
            e.sent_pitch = e.pitch;
            e.sent_on_ground = e.on_ground;
            if e.info.track_deltas && e.vel.distance_squared(e.sent_vel) > 1.0e-7 {
                out.push(Bytes::from(encode_packet(&play::SetEntityMotion { entity_id: VarInt(id), velocity: LpVec3(e.vel.x, e.vel.y, e.vel.z) })));
                e.sent_vel = e.vel;
            }
        }
        if is_mob && angle(e.head_yaw) != angle(e.sent_head_yaw) {
            out.push(Bytes::from(encode_packet(&play::RotateHead { entity_id: VarInt(id), head_yaw: Angle(e.head_yaw) })));
            e.sent_head_yaw = e.head_yaw;
        }
        if e.metadata_dirty {
            e.metadata_dirty = false;
            out.push(Bytes::from(encode_packet(&play::SetEntityData { entity_id: VarInt(id), metadata: RemainingBytes(entity_metadata(e, false)) })));
        }
        if e.equipment_dirty {
            e.equipment_dirty = false;
            if let EntityKind::Mob(m) = &e.kind {
                let equipment: Vec<(u8, Slot)> = m.equipment.iter().enumerate().map(|(i, s)| (i as u8, s.clone())).collect();
                out.push(Bytes::from(encode_packet(&play::SetEquipment { entity_id: id, equipment })));
            }
        }
        out
    }

    /// Sends a packet to everyone who sees a (non-player) entity.
    pub fn broadcast_entity(&self, world: usize, id: i32, payload: Bytes) {
        if let Some(e) = self.worlds[world].entities.get(&id) {
            for v in &e.viewers {
                if let Some(p) = self.players.get(v) {
                    p.conn.send_payload(payload.clone());
                }
            }
        }
    }

    /// Sends a packet to every player within `radius` blocks of a point.
    pub fn broadcast_near(&self, world: usize, at: Vec3, radius: f64, payload: Bytes) {
        for p in self.players.values().filter(|p| p.world == world) {
            let d = Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(at);
            if d <= radius * radius {
                p.conn.send_payload(payload.clone());
            }
        }
    }
}

fn items_fire_resistant(e: &Entity) -> bool {
    match &e.kind {
        EntityKind::Item { stack: Slot(Some(s)), .. } => paper_data::item_data::by_id(s.item as u32).fire_resistant,
        _ => false,
    }
}

/// Whether a block is listed by a tool rule (`#tag` or block names).
pub fn block_in(list: &[String], state: u32) -> bool {
    let block = blocks::block_id_of_state(state) as i32;
    list.iter().any(|b| match b.strip_prefix('#') {
        Some(tag) => paper_data::synced::tag("minecraft:block", tag).is_some_and(|ids| ids.contains(&block)),
        None => blocks::block_of_state(state).name == b,
    })
}

/// The metadata of an entity: everything non-default when spawning, or the fields that
/// change during play otherwise.
pub fn entity_metadata(e: &Entity, spawning: bool) -> Vec<u8> {
    let info = e.info;
    let mut m = Metadata::new();
    let mut shared = 0i8;
    if let EntityKind::Mob(mob) = &e.kind {
        if mob.living.fire_ticks > 0 {
            shared |= flags::ON_FIRE;
        }
    }
    if shared != 0 || !spawning {
        m = m.byte(0, shared);
    }
    if e.custom_name.is_some() {
        m = m.optional_component(2, e.custom_name.as_ref());
        if e.custom_name_visible {
            m = m.boolean(3, true);
        }
    }
    if e.no_gravity {
        m = m.boolean(5, true);
    }
    match &e.kind {
        EntityKind::Item { stack, .. } => {
            if let Some(i) = info.data_index("DATA_ITEM") {
                m = m.item_stack(i, stack);
            }
        }
        EntityKind::Mob(mob) => {
            if let Some(i) = info.data_index("DATA_HEALTH_ID") {
                m = m.float(i, mob.living.health.max(0.0));
            }
            if mob.baby {
                if let Some(i) = info.data_index("DATA_BABY_ID") {
                    m = m.boolean(i, true);
                }
            }
            if let Some(i) = info.data_index("DATA_SWELL_DIR") {
                m = m.varint(i, if mob.brain.swelling { 1 } else { -1 });
            }
            if let Some(i) = info.data_index("DATA_WOOL_ID") {
                m = m.byte(i, (mob.variant & 15) as i8 | if mob.sheared { 0x10 } else { 0 });
            }
            if let Some(i) = info.data_index("DATA_MOB_FLAGS_ID") {
                m = m.byte(i, if mob.brain.target.is_some() { 4 } else { 0 });
            }
            if let Some(merchant) = mob.merchant.as_deref() {
                if let Some(i) = info.data_index("DATA_VILLAGER_DATA") {
                    let ty = paper_data::registries::id("minecraft:villager_type", &merchant.villager_type).unwrap_or(2) as i32;
                    let profession = paper_data::registries::id("minecraft:villager_profession", &merchant.profession).unwrap_or(0) as i32;
                    m = m.villager_data(i, ty, profession, merchant.level);
                }
                if let Some(i) = info.data_index("DATA_VILLAGER_DATA_FINALIZED") {
                    m = m.boolean(i, true);
                }
                if let Some(i) = info.data_index("DATA_UNHAPPY_COUNTER") {
                    if merchant.unhappy > 0 || !spawning {
                        m = m.varint(i, merchant.unhappy);
                    }
                }
            }
        }
        EntityKind::Arrow { critical, .. } => {
            if let Some(i) = info.data_index("ID_FLAGS") {
                m = m.byte(i, *critical as i8);
            }
        }
        EntityKind::ExperienceOrb { value } => {
            if let Some(i) = info.data_index("DATA_VALUE") {
                m = m.varint(i, *value);
            }
        }
        EntityKind::Tnt { fuse } => {
            if let Some(i) = info.data_index("DATA_FUSE_ID") {
                m = m.varint(i, *fuse);
            }
        }
        _ => {}
    }
    m.finish()
}
