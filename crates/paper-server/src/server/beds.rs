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

/// The rule a bed follows where it is: its dimension's `minecraft:gameplay/bed_rule`, or
/// `straw_bed_rule` for a straw bed (which lets players sleep at night but never sets spawn,
/// and breaks when they get up).
fn bed_rule(w: &crate::world::World, state: u32) -> paper_data::BedRule {
    let dimension = paper_data::synced::dimension_type(&w.dimension).or_else(|| paper_data::synced::dimension_type("minecraft:overworld"));
    let dimension = dimension.expect("the overworld's dimension type");
    if blocks::block_of_state(state).name == "minecraft:straw_bed" {
        dimension.straw_bed_rule
    } else {
        dimension.bed_rule
    }
}

/// Whether a bed rule's "when" holds now.
fn holds(when: paper_data::BedWhen, dark: bool) -> bool {
    match when {
        paper_data::BedWhen::Always => true,
        paper_data::BedWhen::WhenDark => dark,
        paper_data::BedWhen::Never => false,
    }
}


/// Where a respawn point puts a player (see `Server::respawn_point`): the place, whether it is
/// next to a bed, and whether a point the player had was lost.
#[derive(Default)]
pub(super) struct RespawnPoint {
    pub at: Option<(usize, Location)>,
    pub bed: bool,
    pub missing: bool,
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
        // What the bed lets players do here: beds explode in the Nether and the End
        // (`destroy_on_use`).
        let rule = bed_rule(w, state);
        let explodes = rule.destroy_on_use;
        let (dx, dz) = facing_offset(blocks::property(state, "facing").unwrap_or("north"));
        let foot = head.offset(-dx, 0, -dz);
        if blocks::property(state, "occupied") == Some("true") {
            if explodes {
                self.explode_bed(world, head, foot);
            } else {
                self.action_bar(id, Component::translatable("block.minecraft.bed.occupied", vec![]));
            }
            return true;
        }
        if p.sleeping.is_some() || p.dead {
            return true;
        }
        // What stops the player, as `BedEnterResult` names it, with the message shown.
        let near = |b: BlockPos| (p.loc.x - (b.x as f64 + 0.5)).abs() <= 3.0 && (p.loc.y - b.y as f64).abs() <= 2.0 && (p.loc.z - (b.z as f64 + 0.5)).abs() <= 3.0;
        let dark = super::mobs::sky_darken(&self.worlds[world]) >= 4;
        let mut problem: Option<(&str, Option<&str>)> = if explodes {
            Some(("EXPLOSION", None))
        } else if !near(head) && !near(foot) {
            Some(("TOO_FAR_AWAY", Some("block.minecraft.bed.too_far_away")))
        } else if w.block_at(head.x, head.y + 1, head.z).is_some_and(|s| paper_data::physics::flags(s) & paper_data::physics::SOLID_BLOCK != 0) {
            Some(("OBSTRUCTED", Some("block.minecraft.bed.obstructed")))
        } else {
            None
        };
        if problem.is_none() {
            // The bed becomes the respawn point, by day too (a straw bed never does).
            if holds(rule.can_set_spawn, dark) {
                let changed = p.respawn != Some((world, head));
                if let Some(p) = self.players.get_mut(&id) {
                    p.respawn = Some((world, head));
                    // With the way the player looks (`getRespawnLocation(false)` gives it).
                    (p.respawn_yaw, p.respawn_pitch) = (p.loc.yaw, p.loc.pitch);
                    // A bed's point needs the bed: an earlier forced point's force goes.
                    p.respawn_forced = false;
                }
                if changed {
                    self.send_message(id, &Component::translatable("block.minecraft.set_spawn", vec![]));
                }
            }
            if !holds(rule.can_sleep, dark) {
                problem = Some(("NOT_POSSIBLE_NOW", Some("block.minecraft.bed.no_sleep")));
            } else if self.players[&id].game_mode != GameMode::Creative {
                let around = Aabb::new(Vec3::new(head.x as f64 + 0.5 - 8.0, head.y as f64 - 5.0, head.z as f64 + 0.5 - 8.0), Vec3::new(head.x as f64 + 0.5 + 8.0, head.y as f64 + 5.0, head.z as f64 + 0.5 + 8.0));
                let monsters = self.worlds[world].entities_in(&around).into_iter().filter_map(|e| self.worlds[world].entities.get(&e)).any(|e| e.is_alive() && e.info.category == "MONSTER" && e.bounding_box().intersects(&around));
                if monsters {
                    problem = Some(("NOT_SAFE", Some("block.minecraft.bed.not_safe")));
                }
            }
        }
        // Plugins may let the player sleep anyway, or stop it (`PlayerBedEnterEvent`).
        let result = problem.map_or("OK", |(r, _)| r);
        let message = problem.and_then(|(_, m)| m);
        match crate::plugins::events::on_bed_enter(self, id, world, head, result, dark, !explodes, message) {
            crate::plugins::events::BedUse::Allow => problem = None,
            crate::plugins::events::BedUse::Deny => return true,
            crate::plugins::events::BedUse::Default => {}
        }
        if let Some((result, message)) = problem {
            if result == "EXPLOSION" {
                self.explode_bed(world, head, foot);
            } else if let Some(message) = message {
                self.action_bar(id, Component::translatable(message, vec![]));
            }
            return true;
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

    /// A bed used where beds explode (`AbstractBedBlock.destroyOnUse`): both halves go, and a
    /// blast takes their place.
    fn explode_bed(&mut self, world: usize, head: BlockPos, foot: BlockPos) {
        self.set_block_and_update(world, head, blocks::AIR);
        if self.worlds[world].block_at(foot.x, foot.y, foot.z).is_some_and(is_bed) {
            self.set_block_and_update(world, foot, blocks::AIR);
        }
        let center = Vec3::new(head.x as f64 + 0.5, head.y as f64 + 0.5, head.z as f64 + 0.5);
        self.explode(world, center, 5.0, true, None);
    }

    /// Gets a player out of bed (`stopSleepInBed`), unless plugins keep it there
    /// (`PlayerBedLeaveEvent`).
    pub(super) fn wake_up(&mut self, id: i32) {
        let Some(head) = self.players.get(&id).and_then(|p| p.sleeping) else { return };
        if !crate::plugins::events::on_bed_leave(self, id, head) {
            return;
        }
        let Some(p) = self.players.get_mut(&id) else { return };
        let Some(head) = p.sleeping.take() else { return };
        p.sleep_ticks = 0;
        p.metadata_dirty = true;
        let world = p.world;
        let bed = self.worlds[world].block_at(head.x, head.y, head.z).filter(|s| is_bed(*s));
        if let Some(state) = bed {
            if let Some(free) = blocks::with_property(state, "occupied", "false") {
                self.set_block_and_update(world, head, free);
            }
        }
        let Some(here) = self.players.get(&id).map(|p| p.loc) else { return };
        // `LivingEntity.stopSleeping`: next to the bed (or above it), facing it; where the
        // player lies if the bed is gone.
        let target = match bed {
            Some(_) => {
                let up = self.stand_up_position(world, head, here.yaw).unwrap_or(Location { x: head.x as f64 + 0.5, y: head.y as f64 + 1.1, z: head.z as f64 + 0.5, yaw: 0.0, pitch: 0.0 });
                Location { yaw: look_at_yaw(&up, head), pitch: 0.0, ..up }
            }
            None => here,
        };
        // The player is there already when `PlayerTeleportEvent` (EXIT_BED) comes: plugins see
        // it from there, and stopping it leaves the player there, as on Paper.
        if let Some(p) = self.players.get_mut(&id) {
            p.loc = target;
        }
        self.teleport_for(id, world, target, "EXIT_BED");
        // The wake-up animation (0 since 26.3, which moved arm swings to their own packet).
        let packet = play::Animate { entity_id: VarInt(id), animation: 0 };
        self.send(id, &packet);
        self.broadcast_to_viewers(id, &packet);
        // A straw bed breaks when its sleeper gets up (`destroy_on_leave`), the other half
        // with it.
        if bed.is_some_and(|state| bed_rule(&self.worlds[world], state).destroy_on_leave) {
            self.set_block_and_update(world, head, blocks::AIR);
            let at = Vec3::new(head.x as f64 + 0.5, head.y as f64 + 0.5, head.z as f64 + 0.5);
            self.play_sound(world, at, "minecraft:block.straw_bed.break_leave", super::survival::category::BLOCKS, 1.0, 1.0, None);
        }
    }

    /// Where a player gets up from the bed whose head is at `head`, looking towards `yaw`
    /// (`BedBlock.findStandUpPosition`): beside the head on the side the player does not
    /// look towards, then on round the bed, then on the bed itself; a spot that is safe
    /// (no fire, lava, cactus, ...) if there is one, else any with room. Under a bunk bed's
    /// upper bed, the spots one block down come before the bed itself.
    pub fn stand_up_position(&self, world: usize, head: BlockPos, yaw: f32) -> Option<Location> {
        let w = &self.worlds[world];
        let state = w.block_at(head.x, head.y, head.z)?;
        let (fx, fz) = if is_bed(state) { facing_offset(blocks::property(state, "facing").unwrap_or("north")) } else { (0, 1) };
        // The side: clockwise from the bed's facing, unless the player looks that way.
        let (cx, cz) = (-fz, fx);
        // `Direction.isFacingAngle`, with `Mth`'s sine table (which is exact at right angles).
        let radians = (yaw * (std::f64::consts::PI / 180.0) as f32) as f64;
        let looks_that_way = cx as f32 * -paper_worldgen::math::mth_sin(radians) + cz as f32 * paper_worldgen::math::mth_cos(radians) > 0.0;
        let (sx, sz) = if looks_that_way { (-cx, -cz) } else { (cx, cz) };
        let around = [
            (sx, sz),
            (sx - fx, sz - fz),
            (sx - fx * 2, sz - fz * 2),
            (-fx * 2, -fz * 2),
            (-sx - fx * 2, -sz - fz * 2),
            (-sx - fx, -sz - fz),
            (-sx, -sz),
            (-sx + fx, -sz + fz),
            (fx, fz),
            (sx + fx, sz + fz),
        ];
        let on = [(0, 0), (-fx, -fz)];
        let first = |base: BlockPos, offsets: &[(i32, i32)], only_safe: bool| offsets.iter().find_map(|&(dx, dz)| safe_dismount_location(w, base.offset(dx, 0, dz), only_safe));
        let below = head.offset(0, -1, 0);
        let bunk = w.block_at(below.x, below.y, below.z).is_some_and(is_bed);
        let found = if bunk {
            [true, false].into_iter().find_map(|only_safe| first(head, &around, only_safe).or_else(|| first(below, &around, only_safe)).or_else(|| first(head, &on, only_safe)))
        } else {
            let all: Vec<(i32, i32)> = around.iter().chain(&on).copied().collect();
            [true, false].into_iter().find_map(|only_safe| first(head, &all, only_safe))
        };
        found.map(|at| Location { x: at.x, y: at.y, z: at.z, yaw: 0.0, pitch: 0.0 })
    }

    /// Where a player with a respawn point respawns (`ServerPlayer.findRespawnAndUseSpawnBlock`):
    /// next to its bed if the bed is still there and not walled in; at the block itself if the
    /// point was set with force (`setRespawnLocation(location, true)`) and there is room;
    /// otherwise nowhere, and the player is told and the point forgotten.
    pub(super) fn respawn_point(&mut self, id: i32) -> RespawnPoint {
        self.find_respawn_point(id, true)
    }

    /// Where the player would respawn, without forgetting a point that is no good
    /// (`Player.getRespawnLocation(true)`).
    pub(crate) fn peek_respawn_point(&mut self, id: i32) -> Option<(usize, Location)> {
        self.find_respawn_point(id, false).at
    }

    fn find_respawn_point(&mut self, id: i32, use_it: bool) -> RespawnPoint {
        let Some(p) = self.players.get(&id) else { return RespawnPoint::default() };
        let (forced, yaw, pitch) = (p.respawn_forced, p.respawn_yaw, p.respawn_pitch);
        let Some((world, head)) = p.respawn.filter(|(w, _)| *w < self.worlds.len() && !self.worlds[*w].unloaded) else { return RespawnPoint::default() };
        if let Some((at, bed)) = self.respawn_point_at(world, head, forced, yaw, pitch) {
            return RespawnPoint { at: Some((world, at)), bed, missing: false };
        }
        if !use_it {
            return RespawnPoint::default();
        }
        if let Some(p) = self.players.get_mut(&id) {
            p.respawn = None;
            p.respawn_forced = false;
        }
        self.send_message(id, &Component::translatable("block.minecraft.spawn.not_valid", vec![]));
        RespawnPoint { at: None, bed: false, missing: true }
    }

    /// Where a player whose respawn point is `head` in `world` respawns, and whether that is
    /// next to a bed: next to the bed, facing it, if the bed is still there and not walled in
    /// (`RespawnPosAngle.of`); at the block itself, facing `yaw` and `pitch`, if the point was
    /// set with force and there is room. `None` if the point is no good.
    pub(crate) fn respawn_point_at(&mut self, world: usize, head: BlockPos, forced: bool, yaw: f32, pitch: f32) -> Option<(Location, bool)> {
        if world >= self.worlds.len() || self.worlds[world].unloaded {
            return None;
        }
        // A bed far away is in chunks no one keeps loaded: they are loaded to look, as reading
        // the blocks does in vanilla (an unloaded chunk is not a missing bed, nor a wall
        // round it). The spots to stand on are up to three blocks away.
        for cx in (head.x - 3) >> 4..=(head.x + 3) >> 4 {
            for cz in (head.z - 3) >> 4..=(head.z + 3) >> 4 {
                let chunk = crate::world::chunk::ChunkPos::new(cx, cz);
                if !self.worlds[world].chunks.contains_key(&chunk) {
                    self.load_chunk_now(world, chunk);
                }
            }
        }
        let w = &self.worlds[world];
        // A bed sets spawn where its rule lets it (`BED_RULE`; never in the Nether, never a
        // straw bed): otherwise it is no more than a forced point's block.
        let dark = super::mobs::sky_darken(w) >= 4;
        if w.block_at(head.x, head.y, head.z).is_some_and(|s| is_bed(s) && holds(bed_rule(w, s).can_set_spawn, dark)) {
            return self.stand_up_position(world, head, yaw).map(|at| (Location { yaw: look_at_yaw(&at, head), pitch: 0.0, ..at }, true));
        }
        let room = |y: i32| w.block_at(head.x, y, head.z).is_some_and(possible_to_respawn_in);
        (forced && room(head.y) && room(head.y + 1)).then(|| (Location { x: head.x as f64 + 0.5, y: head.y as f64 + 0.1, z: head.z as f64 + 0.5, yaw, pitch }, false))
    }

    /// Sleeping players: the count shown to everyone, skipping the night when enough have
    /// slept long enough (`players_sleeping_percentage`), waking up when it is day.
    pub(super) fn tick_sleeping(&mut self) {
        for world in 0..self.worlds.len() {
            let dark = super::mobs::sky_darken(&self.worlds[world]) >= 4;
            let sleepers: Vec<i32> = self.players.values().filter(|p| p.world == world && p.sleeping.is_some()).map(|p| p.entity_id).collect();
            // A sleeper whose bed is gone gets up where it lies (`LivingEntity.tick`,
            // `checkBedExists`): no bed-leave event, no wake-up animation, no move.
            let w = &self.worlds[world];
            let (sleepers, bed_gone): (Vec<i32>, Vec<i32>) =
                sleepers.into_iter().partition(|id| self.players.get(id).and_then(|p| p.sleeping).is_some_and(|head| w.block_at(head.x, head.y, head.z).is_some_and(is_bed)));
            for id in bed_gone {
                if let Some(p) = self.players.get_mut(&id) {
                    p.sleeping = None;
                    p.sleep_ticks = 0;
                    p.metadata_dirty = true;
                }
            }
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
            // Players a plugin set to ignore sleeping (`setSleepingIgnored`) are not counted.
            let active = self.players.values().filter(|p| p.world == world && p.game_mode != GameMode::Spectator && !p.api.sleeping_ignored).count();
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
                self.broadcast_time(world);
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

/// `Block.isPossibleToRespawnInThis`: a block a player can be put in, for the feet and the
/// head at a forced respawn point. Neither solid (carpet and thin snow are not) nor a liquid
/// (water and lava themselves, not a waterlogged block); pressure plates, signs and banners
/// always do, solid as they are for fluids.
/// Where a player can stand in the block at `pos` (`DismountHelper.findSafeDismountLocation`):
/// on what is there if it is lower than a full block, else on the block below if that is a
/// full one (ladders, vines, scaffolding and open trapdoors are no floor); with room for a
/// player, all inside the world border, not in an end portal or gateway, and, if
/// `only_safe`, not in or on anything that hurts.
fn safe_dismount_location(w: &crate::world::World, pos: BlockPos, only_safe: bool) -> Option<Vec3> {
    let state = w.block_at(pos.x, pos.y, pos.z)?;
    if only_safe && dangerous(state) {
        return None;
    }
    // `nonClimbableShape`: the floor's top, if it has one.
    let top = |state: u32| {
        let climbable = crate::world::block_updates::state::in_tag!(state, "climbable")
            || crate::world::block_updates::state::in_tag!(state, "trapdoors") && blocks::property(state, "open") == Some("true");
        if climbable {
            return None;
        }
        paper_data::physics::collision_boxes(state).iter().map(|b| b.max[1]).reduce(f64::max)
    };
    let under = w.block_at(pos.x, pos.y - 1, pos.z);
    let floor = match top(state) {
        Some(top) => top,
        None => under.and_then(top).filter(|&top| top >= 1.0)? - 1.0,
    };
    if floor >= 1.0 || only_safe && floor <= 0.0 && under.is_some_and(dangerous) {
        return None;
    }
    let at = Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + floor, pos.z as f64 + 0.5);
    let player = Aabb::new(Vec3::new(at.x - 0.3, at.y, at.z - 0.3), Vec3::new(at.x + 0.3, at.y + 1.8, at.z + 0.3));
    if !crate::world::entity::block_boxes(w, &player).is_empty() {
        return None;
    }
    let portal = |state: u32| crate::world::block_updates::state::in_tag!(state, "invalid_spawn_inside");
    if portal(state) || w.block_at(pos.x, pos.y + 1, pos.z).is_some_and(portal) {
        return None;
    }
    w.border.contains_box(player.min.x, player.min.z, player.max.x, player.max.z).then_some(at)
}

/// Whether a block hurts a player in or on it (`EntityType.isBlockDangerous`).
fn dangerous(state: u32) -> bool {
    let name = blocks::block_of_state(state).name;
    crate::world::block_updates::state::in_tag!(state, "fire")
        || matches!(name, "minecraft:lava" | "minecraft:magma_block" | "minecraft:lava_cauldron" | "minecraft:wither_rose" | "minecraft:sweet_berry_bush" | "minecraft:cactus" | "minecraft:powder_snow")
        || matches!(name, "minecraft:campfire" | "minecraft:soul_campfire") && blocks::property(state, "lit") == Some("true")
}

/// The yaw from `from` towards the bottom centre of `block`, as a player getting up from a
/// bed or respawning next to it turns (`RespawnPosAngle.calculateLookAtYaw`,
/// `LivingEntity.stopSleeping`).
fn look_at_yaw(from: &Location, block: BlockPos) -> f32 {
    let (dx, dz) = (block.x as f64 + 0.5 - from.x, block.z as f64 + 0.5 - from.z);
    let yaw = (dz.atan2(dx).to_degrees() - 90.0) % 360.0;
    (if yaw >= 180.0 {
        yaw - 360.0
    } else if yaw < -180.0 {
        yaw + 360.0
    } else {
        yaw
    }) as f32
}

fn possible_to_respawn_in(state: u32) -> bool {
    let name = blocks::block_of_state(state).name;
    if name.ends_with("_pressure_plate") || name.ends_with("_sign") || name.ends_with("_banner") {
        return true;
    }
    !paper_data::physics::is_solid(state) && !matches!(name, "minecraft:water" | "minecraft:lava" | "minecraft:bubble_column")
}
