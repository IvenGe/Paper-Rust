//! Summoning the dragon again: end crystals placed on the four sides of the exit portal
//! (`EndCrystalItem.useOn`, `EndDragonFight.tryRespawn`) start `DragonRespawnAnimation`.
//! The crystals beam at the sky, then at each pillar in turn, which is blown up and rebuilt
//! with a new crystal, and the dragon appears; breaking one of the four stops it.

use super::Server;
use crate::config::GameMode;
use crate::world::dragon_fight::{Respawn, RespawnStage};
use crate::world::entity::{Aabb, Entity, EntityKind, Vec3};
use paper_data::{blocks, entity_types, items};
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::play::UseItemOn;
use paper_protocol::types::Slot;
use paper_worldgen::feature::Spike;
use rand::Rng;

const END_CRYSTAL: &str = "minecraft:end_crystal";

/// Where the crystals beam while the dragon is being called.
const SKY: BlockPos = BlockPos { x: 0, y: 128, z: 0 };

impl Server {
    /// An end crystal used on obsidian or bedrock with air above and nothing in the way
    /// (`EndCrystalItem.useOn`): a crystal without its base stands there, and in an End with
    /// a dragon fight the dragon may be called back. Returns whether the item was used.
    pub(super) fn use_end_crystal(&mut self, player: i32, use_on: &UseItemOn) -> bool {
        let slot = self.hand_slot(player, use_on.hand.0);
        let Some(p) = self.players.get(&player) else { return false };
        let Some(stack) = p.inventory[slot].0.clone().filter(|s| items::get(s.item as u32).is_some_and(|i| i.name == END_CRYSTAL)) else { return false };
        if !matches!(p.game_mode, GameMode::Survival | GameMode::Creative) {
            return false;
        }
        let (world, creative) = (p.world, p.game_mode == GameMode::Creative);
        let base = self.worlds[world].block_at(use_on.pos.x, use_on.pos.y, use_on.pos.z).map(|s| blocks::block_of_state(s).name);
        if !matches!(base, Some("minecraft:obsidian" | "minecraft:bedrock")) {
            return false;
        }
        let above = use_on.pos.offset(0, 1, 0);
        if !self.worlds[world].block_at(above.x, above.y, above.z).is_some_and(blocks::is_air) {
            return false;
        }
        let (x, y, z) = (above.x as f64, above.y as f64, above.z as f64);
        let area = Aabb::new(Vec3::new(x, y, z), Vec3::new(x + 1.0, y + 2.0, z + 1.0));
        let players_in_way = self.players.values().any(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator && Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8).intersects(&area));
        if players_in_way || !self.worlds[world].entities_in(&area).is_empty() {
            return false;
        }
        let Some(info) = entity_types::by_name(END_CRYSTAL) else { return false };
        let e = Entity::new(self.next_entity_id(), info, Vec3::new(x + 0.5, y, z + 0.5), EntityKind::Crystal { beam_target: None, show_bottom: false });
        let crystal = self.spawn_entity(world, e);
        // Plugins may take it back (`EntityPlaceEvent`): the item stays.
        if !crate::plugins::events::on_entity_place(self, world, crystal, player, use_on.pos, use_on.face.0.clamp(0, 5) as usize, use_on.hand.0) {
            if let Some(e) = self.worlds[world].entities.get_mut(&crystal) {
                e.removed = true;
            }
            self.resync_inventory(player);
            return true;
        }
        self.game_event(world, "minecraft:entity_place", Vec3::new(x + 0.5, y + 0.5, z + 0.5), Some(player), None);
        if self.worlds[world].dragon_fight.is_some() {
            self.try_respawn_dragon(world);
        }
        if !creative {
            let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            self.set_inventory_slot(player, slot, rest);
        }
        self.award_stat(player, "minecraft:used", END_CRYSTAL, 1);
        true
    }

    /// `EndDragonFight.tryRespawn`: once the dragon is dead, a crystal touching the block two
    /// out from the portal's centre (one up) on each of its four sides starts the respawn.
    pub(super) fn try_respawn_dragon(&mut self, world: usize) {
        let Some(fight) = self.worlds[world].dragon_fight.as_ref() else { return };
        if !fight.dragon_killed || fight.respawn.is_some() {
            return;
        }
        if fight.exit_portal.is_none() && self.find_exit_portal(world, false).is_none() {
            self.place_exit_portal(world, true);
        }
        let Some(portal) = self.worlds[world].dragon_fight.as_ref().and_then(|f| f.exit_portal) else { return };
        let mut crystals = Vec::new();
        for (dx, dz) in [(0, -2), (2, 0), (0, 2), (-2, 0)] {
            let (x, y, z) = ((portal.x + dx) as f64, (portal.y + 1) as f64, (portal.z + dz) as f64);
            let area = Aabb::new(Vec3::new(x, y, z), Vec3::new(x + 1.0, y + 1.0, z + 1.0));
            let w = &self.worlds[world];
            let found: Vec<i32> = w.entities_in(&area).into_iter().filter(|id| w.entities.get(id).is_some_and(|e| matches!(e.kind, EntityKind::Crystal { .. }))).collect();
            if found.is_empty() {
                return;
            }
            crystals.extend(found);
        }
        // `respawnDragon`: the open portal closes for the summoning.
        self.place_exit_portal(world, false);
        if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
            f.respawn = Some(Respawn { stage: RespawnStage::Start, time: 0, crystals: Some(crystals) });
        }
    }

    /// One tick of a respawn (`DragonRespawnAnimation.tick`), while players are near.
    pub(super) fn tick_dragon_respawn(&mut self, world: usize, loaded: bool) {
        let Some(respawn) = self.worlds[world].dragon_fight.as_ref().and_then(|f| f.respawn.as_ref()) else { return };
        if respawn.crystals.is_none() {
            // Read from a saved world: start over with the crystals there are.
            if !loaded {
                return;
            }
            if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
                f.respawn = None;
            }
            self.try_respawn_dragon(world);
        }
        let Some(r) = self.worlds[world].dragon_fight.as_mut().and_then(|f| f.respawn.as_mut()) else { return };
        let (stage, time) = (r.stage, r.time);
        r.time += 1;
        let crystals = r.crystals.clone().unwrap_or_default();
        match stage {
            RespawnStage::Start => {
                for c in &crystals {
                    self.set_beam(world, *c, Some(SKY));
                }
                self.set_respawn_stage(world, RespawnStage::PreparingToSummonPillars);
            }
            RespawnStage::PreparingToSummonPillars => {
                if time < 100 {
                    if matches!(time, 0 | 50 | 51 | 52) || time >= 95 {
                        self.broadcast_level_event(world, 3001, SKY, 0, false);
                    }
                } else {
                    self.set_respawn_stage(world, RespawnStage::SummoningPillars);
                }
            }
            RespawnStage::SummoningPillars => {
                let (start, end) = (time % 40 == 0, time % 40 == 39);
                if !start && !end {
                    return;
                }
                let spikes = paper_worldgen::feature::spikes_for_seed(self.worlds[world].seed);
                match spikes.get((time / 40) as usize) {
                    Some(spike) if start => {
                        let top = BlockPos::new(spike.x, spike.height + 1, spike.z);
                        for c in &crystals {
                            self.set_beam(world, *c, Some(top));
                        }
                    }
                    Some(spike) => self.rebuild_spike(world, spike),
                    None if start => self.set_respawn_stage(world, RespawnStage::SummoningDragon),
                    None => {}
                }
            }
            RespawnStage::SummoningDragon => {
                if time >= 100 {
                    // `END`: the fight goes on with a new dragon; the pillars' crystals heal
                    // it, and the four crystals blow up.
                    if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
                        f.respawn = None;
                        f.dragon_killed = false;
                    }
                    self.spawn_fight_dragon(world);
                    self.reset_spike_crystals(world);
                    for c in crystals {
                        self.set_beam(world, c, None);
                        let Some(at) = self.worlds[world].entities.get(&c).filter(|e| !e.removed).map(|e| e.pos) else { continue };
                        self.explode_with(world, at, 6.0, false, false, Some(c));
                        if let Some(e) = self.worlds[world].entities.get_mut(&c) {
                            e.removed = true;
                        }
                    }
                } else if time >= 80 {
                    self.broadcast_level_event(world, 3001, SKY, 0, false);
                } else if time == 0 {
                    for c in &crystals {
                        self.set_beam(world, *c, Some(SKY));
                    }
                } else if time < 5 {
                    self.broadcast_level_event(world, 3001, SKY, 0, false);
                }
            }
        }
    }

    /// `EndDragonFight.onCrystalDestroyed` during a respawn: one of the four was broken, so the
    /// respawn stops and the portal opens again. Returns whether it was one of them.
    pub(super) fn respawn_crystal_destroyed(&mut self, world: usize, crystal: i32) -> bool {
        let summoning = self.worlds[world].dragon_fight.as_ref().and_then(|f| f.respawn.as_ref()).is_some_and(|r| r.crystals.as_ref().is_some_and(|c| c.contains(&crystal)));
        if !summoning {
            return false;
        }
        if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
            f.respawn = None;
        }
        self.reset_spike_crystals(world);
        self.place_exit_portal(world, true);
        true
    }

    fn set_respawn_stage(&mut self, world: usize, stage: RespawnStage) {
        if let Some(r) = self.worlds[world].dragon_fight.as_mut().and_then(|f| f.respawn.as_mut()) {
            r.stage = stage;
            r.time = 0;
        }
    }

    fn set_beam(&mut self, world: usize, crystal: i32, target: Option<BlockPos>) {
        if let Some(e) = self.worlds[world].entities.get_mut(&crystal) {
            if let EntityKind::Crystal { beam_target, .. } = &mut e.kind {
                *beam_target = target;
                e.metadata_dirty = true;
            }
        }
    }

    /// `resetSpikeCrystals`: the crystals on the pillars can be broken again and stop beaming.
    fn reset_spike_crystals(&mut self, world: usize) {
        let w = &self.worlds[world];
        let mut found = Vec::new();
        for spike in paper_worldgen::feature::spikes_for_seed(w.seed) {
            let r = spike.radius as f64;
            let area = Aabb::new(Vec3::new(spike.x as f64 - r, w.min_y as f64, spike.z as f64 - r), Vec3::new(spike.x as f64 + r, w.max_y() as f64, spike.z as f64 + r));
            found.extend(w.entities_in(&area).into_iter().filter(|id| w.entities.get(id).is_some_and(|e| matches!(e.kind, EntityKind::Crystal { .. }))));
        }
        for id in found {
            if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                e.flags.invulnerable = false;
            }
            self.set_beam(world, id, None);
        }
    }

    /// A pillar torn down and built again (`SUMMONING_PILLARS`): what stands around its top
    /// goes, an explosion, then `EndSpikeFeature.placeSpike` with an unbreakable crystal
    /// beaming at the sky.
    fn rebuild_spike(&mut self, world: usize, spike: &Spike) {
        use crate::world::block_updates::flags::ALL as UPDATE;
        let (sx, sz, height) = (spike.x, spike.z, spike.height);
        for y in height - 10..=height + 10 {
            for z in sz - 10..=sz + 10 {
                for x in sx - 10..=sx + 10 {
                    self.remove_block_for_respawn(world, BlockPos::new(x, y, z));
                }
            }
        }
        self.explode_with(world, Vec3::new(sx as f64 + 0.5, height as f64, sz as f64 + 0.5), 5.0, true, false, None);
        let state = |n: &str| blocks::default_state(n).unwrap_or(blocks::AIR);
        let (obsidian, bedrock) = (state("minecraft:obsidian"), state("minecraft:bedrock"));
        let radius = spike.radius;
        let min_y = self.worlds[world].min_y;
        for z in sz - radius..=sz + radius {
            for y in min_y..=height + 10 {
                for x in sx - radius..=sx + radius {
                    let (dx, dz) = (x - sx, z - sz);
                    let pos = BlockPos::new(x, y, z);
                    if dx * dx + dz * dz <= radius * radius + 1 && y < height {
                        self.set_block_with_flags(world, pos, obsidian, UPDATE);
                    } else if y > 65 {
                        self.set_block_with_flags(world, pos, blocks::AIR, UPDATE);
                    }
                }
            }
        }
        if spike.guarded {
            let bars = state("minecraft:iron_bars");
            for dx in -2i32..=2 {
                for dz in -2i32..=2 {
                    for dy in 0..=3 {
                        let (x_side, z_side, top) = (dx.abs() == 2, dz.abs() == 2, dy == 3);
                        if !(x_side || z_side || top) {
                            continue;
                        }
                        let ns = x_side || top;
                        let we = z_side || top;
                        let mut s = bars;
                        for (side, on) in [("north", ns && dz != -2), ("south", ns && dz != 2), ("west", we && dx != -2), ("east", we && dx != 2)] {
                            s = blocks::with_property(s, side, if on { "true" } else { "false" }).unwrap_or(s);
                        }
                        self.set_block_with_flags(world, BlockPos::new(sx + dx, height + dy, sz + dz), s, UPDATE);
                    }
                }
            }
        }
        if let Some(info) = entity_types::by_name(END_CRYSTAL) {
            let mut e = Entity::new(self.next_entity_id(), info, Vec3::new(sx as f64 + 0.5, (height + 1) as f64, sz as f64 + 0.5), EntityKind::Crystal { beam_target: Some(SKY), show_bottom: true });
            e.yaw = rand::thread_rng().gen::<f32>() * 360.0;
            e.flags.invulnerable = true;
            self.spawn_entity(world, e);
        }
        let top = BlockPos::new(sx, height + 1, sz);
        self.set_block_with_flags(world, top.offset(0, -1, 0), bedrock, UPDATE);
        let fire = crate::world::block_updates::fire::fire_state_for(&self.worlds[world], top);
        self.set_block_with_flags(world, top, fire, UPDATE);
    }

    /// `Level.removeBlock`: air, or the water a waterlogged block held.
    fn remove_block_for_respawn(&mut self, world: usize, pos: BlockPos) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        if blocks::is_air(state) {
            return;
        }
        let rest = if blocks::property(state, "waterlogged") == Some("true") || blocks::block_of_state(state).name == "minecraft:water" { blocks::default_state("minecraft:water").unwrap_or(blocks::AIR) } else { blocks::AIR };
        self.set_block_with_flags(world, pos, rest, crate::world::block_updates::flags::ALL);
    }
}
