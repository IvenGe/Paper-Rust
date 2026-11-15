//! Firework rockets (vanilla's `FireworkRocketItem` and `FireworkRocketEntity`): used on a
//! block one flies up from there; used while gliding one is attached to the glider, whose
//! client does the boosting. At the end of its flight a rocket explodes, and one with
//! explosions hurts what is near. Plugins hear of the boost (`PlayerElytraBoostEvent`) and
//! of each explosion (`FireworkExplodeEvent`, which keeps a rocket flying when cancelled).

use super::survival::{Damage, category};
use super::Server;
use crate::config::GameMode;
use crate::plugins::events;
use crate::world::entity::{Aabb, Entity, EntityKind, Firework, Vec3};
use paper_data::items;
use paper_protocol::codec::Reader;
use paper_protocol::packets::play::UseItemOn;
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

pub const ROCKET: &str = "minecraft:firework_rocket";

/// A rocket's `fireworks` component: its flight duration and how many explosions it has.
pub fn fireworks_of(stack: &ItemStack) -> (i32, usize) {
    if let Some(ty) = paper_data::registries::id("minecraft:data_component_type", "minecraft:fireworks").map(|t| t as i32) {
        if stack.removed.contains(&ty) {
            return (0, 0);
        }
        if let Some((_, data)) = stack.added.iter().find(|(t, _)| *t == ty) {
            let mut r = Reader::new(data);
            let flight = r.varint().unwrap_or(0);
            let explosions = r.varint().unwrap_or(0).max(0) as usize;
            return (flight, explosions);
        }
    }
    let raw = paper_data::item_data::by_id(stack.item as u32).raw.as_ref().map(|r| r["fireworks"].clone()).unwrap_or_default();
    // A plain rocket flies one level high (its default `Fireworks(1, [])`).
    let plain = items::get(stack.item as u32).is_some_and(|i| i.name == ROCKET) as i64;
    (raw["flight_duration"].as_i64().unwrap_or(plain) as i32, raw["explosions"].as_array().map_or(0, Vec::len))
}

impl Server {
    /// A rocket at `pos` (`FireworkRocketEntity(level, x, y, z, stack)`): drifting a little
    /// and rising, for ten ticks per flight duration plus one, and a few more at random.
    pub fn new_firework(&self, pos: Vec3, item: Slot, owner: Option<i32>) -> Option<Entity> {
        let info = paper_data::entity_types::by_name(ROCKET)?;
        let mut rng = rand::thread_rng();
        let flight = item.0.as_ref().map_or(0, |s| fireworks_of(s).0);
        let lifetime = 10 * (1 + flight) + rng.gen_range(0..6) + rng.gen_range(0..7);
        let triangle = |rng: &mut rand::rngs::ThreadRng| 0.002297 * (rng.gen::<f64>() - rng.gen::<f64>());
        let mut e = Entity::new(self.next_entity_id(), info, pos, EntityKind::Firework(Box::new(Firework { item, life: 0, lifetime, attached: None, shot_at_angle: false, owner, spawning: None })));
        e.vel = Vec3::new(triangle(&mut rng), 0.05, triangle(&mut rng));
        Some(e)
    }

    /// A firework rocket used in the air (`FireworkRocketItem.use`): only while gliding, it
    /// boosts the player, unless plugins stop it (`PlayerElytraBoostEvent`, which may also
    /// leave the rocket in hand). Returns whether it was used.
    pub(super) fn use_firework(&mut self, player: i32, hand: i32) -> bool {
        let slot = self.hand_slot(player, hand);
        let Some(p) = self.players.get(&player) else { return false };
        if !p.gliding {
            return false;
        }
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        let (world, at, uuid, creative) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z), p.uuid(), p.game_mode == GameMode::Creative);
        let Some(mut e) = self.new_firework(at, Slot(Some(stack.with_count(1))), Some(player)) else { return false };
        if let EntityKind::Firework(f) = &mut e.kind {
            f.attached = Some(player);
            f.spawning = Some(uuid);
        }
        let id = self.spawn_entity(world, e);
        let consume = match events::on_elytra_boost(self, world, player, id, &stack, hand) {
            Some(consume) => consume,
            None => {
                self.worlds[world].remove_entity(id);
                self.resync_inventory(player);
                return true;
            }
        };
        // It is launched as any projectile (`ProjectileLaunchEvent`).
        if !self.launch(world, id) {
            self.resync_inventory(player);
            return true;
        }
        self.award_stat(player, "minecraft:used", ROCKET, 1);
        if consume && !creative {
            let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            self.set_inventory_slot(player, slot, rest);
        } else {
            self.resync_inventory(player);
        }
        true
    }

    /// A firework rocket used on a block (`FireworkRocketItem.useOn`): it flies up from just
    /// off the clicked face. Returns whether it was used.
    pub(super) fn use_firework_on(&mut self, player: i32, use_on: &UseItemOn) -> bool {
        let slot = self.hand_slot(player, use_on.hand.0);
        let Some(p) = self.players.get(&player) else { return false };
        let Some(stack) = p.inventory[slot].0.clone().filter(|s| items::get(s.item as u32).is_some_and(|i| i.name == ROCKET)) else { return false };
        if !matches!(p.game_mode, GameMode::Survival | GameMode::Creative) || !(0..6).contains(&use_on.face.0) {
            return false;
        }
        let (world, creative) = (p.world, p.game_mode == GameMode::Creative);
        let (dx, dy, dz) = crate::world::hanging::STEP[use_on.face.0 as usize];
        let at = Vec3::new(
            use_on.pos.x as f64 + use_on.cursor_x as f64 + dx as f64 * 0.15,
            use_on.pos.y as f64 + use_on.cursor_y as f64 + dy as f64 * 0.15,
            use_on.pos.z as f64 + use_on.cursor_z as f64 + dz as f64 * 0.15,
        );
        let Some(e) = self.new_firework(at, Slot(Some(stack.with_count(1))), Some(player)) else { return false };
        let id = self.spawn_entity(world, e);
        if !self.launch(world, id) {
            return true;
        }
        self.award_stat(player, "minecraft:used", ROCKET, 1);
        if !creative {
            let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            self.set_inventory_slot(player, slot, rest);
        }
        true
    }

    /// A rocket's tick (`FireworkRocketEntity.tick`): one attached to a glider stays with it;
    /// another rises faster and faster (or flies straight where it was shot), exploding early
    /// on what it hits. At the end of its flight it explodes, unless plugins keep it flying.
    pub(super) fn tick_firework(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let EntityKind::Firework(f) = &e.kind else { return };
        let (attached, shot_at_angle, owner, life) = (f.attached, f.shot_at_angle, f.owner, f.life);
        let silent = e.flags.silent;
        if let Some(target) = attached {
            let at = match self.players.get(&target) {
                Some(p) if p.world == world && !p.dead => Some(Vec3::new(p.loc.x, p.loc.y, p.loc.z)),
                Some(_) => None,
                None => self.worlds[world].entities.get(&target).filter(|t| !t.removed).map(|t| t.pos),
            };
            if let (Some(at), Some(e)) = (at, self.worlds[world].entities.get_mut(&id)) {
                e.pos = at;
            }
            self.worlds[world].reindex_entity(id);
        } else {
            let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
            if !shot_at_angle {
                let acceleration = if e.horizontal_collision { 1.0 } else { 1.15 };
                e.vel = Vec3::new(e.vel.x * acceleration, e.vel.y + 0.04, e.vel.z * acceleration);
            }
            let (from, vel) = (e.pos, e.vel);
            let hit = self.firework_hits(world, id, owner, from, vel);
            let Some(mut e) = self.worlds[world].entities.remove(&id) else { return };
            e.move_by(&self.worlds[world], vel, 0.0);
            e.vel = vel;
            self.worlds[world].entities.insert(id, e);
            self.worlds[world].reindex_entity(id);
            match hit {
                // On an entity it always explodes; on a block only with explosions.
                Some(true) => return self.try_explode_firework(world, id),
                Some(false) if self.worlds[world].entities.get(&id).and_then(|e| match &e.kind { EntityKind::Firework(f) => f.item.0.as_ref().map(|s| fireworks_of(s).1), _ => None }).unwrap_or(0) > 0 => {
                    return self.try_explode_firework(world, id);
                }
                _ => {}
            }
        }
        if life == 0 && !silent {
            if let Some(pos) = self.worlds[world].entities.get(&id).map(|e| e.pos) {
                self.play_sound(world, pos, "minecraft:entity.firework_rocket.launch", category::AMBIENT, 3.0, 1.0, None);
            }
        }
        let Some(EntityKind::Firework(f)) = self.worlds[world].entities.get_mut(&id).map(|e| &mut e.kind) else { return };
        f.life += 1;
        if f.life > f.lifetime {
            self.try_explode_firework(world, id);
        }
    }

    /// What a free rocket's move this tick runs into: an entity (`Some(true)`, a living
    /// thing other than its owner; plugins may let it pass, `ProjectileHitEvent`) or a block
    /// (`Some(false)`).
    fn firework_hits(&mut self, world: usize, id: i32, owner: Option<i32>, from: Vec3, vel: Vec3) -> Option<bool> {
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
        for p in self.players.values().filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator && Some(p.entity_id) != owner) {
            let b = Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8);
            if b.intersects(&swept) {
                consider(p.entity_id, b);
            }
        }
        for cand in w.entities_in(&swept) {
            if cand == id || Some(cand) == owner {
                continue;
            }
            if let Some(c) = w.entities.get(&cand).filter(|c| c.mob().is_some_and(|m| !m.living.is_dead())) {
                consider(cand, c.bounding_box());
            }
        }
        let target = target.filter(|(_, victim)| events::on_projectile_hit(self, world, id, Some(*victim), None));
        if target.is_some() {
            return Some(true);
        }
        let block_hit = block_hit.filter(|(_, block)| events::on_projectile_hit(self, world, id, None, Some(*block)));
        block_hit.map(|_| false)
    }

    /// The end of a rocket's flight, unless plugins keep it flying (`FireworkExplodeEvent`).
    fn try_explode_firework(&mut self, world: usize, id: i32) {
        if events::on_firework_explode(self, world, id) {
            self.explode_firework(world, id);
        }
    }

    /// `FireworkRocketEntity.explode`: clients show the burst; with explosions it hurts the
    /// glider it boosted and the living things within five blocks it can see, less the
    /// farther they are (`dealExplosionDamage`).
    pub fn explode_firework(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let EntityKind::Firework(f) = &e.kind else { return };
        let (pos, attached, owner) = (e.pos, f.attached, f.owner);
        let explosions = f.item.0.as_ref().map_or(0, |s| fireworks_of(s).1);
        self.entity_event(world, id, 17);
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            e.removed = true;
        }
        if explosions == 0 {
            return;
        }
        let damage = 5.0 + explosions as f32 * 2.0;
        let source = || Damage { kind: "minecraft:fireworks", attacker: owner, direct: Some(id), position: Some(pos) };
        if let Some(target) = attached {
            self.hurt_entity(world, target, source(), damage);
        }
        // Players and mobs near, but not through walls.
        let mut near: Vec<(i32, Vec3, f64)> = self.players.values().filter(|p| p.world == world && !p.dead).map(|p| (p.entity_id, Vec3::new(p.loc.x, p.loc.y, p.loc.z), 1.8)).collect();
        let area = Aabb::of_entity(pos, 0.25, 0.25).inflate(5.0, 5.0, 5.0);
        for cand in self.worlds[world].entities_in(&area) {
            if let Some(c) = self.worlds[world].entities.get(&cand).filter(|c| c.mob().is_some_and(|m| !m.living.is_dead())) {
                near.push((cand, c.pos, c.info.height as f64));
            }
        }
        for (target, at, height) in near {
            if Some(target) == attached || at.distance_squared(pos) > 25.0 {
                continue;
            }
            let w = &self.worlds[world];
            let seen = (0..2).any(|step| super::combat::clip_blocks(w, pos, Vec3::new(at.x, at.y + 0.5 * step as f64 * height, at.z)).is_none());
            if seen {
                let amount = damage * ((5.0 - at.distance_squared(pos).sqrt()) / 5.0).max(0.0).sqrt() as f32;
                self.hurt_entity(world, target, source(), amount);
            }
        }
    }
}
