//! Animals: breeding (vanilla's `Animal.mobInteract`, `BreedGoal` and
//! `spawnChildFromBreeding`), growing up, being tempted by food, and the other things a
//! right-click does to a mob: name tags, milking, shearing and dyeing sheep. Also sheep
//! eating grass to grow their wool back and chickens laying eggs.

use super::mobs::SpawnReason;
use super::player::{HOTBAR_START, OFFHAND_SLOT};
use super::Server;
use crate::config::GameMode;
use crate::world::entity::Vec3;
use bytes::Bytes;
use paper_data::{blocks, items};
use paper_protocol::packets::{encode_packet, play};
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

/// The item tag of what an animal is bred with (`isFood`).
pub fn food_tag(name: &str) -> Option<&'static str> {
    Some(match name {
        "minecraft:cow" | "minecraft:mooshroom" => "minecraft:cow_food",
        "minecraft:sheep" => "minecraft:sheep_food",
        "minecraft:pig" => "minecraft:pig_food",
        "minecraft:chicken" => "minecraft:chicken_food",
        "minecraft:rabbit" => "minecraft:rabbit_food",
        "minecraft:goat" => "minecraft:goat_food",
        "minecraft:hoglin" => "minecraft:hoglin_food",
        "minecraft:strider" => "minecraft:strider_food",
        "minecraft:fox" => "minecraft:fox_food",
        "minecraft:bee" => "minecraft:bee_food",
        "minecraft:armadillo" => "minecraft:armadillo_food",
        "minecraft:camel" => "minecraft:camel_food",
        _ => return None,
    })
}

/// Whether a kind of mob has babies, and whether they grow up (`AgeableMob`; baby zombies and
/// piglins stay small). Kinds whose offspring come from eggs or not at all (parrots, frogs,
/// wandering traders) have none from spawn eggs either.
pub fn has_babies(name: &str) -> Option<bool> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    match name {
        "zombie" | "husk" | "drowned" | "zombie_villager" | "zombified_piglin" | "piglin" | "zoglin" => Some(false),
        "cow" | "mooshroom" | "sheep" | "pig" | "chicken" | "rabbit" | "goat" | "hoglin" | "strider" | "fox" | "bee" | "armadillo" | "camel" | "horse" | "donkey" | "mule" | "llama" | "trader_llama" | "skeleton_horse" | "zombie_horse" | "wolf" | "cat" | "ocelot" | "polar_bear" | "panda" | "turtle" | "axolotl" | "sniffer" | "villager" | "dolphin" | "squid" | "glow_squid" | "happy_ghast" => Some(true),
        _ => None,
    }
}

pub fn is_food(animal: &str, item: i32) -> bool {
    food_tag(animal).and_then(|t| paper_data::synced::tag("minecraft:item", t)).is_some_and(|ids| ids.contains(&item))
}

/// Wool colours by dye id (`DyeColor`).
pub(super) const COLORS: [&str; 16] = ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black"];

fn item_name(stack: &ItemStack) -> &'static str {
    items::get(stack.item as u32).map_or("", |i| i.name)
}

impl Server {
    pub(super) fn entity_event(&self, world: usize, id: i32, status: i8) {
        self.broadcast_entity(world, id, Bytes::from(encode_packet(&play::EntityEvent { entity_id: id, status })));
    }

    /// The inventory slot of a hand.
    pub(super) fn hand_slot(&self, id: i32, hand: i32) -> usize {
        if hand == 1 {
            OFFHAND_SLOT
        } else {
            HOTBAR_START + self.players.get(&id).map_or(0, |p| p.held_slot as usize)
        }
    }

    /// Uses up one of the items in a hand (not in creative).
    pub(super) fn use_held(&mut self, id: i32, hand: i32) {
        let slot = self.hand_slot(id, hand);
        let Some(p) = self.players.get(&id) else { return };
        if p.game_mode == GameMode::Creative {
            return;
        }
        let mut stack = p.inventory[slot].clone();
        if let Some(s) = stack.0.as_mut() {
            s.count -= 1;
        }
        if stack.0.as_ref().is_some_and(|s| s.count <= 0) {
            stack = Slot::EMPTY;
        }
        self.set_inventory_slot(id, slot, stack);
    }

    /// Swaps one held item for `result` (`ItemUtils.createFilledResult`): a bucket for milk.
    fn fill_held(&mut self, id: i32, hand: i32, result: ItemStack) {
        let slot = self.hand_slot(id, hand);
        let Some(p) = self.players.get(&id) else { return };
        let creative = p.game_mode == GameMode::Creative;
        let held = p.inventory[slot].clone();
        if !creative && held.0.as_ref().is_some_and(|s| s.count == 1) {
            self.set_inventory_slot(id, slot, Slot(Some(result)));
            return;
        }
        if !creative {
            self.use_held(id, hand);
        }
        let has = creative && self.players.get(&id).is_some_and(|p| p.inventory.iter().any(|s| s.0.as_ref().is_some_and(|i| i.item == result.item)));
        if !has && self.add_to_inventory(id, &result) > 0 {
            self.player_drop(id, Slot(Some(result)), false);
        }
    }

    /// Right-click on an entity (`Mob.mobInteract` for what this server knows).
    pub(super) fn handle_interact(&mut self, id: i32, target: i32, hand: i32) {
        let Some(p) = self.players.get(&id) else { return };
        if p.dead || p.game_mode == GameMode::Spectator {
            return;
        }
        let (world, at) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z));
        let slot = self.hand_slot(id, hand);
        let held = self.players[&id].inventory[slot].0.clone();
        let Some(e) = self.worlds[world].entities.get(&target) else { return };
        if e.pos.distance_squared(at) > 64.0 {
            return;
        }
        let Some(mob) = e.mob().filter(|m| !m.living.is_dead()) else { return };
        let (name, baby, sheared, age, in_love) = (e.info.name.clone(), mob.baby, mob.sheared, mob.age, mob.in_love);
        let merchant = mob.merchant.is_some();
        let name_tag = held.as_ref().is_some_and(|s| item_name(s) == "minecraft:name_tag");
        // A spawn egg of the mob's own kind makes a baby beside it (`spawnOffspringFromSpawnEgg`).
        if let Some(stack) = held.as_ref().filter(|s| item_name(s).strip_suffix("_spawn_egg") == Some(name.as_str())) {
            if self.spawn_offspring_from_egg(id, world, target, hand, stack) {
                return;
            }
        }
        // Villagers and wandering traders trade, pets are tamed and told to sit, and mounts
        // are saddled and ridden (name tags still name them all).
        if merchant && !name_tag && self.merchant_interact(id, world, target, hand, held.as_ref()) {
            return;
        }
        if !name_tag && super::taming::is_tameable(&name) && self.pet_interact(id, world, target, hand, held.as_ref()) {
            return;
        }
        if !name_tag && self.ride_interact(id, world, target, hand, held.as_ref()) {
            return;
        }
        let Some(stack) = held else { return };
        let item = item_name(&stack);

        // Name tags name mobs, which then stay.
        if item == "minecraft:name_tag" {
            let Some(label) = super::workstations::custom_name_of(&stack) else { return };
            if let Some(e) = self.worlds[world].entities.get_mut(&target) {
                e.custom_name = Some(label);
                e.metadata_dirty = true;
                if let Some(m) = e.mob_mut() {
                    m.persistent = true;
                }
            }
            self.use_held(id, hand);
            return;
        }
        let pos = self.worlds[world].entities[&target].pos;
        match (name.as_str(), item) {
            ("minecraft:cow" | "minecraft:mooshroom" | "minecraft:goat", "minecraft:bucket") if !baby => {
                let Some(milk) = items::id("minecraft:milk_bucket") else { return };
                self.fill_held(id, hand, ItemStack::new(milk as i32, 1));
                let sound = if name == "minecraft:goat" { "minecraft:entity.goat.milk" } else { "minecraft:entity.cow.milk" };
                self.play_sound(world, pos, sound, super::survival::category::PLAYERS, 1.0, 1.0, None);
                return;
            }
            ("minecraft:mooshroom", "minecraft:bowl") if !baby => {
                let Some(stew) = items::id("minecraft:mushroom_stew") else { return };
                self.fill_held(id, hand, ItemStack::new(stew as i32, 1));
                self.play_sound(world, pos, "minecraft:entity.mooshroom.milk", super::survival::category::PLAYERS, 1.0, 1.0, None);
                return;
            }
            ("minecraft:sheep", "minecraft:shears") if !baby && !sheared => {
                self.shear_sheep(world, target);
                // The shears wear (creative players' do not).
                if self.players.get(&id).is_some_and(|p| p.game_mode != GameMode::Creative) {
                    let mut shears = self.players[&id].inventory[slot].clone();
                    super::survival::hurt_item(&mut shears, 1, false);
                    self.set_inventory_slot(id, slot, shears);
                }
                return;
            }
            ("minecraft:sheep", dye) if dye.ends_with("_dye") => {
                let color = COLORS.iter().position(|c| dye == format!("minecraft:{c}_dye"));
                if let Some(color) = color {
                    let mut changed = false;
                    if let Some(e) = self.worlds[world].entities.get_mut(&target) {
                        if let Some(m) = e.mob_mut().filter(|m| !m.sheared && m.variant != color as i32) {
                            m.variant = color as i32;
                            changed = true;
                        }
                        e.metadata_dirty |= changed;
                    }
                    if changed {
                        self.use_held(id, hand);
                    }
                }
                return;
            }
            _ => {}
        }
        if !is_food(&name, stack.item) {
            return;
        }
        // Feeding: an adult falls in love, a baby grows up faster (10% of what is left).
        if age == 0 && in_love <= 0 && !baby {
            self.use_held(id, hand);
            if let Some(m) = self.worlds[world].entities.get_mut(&target).and_then(|e| e.mob_mut()) {
                m.in_love = 600;
                m.love_cause = Some(id);
            }
            self.entity_event(world, target, 18);
        } else if baby {
            self.use_held(id, hand);
            let seconds = (-age / 20) as f32 * 0.1;
            self.age_up(world, target, seconds as i32);
        } else {
            return;
        }
        // Some animals make a sound eating (`playEatingSound`).
        let sound = format!("{}.eat", name.replace("minecraft:", "minecraft:entity."));
        if paper_data::registries::id("minecraft:sound_event", &sound).is_some() {
            self.play_sound(world, pos, &sound, super::survival::category::NEUTRAL, 1.0, 1.0, None);
        }
    }

    /// `AgeableMob.ageUp`: brings a baby `seconds` closer to growing up.
    fn age_up(&mut self, world: usize, id: i32, seconds: i32) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let Some(m) = e.mob_mut() else { return };
        m.age = (m.age + seconds * 20).min(0);
        if m.age == 0 && m.baby {
            m.baby = false;
            e.metadata_dirty = true;
        }
    }

    pub(super) fn shear_sheep(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let pos = e.pos;
        let Some(m) = e.mob_mut() else { return };
        m.sheared = true;
        let color = COLORS[(m.variant & 15) as usize];
        e.metadata_dirty = true;
        let mut rng = rand::thread_rng();
        if let Some(wool) = items::id(&format!("minecraft:{color}_wool")) {
            for _ in 0..rng.gen_range(1..=3) {
                let vel = Vec3::new((rng.gen::<f64>() - rng.gen::<f64>()) * 0.1, rng.gen::<f64>() * 0.05, (rng.gen::<f64>() - rng.gen::<f64>()) * 0.1);
                self.spawn_item(world, Vec3::new(pos.x, pos.y + 1.0, pos.z), Slot::of(wool as i32, 1), vel, 10);
            }
        }
        self.play_sound(world, pos, "minecraft:entity.sheep.shear", super::survival::category::PLAYERS, 1.0, 1.0, None);
    }

    /// Per tick for mobs that age: growing up, breeding cooldown, love, eggs, grass.
    pub(super) fn tick_animal(&mut self, world: usize, id: i32) {
        let griefing = self.worlds[world].rules.bool("mob_griefing");
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        if !crate::world::entity_nbt::ages(&e.info.name) {
            return;
        }
        let name = e.info.name.clone();
        let pos = e.pos;
        let Some(m) = e.mob_mut() else { return };
        if m.living.is_dead() {
            return;
        }
        if m.age < 0 {
            m.age += 1;
        } else if m.age > 0 {
            m.age -= 1;
        }
        let grew = m.baby && m.age >= 0;
        if grew {
            m.baby = false;
        }
        if m.age != 0 {
            m.in_love = 0;
        } else if m.in_love > 0 {
            m.in_love -= 1;
        }
        let mut lay_egg = false;
        if name == "minecraft:chicken" && !m.baby {
            m.egg_time -= 1;
            if m.egg_time <= 0 {
                m.egg_time = rand::thread_rng().gen_range(6000..12000);
                lay_egg = true;
            }
        }
        let (baby, sheared) = (m.baby, m.sheared);
        if grew {
            e.metadata_dirty = true;
        }
        if lay_egg {
            if let Some(egg) = items::id("minecraft:egg") {
                self.spawn_item(world, pos, Slot::of(egg as i32, 1), Vec3::ZERO, 10);
            }
            let pitch = (rand::thread_rng().gen::<f32>() - rand::thread_rng().gen::<f32>()) * 0.2 + 1.0;
            self.play_sound(world, pos, "minecraft:entity.chicken.egg", super::survival::category::NEUTRAL, 1.0, pitch, None);
        }
        // Sheep eat grass now and then (`EatBlockGoal`), growing their wool back.
        if name == "minecraft:sheep" && rand::thread_rng().gen_range(0..if baby { 50 } else { 1000 }) == 0 && (sheared || baby) {
            let (x, y, z) = (pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32);
            let w = &self.worlds[world];
            let here = w.block_at(x, y, z).map(|s| blocks::block_of_state(s).name);
            let below = w.block_at(x, y - 1, z).map(|s| blocks::block_of_state(s).name);
            let ate = if here == Some("minecraft:short_grass") {
                if griefing {
                    self.set_block_and_update(world, paper_protocol::codec::BlockPos::new(x, y, z), blocks::AIR);
                }
                true
            } else if below == Some("minecraft:grass_block") {
                if griefing {
                    if let Some(dirt) = blocks::default_state("minecraft:dirt") {
                        self.set_block_and_update(world, paper_protocol::codec::BlockPos::new(x, y - 1, z), dirt);
                    }
                }
                true
            } else {
                false
            };
            if ate {
                self.entity_event(world, id, 10);
                if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                    if let Some(m) = e.mob_mut() {
                        m.sheared = false;
                    }
                    e.metadata_dirty = true;
                }
                if baby {
                    self.age_up(world, id, 60);
                }
            }
        }
    }

    /// A baby of a mob from a spawn egg of its kind, if that kind has babies; sheep take the
    /// colour of the mob. Returns whether one was born.
    fn spawn_offspring_from_egg(&mut self, player: i32, world: usize, parent: i32, hand: i32, stack: &ItemStack) -> bool {
        let Some(e) = self.worlds[world].entities.get(&parent) else { return false };
        let (name, pos, variant) = (e.info.name.clone(), e.pos, e.mob().map_or(0, |m| m.variant));
        let Some(grows_up) = has_babies(&name) else { return false };
        let Some(baby) = self.spawn_mob(world, &name, pos, SpawnReason::Egg) else { return false };
        if let Some(e) = self.worlds[world].entities.get_mut(&baby) {
            if let Some(m) = e.mob_mut() {
                m.baby = true;
                if grows_up {
                    m.age = -24000;
                }
                if name == "minecraft:sheep" {
                    m.variant = variant;
                }
            }
            if let Some(label) = super::workstations::custom_name_of(stack) {
                e.custom_name = Some(label);
            }
            e.metadata_dirty = true;
        }
        self.use_held(player, hand);
        true
    }

    /// Two animals in love that met for three seconds have a baby (`spawnChildFromBreeding`).
    pub(super) fn breed(&mut self, world: usize, a: i32, b: i32) {
        let (Some(ea), Some(eb)) = (self.worlds[world].entities.get(&a), self.worlds[world].entities.get(&b)) else { return };
        let (name, pos) = (ea.info.name.clone(), ea.pos);
        let colors = (ea.mob().map_or(0, |m| m.variant), eb.mob().map_or(0, |m| m.variant));
        // The player who fed either parent counts the breeding.
        let breeder = ea.mob().and_then(|m| m.love_cause).or_else(|| eb.mob().and_then(|m| m.love_cause));
        for parent in [a, b] {
            if let Some(m) = self.worlds[world].entities.get_mut(&parent).and_then(|e| e.mob_mut()) {
                m.age = 6000;
                m.in_love = 0;
                m.love_cause = None;
                m.brain.love_time = 0;
                m.brain.path.clear();
            }
        }
        let Some(baby) = self.spawn_mob(world, &name, pos, SpawnReason::Breeding) else { return };
        if let Some(e) = self.worlds[world].entities.get_mut(&baby) {
            if let Some(m) = e.mob_mut() {
                m.baby = true;
                m.age = -24000;
                if name == "minecraft:sheep" {
                    m.variant = if rand::thread_rng().gen::<bool>() { colors.0 } else { colors.1 };
                }
            }
            e.metadata_dirty = true;
        }
        self.entity_event(world, a, 18);
        if let Some(player) = breeder {
            self.award_stat(player, super::stats::CUSTOM, "minecraft:animals_bred", 1);
        }
        if self.worlds[world].rules.bool("mob_drops") {
            let xp = rand::thread_rng().gen_range(1..=7);
            self.award_experience(world, pos, xp);
        }
    }
}
