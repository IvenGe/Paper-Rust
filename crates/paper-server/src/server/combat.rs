//! Fighting: players attacking, mobs being hurt and dying, bows and arrows, explosions.
//! Formulas follow vanilla's `Player.attack`, `LivingEntity.hurtServer`, `BowItem`,
//! `AbstractArrow` and `ServerExplosion`.

use super::loot::{enchantment_level, LootContext};
use super::player::HOTBAR_START;
use super::survival::{self, category, Damage};
use super::Server;
use crate::config::GameMode;
use crate::world::entity::{block_boxes, Aabb, Entity, EntityKind, Vec3};
use crate::world::World;
use bytes::Bytes;
use paper_data::{blocks, entity_types, item_data, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::{encode_packet, play};
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;
use std::collections::HashSet;

/// Where a segment first hits a block's collision box: (fraction of the segment, block).
pub fn clip_blocks(world: &World, from: Vec3, to: Vec3) -> Option<(f64, BlockPos)> {
    let region = Aabb::new(from, from).expand_towards(to.sub(from)).inflate(1.0e-3, 1.0e-3, 1.0e-3);
    let mut best: Option<(f64, BlockPos)> = None;
    for b in block_boxes(world, &region) {
        if let Some(t) = b.clip_ray(from, to) {
            if best.is_none_or(|(bt, _)| t < bt) {
                let c = b.center();
                best = Some((t, BlockPos::new(c.x.floor() as i32, c.y.floor() as i32, c.z.floor() as i32)));
            }
        }
    }
    best
}

/// Entity tag check, e.g. `entity_in_tag(info, "minecraft:undead")`.
fn entity_in_tag(name: &str, tag: &str) -> bool {
    let Some(info) = entity_types::by_name(name) else { return false };
    paper_data::synced::tag("minecraft:entity_type", tag).is_some_and(|ids| ids.contains(&info.id))
}

impl Server {
    // --- block breaking ------------------------------------------------------------------------

    /// Drops, tool wear and exhaustion after a player broke a block.
    pub(super) fn after_player_broke_block(&mut self, id: i32, world: usize, pos: BlockPos, old: u32) {
        let Some(player) = self.players.get(&id) else { return };
        if player.game_mode == GameMode::Creative {
            return;
        }
        let slot = HOTBAR_START + player.held_slot as usize;
        let held = player.inventory[slot].clone();
        if self.worlds[world].rules.bool("block_drops") {
            self.drop_block_loot(world, pos, old, Some(&held), None);
        }
        self.add_exhaustion(id, 0.005);
        // Tools wear when they break something that takes time to break.
        let hardness = blocks::block_of_state(old).hardness;
        if let Some(stack) = held.0.as_ref() {
            let per_block = item_data::by_id(stack.item as u32).tool.as_ref().map_or(0, |t| t.damage_per_block);
            if hardness != 0.0 && per_block > 0 {
                let p = self.players.get_mut(&id).unwrap();
                if survival::hurt_item(&mut p.inventory[slot], per_block, false) {
                    let (w, at) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z));
                    self.play_sound(w, at, "minecraft:entity.item.break", category::PLAYERS, 0.8, 0.8 + rand::thread_rng().gen::<f32>() * 0.4, None);
                }
                let item = self.players[&id].inventory[slot].clone();
                self.set_inventory_slot(id, slot, item);
            }
        }
    }

    // --- player attacks ------------------------------------------------------------------------

    /// A player hits an entity (left click).
    pub(super) fn handle_attack(&mut self, id: i32, target: i32) {
        let Some(p) = self.players.get(&id) else { return };
        if p.dead || p.game_mode == GameMode::Spectator || target == id {
            return;
        }
        let world = p.world;
        let eye = Vec3::new(p.loc.x, p.loc.y + p.eye_height(), p.loc.z);
        // The target's box, and whether it can be hit at all.
        let (target_box, _, living) = if let Some(t) = self.players.get(&target) {
            if t.world != world || t.dead || t.game_mode == GameMode::Spectator {
                return;
            }
            let pos = Vec3::new(t.loc.x, t.loc.y, t.loc.z);
            (Aabb::of_entity(pos, 0.6, 1.8), pos, true)
        } else {
            let Some(e) = self.worlds[world].entities.get(&target) else { return };
            match &e.kind {
                EntityKind::Mob(m) if !m.living.is_dead() => (e.bounding_box(), e.pos, true),
                EntityKind::Item { .. } | EntityKind::ExperienceOrb { .. } | EntityKind::Arrow { .. } => return,
                EntityKind::Mob(_) => return,
                _ => (e.bounding_box(), e.pos, false),
            }
        };
        // Vanilla allows the interaction range (3) plus a margin of 3 for latency.
        let nearest = Vec3::new(eye.x.clamp(target_box.min.x, target_box.max.x), eye.y.clamp(target_box.min.y, target_box.max.y), eye.z.clamp(target_box.min.z, target_box.max.z));
        if nearest.distance_squared(eye) > 36.0 {
            return;
        }

        let p = &self.players[&id];
        let weapon = p.held_item().clone();
        let data = weapon.0.as_ref().map(|s| item_data::by_id(s.item as u32));
        let mut base = 1.0 + data.map_or(0.0, |d| d.modifier("minecraft:attack_damage", "mainhand") + d.modifier("attack_damage", "mainhand")) as f32;
        let speed = 4.0 + data.map_or(0.0, |d| d.modifier("minecraft:attack_speed", "mainhand") + d.modifier("attack_speed", "mainhand")) as f32;
        let delay = 20.0 / speed.max(0.1);
        let strength = (((self.tick.saturating_sub(p.last_swing)) as f32 + 0.5) / delay).clamp(0.0, 1.0);
        let target_type = if self.players.contains_key(&target) { "minecraft:player" } else { self.worlds[world].entities[&target].info.name.as_str() };
        let level = |e: &str| enchantment_level(Some(&weapon), e);
        let mut enchant = 0.0f32;
        if level("minecraft:sharpness") > 0 {
            enchant += 0.5 * level("minecraft:sharpness") as f32 + 0.5;
        }
        if level("minecraft:smite") > 0 && entity_in_tag(target_type, "minecraft:sensitive_to_smite") {
            enchant += 2.5 * level("minecraft:smite") as f32;
        }
        if level("minecraft:bane_of_arthropods") > 0 && entity_in_tag(target_type, "minecraft:sensitive_to_bane_of_arthropods") {
            enchant += 2.5 * level("minecraft:bane_of_arthropods") as f32;
        }
        if let Some(level) = p.effects.iter().find(|e| e.name() == "minecraft:strength").map(|e| e.amplifier) {
            base += 3.0 * (level + 1) as f32;
        }
        if let Some(level) = p.effects.iter().find(|e| e.name() == "minecraft:weakness").map(|e| e.amplifier) {
            base = (base - 4.0 * (level + 1) as f32).max(0.0);
        }
        base *= 0.2 + strength * strength * 0.8;
        enchant *= strength;
        let full = strength > 0.9;
        let sprint_knockback = p.sprinting && full;
        let feet = self.worlds[world].block_at(p.loc.x.floor() as i32, p.loc.y.floor() as i32, p.loc.z.floor() as i32).unwrap_or(blocks::AIR);
        let in_water = survival::fluid_of(feet).is_some_and(|(f, _)| f == survival::Fluid::Water);
        let critical = full && p.fall_distance > 0.0 && !p.on_ground && !in_water && !p.sprinting && living && blocks::block_of_state(feet).name != "minecraft:ladder";
        if critical {
            base *= 1.5;
        }
        let total = base + enchant;
        let is_sword = weapon.0.as_ref().is_some_and(|s| paper_data::synced::tag("minecraft:item", "minecraft:swords").is_some_and(|ids| ids.contains(&s.item)));
        let sweep = full && !critical && !sprint_knockback && p.on_ground && !p.moved_this_tick && is_sword;
        let (yaw, at) = (p.loc.yaw, Vec3::new(p.loc.x, p.loc.y, p.loc.z));
        if let Some(p) = self.players.get_mut(&id) {
            p.last_swing = self.tick;
        }
        if sprint_knockback {
            self.play_sound(world, at, "minecraft:entity.player.attack.knockback", category::PLAYERS, 1.0, 1.0, None);
        }

        let hurt = self.hurt_entity(world, target, Damage::by("minecraft:player_attack", id, at), total);
        if !hurt {
            self.play_sound(world, at, "minecraft:entity.player.attack.nodamage", category::PLAYERS, 1.0, 1.0, None);
            return;
        }
        let knockback = level("minecraft:knockback") as f64 + if sprint_knockback { 1.0 } else { 0.0 };
        let (sx, cz) = ((yaw as f64).to_radians().sin(), -(yaw as f64).to_radians().cos());
        if knockback > 0.0 {
            self.knockback_entity(world, target, knockback * 0.5, sx, cz);
            if let Some(p) = self.players.get_mut(&id) {
                if p.sprinting {
                    p.sprinting = false;
                    p.metadata_dirty = true;
                }
            }
        }
        if sweep {
            let ratio = { let l = level("minecraft:sweeping_edge") as f32; l / (l + 1.0) };
            let sweep_damage = 1.0 + ratio * total;
            let area = target_box.inflate(1.0, 0.25, 1.0);
            let mut victims: Vec<i32> = self.worlds[world].entities_in(&area).into_iter().filter(|e| *e != target).collect();
            victims.extend(self.players.values().filter(|o| o.entity_id != id && o.entity_id != target && o.world == world && Aabb::of_entity(Vec3::new(o.loc.x, o.loc.y, o.loc.z), 0.6, 1.8).intersects(&area)).map(|o| o.entity_id));
            for v in victims {
                let pos = self.entity_position(world, v);
                if pos.is_some_and(|pos| pos.distance_squared(at) < 9.0) {
                    if self.hurt_entity(world, v, Damage::by("minecraft:player_attack", id, at), sweep_damage) {
                        self.knockback_entity(world, v, 0.4, sx, cz);
                    }
                }
            }
            self.play_sound(world, at, "minecraft:entity.player.attack.sweep", category::PLAYERS, 1.0, 1.0, None);
        }
        if critical {
            self.play_sound(world, at, "minecraft:entity.player.attack.crit", category::PLAYERS, 1.0, 1.0, None);
            self.broadcast_animation(world, target, 4);
        }
        if !critical && !sweep {
            let sound = if full { "minecraft:entity.player.attack.strong" } else { "minecraft:entity.player.attack.weak" };
            self.play_sound(world, at, sound, category::PLAYERS, 1.0, 1.0, None);
        }
        if enchant > 0.0 {
            self.broadcast_animation(world, target, 5);
        }
        let fire = level("minecraft:fire_aspect");
        if fire > 0 {
            self.set_on_fire(world, target, 80 * fire);
        }
        // Weapons wear with use (`weapon.item_damage_per_attack`).
        if let Some(stack) = weapon.0.as_ref() {
            // An empty `weapon` component means the default of one point per attack.
            let per_attack = item_data::by_id(stack.item as u32).raw.as_ref().and_then(|r| r.get("weapon")).map_or(0, |w| w.get("item_damage_per_attack").and_then(|v| v.as_i64()).unwrap_or(1)) as i32;
            if per_attack > 0 && self.players.get(&id).is_some_and(|p| p.game_mode != GameMode::Creative) {
                let slot = HOTBAR_START + self.players[&id].held_slot as usize;
                let mut item = self.players[&id].inventory[slot].clone();
                if survival::hurt_item(&mut item, per_attack, false) {
                    self.play_sound(world, at, "minecraft:entity.item.break", category::PLAYERS, 0.8, 1.0, None);
                }
                self.set_inventory_slot(id, slot, item);
            }
        }
        self.add_exhaustion(id, 0.1);
    }

    /// Plays an `Animate` animation (4 critical hit, 5 magic critical hit) for viewers.
    fn broadcast_animation(&self, world: usize, entity: i32, animation: u8) {
        let payload = Bytes::from(encode_packet(&play::Animate { entity_id: VarInt(entity), animation }));
        if let Some(p) = self.players.get(&entity) {
            p.conn.send_payload(payload.clone());
            for v in &p.seen_by {
                if let Some(viewer) = self.players.get(v) {
                    viewer.conn.send_payload(payload.clone());
                }
            }
        } else {
            self.broadcast_entity(world, entity, payload);
        }
    }

    pub fn entity_position(&self, world: usize, id: i32) -> Option<Vec3> {
        if let Some(p) = self.players.get(&id) {
            return Some(Vec3::new(p.loc.x, p.loc.y, p.loc.z));
        }
        self.worlds.get(world)?.entities.get(&id).map(|e| e.pos)
    }

    /// Sets a player or mob on fire for `ticks` (if longer than it already burns).
    pub fn set_on_fire(&mut self, world: usize, id: i32, ticks: i32) {
        if let Some(p) = self.players.get_mut(&id) {
            if p.fire_ticks < ticks {
                p.fire_ticks = ticks;
                p.metadata_dirty = true;
            }
            return;
        }
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            let immune = e.info.fire_immune;
            if let EntityKind::Mob(m) = &mut e.kind {
                if !immune && m.living.fire_ticks < ticks {
                    m.living.fire_ticks = ticks;
                    e.metadata_dirty = true;
                }
            }
        }
    }

    /// Knockback for a player or mob (`LivingEntity.knockback`).
    pub fn knockback_entity(&mut self, world: usize, id: i32, strength: f64, dx: f64, dz: f64) {
        if self.players.contains_key(&id) {
            self.knockback_player(id, strength, dx, dz);
            return;
        }
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let resistance = e.info.attribute("knockback_resistance").unwrap_or(0.0);
        let strength = strength * (1.0 - resistance);
        if strength <= 0.0 {
            return;
        }
        let (mut dx, mut dz) = (dx, dz);
        let mut rng = rand::thread_rng();
        while dx * dx + dz * dz < 1.0e-5 {
            dx = (rng.gen::<f64>() - rng.gen::<f64>()) * 0.01;
            dz = (rng.gen::<f64>() - rng.gen::<f64>()) * 0.01;
        }
        let len = (dx * dx + dz * dz).sqrt();
        let (nx, nz) = (dx / len * strength, dz / len * strength);
        let v = e.vel;
        e.vel = Vec3::new(v.x / 2.0 - nx, if e.on_ground { (v.y / 2.0 + strength).min(0.4) } else { v.y }, v.z / 2.0 - nz);
        e.force_sync = true;
    }

    // --- hurting entities ----------------------------------------------------------------------

    /// Hurts a player, mob or other entity. Returns whether the hit landed.
    pub fn hurt_entity(&mut self, world: usize, id: i32, damage: Damage, mut amount: f32) -> bool {
        if self.players.contains_key(&id) {
            return self.hurt_player(id, damage, amount);
        }
        let tick = self.tick;
        let Some(e) = self.worlds[world].entities.get(&id) else { return false };
        match &e.kind {
            EntityKind::Item { stack, .. } => {
                // Items burn and blow up; fire-resistant ones survive fire.
                let fire_resistant = stack.0.as_ref().is_some_and(|s| item_data::by_id(s.item as u32).fire_resistant);
                if damage.is("is_fire") && fire_resistant || stack.0.as_ref().is_some_and(|s| items::get(s.item as u32).is_some_and(|i| i.name == "minecraft:nether_star")) && damage.is("is_explosion") {
                    return false;
                }
                let e = self.worlds[world].entities.get_mut(&id).unwrap();
                e.removed = true;
                return true;
            }
            EntityKind::ExperienceOrb { .. } => {
                if damage.is("is_fire") || damage.is("is_explosion") {
                    self.worlds[world].entities.get_mut(&id).unwrap().removed = true;
                }
                return false;
            }
            EntityKind::Mob(m) => {
                if m.living.is_dead() || e.removed {
                    return false;
                }
            }
            _ => return false,
        }
        if damage.is("is_fire") && e.info.fire_immune && !damage.is("bypasses_invulnerability") {
            return false;
        }
        if damage.is("is_fire") && !damage.is("bypasses_effects") && self.effect_level(world, id, "minecraft:fire_resistance").is_some() {
            return false;
        }
        if let Some(level) = self.effect_level(world, id, "minecraft:resistance").filter(|_| !damage.is("bypasses_resistance")) {
            amount = (amount * (25 - (level + 1) * 5) as f32 / 25.0).max(0.0);
        }
        if !crate::plugins::events::on_entity_damage(self, id, &damage, &mut amount) {
            return false;
        }
        let e = self.worlds[world].entities.get_mut(&id).unwrap();
        let info = e.info;
        let pos = e.pos;
        let EntityKind::Mob(mob) = &mut e.kind else { return false };
        let living = &mut mob.living;
        let full = if living.invulnerable_time > 10 && !damage.is("bypasses_cooldown") {
            if amount <= living.last_hurt {
                return false;
            }
            let extra = amount - living.last_hurt;
            living.last_hurt = amount;
            amount = extra;
            false
        } else {
            living.last_hurt = amount;
            living.invulnerable_time = 20;
            living.hurt_time = 10;
            true
        };
        if !damage.is("bypasses_armor") {
            let armor = info.attribute("armor").unwrap_or(0.0) as f32
                + mob.equipment[2..6].iter().filter_map(|s| s.0.as_ref()).map(|s| item_data::by_id(s.item as u32)).map(|d| (d.modifier("minecraft:armor", "feet") + d.modifier("minecraft:armor", "legs") + d.modifier("minecraft:armor", "chest") + d.modifier("minecraft:armor", "head")) as f32).sum::<f32>();
            amount = survival::damage_after_armor(amount, armor, info.attribute("armor_toughness").unwrap_or(0.0) as f32);
        }
        let living = &mut mob.living;
        living.health = (living.health - amount.max(0.0)).max(0.0);
        if let Some(attacker) = damage.attacker {
            living.last_attacker = Some((attacker, tick));
        }
        let dead = living.health <= 0.0;
        // Passive animals panic; hostile and neutral mobs turn on whoever hurt them.
        if let Some(attacker) = damage.attacker.filter(|a| *a != id) {
            if info.category == "CREATURE" || info.category == "WATER_CREATURE" || info.category == "AMBIENT" {
                mob.brain.panic_ticks = 60 + rand::thread_rng().gen_range(0..40);
                mob.brain.path.clear();
            } else if !damage.is("no_anger") {
                mob.brain.target = Some(attacker);
            }
        }
        e.metadata_dirty = true;

        if full {
            let event = play::DamageEvent { entity_id: id, source_type: damage.network_id(), cause_id: damage.attacker, direct_id: damage.direct, source_position: damage.position.map(|v| (v.x, v.y, v.z)) };
            self.broadcast_entity(world, id, Bytes::from(encode_packet(&event)));
            if !damage.is("no_knockback") {
                if let Some(from) = damage.position {
                    self.knockback_entity(world, id, 0.4, from.x - pos.x, from.z - pos.z);
                }
            }
            if !dead {
                if let Some(sound) = info.hurt_sound.as_deref() {
                    self.play_sound(world, pos, sound, mob_category(info), 1.0, Self::mob_pitch(world, self, id), None);
                }
            }
        }
        if dead {
            self.kill_mob(world, id, damage);
        }
        true
    }

    fn mob_pitch(world: usize, server: &Server, id: i32) -> f32 {
        let baby = server.worlds[world].entities.get(&id).and_then(|e| e.mob()).is_some_and(|m| m.baby);
        let mut rng = rand::thread_rng();
        (rng.gen::<f32>() - rng.gen::<f32>()) * 0.2 + if baby { 1.5 } else { 1.0 }
    }

    /// A mob died: death animation, sound, loot and experience.
    fn kill_mob(&mut self, world: usize, id: i32, damage: Damage) {
        let tick = self.tick;
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (info, pos) = (e.info, e.pos);
        let Some(mob) = e.mob() else { return };
        let on_fire = mob.living.fire_ticks > 0;
        let baby = mob.baby;
        // Kill credit goes to a player who hurt it in the last five seconds.
        let killer = mob.living.last_attacker.filter(|(_, t)| tick.saturating_sub(*t) < 100).map(|(a, _)| a).filter(|a| self.players.contains_key(a));
        let equipment: Vec<Slot> = mob.equipment.iter().filter(|s| !s.is_empty()).cloned().collect();
        if !crate::plugins::events::on_entity_death(self, world, id) {
            // A plugin cancelled the death: back to half a heart.
            if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                m.living.health = 1.0;
            }
            return;
        }
        if let Some(sound) = info.death_sound.as_deref() {
            self.play_sound(world, pos, sound, mob_category(info), 1.0, Self::mob_pitch(world, self, id), None);
        }
        self.broadcast_entity(world, id, Bytes::from(encode_packet(&play::EntityEvent { entity_id: id, status: 3 })));
        if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
            m.living.death_time = 1;
            m.brain.target = None;
            m.brain.path.clear();
        }
        let rules = &self.worlds[world].rules;
        // Baby animals drop nothing; baby monsters (zombies) drop as adults, with 2.5 times
        // the experience.
        let baby_animal = baby && info.category != "MONSTER";
        if rules.bool("mob_drops") && !baby_animal {
            let tool = killer.and_then(|k| self.players.get(&k)).map(|p| p.held_item().clone());
            let table = format!("minecraft:entities/{}", info.name.strip_prefix("minecraft:").unwrap_or(&info.name));
            let drops = if self.loot.available() {
                let context = LootContext { tool: tool.as_ref(), killed_by_player: killer.is_some(), on_fire, explosion_radius: None, block_state: None };
                self.loot.roll(&table, &context)
            } else {
                Vec::new()
            };
            for stack in drops.into_iter().chain(equipment.into_iter().filter(|_| rand::thread_rng().gen::<f32>() < 0.085)) {
                let mut rng = rand::thread_rng();
                let vel = Vec3::new(rng.gen_range(-0.1..0.1), 0.2, rng.gen_range(-0.1..0.1));
                self.spawn_item(world, Vec3::new(pos.x, pos.y + 0.5, pos.z), stack, vel, 10);
            }
            if killer.is_some() && info.experience > 0 {
                let xp = if info.category == "MONSTER" { info.experience } else { info.experience + rand::thread_rng().gen_range(0..3) };
                let xp = if baby { (xp as f64 * 2.5) as i32 } else { xp };
                self.award_experience(world, pos, xp);
            }
        }
        if let Some(k) = killer {
            if let Some(p) = self.players.get(&k) {
                log::debug!("{} killed {} with {}", p.name(), info.name, damage.kind);
            }
        }
    }

    /// Death animation countdown; the body vanishes after a second (`tickDeath`).
    pub(super) fn tick_dying(&mut self, world: usize, id: i32) -> bool {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return false };
        let Some(m) = e.mob_mut() else { return false };
        if m.living.death_time == 0 {
            return false;
        }
        m.living.death_time += 1;
        if m.living.death_time >= 20 {
            e.removed = true;
            let payload = Bytes::from(encode_packet(&play::EntityEvent { entity_id: id, status: 60 }));
            self.broadcast_entity(world, id, payload);
        }
        true
    }

    // --- bows and arrows -----------------------------------------------------------------------

    /// Releases a drawn bow (`BowItem.releaseUsing`).
    pub(super) fn shoot_bow(&mut self, id: i32, bow_slot: usize, used_ticks: i32) {
        let f = used_ticks as f32 / 20.0;
        let power = ((f * f + f * 2.0) / 3.0).min(1.0);
        if power < 0.1 {
            return;
        }
        let Some(p) = self.players.get(&id) else { return };
        let creative = p.game_mode == GameMode::Creative;
        let bow = p.inventory[bow_slot].clone();
        let infinity = enchantment_level(Some(&bow), "minecraft:infinity") > 0;
        // The arrow comes from the off hand, then the hotbar and inventory.
        let mut arrow_slot = None;
        for slot in std::iter::once(super::player::OFFHAND_SLOT).chain(HOTBAR_START..HOTBAR_START + 9).chain(9..HOTBAR_START) {
            if p.inventory[slot].0.as_ref().is_some_and(|s| survival::is_arrow(s.item)) {
                arrow_slot = Some(slot);
                break;
            }
        }
        if arrow_slot.is_none() && !creative {
            return;
        }
        let (world, loc) = (p.world, p.loc);
        let origin = Vec3::new(loc.x, loc.y + p.eye_height() - 0.1, loc.z);
        let mut damage = 2.0;
        let power_level = enchantment_level(Some(&bow), "minecraft:power");
        if power_level > 0 {
            damage += 0.5 * power_level as f64 + 0.5;
        }
        let keep_arrow = creative || infinity && arrow_slot.is_some_and(|s| p.inventory[s].0.as_ref().is_some_and(|a| items::get(a.item as u32).is_some_and(|i| i.name == "minecraft:arrow")));
        self.spawn_arrow(world, origin, loc.yaw, loc.pitch, power as f64 * 3.0, 1.0, Some(id), damage, power >= 1.0, !keep_arrow);
        let pitch = 1.0 / (rand::thread_rng().gen::<f32>() * 0.4 + 1.2) + power * 0.5;
        self.play_sound(world, Vec3::new(loc.x, loc.y, loc.z), "minecraft:entity.arrow.shoot", category::PLAYERS, 1.0, pitch, None);
        let Some(p) = self.players.get_mut(&id) else { return };
        if !creative {
            if survival::hurt_item(&mut p.inventory[bow_slot], 1, false) {
                p.equipment_dirty = true;
            }
            if !keep_arrow {
                if let Some(slot) = arrow_slot {
                    if let Some(a) = p.inventory[slot].0.as_mut() {
                        a.count -= 1;
                        if a.count <= 0 {
                            p.inventory[slot] = Slot::EMPTY;
                        }
                    }
                }
            }
            self.resync_inventory(id);
        }
    }

    /// Spawns an arrow flying along a rotation (`Projectile.shootFromRotation`).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_arrow(&mut self, world: usize, origin: Vec3, yaw: f32, pitch: f32, velocity: f64, inaccuracy: f64, owner: Option<i32>, damage: f64, critical: bool, pickup: bool) -> i32 {
        let dir = Vec3::from_rotation(yaw, pitch);
        self.spawn_arrow_towards(world, origin, dir, velocity, inaccuracy, owner, damage, critical, pickup)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_arrow_towards(&mut self, world: usize, origin: Vec3, dir: Vec3, velocity: f64, inaccuracy: f64, owner: Option<i32>, damage: f64, critical: bool, pickup: bool) -> i32 {
        let mut rng = rand::thread_rng();
        let mut triangle = || (rng.gen::<f64>() - rng.gen::<f64>()) * 0.0172275 * inaccuracy;
        let d = dir.normalize();
        let vel = Vec3::new(d.x + triangle(), d.y + triangle(), d.z + triangle()).scale(velocity);
        let info = entity_types::by_name("minecraft:arrow").expect("arrow entity type");
        let mut e = Entity::new(self.next_entity_id(), info, origin, EntityKind::Arrow { damage, owner, in_ground: false, life: 0, pickup, critical });
        e.vel = vel;
        let h = vel.horizontal_length();
        e.yaw = (vel.x.atan2(vel.z).to_degrees()) as f32;
        e.pitch = (vel.y.atan2(h).to_degrees()) as f32;
        e.yaw = -e.yaw;
        e.pitch = -e.pitch;
        self.spawn_entity(world, e)
    }

    /// Arrow flight, hits and pickup (`AbstractArrow.tick`).
    pub(super) fn tick_arrow(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let EntityKind::Arrow { damage, owner, in_ground, critical, pickup, .. } = e.kind else { return };
        if in_ground {
            // Stuck: wait to be picked up, despawn after a minute, fall when the block goes.
            let below = e.pos.add(e.vel.normalize().scale(0.05));
            let stuck = crate::world::entity::collides(&self.worlds[world], &Aabb::new(below, below).inflate(0.06, 0.06, 0.06));
            let e = self.worlds[world].entities.get_mut(&id).unwrap();
            if let EntityKind::Arrow { life, in_ground, .. } = &mut e.kind {
                *life += 1;
                if *life >= 1200 {
                    e.removed = true;
                }
                if !stuck {
                    *in_ground = false;
                    e.vel = Vec3::new(0.0, -0.05, 0.0);
                    e.metadata_dirty = true;
                }
            }
            if pickup {
                self.pick_up_arrow(world, id);
            }
            return;
        }
        let (from, vel) = (e.pos, e.vel);
        let to = from.add(vel);
        let w = &self.worlds[world];
        let block_hit = clip_blocks(w, from, to);
        let end = block_hit.map_or(to, |(t, _)| from.add(vel.scale(t)));
        // Entities along the path (the shooter only after the first few ticks).
        let swept = Aabb::of_entity(from, 0.5, 0.5).expand_towards(vel).inflate(1.0, 1.0, 1.0);
        let mut target: Option<(f64, i32)> = None;
        let age = e.age;
        let mut consider = |cand: i32, b: Aabb| {
            if Some(cand) == owner && age < 5 {
                return;
            }
            if let Some(t) = b.inflate(0.3, 0.3, 0.3).clip_ray(from, end) {
                if target.is_none_or(|(bt, _)| t < bt) {
                    target = Some((t, cand));
                }
            }
        };
        for p in self.players.values().filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator) {
            let b = Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8);
            if b.intersects(&swept) {
                consider(p.entity_id, b);
            }
        }
        for cand in w.entities_in(&swept) {
            if let Some(c) = w.entities.get(&cand) {
                if matches!(&c.kind, EntityKind::Mob(m) if !m.living.is_dead()) {
                    consider(cand, c.bounding_box());
                }
            }
        }

        if let Some((_, victim)) = target {
            let speed = vel.length();
            let mut amount = (speed * damage).clamp(0.0, i32::MAX as f64).ceil() as i32;
            if critical {
                amount += rand::thread_rng().gen_range(0..amount / 2 + 2);
            }
            let source = Damage { kind: "minecraft:arrow", attacker: owner, direct: Some(id), position: Some(from) };
            if self.hurt_entity(world, victim, source, amount as f32) {
                let at = self.entity_position(world, victim).unwrap_or(from);
                self.play_sound(world, at, "minecraft:entity.arrow.hit", category::NEUTRAL, 1.0, 1.2 / (rand::thread_rng().gen::<f32>() * 0.2 + 0.9), None);
                if let Some(o) = owner.filter(|o| self.players.contains_key(o) && victim != *o && self.players.contains_key(&victim)) {
                    self.send(o, &play::GameEvent { event: 6, value: 0.0 });
                }
                if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                    e.removed = true;
                }
            } else if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                // Bounced off.
                e.vel = e.vel.scale(-0.1);
                e.yaw += 180.0;
            }
            return;
        }

        let e = self.worlds[world].entities.get_mut(&id).unwrap();
        if let Some((_, block)) = block_hit {
            // Stick into the block, just short of its surface.
            e.pos = end.sub(vel.normalize().scale(0.05));
            e.vel = vel.normalize().scale(0.0001);
            if let EntityKind::Arrow { in_ground, critical, life, .. } = &mut e.kind {
                *in_ground = true;
                *critical = false;
                *life = 0;
            }
            e.metadata_dirty = true;
            e.force_sync = true;
            let _ = block;
            let at = e.pos;
            self.play_sound(world, at, "minecraft:entity.arrow.hit", category::NEUTRAL, 1.0, 1.2 / (rand::thread_rng().gen::<f32>() * 0.2 + 0.9), None);
            return;
        }
        e.pos = to;
        let in_water = self.worlds[world].block_at(to.x.floor() as i32, to.y.floor() as i32, to.z.floor() as i32).and_then(survival::fluid_of).is_some_and(|(f, _)| f == survival::Fluid::Water);
        let e = self.worlds[world].entities.get_mut(&id).unwrap();
        e.vel = e.vel.scale(if in_water { 0.6 } else { 0.99 });
        if !e.no_gravity {
            e.vel.y -= 0.05;
        }
        let h = e.vel.horizontal_length();
        e.yaw = -(e.vel.x.atan2(e.vel.z).to_degrees() as f32);
        e.pitch = -(e.vel.y.atan2(h).to_degrees() as f32);
    }

    fn pick_up_arrow(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let area = e.bounding_box().inflate(1.0, 0.5, 1.0);
        let collector = self
            .players
            .values()
            .find(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator && Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8).intersects(&area))
            .map(|p| p.entity_id);
        let Some(player) = collector else { return };
        let Some(arrow_item) = items::id("minecraft:arrow") else { return };
        let stack = ItemStack::new(arrow_item as i32, 1);
        if self.players[&player].game_mode != GameMode::Creative && self.add_to_inventory(player, &stack) > 0 {
            return;
        }
        let packet = play::TakeItemEntity { collected_id: VarInt(id), collector_id: VarInt(player), count: VarInt(1) };
        let payload = Bytes::from(encode_packet(&packet));
        let mut viewers: HashSet<i32> = self.worlds[world].entities.get(&id).map(|e| e.viewers.clone()).unwrap_or_default();
        viewers.insert(player);
        for v in viewers {
            if let Some(p) = self.players.get(&v) {
                p.conn.send_payload(payload.clone());
            }
        }
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            e.removed = true;
        }
    }

    // --- explosions ----------------------------------------------------------------------------

    /// An explosion (`ServerExplosion.explode`): breaks blocks along rays, hurts and pushes
    /// entities, and tells nearby clients.
    pub fn explode(&mut self, world: usize, center: Vec3, radius: f32, destroy_blocks: bool, source: Option<i32>) {
        let mut rng = rand::thread_rng();
        let mut affected: HashSet<(i32, i32, i32)> = HashSet::new();
        if destroy_blocks {
            let w = &self.worlds[world];
            for j in 0..16 {
                for k in 0..16 {
                    for l in 0..16 {
                        if j != 0 && j != 15 && k != 0 && k != 15 && l != 0 && l != 15 {
                            continue;
                        }
                        let dir = Vec3::new(j as f64 / 15.0 * 2.0 - 1.0, k as f64 / 15.0 * 2.0 - 1.0, l as f64 / 15.0 * 2.0 - 1.0).normalize();
                        let mut h = radius * (0.7 + rng.gen::<f32>() * 0.6);
                        let mut p = center;
                        while h > 0.0 {
                            let (x, y, z) = (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);
                            if !w.in_bounds(y) {
                                break;
                            }
                            let Some(state) = w.block_at(x, y, z) else { break };
                            if !blocks::is_air(state) {
                                let block = blocks::block_of_state(state);
                                let fluid_resistance = if survival::fluid_of(state).is_some() { 100.0 } else { 0.0 };
                                let resistance = block.blast_resistance.max(fluid_resistance);
                                h -= (resistance + 0.3) * 0.3;
                                if h > 0.0 && block.hardness >= 0.0 {
                                    affected.insert((x, y, z));
                                }
                            }
                            p = p.add(dir.scale(0.3));
                            h -= 0.225_000_01;
                        }
                    }
                }
            }
        }

        // Entities in range are hurt and pushed by how much of them the blast can see.
        let reach = radius as f64 * 2.0;
        let area = Aabb::new(center.sub(Vec3::new(reach + 1.0, reach + 1.0, reach + 1.0)), center.add(Vec3::new(reach + 1.0, reach + 1.0, reach + 1.0)));
        let mut targets: Vec<(i32, Aabb, Vec3, bool)> = Vec::new();
        for p in self.players.values().filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator) {
            let pos = Vec3::new(p.loc.x, p.loc.y, p.loc.z);
            let b = Aabb::of_entity(pos, 0.6, 1.8);
            if b.intersects(&area) {
                targets.push((p.entity_id, b, Vec3::new(pos.x, pos.y + p.eye_height(), pos.z), true));
            }
        }
        for id in self.worlds[world].entities_in(&area) {
            if Some(id) == source {
                continue;
            }
            if let Some(e) = self.worlds[world].entities.get(&id) {
                let eye_y = if matches!(e.kind, EntityKind::Tnt { .. }) { e.pos.y } else { e.pos.y + e.info.eye_height as f64 };
                targets.push((id, e.bounding_box(), Vec3::new(e.pos.x, eye_y, e.pos.z), false));
            }
        }
        let mut player_knockback: Vec<(i32, Vec3)> = Vec::new();
        for (id, b, eye, is_player) in targets {
            let feet = Vec3::new(eye.x, b.min.y, eye.z);
            let dist = feet.sub(center).length() / reach;
            if dist > 1.0 {
                continue;
            }
            let dir = eye.sub(center);
            if dir.length_squared() < 1.0e-12 {
                continue;
            }
            let dir = dir.normalize();
            let seen = seen_percent(&self.worlds[world], center, &b);
            let impact = (1.0 - dist) * seen;
            let damage = ((impact * impact + impact) / 2.0 * 7.0 * reach + 1.0) as f32;
            let damage_source = Damage { kind: if source.is_some_and(|s| self.players.contains_key(&s)) { "minecraft:player_explosion" } else { "minecraft:explosion" }, attacker: source, direct: source, position: Some(center) };
            self.hurt_entity(world, id, damage_source, damage);
            let push = dir.scale(impact);
            if is_player {
                let creative_flying = self.players.get(&id).is_some_and(|p| p.game_mode == GameMode::Creative && p.flying);
                if !creative_flying {
                    player_knockback.push((id, push));
                }
            } else if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                let resistance = e.info.attribute("explosion_knockback_resistance").unwrap_or(0.0);
                e.vel = e.vel.add(push.scale(1.0 - resistance));
                e.force_sync = true;
            }
        }

        // Blocks: drop their loot (some of it) and vanish; TNT is set off.
        let mut broken = 0;
        if destroy_blocks && !affected.is_empty() {
            let decay = match source.and_then(|s| self.worlds[world].entities.get(&s)).map(|e| e.info.name.as_str()) {
                Some("minecraft:tnt") => self.worlds[world].rules.bool("tnt_explosion_drop_decay"),
                Some(_) => self.worlds[world].rules.bool("mob_explosion_drop_decay"),
                None => self.worlds[world].rules.bool("block_explosion_drop_decay"),
            };
            let drops = self.worlds[world].rules.bool("block_drops");
            let mut list: Vec<(i32, i32, i32)> = affected.into_iter().collect();
            list.sort_unstable();
            for (x, y, z) in list {
                let Some(state) = self.worlds[world].block_at(x, y, z) else { continue };
                if blocks::is_air(state) {
                    continue;
                }
                let pos = BlockPos::new(x, y, z);
                if blocks::block_of_state(state).name == "minecraft:tnt" {
                    self.set_block_and_update(world, pos, blocks::AIR);
                    if self.worlds[world].rules.bool("tnt_explodes") {
                        self.prime_tnt(world, pos, rng.gen_range(10..30), source);
                    }
                    broken += 1;
                    continue;
                }
                if drops {
                    self.drop_block_loot(world, pos, state, None, if decay { Some(radius) } else { None });
                }
                if self.set_block_and_update(world, pos, blocks::AIR) {
                    broken += 1;
                }
            }
        }

        // The packet with the particles, the sound and each player's own push.
        let particle_name = if radius >= 2.0 && destroy_blocks { "minecraft:explosion_emitter" } else { "minecraft:explosion" };
        let particle = paper_data::registries::id("minecraft:particle_type", particle_name).unwrap_or(0) as i32;
        let sound = match paper_data::registries::id("minecraft:sound_event", "minecraft:entity.generic.explode") {
            Some(i) => play::SoundRef::Id(i as i32),
            None => play::SoundRef::Named { name: "minecraft:entity.generic.explode".into(), fixed_range: None },
        };
        let recipients: Vec<i32> = self.players.values().filter(|p| p.world == world && Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(center) < 4096.0).map(|p| p.entity_id).collect();
        for r in recipients {
            let knockback = player_knockback.iter().find(|(id, _)| *id == r).map(|(_, v)| (v.x, v.y, v.z));
            self.send(r, &play::Explode { center: (center.x, center.y, center.z), radius, block_count: broken, knockback, particle, sound: sound.clone() });
        }
    }

    /// Lights TNT at a block position.
    pub fn prime_tnt(&mut self, world: usize, pos: BlockPos, fuse: i32, _igniter: Option<i32>) {
        let Some(info) = entity_types::by_name("minecraft:tnt") else { return };
        let mut e = Entity::new(self.next_entity_id(), info, Vec3::new(pos.x as f64 + 0.5, pos.y as f64, pos.z as f64 + 0.5), EntityKind::Tnt { fuse });
        let angle = rand::thread_rng().gen::<f64>() * std::f64::consts::TAU;
        e.vel = Vec3::new(-angle.sin() * 0.02, 0.2, -angle.cos() * 0.02);
        let at = e.pos;
        self.spawn_entity(world, e);
        self.play_sound(world, at, "minecraft:entity.tnt.primed", category::BLOCKS, 1.0, 1.0, None);
    }
}

/// How much of a box the explosion can see (`getSeenPercent`).
fn seen_percent(world: &World, center: Vec3, b: &Aabb) -> f64 {
    let size = b.max.sub(b.min);
    let step = |d: f64| 1.0 / (d * 2.0 + 1.0);
    let (sx, sy, sz) = (step(size.x), step(size.y), step(size.z));
    if sx < 0.0 || sy < 0.0 || sz < 0.0 {
        return 0.0;
    }
    let ox = (1.0 - (1.0 / sx).floor() * sx) / 2.0;
    let oz = (1.0 - (1.0 / sz).floor() * sz) / 2.0;
    let (mut seen, mut total) = (0, 0);
    let mut fx = 0.0;
    while fx <= 1.0 {
        let mut fy = 0.0;
        while fy <= 1.0 {
            let mut fz = 0.0;
            while fz <= 1.0 {
                let p = Vec3::new(b.min.x + size.x * fx + ox, b.min.y + size.y * fy, b.min.z + size.z * fz + oz);
                if clip_blocks(world, p, center).is_none() {
                    seen += 1;
                }
                total += 1;
                fz += sz;
            }
            fy += sy;
        }
        fx += sx;
    }
    if total == 0 {
        0.0
    } else {
        seen as f64 / total as f64
    }
}

fn mob_category(info: &entity_types::EntityTypeInfo) -> i32 {
    if info.category == "MONSTER" {
        category::HOSTILE
    } else {
        category::NEUTRAL
    }
}
