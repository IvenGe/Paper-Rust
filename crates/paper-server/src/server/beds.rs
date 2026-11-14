//! Beds (vanilla's `BedBlock` and `ServerPlayer.startSleepInBed`): setting the respawn
//! point, sleeping through the night, waking up; beds explode where the dimension's bed
//! rule says so (the Nether and the End).

use super::player::Location;
use super::Server;
use crate::config::GameMode;
use crate::world::entity::{Aabb, Vec3};
use paper_data::blocks;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::text::Component;

fn facing_offset(facing: &str) -> (i32, i32) {
    match facing {
        "north" => (0, -1),
        "south" => (0, 1),
        "west" => (-1, 0),
        _ => (1, 0),
    }
}

fn is_bed(state: u32) -> bool {
    blocks::block_of_state(state).name.ends_with("_bed")
}

impl Server {
    /// Right-click on a bed. Returns whether the click was used.
    pub(super) fn use_bed(&mut self, id: i32, pos: BlockPos) -> bool {
        let Some(p) = self.players.get(&id) else { return false };
        let world = p.world;
        let w = &self.worlds[world];
        let Some(mut state) = w.block_at(pos.x, pos.y, pos.z) else { return false };
        if !is_bed(state) {
            return false;
        }
        // Work from the head half.
        let mut head = pos;
        if blocks::property(state, "part") != Some("head") {
            let (dx, dz) = facing_offset(blocks::property(state, "facing").unwrap_or("north"));
            head = pos.offset(dx, 0, dz);
            match w.block_at(head.x, head.y, head.z) {
                Some(s) if is_bed(s) => state = s,
                _ => return true,
            }
        }
        let dimension = w.dimension.clone();
        // Beds explode outside the overworld (`bed_rule` with `explodes`).
        if dimension != "minecraft:overworld" {
            let (dx, dz) = facing_offset(blocks::property(state, "facing").unwrap_or("north"));
            let foot = head.offset(-dx, 0, -dz);
            self.set_block_and_update(world, head, blocks::AIR);
            if self.worlds[world].block_at(foot.x, foot.y, foot.z).is_some_and(is_bed) {
                self.set_block_and_update(world, foot, blocks::AIR);
            }
            let center = Vec3::new(head.x as f64 + 0.5, head.y as f64 + 0.5, head.z as f64 + 0.5);
            self.explode(world, center, 5.0, true, None);
            return true;
        }
        if blocks::property(state, "occupied") == Some("true") {
            self.action_bar(id, Component::translatable("block.minecraft.bed.occupied", vec![]));
            return true;
        }
        if p.sleeping.is_some() || p.dead {
            return true;
        }
        // Within reach of either half.
        let (dx, dz) = facing_offset(blocks::property(state, "facing").unwrap_or("north"));
        let foot = head.offset(-dx, 0, -dz);
        let near = |b: BlockPos| (p.loc.x - (b.x as f64 + 0.5)).abs() <= 3.0 && (p.loc.y - b.y as f64).abs() <= 2.0 && (p.loc.z - (b.z as f64 + 0.5)).abs() <= 3.0;
        if !near(head) && !near(foot) {
            self.action_bar(id, Component::translatable("block.minecraft.bed.too_far_away", vec![]));
            return true;
        }
        if w.block_at(head.x, head.y + 1, head.z).is_some_and(|s| paper_data::physics::flags(s) & paper_data::physics::SOLID_BLOCK != 0) {
            self.action_bar(id, Component::translatable("block.minecraft.bed.obstructed", vec![]));
            return true;
        }
        // The bed becomes the respawn point, by day too.
        let changed = p.respawn != Some((world, head));
        let creative = p.game_mode == GameMode::Creative;
        if let Some(p) = self.players.get_mut(&id) {
            p.respawn = Some((world, head));
        }
        if changed {
            self.send_message(id, &Component::translatable("block.minecraft.set_spawn", vec![]));
        }
        let dark = super::mobs::sky_darken(&self.worlds[world]) >= 4;
        if !dark {
            self.action_bar(id, Component::translatable("block.minecraft.bed.no_sleep", vec![]));
            return true;
        }
        if !creative {
            let around = Aabb::new(Vec3::new(head.x as f64 + 0.5 - 8.0, head.y as f64 - 5.0, head.z as f64 + 0.5 - 8.0), Vec3::new(head.x as f64 + 0.5 + 8.0, head.y as f64 + 5.0, head.z as f64 + 0.5 + 8.0));
            let monsters = self.worlds[world].entities_in(&around).into_iter().filter_map(|e| self.worlds[world].entities.get(&e)).any(|e| e.is_alive() && e.info.category == "MONSTER" && e.bounding_box().intersects(&around));
            if monsters {
                self.action_bar(id, Component::translatable("block.minecraft.bed.not_safe", vec![]));
                return true;
            }
        }
        // Lie down.
        if let Some(occupied) = blocks::with_property(state, "occupied", "true") {
            self.set_block_and_update(world, head, occupied);
        }
        if let Some(p) = self.players.get_mut(&id) {
            p.sleeping = Some(head);
            p.sleep_ticks = 0;
            p.metadata_dirty = true;
        }
        self.award_stat(id, super::stats::CUSTOM, "minecraft:sleep_in_bed", 1);
        let at = Location { x: head.x as f64 + 0.5, y: head.y as f64 + 0.6875, z: head.z as f64 + 0.5, ..self.players[&id].loc };
        self.teleport(id, at);
        true
    }

    /// Gets a player out of bed (`stopSleepInBed`).
    pub(super) fn wake_up(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        let Some(head) = p.sleeping.take() else { return };
        p.sleep_ticks = 0;
        p.metadata_dirty = true;
        let world = p.world;
        if let Some(state) = self.worlds[world].block_at(head.x, head.y, head.z).filter(|s| is_bed(*s)) {
            if let Some(free) = blocks::with_property(state, "occupied", "false") {
                self.set_block_and_update(world, head, free);
            }
        }
        let target = self.stand_up_position(world, head).unwrap_or(Location { x: head.x as f64 + 0.5, y: head.y as f64 + 1.0, z: head.z as f64 + 0.5, yaw: 0.0, pitch: 0.0 });
        let target = Location { yaw: self.players[&id].loc.yaw, pitch: self.players[&id].loc.pitch, ..target };
        self.teleport(id, target);
        // The wake-up animation.
        let packet = play::Animate { entity_id: VarInt(id), animation: 2 };
        self.send(id, &packet);
        self.broadcast_to_viewers(id, &packet);
    }

    /// A free spot next to a bed (`BedBlock.findStandUpPosition`, simplified): around the
    /// head, then around the foot, on solid ground with room to stand.
    pub fn stand_up_position(&self, world: usize, head: BlockPos) -> Option<Location> {
        let w = &self.worlds[world];
        let state = w.block_at(head.x, head.y, head.z)?;
        let (dx, dz) = if is_bed(state) { facing_offset(blocks::property(state, "facing").unwrap_or("north")) } else { (0, 0) };
        let foot = head.offset(-dx, 0, -dz);
        let free = |x: i32, y: i32, z: i32| w.block_at(x, y, z).is_some_and(|s| !blocks::has_collision(s) && super::survival::fluid_of(s).is_none());
        for base in [head, foot] {
            for (ox, oz) in [(0, 1), (0, -1), (1, 0), (-1, 0), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                for oy in [0, 1, -1] {
                    let (x, y, z) = (base.x + ox, base.y + oy, base.z + oz);
                    let ground = w.block_at(x, y - 1, z).is_some_and(blocks::has_collision);
                    if ground && free(x, y, z) && free(x, y + 1, z) {
                        return Some(Location { x: x as f64 + 0.5, y: y as f64, z: z as f64 + 0.5, yaw: 0.0, pitch: 0.0 });
                    }
                }
            }
        }
        None
    }

    /// The respawn point at a bed, if the bed is still there; otherwise tells the player and
    /// forgets it.
    pub(super) fn bed_respawn(&mut self, id: i32) -> Option<(usize, Location)> {
        let (world, head) = self.players.get(&id)?.respawn.filter(|(w, _)| *w < self.worlds.len())?;
        let bed = self.worlds[world].block_at(head.x, head.y, head.z).is_some_and(is_bed);
        match bed.then(|| self.stand_up_position(world, head)).flatten() {
            Some(at) => Some((world, at)),
            None => {
                if let Some(p) = self.players.get_mut(&id) {
                    p.respawn = None;
                }
                self.send_message(id, &Component::translatable("block.minecraft.spawn.not_valid", vec![]));
                None
            }
        }
    }

    /// Sleeping players: the count shown to everyone, skipping the night when enough have
    /// slept long enough (`players_sleeping_percentage`), waking up when it is day.
    pub(super) fn tick_sleeping(&mut self) {
        for world in 0..self.worlds.len() {
            let dark = super::mobs::sky_darken(&self.worlds[world]) >= 4;
            let sleepers: Vec<i32> = self.players.values().filter(|p| p.world == world && p.sleeping.is_some()).map(|p| p.entity_id).collect();
            if sleepers.is_empty() {
                continue;
            }
            if !dark {
                for id in sleepers {
                    self.wake_up(id);
                }
                continue;
            }
            for id in &sleepers {
                if let Some(p) = self.players.get_mut(id) {
                    p.sleep_ticks = (p.sleep_ticks + 1).min(100);
                }
            }
            let active = self.players.values().filter(|p| p.world == world && p.game_mode != GameMode::Spectator).count();
            let percentage = self.worlds[world].rules.int("players_sleeping_percentage").max(0) as usize;
            let needed = ((active * percentage).div_ceil(100)).max(1);
            let deep = sleepers.iter().filter(|id| self.players.get(id).is_some_and(|p| p.sleep_ticks >= 100)).count();
            if self.tick % 20 == 0 && percentage < 100 {
                let message = if sleepers.len() >= needed { Component::translatable("sleep.skipping_night", vec![]) } else { Component::translatable("sleep.players_sleeping", vec![Component::text(sleepers.len().to_string()), Component::text(needed.to_string())]) };
                let ids: Vec<i32> = self.players.values().filter(|p| p.world == world).map(|p| p.entity_id).collect();
                for id in ids {
                    self.action_bar(id, message.clone());
                }
            }
            if deep >= needed {
                // Morning.
                let w = &mut self.worlds[world];
                let next = w.day_time + 24000;
                w.day_time = next - next.rem_euclid(24000);
                let payload = self.time_payload(world);
                self.broadcast_payload_in_world(world, payload);
                // Sleeping also clears rain and thunder.
                if self.worlds[world].rules.bool("advance_weather") {
                    self.reset_weather_cycle(world);
                }
                for id in sleepers {
                    self.wake_up(id);
                }
            }
        }
    }

    fn action_bar(&self, id: i32, message: Component) {
        if let Some(p) = self.players.get(&id) {
            p.conn.send(&play::SetActionBarText { text: message });
        }
    }
}
