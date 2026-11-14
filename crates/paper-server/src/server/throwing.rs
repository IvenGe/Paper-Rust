//! Thrown items (`ThrowableItemProjectile`): snowballs, eggs, ender pearls, bottles o'
//! enchanting and splash and lingering potions, thrown by players and (all but ender pearls)
//! by dispensers; small fireballs from dispensed fire charges (`SmallFireball`); the clouds
//! lingering potions leave (`AreaEffectCloud`); and spawn eggs (`SpawnEggItem`) on blocks and
//! spawners.

use super::effects::ActiveEffect;
use super::mobs::SpawnReason;
use super::player::{HOTBAR_START, OFFHAND_SLOT};
use super::survival::{category, Damage};
use super::Server;
use crate::config::GameMode;
use crate::world::entity::{Aabb, Entity, EntityKind, Vec3};
use paper_data::{blocks, effects, entity_types, items};
use paper_protocol::codec::BlockPos;
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

/// What an item becomes when thrown: the entity, the pitch offset and speed it leaves with
/// (`shootFromRotation`), its gravity, and its throw sound.
pub fn throwable(item: &str) -> Option<(&'static str, f32, f64, f64, &'static str)> {
    Some(match item {
        "minecraft:snowball" => ("minecraft:snowball", 0.0, 1.5, 0.03, "minecraft:entity.snowball.throw"),
        "minecraft:egg" | "minecraft:blue_egg" | "minecraft:brown_egg" => ("minecraft:egg", 0.0, 1.5, 0.03, "minecraft:entity.egg.throw"),
        "minecraft:ender_pearl" => ("minecraft:ender_pearl", 0.0, 1.5, 0.03, "minecraft:entity.ender_pearl.throw"),
        "minecraft:experience_bottle" => ("minecraft:experience_bottle", -20.0, 0.7, 0.07, "minecraft:entity.experience_bottle.throw"),
        "minecraft:splash_potion" => ("minecraft:splash_potion", -20.0, 0.5, 0.05, "minecraft:entity.splash_potion.throw"),
        "minecraft:lingering_potion" => ("minecraft:lingering_potion", -20.0, 0.5, 0.05, "minecraft:entity.lingering_potion.throw"),
        _ => return None,
    })
}

/// Colour of a potion's splash (`PotionContents.getColor`), as far as this server knows:
/// water's blue.
const SPLASH_COLOR: i32 = 0x385DC6;

fn item_name(stack: &ItemStack) -> &'static str {
    items::get(stack.item as u32).map_or("", |i| i.name)
}

impl Server {
    /// A player throws the held item (`ThrowableItem.use`).
    pub(super) fn throw_held(&mut self, player: i32, hand: i32) -> bool {
        let Some(p) = self.players.get(&player) else { return false };
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        let Some((entity, pitch_offset, speed, gravity, sound)) = throwable(item_name(&stack)) else { return false };
        // Ender pearls cool down for a second.
        if entity == "minecraft:ender_pearl" && p.cooldowns.get("minecraft:ender_pearl").is_some_and(|until| *until > self.tick) {
            return false;
        }
        let (world, loc, creative) = (p.world, p.loc, p.game_mode == GameMode::Creative);
        let eye = Vec3::new(loc.x, loc.y + p.eye_height() - 0.1, loc.z);
        let (yaw, pitch) = ((loc.yaw as f64).to_radians(), (loc.pitch as f64).to_radians());
        let dir = Vec3::new(-yaw.sin() * pitch.cos(), -((loc.pitch + pitch_offset) as f64).to_radians().sin(), yaw.cos() * pitch.cos());
        self.spawn_thrown(world, entity, eye, dir, speed, 1.0, gravity, Slot(Some(stack.with_count(1))), Some(player));
        let mut rng = rand::thread_rng();
        self.play_sound(world, Vec3::new(loc.x, loc.y, loc.z), sound, category::NEUTRAL, 0.5, 0.4 / (rng.gen::<f32>() * 0.4 + 0.8), None);
        if entity == "minecraft:ender_pearl" {
            self.set_cooldown(player, "minecraft:ender_pearl", 20);
        }
        if !creative {
            let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            self.set_inventory_slot(player, slot, rest);
        }
        self.award_stat(player, "minecraft:used", item_name(&stack), 1);
        true
    }

    /// Spawns a thrown item flying along `dir` (`shoot`: normalised, a little noise, times
    /// `speed`).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_thrown(&mut self, world: usize, entity: &str, at: Vec3, dir: Vec3, speed: f64, inaccuracy: f64, gravity: f64, item: Slot, owner: Option<i32>) -> Option<i32> {
        let info = entity_types::by_name(entity)?;
        let mut rng = rand::thread_rng();
        let mut noise = || (rng.gen::<f64>() - rng.gen::<f64>()) * 0.0172275 * inaccuracy;
        let d = dir.normalize();
        let vel = Vec3::new(d.x + noise(), d.y + noise(), d.z + noise()).scale(speed);
        let mut e = Entity::new(self.next_entity_id(), info, at, EntityKind::Thrown { item, owner, gravity, left_owner: false });
        e.vel = vel;
        e.yaw = -(vel.x.atan2(vel.z).to_degrees()) as f32;
        e.pitch = -(vel.y.atan2(vel.horizontal_length()).to_degrees()) as f32;
        Some(self.spawn_entity(world, e))
    }

    /// A thrown item's flight (`ThrowableProjectile.tick`) and what happens where it lands.
    pub(super) fn tick_thrown(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let EntityKind::Thrown { owner, gravity, left_owner, .. } = e.kind else { return };
        let fireball = matches!(e.info.name.as_str(), "minecraft:small_fireball" | "minecraft:dragon_fireball" | "minecraft:fireball");
        let dragon_fireball = e.info.name == "minecraft:dragon_fireball";
        let large_fireball = e.info.name == "minecraft:fireball";
        let (from, mut vel) = (e.pos, e.vel);
        if fireball {
            // `AbstractHurtingProjectile.applyInertia`: it keeps speeding up along its way, less
            // in water. It burns out when it reaches unloaded chunks.
            let w = &self.worlds[world];
            if !w.chunks.contains_key(&from.chunk()) {
                if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                    e.removed = true;
                }
                return;
            }
            let wet = w.block_at(from.x.floor() as i32, from.y.floor() as i32, from.z.floor() as i32).and_then(super::survival::fluid_of).is_some_and(|(f, _)| f == super::survival::Fluid::Water);
            vel = vel.add(vel.normalize().scale(0.1)).scale(if wet { 0.8 } else { 0.95 });
        }
        let to = from.add(vel);
        let w = &self.worlds[world];
        let block_hit = super::combat::clip_blocks(w, from, to);
        let end = block_hit.map_or(to, |(t, _)| from.add(vel.scale(t)));
        // The first living thing on the way (its owner once the item has left it).
        let swept = Aabb::of_entity(from, 0.25, 0.25).expand_towards(vel).inflate(1.0, 1.0, 1.0);
        let mut target: Option<(f64, i32)> = None;
        let mut consider = |cand: i32, b: Aabb| {
            if let Some(t) = b.inflate(0.3, 0.3, 0.3).clip_ray(from, end) {
                if target.is_none_or(|(bt, _)| t < bt) {
                    target = Some((t, cand));
                }
            }
        };
        let mut owner_near = false;
        for p in self.players.values().filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator) {
            let b = Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8);
            if Some(p.entity_id) == owner && !left_owner {
                owner_near |= b.inflate(1.0, 1.0, 1.0).intersects(&Aabb::of_entity(from, 0.25, 0.25));
                continue;
            }
            if b.intersects(&swept) {
                consider(p.entity_id, b);
            }
        }
        for cand in w.entities_in(&swept) {
            if cand == id || Some(cand) == owner && (!left_owner || dragon_fireball) {
                continue;
            }
            if let Some(c) = w.entities.get(&cand).filter(|c| c.mob().is_some_and(|m| !m.living.is_dead())) {
                consider(cand, c.bounding_box());
            }
        }
        if let Some((t, victim)) = target {
            let at = from.add(vel.scale(t));
            if dragon_fireball {
                self.dragon_fireball_hits(world, id, at, owner);
            } else if large_fireball {
                self.large_fireball_hits(world, id, at, Some(victim));
            } else if fireball {
                self.fireball_hits(world, id, at, vel, Some(victim));
            } else {
                self.thrown_hits(world, id, at, Some(victim));
            }
            return;
        }
        if let Some((_, block)) = block_hit {
            let _ = block;
            if dragon_fireball {
                self.dragon_fireball_hits(world, id, end, owner);
            } else if large_fireball {
                self.large_fireball_hits(world, id, end, None);
            } else if fireball {
                self.fireball_hits(world, id, end, vel, None);
            } else {
                self.thrown_hits(world, id, end, None);
            }
            return;
        }
        let in_water = w.block_at(to.x.floor() as i32, to.y.floor() as i32, to.z.floor() as i32).and_then(super::survival::fluid_of).is_some_and(|(f, _)| f == super::survival::Fluid::Water);
        let below = (to.y as i32) < w.min_y - 64;
        let e = self.worlds[world].entities.get_mut(&id).unwrap();
        e.pos = to;
        let drag = if in_water { 0.8 } else { 0.99 };
        e.vel = if fireball { vel } else { Vec3::new(vel.x * drag, vel.y * drag - gravity, vel.z * drag) };
        e.yaw = -(e.vel.x.atan2(e.vel.z).to_degrees()) as f32;
        e.pitch = -(e.vel.y.atan2(e.vel.horizontal_length()).to_degrees()) as f32;
        if let EntityKind::Thrown { left_owner, .. } = &mut e.kind {
            *left_owner |= !owner_near;
        }
        if below {
            e.removed = true;
        }
        if e.pos.chunk() != from.chunk() {
            self.worlds[world].reindex_entity(id);
        }
    }

    /// A small fireball hits (`SmallFireball.onHitEntity`, `onHitBlock`): what it hit burns
    /// for five seconds and takes 5 damage; a block it hits catches fire on that side (when a
    /// mob shot it, only with mob griefing).
    fn fireball_hits(&mut self, world: usize, id: i32, at: Vec3, vel: Vec3, victim: Option<i32>) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let EntityKind::Thrown { owner, .. } = e.kind.clone() else { return };
        e.removed = true;
        e.pos = at;
        match victim {
            Some(v) => {
                self.set_on_fire(world, v, 100);
                let damage = Damage { kind: "minecraft:fireball", attacker: owner, direct: Some(id), position: Some(at) };
                self.hurt_entity(world, v, damage, 5.0);
            }
            None => {
                let by_mob = owner.is_some_and(|o| !self.players.contains_key(&o));
                if by_mob && !self.worlds[world].rules.bool("mob_griefing") {
                    return;
                }
                let before = at.sub(vel.normalize().scale(0.01));
                let pos = BlockPos::new(before.x.floor() as i32, before.y.floor() as i32, before.z.floor() as i32);
                if self.worlds[world].block_at(pos.x, pos.y, pos.z).is_some_and(blocks::is_air) {
                    let fire = crate::world::block_updates::fire::fire_state_for(&self.worlds[world], pos);
                    self.set_block_and_update(world, pos, fire);
                }
            }
        }
    }

    /// A ghast's fireball hits (`LargeFireball.onHit`): what it hit takes 6 damage, and it
    /// explodes with power 1, breaking blocks only with mob griefing (the explosion sets no
    /// fires here).
    fn large_fireball_hits(&mut self, world: usize, id: i32, at: Vec3, victim: Option<i32>) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let EntityKind::Thrown { owner, .. } = e.kind.clone() else { return };
        e.removed = true;
        e.pos = at;
        if let Some(v) = victim {
            let damage = Damage { kind: "minecraft:fireball", attacker: owner, direct: Some(id), position: Some(at) };
            self.hurt_entity(world, v, damage, 6.0);
        }
        let griefing = self.worlds[world].rules.bool("mob_griefing");
        self.explode(world, at, 1.0, griefing, owner);
    }

    /// A player strikes a ghast's fireball (`ProjectileDeflection.AIM_DEFLECT`): it flies where
    /// they look, and is theirs from then on.
    pub(super) fn deflect_fireball(&mut self, world: usize, id: i32, player: i32) {
        let Some(p) = self.players.get(&player) else { return };
        let look = Vec3::from_rotation(p.loc.yaw, p.loc.pitch);
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        e.vel = look.normalize();
        e.force_sync = true;
        if let EntityKind::Thrown { owner, left_owner, .. } = &mut e.kind {
            *owner = Some(player);
            *left_owner = false;
        }
    }

    /// Where a thrown item lands (`onHit`): what it does to what it hit and around it.
    fn thrown_hits(&mut self, world: usize, id: i32, at: Vec3, victim: Option<i32>) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let EntityKind::Thrown { item, owner, .. } = e.kind.clone() else { return };
        let name = e.info.name.clone();
        e.removed = true;
        e.pos = at;
        // A hit entity takes the throw (`thrown` damage: nothing, but knockback; snowballs
        // hurt blazes).
        if let Some(v) = victim {
            let blaze = self.worlds[world].entities.get(&v).is_some_and(|e| e.info.name == "minecraft:blaze");
            let amount = if name == "minecraft:snowball" && blaze { 3.0 } else { 0.0 };
            let damage = Damage { kind: "minecraft:thrown", attacker: owner, direct: Some(id), position: Some(at) };
            self.hurt_entity(world, v, damage, amount);
        }
        let mut rng = rand::thread_rng();
        match name.as_str() {
            "minecraft:snowball" | "minecraft:egg" => {
                self.entity_event(world, id, 3);
                // One egg in eight hatches a chick, one of those 32 four (`ThrownEgg`).
                if name == "minecraft:egg" && rng.gen_range(0..8) == 0 {
                    let count = if rng.gen_range(0..32) == 0 { 4 } else { 1 };
                    for _ in 0..count {
                        if let Some(chick) = self.spawn_mob(world, "minecraft:chicken", at, SpawnReason::Natural) {
                            if let Some(m) = self.worlds[world].entities.get_mut(&chick).and_then(|e| e.mob_mut()) {
                                m.baby = true;
                                m.age = -24000;
                            }
                        }
                    }
                }
            }
            "minecraft:ender_pearl" => self.pearl_lands(world, at, owner),
            "minecraft:experience_bottle" => {
                self.broadcast_level_event(world, 2002, BlockPos::new(at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32), -13083194, false);
                let amount = 3 + rng.gen_range(0..5) + rng.gen_range(0..5);
                self.award_experience(world, at, amount);
            }
            "minecraft:splash_potion" => self.splash(world, at, &item, owner, victim),
            "minecraft:lingering_potion" => {
                let potion = item.0.as_ref().and_then(super::effects::potion_of);
                self.broadcast_level_event(world, 2002, BlockPos::new(at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32), SPLASH_COLOR, false);
                if let Some(potion) = potion.filter(|p| !effects::potion(p).is_empty()) {
                    self.spawn_cloud(world, at, potion, owner);
                }
            }
            _ => {}
        }
    }

    /// An ender pearl lands: its thrower comes after it (5 damage, a chance of an
    /// endermite).
    fn pearl_lands(&mut self, world: usize, at: Vec3, owner: Option<i32>) {
        let Some(p) = owner.and_then(|o| self.players.get(&o)) else { return };
        if p.world != world || p.dead {
            return;
        }
        let id = p.entity_id;
        let loc = super::player::Location { x: at.x, y: at.y, z: at.z, ..p.loc };
        if rand::thread_rng().gen::<f32>() < 0.05 && self.worlds[world].rules.bool("spawn_mobs") {
            self.spawn_mob(world, "minecraft:endermite", at, SpawnReason::Natural);
        }
        if self.players.get(&id).is_some_and(|p| p.vehicle.is_some()) {
            self.dismount_quietly(id);
        }
        self.teleport(id, loc);
        if let Some(p) = self.players.get_mut(&id) {
            p.fall_distance = 0.0;
        }
        self.hurt_entity(world, id, Damage::new("minecraft:ender_pearl"), 5.0);
        self.play_sound(world, at, "minecraft:entity.player.teleport", category::PLAYERS, 1.0, 1.0, None);
    }

    /// A splash potion's effects on everything living within four blocks, weaker further
    /// away (`ThrownSplashPotion.applySplash`); water puts out fires and hurts endermen.
    fn splash(&mut self, world: usize, at: Vec3, item: &Slot, owner: Option<i32>, direct: Option<i32>) {
        let potion = item.0.as_ref().and_then(super::effects::potion_of);
        let list: Vec<(String, i32, i32)> = potion.as_deref().map(|p| effects::potion(p).to_vec()).unwrap_or_default();
        let instant = list.iter().any(|(e, _, _)| effects::by_name(e).is_some_and(|i| i.instantaneous));
        let pos = BlockPos::new(at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32);
        self.broadcast_level_event(world, if instant { 2007 } else { 2002 }, pos, SPLASH_COLOR, false);
        let area = Aabb::of_entity(at, 0.25, 0.25).inflate(4.0, 2.0, 4.0);
        let mut targets: Vec<(i32, f64)> = self.players.values().filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator).map(|p| (p.entity_id, Vec3::new(p.loc.x, p.loc.y, p.loc.z))).filter(|(_, p)| area.intersects(&Aabb::of_entity(*p, 0.6, 1.8))).map(|(id, p)| (id, p.distance_squared(at))).collect();
        let w = &self.worlds[world];
        targets.extend(w.entities_in(&area).into_iter().filter_map(|i| w.entities.get(&i)).filter(|e| e.mob().is_some_and(|m| !m.living.is_dead())).map(|e| (e.id, e.pos.distance_squared(at))));
        if potion.as_deref() == Some("minecraft:water") || list.is_empty() {
            // Water: fires go out, water-sensitive mobs get hurt.
            for (target, d2) in targets {
                if d2 >= 16.0 {
                    continue;
                }
                let sensitive = self.worlds[world].entities.get(&target).is_some_and(|e| matches!(e.info.name.as_str(), "minecraft:enderman" | "minecraft:blaze" | "minecraft:snow_golem" | "minecraft:strider"));
                if sensitive {
                    let mut damage = Damage::new("minecraft:indirect_magic");
                    damage.attacker = owner;
                    self.hurt_entity(world, target, damage, 1.0);
                }
                self.extinguish(world, target);
            }
            return;
        }
        for (target, d2) in targets {
            if d2 >= 16.0 {
                continue;
            }
            let scale = if Some(target) == direct { 1.0 } else { 1.0 - d2.sqrt() / 4.0 };
            for (effect, duration, amplifier) in &list {
                let Some(info) = effects::by_name(effect) else { continue };
                if info.instantaneous {
                    self.apply_instant_scaled(world, target, effect, *amplifier, scale, owner);
                } else {
                    let duration = (scale * *duration as f64 + 0.5) as i32;
                    if duration > 20 {
                        if let Some(e) = ActiveEffect::new(effect, duration, *amplifier) {
                            self.add_effect(world, target, e);
                        }
                    }
                }
            }
        }
    }

    /// Puts out a burning player or mob.
    fn extinguish(&mut self, world: usize, target: i32) {
        if let Some(p) = self.players.get_mut(&target) {
            if p.fire_ticks > 0 {
                p.fire_ticks = 0;
                p.metadata_dirty = true;
            }
        } else if let Some(e) = self.worlds[world].entities.get_mut(&target) {
            if let Some(m) = e.mob_mut().filter(|m| m.living.fire_ticks > 0) {
                m.living.fire_ticks = 0;
                e.metadata_dirty = true;
            }
        }
    }

    /// Instant health and damage at a fraction of their power (`applyInstantenousEffect`).
    fn apply_instant_scaled(&mut self, world: usize, target: i32, effect: &str, amplifier: i32, scale: f64, owner: Option<i32>) {
        let undead = !self.players.contains_key(&target) && self.worlds[world].entities.get(&target).is_some_and(|e| super::effects::is_undead(&e.info.name));
        let harm = (effect == "minecraft:instant_damage") != undead;
        if harm {
            let amount = (scale * (6 << amplifier.clamp(0, 30)) as f64 + 0.5) as i32;
            let mut damage = Damage::new("minecraft:indirect_magic");
            damage.attacker = owner;
            self.hurt_entity(world, target, damage, amount as f32);
        } else {
            let amount = (scale * (4 << amplifier.clamp(0, 30)) as f64 + 0.5) as i32;
            self.heal_entity(world, target, amount as f32);
        }
    }

    /// A lingering potion's cloud: three blocks wide, shrinking over half a minute and with
    /// each use (`AreaEffectCloud`).
    fn spawn_cloud(&mut self, world: usize, at: Vec3, potion: String, owner: Option<i32>) {
        let Some(info) = entity_types::by_name("minecraft:area_effect_cloud") else { return };
        let kind = EntityKind::Cloud { potion, owner, radius: 3.0, duration: 600, wait: 10, uses: Default::default(), growth: -3.0 / 600.0, on_use: -0.5, breath: false };
        let e = Entity::new(self.next_entity_id(), info, at, kind);
        self.spawn_entity(world, e);
    }

    /// A cloud's tick: after a short wait it gives a quarter of the potion's effects to
    /// what stands in it (each at most once a second), shrinking as it does.
    pub(super) fn tick_cloud(&mut self, world: usize, id: i32) {
        let tick = self.tick;
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let (pos, age) = (e.pos, e.age);
        let EntityKind::Cloud { potion, owner, radius, duration, wait, uses, growth, on_use, .. } = &mut e.kind else { return };
        let on_use = *on_use;
        if age >= *wait + *duration {
            e.removed = true;
            return;
        }
        let waiting = age < *wait;
        if !waiting {
            *radius += *growth;
            if *radius < 0.5 {
                e.removed = true;
                return;
            }
        }
        e.metadata_dirty |= age == *wait || age % 20 == 0;
        if waiting || age % 5 != 0 {
            return;
        }
        let (potion, owner, r) = (potion.clone(), *owner, *radius as f64);
        uses.retain(|(_, until)| *until > tick);
        let used: Vec<i32> = uses.iter().map(|(t, _)| *t).collect();
        // Like vanilla: what touches the cloud's box (as wide as the cloud, half a block
        // tall) and stands within the radius of its centre.
        let w = &self.worlds[world];
        let area = Aabb::of_entity(pos, r * 2.0, 0.5);
        let near = |p: Vec3| (p.x - pos.x).powi(2) + (p.z - pos.z).powi(2) <= r * r;
        let mut targets: Vec<i32> = self
            .players
            .values()
            .filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator)
            .map(|p| (p.entity_id, Vec3::new(p.loc.x, p.loc.y, p.loc.z)))
            .filter(|(_, p)| area.intersects(&Aabb::of_entity(*p, 0.6, 1.8)) && near(*p))
            .map(|(id, _)| id)
            .collect();
        targets.extend(w.entities_in(&area).into_iter().filter(|o| *o != id).filter(|o| w.entities.get(o).is_some_and(|e| e.mob().is_some_and(|m| !m.living.is_dead()) && near(e.pos))));
        targets.retain(|t| !used.contains(t));
        if targets.is_empty() {
            return;
        }
        let list = effects::potion(&potion).to_vec();
        for target in &targets {
            for (effect, duration, amplifier) in &list {
                if effects::by_name(effect).is_some_and(|i| i.instantaneous) {
                    self.apply_instant_scaled(world, *target, effect, *amplifier, 0.5, owner);
                } else if let Some(e) = ActiveEffect::new(effect, duration / 4, *amplifier) {
                    self.add_effect(world, *target, e);
                }
            }
        }
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            if let EntityKind::Cloud { radius, uses, .. } = &mut e.kind {
                for t in targets {
                    uses.push((t, tick + 20));
                    *radius += on_use;
                }
                if *radius < 0.5 {
                    e.removed = true;
                }
            }
        }
    }

    /// A spawn egg used on a block (`SpawnEggItem.useOn`): on a spawner it sets what spawns;
    /// elsewhere the mob appears on the clicked face. Returns whether it was used.
    /// A spawn egg used while looking at water or lava (`SpawnEggItem.use`): the mob appears
    /// in it.
    pub(super) fn use_spawn_egg_on_fluid(&mut self, player: i32, hand: i32, yaw: f32, pitch: f32) -> bool {
        let Some(p) = self.players.get(&player) else { return false };
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        let Some(entity) = item_name(&stack).strip_suffix("_spawn_egg") else { return false };
        if entity_types::by_name(entity).is_none() || !matches!(p.game_mode, GameMode::Survival | GameMode::Creative) {
            return false;
        }
        let (world, creative) = (p.world, p.game_mode == GameMode::Creative);
        let eye = Vec3::new(p.loc.x, p.loc.y + p.eye_height(), p.loc.z);
        let Some((hit, _, Some(_))) = super::portals::raycast(&self.worlds[world], eye, Vec3::from_rotation(yaw, pitch), 4.5, true) else { return false };
        let liquid = self.worlds[world].block_at(hit.x, hit.y, hit.z).is_some_and(|s| matches!(blocks::block_of_state(s).name, "minecraft:water" | "minecraft:lava"));
        if !liquid {
            return false;
        }
        let at = Vec3::new(hit.x as f64 + 0.5, hit.y as f64, hit.z as f64 + 0.5);
        let Some(id) = self.spawn_mob(world, entity, at, SpawnReason::Egg) else { return false };
        if let Some(label) = super::workstations::custom_name_of(&stack) {
            if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                e.custom_name = Some(label);
                e.metadata_dirty = true;
            }
        }
        if !creative {
            let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            self.set_inventory_slot(player, slot, rest);
        }
        self.award_stat(player, "minecraft:used", item_name(&stack), 1);
        true
    }

    pub(super) fn use_spawn_egg(&mut self, player: i32, use_on: &paper_protocol::packets::play::UseItemOn) -> bool {
        let Some(p) = self.players.get(&player) else { return false };
        let hand = use_on.hand.0;
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        let Some(entity) = item_name(&stack).strip_suffix("_spawn_egg") else { return false };
        if entity_types::by_name(entity).is_none() || !matches!(p.game_mode, GameMode::Survival | GameMode::Creative) {
            return false;
        }
        let (world, creative) = (p.world, p.game_mode == GameMode::Creative);
        let pos = use_on.pos;
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return false };
        if blocks::block_of_state(state).name == "minecraft:spawner" {
            self.set_spawner_entity(world, pos, entity);
            if !creative {
                let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
                self.set_inventory_slot(player, slot, rest);
            }
            self.award_stat(player, "minecraft:used", item_name(&stack), 1);
            return true;
        }
        let face = use_on.face.0;
        let (dx, dy, dz) = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)][face.clamp(0, 5) as usize];
        let spawn_at = if blocks::has_collision(state) { BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz) } else { pos };
        let at = Vec3::new(spawn_at.x as f64 + 0.5, spawn_at.y as f64, spawn_at.z as f64 + 0.5);
        let Some(id) = self.spawn_mob(world, entity, at, SpawnReason::Egg) else { return false };
        // A name given at an anvil names the mob.
        if let Some(label) = super::workstations::custom_name_of(&stack) {
            if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                e.custom_name = Some(label);
                e.metadata_dirty = true;
            }
        }
        if !creative {
            let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            self.set_inventory_slot(player, slot, rest);
        }
        self.award_stat(player, "minecraft:used", item_name(&stack), 1);
        true
    }
}

impl Server {
    /// Starts an item cooldown for a player (`ItemCooldowns.addCooldown`).
    pub fn set_cooldown(&mut self, player: i32, group: &str, ticks: i32) {
        let until = self.tick + ticks.max(0) as u64;
        let Some(p) = self.players.get_mut(&player) else { return };
        p.cooldowns.insert(group.to_owned(), until);
        p.conn.send(&paper_protocol::packets::play::Cooldown { group: group.to_owned(), duration: paper_protocol::codec::VarInt(ticks) });
    }

    /// A spawn egg on a spawner: it spawns that mob from now on, and clients show it spinning.
    pub fn set_spawner_entity(&mut self, world: usize, pos: BlockPos, entity: &str) {
        use paper_nbt::{Compound, Tag};
        let chunk = crate::world::chunk::ChunkPos::of_block(pos.x, pos.z);
        let key = (pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8);
        let Some(c) = self.worlds[world].chunks.get_mut(&chunk) else { return };
        let nbt = c.block_entities.entry(key).or_insert_with(|| Compound::new().with("id", Tag::String("minecraft:mob_spawner".into())));
        nbt.insert("SpawnData", Tag::Compound(Compound::new().with("entity", Tag::Compound(Compound::new().with("id", Tag::String(entity.to_owned()))))));
        let data = nbt.clone();
        c.dirty = true;
        let Some(ty) = paper_data::registries::id("minecraft:block_entity_type", "minecraft:mob_spawner") else { return };
        let packet = paper_protocol::packets::play::BlockEntityData { pos, block_entity_type: paper_protocol::codec::VarInt(ty as i32), data: paper_protocol::codec::Nbt(Some(Tag::Compound(data))) };
        self.broadcast_to_chunk_viewers(world, chunk, &packet);
    }
}
