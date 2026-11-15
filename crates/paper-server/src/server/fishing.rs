//! Fishing (vanilla's `FishingRodItem` and `FishingHook`): a cast bobber flies until it hooks
//! what it hits or lands in water, where after a wait a fish is lured, swims up and bites;
//! reeling in then catches it (the `gameplay/fishing` loot table, with Luck of the Sea and
//! open-water treasure), pulls a hooked entity, or just brings the bobber back, wearing the
//! rod. Plugins hear of each step (`PlayerFishEvent`) and may change the hook's waits
//! (Bukkit's `FishHook`).

use super::survival::{category, Fluid};
use super::Server;
use crate::config::GameMode;
use crate::plugins::events;
use crate::world::entity::{Aabb, Entity, EntityKind, FishingHook, HookState, Vec3};
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use rand::Rng;

pub const BOBBER: &str = "minecraft:fishing_bobber";
const ROD: &str = "minecraft:fishing_rod";

/// `Mth.nextInt`: a whole number in `min..=max` (`min` when the range is empty).
fn next_int(rng: &mut impl Rng, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        rng.gen_range(min..=max)
    }
}

/// `RandomSource.triangle`: around `mode`, at most `deviation` away.
fn triangle(rng: &mut impl Rng, mode: f64, deviation: f64) -> f64 {
    mode + deviation * (rng.gen::<f64>() - rng.gen::<f64>())
}

/// The kinds of blocks around a bobber, for open water (`FishingHook.OpenWaterType`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenWater {
    AboveWater,
    InsideWater,
    Invalid,
}

impl Server {
    fn holds_rod(&self, player: i32) -> bool {
        self.players.get(&player).is_some_and(|p| {
            let rod = |s: &Slot| s.0.as_ref().and_then(|s| items::get(s.item as u32)).is_some_and(|i| i.name == ROD);
            rod(p.held_item()) || rod(&p.inventory[super::player::OFFHAND_SLOT])
        })
    }

    /// A fishing rod used (`FishingRodItem.use`): reels in the bobber that is out, else casts
    /// one, unless plugins stop it (`PlayerFishEvent` `FISHING`). Returns whether the rod
    /// was used.
    pub(super) fn use_fishing_rod(&mut self, player: i32, hand: i32) -> bool {
        let slot = self.hand_slot(player, hand);
        let Some(p) = self.players.get(&player) else { return false };
        let Some(rod) = p.inventory[slot].0.clone().filter(|s| items::get(s.item as u32).is_some_and(|i| i.name == ROD)) else { return false };
        let (world, at, creative) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z), p.game_mode == GameMode::Creative);
        let mut rng = rand::thread_rng();
        let out = p.fishing.filter(|h| self.worlds[world].entities.get(h).is_some_and(|e| !e.removed && matches!(e.kind, EntityKind::Fishing(_))));
        if let Some(hook) = out {
            let wear = self.retrieve_hook(world, hook, &Slot(Some(rod)));
            if wear > 0 && !creative {
                self.wear_player_item(player, slot, wear, false);
                let worn = self.players[&player].inventory[slot].clone();
                self.set_inventory_slot(player, slot, worn);
            }
            let pitch = 0.4 / (rng.gen::<f32>() * 0.4 + 0.8);
            self.play_sound(world, at, "minecraft:entity.fishing_bobber.retrieve", category::NEUTRAL, 1.0, pitch, None);
            return true;
        }
        // Lure takes 5 seconds off each wait a level; Luck of the Sea brings better catches.
        let rod_slot = Slot(Some(rod));
        let lure = super::loot::enchantment_level(Some(&rod_slot), "minecraft:lure") * 100;
        let luck = super::loot::enchantment_level(Some(&rod_slot), "minecraft:luck_of_the_sea");
        let Some(e) = self.new_bobber(player, luck, lure) else { return false };
        let id = e.id;
        self.spawn_entity(world, e);
        if let Some(p) = self.players.get_mut(&player) {
            p.fishing = Some(id);
        }
        if events::on_fish(self, world, player, None, id, Some(hand), "FISHING", 0).is_none() {
            self.worlds[world].remove_entity(id);
            if let Some(p) = self.players.get_mut(&player) {
                p.fishing = None;
            }
            return true;
        }
        let pitch = 0.4 / (rng.gen::<f32>() * 0.4 + 0.8);
        self.play_sound(world, at, "minecraft:entity.fishing_bobber.throw", category::NEUTRAL, 0.5, pitch, None);
        // It is launched as any projectile (`ProjectileLaunchEvent`).
        if !self.launch(world, id) {
            if let Some(p) = self.players.get_mut(&player) {
                p.fishing = None;
            }
            return true;
        }
        self.award_stat(player, "minecraft:used", ROD, 1);
        true
    }

    /// A bobber cast by `player` (`FishingHook(Player, ...)`): from beside their eyes, the way
    /// they look, a little off at random.
    fn new_bobber(&self, player: i32, luck: i32, lure_speed: i32) -> Option<Entity> {
        let p = self.players.get(&player)?;
        let info = paper_data::entity_types::by_name(BOBBER)?;
        let (yaw, pitch) = (p.loc.yaw, p.loc.pitch);
        let cos_y = (-yaw.to_radians() - std::f32::consts::PI).cos() as f64;
        let sin_y = (-yaw.to_radians() - std::f32::consts::PI).sin() as f64;
        let cos_x = -(-pitch.to_radians()).cos() as f64;
        let sin_x = (-pitch.to_radians()).sin() as f64;
        let at = Vec3::new(p.loc.x - sin_y * 0.3, p.loc.y + p.eye_height(), p.loc.z - cos_y * 0.3);
        let mut rng = rand::thread_rng();
        let dir = Vec3::new(-sin_y, (-(sin_x / cos_x)).clamp(-5.0, 5.0), -cos_y);
        let length = dir.length();
        let mut scale = || 0.6 / length + triangle(&mut rng, 0.5, 0.0103365);
        let vel = Vec3::new(dir.x * scale(), dir.y * scale(), dir.z * scale());
        let mut e = Entity::new(self.next_entity_id(), info, at, EntityKind::Fishing(Box::new(FishingHook::new(player, luck, lure_speed))));
        e.vel = vel;
        e.yaw = (vel.x.atan2(vel.z).to_degrees()) as f32;
        e.pitch = (vel.y.atan2((vel.x * vel.x + vel.z * vel.z).sqrt()).to_degrees()) as f32;
        e.head_yaw = e.yaw;
        Some(e)
    }

    /// Takes a bobber away, and from its player.
    fn discard_hook(&mut self, world: usize, id: i32) {
        let owner = self.worlds[world].entities.get(&id).and_then(|e| match &e.kind {
            EntityKind::Fishing(h) => Some(h.owner),
            _ => None,
        });
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            e.removed = true;
        }
        if let Some(p) = owner.and_then(|o| self.players.get_mut(&o)) {
            if p.fishing == Some(id) {
                p.fishing = None;
            }
        }
    }

    /// A bobber's tick (`FishingHook.tick`).
    pub(super) fn tick_fishing(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let EntityKind::Fishing(h) = &e.kind else { return };
        let (owner, pos) = (h.owner, e.pos);
        // It goes when its player does, dies, leaves, lets go of the rod or is over 32 blocks
        // away (`shouldStopFishing`).
        let near = self.players.get(&owner).is_some_and(|p| p.world == world && !p.dead && Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(pos) <= 1024.0);
        if !near || !self.holds_rod(owner) {
            self.discard_hook(world, id);
            return;
        }
        let w = &self.worlds[world];
        let block = BlockPos::new(pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32);
        let fluid_height = match super::survival::fluid_surface(w, block.x, block.y, block.z) {
            Some((Fluid::Water, surface)) => surface - block.y as f64,
            _ => 0.0,
        };
        let in_water = fluid_height > 0.0;
        let mut rng = rand::thread_rng();
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let on_ground = e.on_ground;
        let EntityKind::Fishing(h) = &mut e.kind else { return };
        if on_ground {
            h.life += 1;
            if h.life >= 1200 {
                self.discard_hook(world, id);
                return;
            }
        } else {
            h.life = 0;
        }
        match h.state {
            HookState::Flying => {
                if h.hooked.is_some() {
                    h.state = HookState::HookedInEntity;
                    e.vel = Vec3::ZERO;
                    return;
                }
                if in_water {
                    h.state = HookState::Bobbing;
                    e.vel = Vec3::new(e.vel.x * 0.3, e.vel.y * 0.2, e.vel.z * 0.3);
                    return;
                }
                self.hook_collisions(world, id);
            }
            HookState::HookedInEntity => {
                // It sits in what it hooked, until that is gone.
                let hooked = h.hooked;
                let at = hooked.and_then(|t| match self.players.get(&t) {
                    Some(p) if p.world == world && !p.dead => Some(Vec3::new(p.loc.x, p.loc.y + 1.8 * 0.8, p.loc.z)),
                    Some(_) => None,
                    None => self.worlds[world].entities.get(&t).filter(|t| !t.removed && t.is_alive()).map(|t| Vec3::new(t.pos.x, t.pos.y + t.info.height as f64 * 0.8, t.pos.z)),
                });
                let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
                match at {
                    Some(at) => e.pos = at,
                    None => {
                        if let EntityKind::Fishing(h) = &mut e.kind {
                            h.hooked = None;
                            h.state = HookState::Flying;
                        }
                        e.metadata_dirty = true;
                    }
                }
                self.worlds[world].reindex_entity(id);
                return;
            }
            HookState::Bobbing => {
                let float = pos.y + e.vel.y - block.y as f64 - fluid_height;
                let float = if float.abs() < 0.01 { float + float.signum() * 0.1 } else { float };
                e.vel = Vec3::new(e.vel.x * 0.9, e.vel.y - float * rng.gen::<f64>() * 0.2, e.vel.z * 0.9);
                let (nibble, until_hooked, open, out_of_water) = (h.nibble, h.time_until_hooked, h.open_water, h.out_of_water_time);
                let open_water = if nibble <= 0 && until_hooked <= 0 { true } else { open && out_of_water < 10 && self.calculate_open_water(world, block) };
                let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
                let EntityKind::Fishing(h) = &mut e.kind else { return };
                h.open_water = open_water;
                if in_water {
                    h.out_of_water_time = (h.out_of_water_time - 1).max(0);
                    if h.biting {
                        e.vel.y -= 0.1 * rng.gen::<f64>() * rng.gen::<f64>();
                    }
                    self.catching_fish(world, id, block);
                } else {
                    h.out_of_water_time = (h.out_of_water_time + 1).min(10);
                }
            }
        }
        // Then it moves: down through the air, along with the water.
        let Some(mut e) = self.worlds[world].entities.remove(&id) else { return };
        if !in_water {
            e.vel.y -= 0.03;
        }
        let d = e.vel;
        let m = e.move_by(&self.worlds[world], d, 0.0);
        if let EntityKind::Fishing(h) = &e.kind {
            if h.state == HookState::Flying && (m.on_ground || m.collided_x || m.collided_z) {
                e.vel = Vec3::ZERO;
            }
        }
        e.vel = e.vel.scale(0.92);
        if e.vel.length_squared() > 1.0e-7 {
            e.yaw = (e.vel.x.atan2(e.vel.z).to_degrees()) as f32;
            e.pitch = (e.vel.y.atan2((e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt()).to_degrees()) as f32;
        }
        self.worlds[world].entities.insert(id, e);
        self.worlds[world].reindex_entity(id);
    }

    /// While flying, the bobber hooks the first thing on its way (`checkCollision`; plugins may
    /// let it pass, `ProjectileHitEvent`), or stops short of a block.
    fn hook_collisions(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let EntityKind::Fishing(h) = &e.kind else { return };
        let (owner, from, vel) = (h.owner, e.pos, e.vel);
        let w = &self.worlds[world];
        let to = from.add(vel);
        let block_hit = super::combat::clip_blocks(w, from, to);
        let end = block_hit.map_or(to, |(t, _)| from.add(vel.scale(t)));
        let swept = Aabb::of_entity(from, 0.25, 0.25).expand_towards(vel).inflate(1.0, 1.0, 1.0);
        let mut target: Option<(f64, i32)> = None;
        let mut consider = |cand: i32, b: Aabb| {
            if let Some(t) = b.inflate(0.3, 0.3, 0.3).clip_ray(from, end) {
                if target.is_none_or(|(best, _)| t < best) {
                    target = Some((t, cand));
                }
            }
        };
        for p in self.players.values().filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator && p.entity_id != owner) {
            let b = Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8);
            if b.intersects(&swept) {
                consider(p.entity_id, b);
            }
        }
        for cand in w.entities_in(&swept) {
            let Some(c) = w.entities.get(&cand).filter(|c| cand != id && !c.removed) else { continue };
            let hookable = match &c.kind {
                EntityKind::Mob(m) => !m.living.is_dead(),
                EntityKind::Item { .. } | EntityKind::Boat { .. } => true,
                _ => false,
            };
            if hookable {
                consider(cand, c.bounding_box());
            }
        }
        let target = target.filter(|(_, victim)| events::on_projectile_hit(self, world, id, Some(*victim), None));
        if let Some((_, victim)) = target {
            if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                if let EntityKind::Fishing(h) = &mut e.kind {
                    h.hooked = Some(victim);
                }
                e.metadata_dirty = true;
            }
            return;
        }
        // `onHitBlock`: it slows to reach the block.
        if let Some((t, _)) = block_hit {
            if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                let length = vel.length();
                if length > 1.0e-9 {
                    e.vel = vel.scale(t);
                }
            }
        }
    }

    /// `FishingHook.calculateOpenWater`: from a layer below to two above, 5x5 layers of only
    /// water (sources without collision) or only air or lily pads, the air above the water.
    fn calculate_open_water(&self, world: usize, at: BlockPos) -> bool {
        let w = &self.worlds[world];
        let kind = |x: i32, y: i32, z: i32| -> OpenWater {
            let Some(state) = w.block_at(x, y, z) else { return OpenWater::Invalid };
            let name = blocks::block_of_state(state).name;
            if blocks::is_air(state) || name == "minecraft:lily_pad" {
                return OpenWater::AboveWater;
            }
            let source = name == "minecraft:water" && blocks::property(state, "level").is_none_or(|l| l == "0");
            if source && !blocks::has_collision(state) {
                OpenWater::InsideWater
            } else {
                OpenWater::Invalid
            }
        };
        let mut previous = OpenWater::Invalid;
        for dy in -1..=2 {
            let mut layer: Option<OpenWater> = None;
            for dx in -2..=2 {
                for dz in -2..=2 {
                    let k = kind(at.x + dx, at.y + dy, at.z + dz);
                    layer = Some(match layer {
                        None => k,
                        Some(l) if l == k => l,
                        Some(_) => OpenWater::Invalid,
                    });
                }
            }
            match layer.unwrap_or(OpenWater::Invalid) {
                OpenWater::AboveWater if previous == OpenWater::Invalid => return false,
                OpenWater::InsideWater if previous == OpenWater::AboveWater => return false,
                OpenWater::Invalid => return false,
                layer => previous = layer,
            }
        }
        true
    }

    /// A bobber in water (`catchingFish`): a fish is lured after a wait (shorter in rain,
    /// longer under a roof and with Lure shorter still), swims up to it and bites; missed, it
    /// swims off. Plugins hear of each (`LURED`, `BITE`, `FAILED_ATTEMPT`).
    fn catching_fish(&mut self, world: usize, id: i32, block: BlockPos) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let EntityKind::Fishing(h) = &e.kind else { return };
        let mut h = (**h).clone();
        let (pos, width) = (e.pos, e.info.width as f64);
        let mut rng = rand::thread_rng();
        let w = &self.worlds[world];
        let mut speed = 1;
        if h.rain_influenced && rng.gen::<f32>() < 0.25 && w.rain_at(block.x, block.y + 1, block.z) {
            speed += 1;
        }
        if h.sky_influenced && rng.gen::<f32>() < 0.5 && w.top_y(block.x, block.z).is_some_and(|top| block.y + 1 < top) {
            speed -= 1;
        }
        let store = |server: &mut Server, h: FishingHook| {
            if let Some(e) = server.worlds[world].entities.get_mut(&id) {
                if let EntityKind::Fishing(old) = &mut e.kind {
                    if old.biting != h.biting {
                        e.metadata_dirty = true;
                    }
                    **old = h;
                }
            }
        };
        if h.nibble > 0 {
            h.nibble -= 1;
            if h.nibble <= 0 {
                h.time_until_lured = 0;
                h.time_until_hooked = 0;
                h.biting = false;
                store(self, h.clone());
                events::on_fish(self, world, h.owner, None, id, None, "FAILED_ATTEMPT", 0);
                return;
            }
        } else if h.time_until_hooked > 0 {
            h.time_until_hooked -= speed;
            if h.time_until_hooked > 0 {
                // The fish's wake, coming in.
                h.fish_angle += triangle(&mut rng, 0.0, 9.188) as f32;
                let angle = h.fish_angle.to_radians();
                let (sin, cos) = (angle.sin() as f64, angle.cos() as f64);
                let x = pos.x + sin * h.time_until_hooked as f64 * 0.1;
                let y = pos.y.floor() + 1.0;
                let z = pos.z + cos * h.time_until_hooked as f64 * 0.1;
                let water = w.block_at(x.floor() as i32, (y - 1.0).floor() as i32, z.floor() as i32).is_some_and(|s| blocks::block_of_state(s).name == "minecraft:water");
                if water {
                    if rng.gen::<f32>() < 0.15 {
                        self.particles(world, "minecraft:bubble", Vec3::new(x, y - 0.1, z), 1, (sin as f32, 0.1, cos as f32), 0.0);
                    }
                    let (xa, za) = (sin as f32 * 0.04, cos as f32 * 0.04);
                    self.particles(world, "minecraft:fishing", Vec3::new(x, y, z), 0, (za, 0.01, -xa), 1.0);
                    self.particles(world, "minecraft:fishing", Vec3::new(x, y, z), 0, (-za, 0.01, xa), 1.0);
                }
            } else {
                // It bites, unless plugins say not (`BITE`).
                store(self, h.clone());
                if events::on_fish(self, world, h.owner, None, id, None, "BITE", 0).is_none() {
                    return;
                }
                let Some(e) = self.worlds[world].entities.get(&id) else { return };
                let EntityKind::Fishing(fresh) = &e.kind else { return };
                h = (**fresh).clone();
                let pitch = 1.0 + (rng.gen::<f32>() - rng.gen::<f32>()) * 0.4;
                self.play_sound(world, pos, "minecraft:entity.fishing_bobber.splash", category::NEUTRAL, 0.25, pitch, None);
                let at = Vec3::new(pos.x, pos.y + 0.5, pos.z);
                let count = (1.0 + width * 20.0) as i32;
                self.particles(world, "minecraft:bubble", at, count, (width as f32, 0.0, width as f32), 0.2);
                self.particles(world, "minecraft:fishing", at, count, (width as f32, 0.0, width as f32), 0.2);
                h.nibble = next_int(&mut rng, 20, 40);
                h.biting = true;
            }
        } else if h.time_until_lured > 0 {
            h.time_until_lured -= speed;
            let mut chance = 0.15f32;
            if h.time_until_lured < 20 {
                chance += (20 - h.time_until_lured) as f32 * 0.05;
            } else if h.time_until_lured < 40 {
                chance += (40 - h.time_until_lured) as f32 * 0.02;
            } else if h.time_until_lured < 60 {
                chance += (60 - h.time_until_lured) as f32 * 0.01;
            }
            if rng.gen::<f32>() < chance {
                let angle = rng.gen_range(0.0f32..360.0).to_radians();
                let distance = rng.gen_range(25.0f32..60.0);
                let (x, y, z) = (pos.x + (angle.sin() * distance * 0.1) as f64, pos.y.floor() + 1.0, pos.z + (angle.cos() * distance * 0.1) as f64);
                if w.block_at(x.floor() as i32, (y - 1.0).floor() as i32, z.floor() as i32).is_some_and(|s| blocks::block_of_state(s).name == "minecraft:water") {
                    self.particles(world, "minecraft:splash", Vec3::new(x, y, z), 2 + rng.gen_range(0..2), (0.1, 0.0, 0.1), 0.0);
                }
            }
            if h.time_until_lured <= 0 {
                // A fish comes, unless plugins say not (`LURED`).
                h.fish_angle = if h.min_lure_angle < h.max_lure_angle { rng.gen_range(h.min_lure_angle..h.max_lure_angle) } else { h.min_lure_angle };
                h.time_until_hooked = next_int(&mut rng, h.min_lure, h.max_lure);
                store(self, h.clone());
                if events::on_fish(self, world, h.owner, None, id, None, "LURED", 0).is_none() {
                    if let Some(EntityKind::Fishing(fresh)) = self.worlds[world].entities.get_mut(&id).map(|e| &mut e.kind) {
                        fresh.time_until_hooked = 0;
                    }
                }
                return;
            }
        } else {
            reset_time_until_lured(&mut h, &mut rng);
        }
        store(self, h);
    }

    /// Reeling in (`FishingHook.retrieve`): a hooked entity is pulled closer, a biting fish
    /// caught (with 1-6 experience), a bobber on the ground pulled out; plugins may stop each
    /// (`CAUGHT_ENTITY`, `CAUGHT_FISH`, `IN_GROUND`, or `REEL_IN` when there was nothing), and
    /// then the bobber stays out. Returns how much the rod wears.
    fn retrieve_hook(&mut self, world: usize, hook: i32, rod: &Slot) -> i32 {
        let Some(e) = self.worlds[world].entities.get(&hook) else { return 0 };
        let EntityKind::Fishing(h) = &e.kind else { return 0 };
        let (owner, hooked, nibble, open_water, luck, pos, on_ground) = (h.owner, h.hooked, h.nibble, h.open_water, h.luck, e.pos, e.on_ground);
        let Some(p) = self.players.get(&owner) else { return 0 };
        let owner_at = Vec3::new(p.loc.x, p.loc.y, p.loc.z);
        let owner_luck = p.attributes.value("luck") as f32;
        let mut wear = 0;
        if let Some(target) = hooked {
            if events::on_fish(self, world, owner, Some(target), hook, None, "CAUGHT_ENTITY", 0).is_none() {
                return 0;
            }
            let hooked = self.worlds[world].entities.get(&hook).and_then(|e| match &e.kind {
                EntityKind::Fishing(h) => h.hooked,
                _ => None,
            });
            if let Some(target) = hooked {
                self.pull_hooked(world, hook);
                self.entity_event(world, hook, 31);
                let item = self.worlds[world].entities.get(&target).is_some_and(|t| matches!(t.kind, EntityKind::Item { .. }));
                wear = if item { 3 } else { 5 };
            }
        } else if nibble > 0 {
            let biome = {
                let b = BlockPos::new(pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32);
                self.biome_at(world, b.x, b.y, b.z)
            };
            let catch = if self.loot.available() {
                let ctx = super::loot::LootContext { tool: Some(rod), luck: luck as f32 + owner_luck, in_open_water: Some(open_water), biome: Some(biome), ..Default::default() };
                self.loot.roll("minecraft:gameplay/fishing", &ctx)
            } else {
                items::id("minecraft:cod").map(|cod| vec![Slot::of(cod as i32, 1)]).unwrap_or_default()
            };
            let fishes = paper_data::synced::tag("minecraft:item", "minecraft:fishes").unwrap_or(&[]);
            let mut rng = rand::thread_rng();
            for stack in catch {
                let (dx, dy, dz) = (owner_at.x - pos.x, owner_at.y - pos.y, owner_at.z - pos.z);
                let vel = Vec3::new(dx * 0.1, dy * 0.1 + (dx * dx + dy * dy + dz * dz).sqrt().sqrt() * 0.08, dz * 0.1);
                let item = stack.0.as_ref().map(|s| s.item);
                let Some(caught) = self.spawn_item_unannounced(world, pos, stack, vel, 10) else { continue };
                // Plugins may change the catch or its experience, or keep the fish
                // (`CAUGHT_FISH`; the bobber then stays out).
                let Some(exp) = events::on_fish(self, world, owner, Some(caught), hook, None, "CAUGHT_FISH", rng.gen_range(1..=6)) else {
                    self.worlds[world].remove_entity(caught);
                    return 0;
                };
                self.keep_spawned_item(world, caught);
                if exp > 0 {
                    self.award_experience(world, Vec3::new(owner_at.x, owner_at.y + 0.5, owner_at.z + 0.5), exp);
                }
                if item.is_some_and(|i| fishes.contains(&i)) {
                    self.award_stat(owner, super::stats::CUSTOM, "minecraft:fish_caught", 1);
                }
            }
            wear = 1;
        }
        if on_ground {
            if events::on_fish(self, world, owner, None, hook, None, "IN_GROUND", 0).is_none() {
                return 0;
            }
            wear = 2;
        }
        if wear == 0 && events::on_fish(self, world, owner, None, hook, None, "REEL_IN", 0).is_none() {
            return 0;
        }
        self.discard_hook(world, hook);
        wear
    }

    /// `pullEntity` (and `FishHook.pullHookedEntity`): what the bobber hooked comes a tenth of
    /// the way to its player (a player's own client keeps its motion, as in vanilla). Returns
    /// whether it had hooked anything.
    pub fn pull_hooked(&mut self, world: usize, hook: i32) -> bool {
        let Some(EntityKind::Fishing(h)) = self.worlds[world].entities.get(&hook).map(|e| &e.kind) else { return false };
        let (owner, Some(target)) = (h.owner, h.hooked) else { return false };
        let pos = self.worlds[world].entities[&hook].pos;
        let Some(p) = self.players.get(&owner) else { return false };
        let pull = Vec3::new(p.loc.x, p.loc.y, p.loc.z).sub(pos).scale(0.1);
        if let Some(t) = self.worlds[world].entities.get_mut(&target) {
            t.vel = t.vel.add(pull);
            t.force_sync = true;
        }
        true
    }

    /// Particles without options at `at` for the world's players within 32 blocks
    /// (`ServerLevel.sendParticles`).
    fn particles(&self, world: usize, name: &str, at: Vec3, count: i32, offset: (f32, f32, f32), speed: f32) {
        let Some(id) = paper_data::registries::id("minecraft:particle_type", name) else { return };
        let mut particle = Vec::new();
        paper_protocol::codec::Encode::encode(&VarInt(id as i32), &mut particle);
        let packet = play::LevelParticles { particle, override_limiter: false, always_show: false, x: at.x, y: at.y, z: at.z, offset, max_speed: (speed, speed, speed), count, randomization: 0 };
        for p in self.players.values().filter(|p| p.world == world) {
            if Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(at) < 32.0 * 32.0 {
                p.conn.send(&packet);
            }
        }
    }
}

/// `resetTimeUntilLured` (Paper's, with Lure never taking the wait below a tick).
fn reset_time_until_lured(h: &mut FishingHook, rng: &mut impl Rng) {
    h.time_until_lured = next_int(rng, h.min_wait, h.max_wait);
    if h.apply_lure {
        h.time_until_lured -= if h.lure_speed >= h.max_wait { h.time_until_lured - 1 } else { h.lure_speed };
    }
}
