//! Status effects on players and mobs (vanilla's `MobEffectInstance` and the effects'
//! `applyEffectTick`), and the effects of consumed items (`Consumable.onConsume`).

use super::survival::Damage;
use super::Server;
use paper_data::effects;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use paper_protocol::types::ItemStack;
use rand::Rng;

/// An effect on a player.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveEffect {
    /// Protocol id in `minecraft:mob_effect`.
    pub id: i32,
    pub amplifier: i32,
    /// Ticks left; -1 lasts forever.
    pub duration: i32,
    pub ambient: bool,
    pub particles: bool,
    pub icon: bool,
}

impl ActiveEffect {
    pub fn new(name: &str, duration: i32, amplifier: i32) -> Option<ActiveEffect> {
        Some(ActiveEffect { id: effects::by_name(name)?.id, amplifier: amplifier.clamp(0, 255), duration, ambient: false, particles: true, icon: true })
    }

    pub fn name(&self) -> &'static str {
        effects::by_id(self.id).map_or("", |e| e.name.as_str())
    }

    fn flags(&self) -> i8 {
        (self.ambient as i8) | (self.particles as i8) << 1 | (self.icon as i8) << 2
    }

    /// Whether `other` replaces this one (`MobEffectInstance.update`): stronger, or as strong
    /// and longer.
    fn replaced_by(&self, other: &ActiveEffect) -> bool {
        other.amplifier > self.amplifier || other.amplifier == self.amplifier && (self.duration != -1 && (other.duration == -1 || other.duration > self.duration))
    }
}

fn effect_id(name: &str) -> i32 {
    effects::by_name(name).map_or(-1, |e| e.id)
}

/// Undead mobs are healed by instant damage and hurt by instant health.
fn is_undead(name: &str) -> bool {
    paper_data::entity_types::by_name(name).is_some_and(|info| paper_data::synced::tag("minecraft:entity_type", "minecraft:undead").is_some_and(|ids| ids.contains(&info.id)))
}

/// Ticks between two applications of a periodic effect, or `None` if it does nothing per
/// tick (`MobEffect.shouldApplyEffectTickThisTick`).
fn interval(name: &str, amplifier: i32) -> Option<i32> {
    let base = match name {
        "minecraft:regeneration" => 50,
        "minecraft:poison" => 25,
        "minecraft:wither" => 40,
        "minecraft:hunger" | "minecraft:saturation" => return Some(1),
        _ => return None,
    };
    Some((base >> amplifier.clamp(0, 31)).max(1))
}

impl Server {
    /// The amplifier of an effect a player or mob has.
    pub fn effect_level(&self, world: usize, target: i32, name: &str) -> Option<i32> {
        let id = effect_id(name);
        if let Some(p) = self.players.get(&target) {
            return p.effects.iter().find(|e| e.id == id).map(|e| e.amplifier);
        }
        let mob = self.worlds.get(world)?.entities.get(&target)?.mob()?;
        mob.living.effects.iter().find(|(e, _, _)| *e == id).map(|(_, amp, _)| *amp)
    }

    /// The effects a player or mob has now.
    pub fn effects_of(&self, world: usize, target: i32) -> Vec<ActiveEffect> {
        if let Some(p) = self.players.get(&target) {
            return p.effects.clone();
        }
        let Some(mob) = self.worlds.get(world).and_then(|w| w.entities.get(&target)).and_then(|e| e.mob()) else { return Vec::new() };
        mob.living.effects.iter().map(|(id, amplifier, duration)| ActiveEffect { id: *id, amplifier: *amplifier, duration: *duration, ambient: false, particles: true, icon: true }).collect()
    }

    /// Gives an effect (instant ones act at once). Returns whether it took effect.
    pub fn add_effect(&mut self, world: usize, target: i32, effect: ActiveEffect) -> bool {
        let Some(info) = effects::by_id(effect.id) else { return false };
        if info.instantaneous {
            self.apply_instant(world, target, &info.name, effect.amplifier);
            return true;
        }
        if let Some(p) = self.players.get_mut(&target) {
            if p.dead {
                return false;
            }
            match p.effects.iter_mut().find(|e| e.id == effect.id) {
                Some(existing) if !existing.replaced_by(&effect) => return false,
                Some(existing) => *existing = effect.clone(),
                None => p.effects.push(effect.clone()),
            }
            if info.name == "minecraft:absorption" {
                p.absorption = p.absorption.max(4.0 * (effect.amplifier + 1) as f32);
            }
            p.metadata_dirty = true;
            let packet = play::UpdateMobEffect { entity_id: VarInt(target), effect: VarInt(effect.id), amplifier: VarInt(effect.amplifier), duration: VarInt(effect.duration), flags: effect.flags() };
            p.conn.send(&packet);
            self.broadcast_to_viewers(target, &packet);
            self.sync_effect_attributes(target);
            return true;
        }
        let Some(e) = self.worlds.get_mut(world).and_then(|w| w.entities.get_mut(&target)) else { return false };
        let Some(mob) = e.mob_mut() else { return false };
        match mob.living.effects.iter_mut().find(|(id, _, _)| *id == effect.id) {
            Some(existing) => {
                if effect.amplifier < existing.1 || effect.amplifier == existing.1 && effect.duration <= existing.2 {
                    return false;
                }
                *existing = (effect.id, effect.amplifier, effect.duration);
            }
            None => mob.living.effects.push((effect.id, effect.amplifier, effect.duration)),
        }
        let packet = play::UpdateMobEffect { entity_id: VarInt(target), effect: VarInt(effect.id), amplifier: VarInt(effect.amplifier), duration: VarInt(effect.duration), flags: effect.flags() };
        self.broadcast_entity(world, target, bytes::Bytes::from(paper_protocol::packets::encode_packet(&packet)));
        true
    }

    /// Removes one effect. Returns whether there was one.
    pub fn remove_effect(&mut self, world: usize, target: i32, id: i32) -> bool {
        if let Some(p) = self.players.get_mut(&target) {
            let before = p.effects.len();
            p.effects.retain(|e| e.id != id);
            if p.effects.len() == before {
                return false;
            }
            if effects::by_id(id).is_some_and(|e| e.name == "minecraft:absorption") {
                p.absorption = 0.0;
            }
            p.metadata_dirty = true;
            let packet = play::RemoveMobEffect { entity_id: VarInt(target), effect: VarInt(id) };
            p.conn.send(&packet);
            self.broadcast_to_viewers(target, &packet);
            self.sync_effect_attributes(target);
            return true;
        }
        let Some(e) = self.worlds.get_mut(world).and_then(|w| w.entities.get_mut(&target)) else { return false };
        let Some(mob) = e.mob_mut() else { return false };
        let before = mob.living.effects.len();
        mob.living.effects.retain(|(e, _, _)| *e != id);
        let removed = mob.living.effects.len() != before;
        if removed {
            let packet = play::RemoveMobEffect { entity_id: VarInt(target), effect: VarInt(id) };
            self.broadcast_entity(world, target, bytes::Bytes::from(paper_protocol::packets::encode_packet(&packet)));
        }
        removed
    }

    /// Removes every effect (milk, death). Returns how many there were.
    pub fn clear_effects(&mut self, world: usize, target: i32) -> usize {
        let ids: Vec<i32> = match self.players.get(&target) {
            Some(p) => p.effects.iter().map(|e| e.id).collect(),
            None => self.worlds.get(world).and_then(|w| w.entities.get(&target)).and_then(|e| e.mob()).map(|m| m.living.effects.iter().map(|(id, _, _)| *id).collect()).unwrap_or_default(),
        };
        for id in &ids {
            self.remove_effect(world, target, *id);
        }
        ids.len()
    }

    /// Instant health and damage, saturation.
    fn apply_instant(&mut self, world: usize, target: i32, name: &str, amplifier: i32) {
        let undead = !self.players.contains_key(&target) && self.worlds.get(world).and_then(|w| w.entities.get(&target)).is_some_and(|e| is_undead(&e.info.name));
        let heal = (4 << amplifier.clamp(0, 30)) as f32;
        let harm = (6 << amplifier.clamp(0, 30)) as f32;
        match (name, undead) {
            ("minecraft:instant_health", false) | ("minecraft:instant_damage", true) => self.heal_entity(world, target, heal),
            ("minecraft:instant_health", true) | ("minecraft:instant_damage", false) => {
                self.hurt_entity(world, target, Damage::new("minecraft:magic"), harm);
            }
            ("minecraft:saturation", _) => {
                if let Some(p) = self.players.get_mut(&target) {
                    p.food = (p.food + amplifier + 1).min(20);
                    p.saturation = (p.saturation + (amplifier + 1) as f32 * 2.0).min(p.food as f32);
                }
            }
            _ => {}
        }
    }

    /// Heals a player or mob.
    pub fn heal_entity(&mut self, world: usize, target: i32, amount: f32) {
        if self.players.contains_key(&target) {
            self.heal_player(target, amount);
            return;
        }
        if let Some(e) = self.worlds.get_mut(world).and_then(|w| w.entities.get_mut(&target)) {
            if let Some(m) = e.mob_mut() {
                if !m.living.is_dead() {
                    m.living.health = (m.living.health + amount).min(m.living.max_health);
                    e.metadata_dirty = true;
                }
            }
        }
    }

    /// Counts effects down and applies the periodic ones.
    pub(super) fn tick_effects(&mut self) {
        // Players.
        for id in self.player_order.clone() {
            let Some(p) = self.players.get_mut(&id) else { continue };
            if p.dead || p.effects.is_empty() {
                continue;
            }
            let world = p.world;
            let mut expired = Vec::new();
            let mut periodic = Vec::new();
            for e in p.effects.iter_mut() {
                if let Some(every) = interval(e.name(), e.amplifier) {
                    if e.duration == -1 || e.duration % every == 0 {
                        periodic.push((e.name(), e.amplifier));
                    }
                }
                if e.duration > 0 {
                    e.duration -= 1;
                    if e.duration == 0 {
                        expired.push(e.id);
                    }
                }
            }
            for (name, amplifier) in periodic {
                self.apply_periodic(world, id, name, amplifier);
            }
            for effect in expired {
                self.remove_effect(world, id, effect);
            }
        }
        // Mobs (only those with effects).
        for world in 0..self.worlds.len() {
            let with_effects: Vec<i32> = self.worlds[world].entities.values().filter(|e| e.mob().is_some_and(|m| !m.living.effects.is_empty() && !m.living.is_dead())).map(|e| e.id).collect();
            for id in with_effects {
                let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) else { continue };
                let mut expired = Vec::new();
                let mut periodic = Vec::new();
                for (effect, amplifier, ticks) in m.living.effects.iter_mut() {
                    let name = effects::by_id(*effect).map_or("", |e| e.name.as_str());
                    if let Some(every) = interval(name, *amplifier) {
                        if *ticks == -1 || *ticks % every == 0 {
                            periodic.push((name, *amplifier));
                        }
                    }
                    if *ticks > 0 {
                        *ticks -= 1;
                        if *ticks == 0 {
                            expired.push(*effect);
                        }
                    }
                }
                for (name, amplifier) in periodic {
                    self.apply_periodic(world, id, name, amplifier);
                }
                for effect in expired {
                    self.remove_effect(world, id, effect);
                }
            }
        }
    }

    fn apply_periodic(&mut self, world: usize, target: i32, name: &str, amplifier: i32) {
        match name {
            "minecraft:regeneration" => self.heal_entity(world, target, 1.0),
            "minecraft:poison" => {
                // Poison never kills.
                let health = self.players.get(&target).map(|p| p.health).or_else(|| self.worlds.get(world).and_then(|w| w.entities.get(&target)).and_then(|e| e.mob()).map(|m| m.living.health)).unwrap_or(0.0);
                if health > 1.0 {
                    self.hurt_entity(world, target, Damage::new("minecraft:magic"), 1.0);
                }
            }
            "minecraft:wither" => {
                self.hurt_entity(world, target, Damage::new("minecraft:wither"), 1.0);
            }
            "minecraft:hunger" => self.add_exhaustion(target, 0.005 * (amplifier + 1) as f32),
            "minecraft:saturation" => self.apply_instant(world, target, name, amplifier),
            _ => {}
        }
    }

    /// Sends a player the attribute values its effects change (speed, jump strength...).
    pub(super) fn sync_effect_attributes(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else { return };
        // Base values of the player's attributes that effects modify.
        let bases: &[(&str, f64)] = &[
            ("minecraft:movement_speed", 0.1),
            ("minecraft:attack_speed", 4.0),
            ("minecraft:attack_damage", 1.0),
            ("minecraft:jump_strength", 0.41999998688697815),
            ("minecraft:safe_fall_distance", 3.0),
            ("minecraft:max_absorption", 0.0),
            ("minecraft:block_break_speed", 1.0),
            ("minecraft:luck", 0.0),
            ("minecraft:gravity", 0.08),
        ];
        let mut attributes = Vec::new();
        for (attribute, base) in bases {
            let Some(attr_id) = paper_data::registries::id("minecraft:attribute", attribute) else { continue };
            let mut modifiers = Vec::new();
            for e in &p.effects {
                for (attr, op, amount, modifier) in effects::by_id(e.id).map(|i| i.modifiers.as_slice()).unwrap_or(&[]) {
                    if attr != attribute {
                        continue;
                    }
                    let op = match op.as_str() {
                        "add_value" => 0,
                        "add_multiplied_base" => 1,
                        _ => 2,
                    };
                    modifiers.push((modifier.clone(), amount * (e.amplifier + 1) as f64, op));
                }
            }
            attributes.push((attr_id as i32, *base, modifiers));
        }
        p.conn.send(&play::UpdateAttributes { entity_id: id, attributes });
    }

    // --- consuming ---------------------------------------------------------------------------

    /// The `on_consume_effects` of an eaten or drunk item, and a potion's effects.
    pub(super) fn consume_effects(&mut self, id: i32, stack: &ItemStack) {
        let Some(world) = self.players.get(&id).map(|p| p.world) else { return };
        let data = paper_data::item_data::by_id(stack.item as u32);
        let mut rng = rand::thread_rng();
        if let Some(list) = data.raw.as_ref().and_then(|r| r.get("consumable")).and_then(|c| c.get("on_consume_effects")).and_then(|l| l.as_array()) {
            for entry in list {
                match entry.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "minecraft:apply_effects" => {
                        let probability = entry.get("probability").and_then(|p| p.as_f64()).unwrap_or(1.0);
                        if rng.gen::<f64>() >= probability {
                            continue;
                        }
                        for e in entry.get("effects").and_then(|e| e.as_array()).unwrap_or(&Vec::new()) {
                            let name = e.get("id").and_then(|i| i.as_str()).unwrap_or("");
                            let duration = e.get("duration").and_then(|d| d.as_i64()).unwrap_or(0) as i32;
                            let amplifier = e.get("amplifier").and_then(|a| a.as_i64()).unwrap_or(0) as i32;
                            if let Some(mut effect) = ActiveEffect::new(name, duration, amplifier) {
                                effect.icon = e.get("show_icon").and_then(|v| v.as_bool()).unwrap_or(true);
                                self.add_effect(world, id, effect);
                            }
                        }
                    }
                    "minecraft:clear_all_effects" => {
                        self.clear_effects(world, id);
                    }
                    "minecraft:remove_effects" => {
                        let names: Vec<String> = match entry.get("effects") {
                            Some(serde_json::Value::String(s)) => vec![s.clone()],
                            Some(serde_json::Value::Array(l)) => l.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
                            _ => Vec::new(),
                        };
                        for name in names {
                            self.remove_effect(world, id, effect_id(&name));
                        }
                    }
                    "minecraft:teleport_randomly" => {
                        let diameter = entry.get("diameter").and_then(|d| d.as_f64()).unwrap_or(16.0);
                        self.teleport_randomly(id, diameter);
                    }
                    _ => {}
                }
            }
        }
        // Potions name their effects in the `potion_contents` component.
        if let Some(potion) = potion_of(stack) {
            for (effect, duration, amplifier) in effects::potion(&potion) {
                if let Some(e) = ActiveEffect::new(effect, *duration, *amplifier) {
                    self.add_effect(world, id, e);
                }
            }
        }
    }

    /// Chorus fruit: up to sixteen tries at a safe spot nearby (`teleport_randomly`).
    fn teleport_randomly(&mut self, id: i32, diameter: f64) {
        let Some(p) = self.players.get(&id) else { return };
        let (world, loc) = (p.world, p.loc);
        let w = &self.worlds[world];
        let mut rng = rand::thread_rng();
        for _ in 0..16 {
            let x = loc.x + (rng.gen::<f64>() - 0.5) * diameter;
            let z = loc.z + (rng.gen::<f64>() - 0.5) * diameter;
            let mut y = (loc.y + (rng.gen_range(0..16) as f64 - 8.0)).clamp(w.min_y as f64, (w.max_y() - 1) as f64).floor() as i32;
            // Down to the ground, then check there is room.
            while y > w.min_y && w.block_at(x.floor() as i32, y - 1, z.floor() as i32).is_some_and(|s| !paper_data::blocks::has_collision(s)) {
                y -= 1;
            }
            let free = |yy: i32| w.block_at(x.floor() as i32, yy, z.floor() as i32).is_some_and(|s| !paper_data::blocks::has_collision(s) && super::survival::fluid_of(s).is_none());
            let ground = w.block_at(x.floor() as i32, y - 1, z.floor() as i32).is_some_and(paper_data::blocks::has_collision);
            if ground && free(y) && free(y + 1) {
                let target = super::player::Location { x, y: y as f64, z, ..loc };
                let at = crate::world::entity::Vec3::new(loc.x, loc.y, loc.z);
                self.play_sound(world, at, "minecraft:item.chorus_fruit.teleport", super::survival::category::PLAYERS, 1.0, 1.0, None);
                self.teleport(id, target);
                if let Some(p) = self.players.get_mut(&id) {
                    p.fall_distance = 0.0;
                }
                return;
            }
        }
    }
}

/// The potion named by a stack's `minecraft:potion_contents` component.
pub fn potion_of(stack: &ItemStack) -> Option<String> {
    let ty = paper_data::registries::id("minecraft:data_component_type", "minecraft:potion_contents")? as i32;
    let (_, data) = stack.added.iter().find(|(t, _)| *t == ty)?;
    let mut r = paper_protocol::codec::Reader::new(data);
    if !r.bool().ok()? {
        return None;
    }
    let id = r.varint().ok()?;
    paper_data::registries::name("minecraft:potion", id as u32).map(str::to_owned)
}

impl Server {
    /// `/effect give <targets> <effect> [seconds] [amplifier] [hideParticles]` and
    /// `/effect clear <targets> [effect]`.
    pub(super) fn effect_command(&mut self, source: &super::commands::CommandSource, args: &[&str]) {
        use paper_protocol::text::Component;
        match args {
            ["give", target, effect, rest @ ..] => {
                let Some(info) = effects::by_name(effect) else { return self.fail(source, format!("Unknown effect '{effect}'")) };
                let seconds: i32 = match rest.first() {
                    Some(&"infinite") => -1,
                    Some(s) => match s.parse::<i32>() {
                        Ok(v) if (1..=1_000_000).contains(&v) => v,
                        _ => return self.fail(source, "Invalid duration"),
                    },
                    None => 30,
                };
                let amplifier: i32 = rest.get(1).and_then(|a| a.parse().ok()).unwrap_or(0).clamp(0, 255);
                let hide = rest.get(2).is_some_and(|h| *h == "true");
                let targets = self.resolve_players(source, target);
                if targets.is_empty() {
                    return self.fail(source, "No player was found");
                }
                let duration = if seconds == -1 { -1 } else if info.instantaneous { 1 } else { seconds * 20 };
                let mut given = 0;
                for t in &targets {
                    let world = self.players[t].world;
                    let effect = ActiveEffect { id: info.id, amplifier, duration, ambient: false, particles: !hide, icon: true };
                    if self.add_effect(world, *t, effect) {
                        given += 1;
                    }
                }
                if given == 0 {
                    return self.fail(source, "Unable to apply this effect (target is either immune to effects, or has something stronger)");
                }
                let name = Component::translatable(format!("effect.{}", info.name.replacen(':', ".", 1)), vec![]);
                self.feedback(source, Component::translatable("commands.effect.give.success.multiple", vec![name, Component::text(given.to_string())]));
            }
            ["clear", rest @ ..] => {
                let target = rest.first().copied().unwrap_or("@s");
                let targets = self.resolve_players(source, target);
                if targets.is_empty() {
                    return self.fail(source, "No player was found");
                }
                let mut cleared = 0;
                for t in &targets {
                    let world = self.players[t].world;
                    cleared += match rest.get(1) {
                        Some(effect) => self.remove_effect(world, *t, effect_id(effect)) as usize,
                        None => self.clear_effects(world, *t),
                    };
                }
                if cleared == 0 {
                    return self.fail(source, "No effect was removed");
                }
                self.feedback(source, Component::translatable("commands.effect.clear.everything.success.multiple", vec![Component::text(targets.len().to_string())]));
            }
            _ => self.fail(source, "Usage: /effect give <targets> <effect> [seconds] [amplifier] [hideParticles] | /effect clear [targets] [effect]"),
        }
    }
}
