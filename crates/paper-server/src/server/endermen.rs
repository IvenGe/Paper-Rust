//! Endermen (`EnderMan`): they turn on players who look them in the eyes (unless the player
//! wears a carved pumpkin), teleport away from water, rain, daylight and projectiles (and
//! towards a far target), take damage from water, and pick up and put down blocks.

use super::survival::{self, category, Damage, Fluid};
use super::Server;
use crate::config::GameMode;
use crate::world::entity::{collides, Aabb, Vec3};
use paper_data::blocks;
use paper_protocol::codec::BlockPos;
use rand::Rng;

pub const ENDERMAN: &str = "minecraft:enderman";

/// The speed bonus of an angry enderman (`SPEED_MODIFIER_ATTACKING`).
const ATTACKING: &str = "minecraft:attacking";

/// `canSurvive` for what endermen carry: flowers on dirt, nether fungi and roots on nylium
/// or soul soil (or dirt), cacti on sand; the rest on any full block.
fn fits_on(carried: u32, below: u32) -> bool {
    let name = blocks::block_of_state(carried).name;
    let below_name = blocks::block_of_state(below).name;
    let below_id = blocks::block_id_of_state(below) as i32;
    let in_tag = |tag: &str| paper_data::synced::tag("minecraft:block", tag).is_some_and(|ids| ids.contains(&below_id));
    let dirt = in_tag("minecraft:dirt") || below_name == "minecraft:farmland";
    if name == "minecraft:cactus" {
        return matches!(below_name, "minecraft:sand" | "minecraft:red_sand" | "minecraft:suspicious_sand");
    }
    if name.ends_with("_fungus") || name.ends_with("_roots") {
        return in_tag("minecraft:nylium") || matches!(below_name, "minecraft:soul_soil" | "minecraft:mycelium") || dirt;
    }
    if name.ends_with("_mushroom") || name.ends_with("_nylium") || name == "minecraft:cactus_flower" {
        return true;
    }
    let flower = paper_data::synced::tag("minecraft:block", "minecraft:small_flowers").is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(carried) as i32)));
    !flower || dirt
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

impl Server {
    /// Whether `player` looks an enderman in the eyes (`isBeingStaredBy`): within a narrow
    /// cone that narrows with distance, nothing in between, and no carved pumpkin on.
    pub fn stares_at(&self, world: usize, enderman: i32, player: i32) -> bool {
        let Some(p) = self.players.get(&player) else { return false };
        let Some(e) = self.worlds[world].entities.get(&enderman) else { return false };
        if p.world != world || p.dead || !matches!(p.game_mode, GameMode::Survival | GameMode::Adventure) {
            return false;
        }
        let disguised = p.inventory[5].0.as_ref().is_some_and(|h| paper_data::synced::tag("minecraft:item", "minecraft:gaze_disguise_equipment").is_some_and(|ids| ids.contains(&h.item)));
        if disguised {
            return false;
        }
        let eye = Vec3::new(p.loc.x, p.loc.y + p.eye_height(), p.loc.z);
        let view = Vec3::from_rotation(p.loc.yaw, p.loc.pitch).normalize();
        let target = Vec3::new(e.pos.x, e.eye_pos().y, e.pos.z);
        let to = target.sub(eye);
        let distance = to.length();
        distance > 0.0 && dot(view, to.scale(1.0 / distance)) > 1.0 - 0.025 / distance && super::combat::clip_blocks(&self.worlds[world], eye, target).is_none()
    }

    /// `EnderMan.teleport()`: somewhere within 32 blocks.
    pub fn enderman_teleport(&mut self, world: usize, id: i32) -> bool {
        let Some(e) = self.worlds[world].entities.get(&id) else { return false };
        let pos = e.pos;
        let mut rng = rand::thread_rng();
        let x = pos.x + (rng.gen::<f64>() - 0.5) * 64.0;
        let y = pos.y + (rng.gen_range(0..64) - 32) as f64;
        let z = pos.z + (rng.gen::<f64>() - 0.5) * 64.0;
        self.enderman_teleport_to(world, id, Vec3::new(x, y, z))
    }

    /// `teleportTowards`: about sixteen blocks closer to `target`.
    pub fn enderman_teleport_towards(&mut self, world: usize, id: i32, target: Vec3) -> bool {
        let Some(e) = self.worlds[world].entities.get(&id) else { return false };
        let pos = e.pos;
        let dir = Vec3::new(pos.x - target.x, pos.y + e.info.height as f64 * 0.5 - target.y, pos.z - target.z).normalize();
        let mut rng = rand::thread_rng();
        let x = pos.x + (rng.gen::<f64>() - 0.5) * 8.0 - dir.x * 16.0;
        let y = pos.y + (rng.gen_range(0..16) - 8) as f64 - dir.y * 16.0;
        let z = pos.z + (rng.gen::<f64>() - 0.5) * 8.0 - dir.z * 16.0;
        self.enderman_teleport_to(world, id, Vec3::new(x, y, z))
    }

    /// `teleport(x, y, z)` and `randomTeleport`: down to solid ground that is not water, then
    /// only if the enderman fits there out of any liquid.
    fn enderman_teleport_to(&mut self, world: usize, id: i32, to: Vec3) -> bool {
        let w = &self.worlds[world];
        let Some(e) = w.entities.get(&id) else { return false };
        let (from, width, height) = (e.pos, e.info.width as f64, e.info.height as f64);
        let (x, z) = (to.x.floor() as i32, to.z.floor() as i32);
        let mut y = (to.y.floor() as i32).clamp(w.min_y, w.min_y + w.height - 1);
        let blocks_motion = |s: u32| blocks::has_collision(s);
        while y > w.min_y && !w.block_at(x, y, z).is_some_and(blocks_motion) {
            y -= 1;
        }
        let Some(ground) = w.block_at(x, y, z) else { return false };
        if !blocks_motion(ground) || survival::fluid_of(ground).is_some_and(|(f, _)| f == Fluid::Water) {
            return false;
        }
        let at = Vec3::new(to.x, (y + 1) as f64, to.z);
        let area = Aabb::of_entity(at, width, height);
        if collides(w, &area) || survival::box_in_fluid(w, &area, Fluid::Water) || survival::box_in_fluid(w, &area, Fluid::Lava) {
            return false;
        }
        let e = self.worlds[world].entities.get_mut(&id).unwrap();
        e.pos = at;
        e.vel = Vec3::ZERO;
        e.force_sync = true;
        if let Some(m) = e.mob_mut() {
            m.brain.path.clear();
        }
        self.worlds[world].reindex_entity(id);
        self.entity_event(world, id, 46);
        self.play_sound(world, from, "minecraft:entity.enderman.teleport", category::HOSTILE, 1.0, 1.0, None);
        self.play_sound(world, at, "minecraft:entity.enderman.teleport", category::HOSTILE, 1.0, 1.0, None);
        true
    }

    /// An enderman's own tick, before its common AI: water, daylight, players' stares and
    /// carried blocks.
    pub(super) fn tick_enderman(&mut self, world: usize, id: i32) {
        let tick = self.worlds[world].age;
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (pos, area, eye) = (e.pos, e.bounding_box(), e.eye_pos());
        let Some(mob) = e.mob() else { return };
        if mob.living.is_dead() {
            return;
        }
        let w = &self.worlds[world];
        let wet = survival::box_in_fluid(w, &area, Fluid::Water) || w.rain_at(pos.x.floor() as i32, eye.y.floor() as i32, pos.z.floor() as i32);
        let mut rng = rand::thread_rng();

        // Water hurts (`isSensitiveToWater`), and being hurt by anything but a mob makes it
        // teleport most of the time (see `hurt_entity`).
        if wet {
            self.hurt_entity(world, id, Damage::new("minecraft:drown"), 1.0);
            if self.worlds[world].entities.get(&id).is_none_or(|e| !e.is_alive()) {
                return;
            }
        }

        // Daylight under the open sky makes it teleport away and calm down.
        let w = &self.worlds[world];
        let Some(e) = w.entities.get(&id) else { return };
        let Some(mob) = e.mob() else { return };
        let day = w.has_skylight && w.day_time.rem_euclid(24000) < 12000;
        if day && tick >= mob.brain.target_changed + 600 {
            let (x, y, z) = (eye.x.floor() as i32, eye.y.floor() as i32, eye.z.floor() as i32);
            let brightness = super::mobs::raw_brightness(w, x, y, z) as f32 / 15.0;
            if brightness > 0.5 && super::mobs::sky_light(w, x, y, z) >= 15 && rng.gen::<f32>() * 30.0 < (brightness - 0.4) * 2.0 {
                self.set_enderman_target(world, id, None);
                self.enderman_teleport(world, id);
            }
        }

        // `EndermanLookForPlayerGoal`: a stare makes it angry after a moment; an angry one
        // teleports away when stared at up close and towards its target from afar.
        let Some(mob) = self.worlds[world].entities.get(&id).and_then(|e| e.mob()) else { return };
        let (target, pending, follow) = (mob.brain.target, mob.brain.stare_target, mob.attributes.value("follow_range"));
        match (target, pending) {
            (None, None) => {
                let starer = self
                    .players
                    .values()
                    .filter(|p| p.world == world && Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(pos) < follow * follow)
                    .map(|p| p.entity_id)
                    .find(|p| self.stares_at(world, id, *p));
                if let Some(p) = starer {
                    if let Some(mob) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                        mob.brain.stare_target = Some(p);
                        mob.brain.stare_time = 5;
                    }
                    if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                        e.metadata_dirty = true;
                        if let Some(m) = e.mob_mut() {
                            m.stared_at = true;
                        }
                    }
                }
            }
            (None, Some(p)) => {
                if !self.stares_at(world, id, p) {
                    if let Some(mob) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                        mob.brain.stare_target = None;
                    }
                } else {
                    let ready = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()).map(|m| {
                        m.brain.stare_time -= 1;
                        m.brain.stare_time <= 0
                    });
                    if ready == Some(true) {
                        self.set_enderman_target(world, id, Some(p));
                        self.play_sound(world, pos, "minecraft:entity.enderman.stare", category::HOSTILE, 2.5, 1.0, None);
                    }
                }
            }
            (Some(t), _) => {
                if let Some(p) = self.players.get(&t) {
                    let at = Vec3::new(p.loc.x, p.loc.y, p.loc.z);
                    if self.stares_at(world, id, t) {
                        if at.distance_squared(pos) < 16.0 {
                            self.enderman_teleport(world, id);
                        }
                        if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                            m.brain.teleport_time = 0;
                        }
                    } else if at.distance_squared(pos) > 256.0 {
                        let due = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()).map(|m| {
                            m.brain.teleport_time += 1;
                            m.brain.teleport_time >= 30
                        });
                        if due == Some(true) && self.enderman_teleport_towards(world, id, Vec3::new(at.x, at.y + p.eye_height(), at.z)) {
                            if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                                m.brain.teleport_time = 0;
                            }
                        }
                    }
                }
            }
        }
        self.sync_enderman_anger(world, id);
        if self.worlds[world].rules.bool("mob_griefing") {
            self.enderman_blocks(world, id);
        }
    }

    /// `EnderMan.setTarget`: angry ones scream (`DATA_CREEPY`) and run faster.
    pub(super) fn set_enderman_target(&mut self, world: usize, id: i32, target: Option<i32>) {
        let tick = self.worlds[world].age;
        if let Some(mob) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
            mob.brain.target = target;
            mob.brain.stare_target = None;
            mob.brain.target_changed = tick;
            mob.brain.path.clear();
        }
        self.sync_enderman_anger(world, id);
    }

    /// Keeps the scream and the speed bonus in step with whether it has a target.
    fn sync_enderman_anger(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let Some(mob) = e.mob_mut() else { return };
        let angry = mob.brain.target.is_some();
        if mob.creepy != angry {
            mob.creepy = angry;
            if !angry {
                mob.stared_at = false;
            }
            if angry {
                mob.attributes.add_from("movement_speed", crate::world::attributes::Modifier::new(ATTACKING, 0.15, crate::world::attributes::Operation::AddValue), crate::world::attributes::Source::Transient);
            } else {
                mob.attributes.remove_modifier("movement_speed", ATTACKING);
            }
            e.metadata_dirty = true;
        }
    }

    /// `EndermanTakeBlockGoal` / `EndermanLeaveBlockGoal`: now and then it picks up a block it
    /// can see nearby, and much later puts it down on solid ground.
    fn enderman_blocks(&mut self, world: usize, id: i32) {
        let mut rng = rand::thread_rng();
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (pos, eye) = (e.pos, e.eye_pos());
        let Some(mob) = e.mob() else { return };
        let w = &self.worlds[world];
        match mob.carried {
            None => {
                if rng.gen_range(0..20) != 0 {
                    return;
                }
                let x = (pos.x - 2.0 + rng.gen::<f64>() * 4.0).floor() as i32;
                let y = (pos.y + rng.gen::<f64>() * 3.0).floor() as i32;
                let z = (pos.z - 2.0 + rng.gen::<f64>() * 4.0).floor() as i32;
                let Some(state) = w.block_at(x, y, z) else { return };
                let block = blocks::block_id_of_state(state) as i32;
                if !paper_data::synced::tag("minecraft:block", "minecraft:enderman_holdable").is_some_and(|ids| ids.contains(&block)) {
                    return;
                }
                // It must see the block: the first block on the way is this one.
                let center = Vec3::new(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5);
                if super::combat::clip_blocks(w, eye, center).is_some_and(|(_, hit)| hit != BlockPos::new(x, y, z)) {
                    return;
                }
                let pos = BlockPos::new(x, y, z);
                self.set_block_and_update(world, pos, blocks::AIR);
                if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                    if let Some(m) = e.mob_mut() {
                        m.carried = Some(blocks::default_state(blocks::block_of_state(state).name).unwrap_or(state));
                    }
                    e.metadata_dirty = true;
                }
            }
            Some(carried) => {
                if rng.gen_range(0..2000) != 0 {
                    return;
                }
                let x = (pos.x - 1.0 + rng.gen::<f64>() * 2.0).floor() as i32;
                let y = (pos.y + rng.gen::<f64>() * 2.0).floor() as i32;
                let z = (pos.z - 1.0 + rng.gen::<f64>() * 2.0).floor() as i32;
                let (Some(here), Some(below)) = (w.block_at(x, y, z), w.block_at(x, y - 1, z)) else { return };
                let entities_in_way = !w.entities_in(&Aabb::new(Vec3::new(x as f64, y as f64, z as f64), Vec3::new(x as f64 + 1.0, y as f64 + 1.0, z as f64 + 1.0))).into_iter().all(|o| o == id);
                // `isCollisionShapeFullBlock` below, and the block must be able to stay there.
                if !blocks::is_air(here) || !paper_data::physics::is_face_sturdy(below, 1) || entities_in_way || !fits_on(carried, below) {
                    return;
                }
                self.set_block_and_update(world, BlockPos::new(x, y, z), carried);
                if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                    if let Some(m) = e.mob_mut() {
                        m.carried = None;
                    }
                    e.metadata_dirty = true;
                }
            }
        }
    }

    /// `EnderMan.hurtServer` for projectiles: it dodges by teleporting (the arrow misses).
    /// Returns true if it got away.
    pub(super) fn enderman_dodges(&mut self, world: usize, id: i32) -> bool {
        (0..64).any(|_| self.enderman_teleport(world, id))
    }
}
