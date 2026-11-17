//! Health, damage, hunger, breath, fire, eating, experience, death and respawn of players,
//! following vanilla's `LivingEntity`, `Player`, `ServerPlayer` and `FoodData`.

use super::player::{Location, HOTBAR_START, OFFHAND_SLOT};
use super::Server;
use crate::config::{Difficulty, GameMode};
use crate::world::entity::{Aabb, Entity, EntityKind, Vec3};
use crate::world::World;
use bytes::Bytes;
use paper_data::{blocks, item_data, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::{encode_packet, play};
use paper_protocol::text::Component;
use paper_protocol::types::{ItemStack, LpVec3, Slot};
use rand::Rng;
use std::collections::HashMap;

/// Sound categories (`SoundSource`).
pub mod category {
    pub const BLOCKS: i32 = 4;
    pub const HOSTILE: i32 = 5;
    pub const NEUTRAL: i32 = 6;
    pub const PLAYERS: i32 = 7;
    pub const AMBIENT: i32 = 8;
}

/// What hurt an entity.
#[derive(Clone, Debug)]
pub struct Damage {
    /// Entry of `minecraft:damage_type`, e.g. `minecraft:fall`.
    pub kind: &'static str,
    /// The entity responsible: an attacking player or mob, or the shooter of an arrow.
    pub attacker: Option<i32>,
    /// The entity that dealt the damage directly (the arrow); same as `attacker` in melee.
    pub direct: Option<i32>,
    /// Where the damage came from, for knockback.
    pub position: Option<Vec3>,
}

impl Damage {
    pub fn new(kind: &'static str) -> Self {
        Self { kind, attacker: None, direct: None, position: None }
    }

    /// Damage dealt in melee by `attacker` standing at `position`.
    pub fn by(kind: &'static str, attacker: i32, position: Vec3) -> Self {
        Self { kind, attacker: Some(attacker), direct: Some(attacker), position: Some(position) }
    }

    /// Whether the damage type is in a `minecraft:damage_type` tag such as `is_fire`.
    pub fn is(&self, tag: &str) -> bool {
        damage_tag(self.kind, tag)
    }

    pub fn network_id(&self) -> i32 {
        paper_data::synced::id("minecraft:damage_type", self.kind).unwrap_or(0)
    }
}

pub fn damage_tag(kind: &str, tag: &str) -> bool {
    let Some(id) = paper_data::synced::id("minecraft:damage_type", kind) else { return false };
    let tag = if tag.contains(':') { tag.to_owned() } else { format!("minecraft:{tag}") };
    paper_data::synced::tag("minecraft:damage_type", &tag).is_some_and(|ids| ids.contains(&id))
}

/// A `minecraft:damage_type` entry.
#[derive(Clone, Debug)]
pub struct DamageTypeInfo {
    pub message_id: String,
    pub exhaustion: f32,
    /// `never`, `always` or `when_caused_by_living_non_player`.
    pub scaling: String,
    /// `hurt`, `thorns`, `drowning`, `burning`, `poking` or `freezing`.
    pub effects: String,
    /// `default`, `fall_variants` or `intentional_game_design`.
    pub death_message_type: String,
}

/// Reads the damage types from the vanilla data; without it, every type gets plausible
/// defaults.
pub fn load_damage_types(vanilla: Option<&crate::vanilla::VanillaData>) -> HashMap<String, DamageTypeInfo> {
    let mut out = HashMap::new();
    for name in paper_data::synced::entries("minecraft:damage_type").unwrap_or(&[]) {
        let path = name.strip_prefix("minecraft:").unwrap_or(name);
        let json = vanilla.and_then(|v| v.read_json(&format!("data/minecraft/damage_type/{path}.json")));
        let info = match json {
            Some(v) => DamageTypeInfo {
                message_id: v["message_id"].as_str().unwrap_or(path).to_owned(),
                exhaustion: v["exhaustion"].as_f64().unwrap_or(0.0) as f32,
                scaling: v["scaling"].as_str().unwrap_or("when_caused_by_living_non_player").to_owned(),
                effects: v["effects"].as_str().unwrap_or("hurt").to_owned(),
                death_message_type: v["death_message_type"].as_str().unwrap_or("default").to_owned(),
            },
            None => DamageTypeInfo {
                message_id: path.to_owned(),
                exhaustion: if matches!(path, "fall" | "drown" | "starve" | "out_of_world" | "generic" | "generic_kill" | "in_wall" | "on_fire" | "cramming") { 0.0 } else { 0.1 },
                scaling: "when_caused_by_living_non_player".into(),
                effects: match path {
                    "drown" => "drowning",
                    "on_fire" | "in_fire" | "lava" | "hot_floor" | "campfire" => "burning",
                    "sweet_berry_bush" => "poking",
                    "freeze" => "freezing",
                    _ => "hurt",
                }
                .into(),
                death_message_type: if path == "fall" { "fall_variants".into() } else { "default".into() },
            },
        };
        out.insert((*name).to_owned(), info);
    }
    out
}

// --- fluids ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fluid {
    Water,
    Lava,
}

/// The fluid in a block state and its amount (1-8, 8 for a source or falling fluid). Looked up
/// in a table worked out once for every state, as it is asked all the time (mobs in water,
/// paths, fluid flow).
pub fn fluid_of(state: u32) -> Option<(Fluid, u8)> {
    static TABLE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..blocks::STATE_COUNT)
            .map(|s| match work_out_fluid(s) {
                Some((Fluid::Water, amount)) => 0x10 | amount,
                Some((Fluid::Lava, amount)) => 0x20 | amount,
                None => 0,
            })
            .collect()
    });
    match table.get(state as usize).copied().unwrap_or(0) {
        0 => None,
        v if v & 0x20 != 0 => Some((Fluid::Lava, v & 0x0F)),
        v => Some((Fluid::Water, v & 0x0F)),
    }
}

fn work_out_fluid(state: u32) -> Option<(Fluid, u8)> {
    let name = blocks::block_of_state(state).name;
    let level = || blocks::property(state, "level").and_then(|l| l.parse::<u8>().ok()).unwrap_or(0);
    let amount = |l: u8| if l == 0 || l >= 8 { 8 } else { 8 - l };
    match name {
        "minecraft:water" => Some((Fluid::Water, amount(level()))),
        "minecraft:lava" => Some((Fluid::Lava, amount(level()))),
        "minecraft:bubble_column" | "minecraft:kelp" | "minecraft:kelp_plant" | "minecraft:seagrass" | "minecraft:tall_seagrass" => Some((Fluid::Water, 8)),
        _ => (blocks::property(state, "waterlogged") == Some("true")).then_some((Fluid::Water, 8)),
    }
}

/// The height of the fluid surface in the block at (x, y, z), as vanilla's
/// `FluidState.getHeight` (a full block when the same fluid is above).
pub fn fluid_surface(world: &World, x: i32, y: i32, z: i32) -> Option<(Fluid, f64)> {
    let (fluid, amount) = fluid_of(world.block_at(x, y, z)?)?;
    let above = world.block_at(x, y + 1, z).and_then(fluid_of).is_some_and(|(f, _)| f == fluid);
    Some((fluid, y as f64 + if above { 1.0 } else { amount as f64 / 9.0 }))
}

/// Whether a box touches `fluid` (vanilla's `updateFluidHeightAndDoFluidPushing`).
pub fn box_in_fluid(world: &World, area: &Aabb, fluid: Fluid) -> bool {
    let b = area.inflate(-0.001, -0.001, -0.001);
    for y in b.min.y.floor() as i32..=b.max.y.floor() as i32 {
        for z in b.min.z.floor() as i32..=b.max.z.floor() as i32 {
            for x in b.min.x.floor() as i32..=b.max.x.floor() as i32 {
                if let Some((f, surface)) = fluid_surface(world, x, y, z) {
                    if f == fluid && surface >= b.min.y {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Whether the point (the eyes) is below the surface of `fluid`.
pub fn point_in_fluid(world: &World, p: Vec3, fluid: Fluid) -> bool {
    let (x, y, z) = (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);
    fluid_surface(world, x, y, z).is_some_and(|(f, surface)| f == fluid && surface > p.y)
}

/// Block states inside a box (deflated slightly, like vanilla's inside-block checks).
fn blocks_inside(world: &World, area: &Aabb) -> Vec<(BlockPos, u32)> {
    let b = area.inflate(-1.0e-5, -1.0e-5, -1.0e-5);
    let mut out = Vec::new();
    for y in b.min.y.floor() as i32..=b.max.y.floor() as i32 {
        for z in b.min.z.floor() as i32..=b.max.z.floor() as i32 {
            for x in b.min.x.floor() as i32..=b.max.x.floor() as i32 {
                if let Some(state) = world.block_at(x, y, z) {
                    if !blocks::is_air(state) {
                        out.push((BlockPos::new(x, y, z), state));
                    }
                }
            }
        }
    }
    out
}

/// Whether an entity's eyes are inside a suffocating block (vanilla's `isInWall`).
pub fn in_wall(world: &World, eye: Vec3, width: f64) -> bool {
    let half = width * 0.8 / 2.0;
    let area = Aabb::new(Vec3::new(eye.x - half, eye.y - 5.0e-7, eye.z - half), Vec3::new(eye.x + half, eye.y + 5.0e-7, eye.z + half));
    blocks_inside(world, &area).into_iter().any(|(pos, state)| {
        let f = paper_data::physics::flags(state);
        f & paper_data::physics::SOLID_BLOCK != 0
            && f & paper_data::physics::FULL_CUBE != 0
            && paper_data::physics::collision_boxes(state).iter().any(|b| {
                let block = Aabb::new(Vec3::new(pos.x as f64 + b.min[0], pos.y as f64 + b.min[1], pos.z as f64 + b.min[2]), Vec3::new(pos.x as f64 + b.max[0], pos.y as f64 + b.max[1], pos.z as f64 + b.max[2]));
                block.intersects(&area)
            })
    })
}

fn is_climbable(state: u32) -> bool {
    let name = blocks::block_of_state(state).name;
    matches!(name, "minecraft:ladder" | "minecraft:vine" | "minecraft:scaffolding" | "minecraft:twisting_vines" | "minecraft:twisting_vines_plant" | "minecraft:weeping_vines" | "minecraft:weeping_vines_plant" | "minecraft:cave_vines" | "minecraft:cave_vines_plant")
        || (name.ends_with("_trapdoor") && blocks::property(state, "open") == Some("true"))
}

// --- items: durability, enchantments, armour -----------------------------------------------

fn component_id(name: &str) -> Option<i32> {
    paper_data::registries::id("minecraft:data_component_type", name).map(|i| i as i32)
}

/// A varint-valued component of a stack (`minecraft:damage`, `minecraft:max_damage`).
fn varint_component(stack: &ItemStack, name: &str) -> Option<i32> {
    let ty = component_id(name)?;
    let (_, data) = stack.added.iter().find(|(t, _)| *t == ty)?;
    paper_protocol::codec::Reader::new(data).varint().ok()
}

pub fn item_damage(stack: &ItemStack) -> i32 {
    varint_component(stack, "minecraft:damage").unwrap_or(0)
}

pub fn max_damage(stack: &ItemStack) -> i32 {
    if component_id("minecraft:unbreakable").is_some_and(|ty| stack.added.iter().any(|(t, _)| *t == ty)) {
        return 0;
    }
    varint_component(stack, "minecraft:max_damage").or_else(|| item_data::by_id(stack.item as u32).max_damage).unwrap_or(0)
}

pub fn set_item_damage(stack: &mut ItemStack, damage: i32) {
    let Some(ty) = component_id("minecraft:damage") else { return };
    stack.added.retain(|(t, _)| *t != ty);
    if damage > 0 {
        let mut data = Vec::new();
        paper_protocol::codec::WriteExt::put_varint(&mut data, damage);
        stack.added.push((ty, data));
        stack.added.sort_by_key(|(t, _)| *t);
    }
}

/// Wears an item down by `amount` (vanilla's `hurtAndBreak`, with Unbreaking). Returns true if
/// it broke; the slot is then emptied. (A player's items go through
/// [`Server::wear_player_item`], which plugins hear of.)
pub fn hurt_item(slot: &mut Slot, amount: i32, armor: bool) -> bool {
    let wear = item_wear(slot, amount, armor);
    add_wear(slot, wear)
}

/// The points of wear `amount` uses would do to the item in `slot` once Unbreaking spared some
/// (`processDurabilityChange`); none for items that do not wear.
pub fn item_wear(slot: &Slot, amount: i32, armor: bool) -> i32 {
    let Some(stack) = slot.0.as_ref() else { return 0 };
    if max_damage(stack) <= 0 || amount <= 0 {
        return 0;
    }
    let unbreaking = super::loot::enchantment_level(Some(slot), "minecraft:unbreaking");
    let mut rng = rand::thread_rng();
    let mut taken = 0;
    for _ in 0..amount {
        // Tools ignore a point of wear with probability level/(level+1); armour less often.
        let ignored = unbreaking > 0 && if armor { rng.gen::<f32>() < 0.6 + 0.4 / (unbreaking as f32 + 1.0) && rng.gen_range(0..unbreaking + 1) > 0 } else { rng.gen_range(0..unbreaking + 1) > 0 };
        if !ignored {
            taken += 1;
        }
    }
    taken
}

/// Adds `wear` points of damage to the item in `slot`. Returns true if that broke it; the slot
/// is then emptied.
pub fn add_wear(slot: &mut Slot, wear: i32) -> bool {
    let Some(stack) = slot.0.as_mut() else { return false };
    let max = max_damage(stack);
    if max <= 0 || wear <= 0 {
        return false;
    }
    let damage = item_damage(stack) + wear;
    if damage >= max {
        *slot = Slot::EMPTY;
        true
    } else {
        set_item_damage(stack, damage);
        false
    }
}

/// Armour points and toughness of worn items (feet, legs, chest, head in `armor`).
pub fn armor_values(armor: &[&Slot]) -> (f32, f32) {
    let slots = ["feet", "legs", "chest", "head"];
    let (mut points, mut toughness) = (0.0, 0.0);
    for (slot, item) in slots.iter().zip(armor) {
        if let Some(stack) = item.0.as_ref() {
            let data = item_data::by_id(stack.item as u32);
            points += data.modifier("minecraft:armor", slot) + data.modifier("armor", slot);
            toughness += data.modifier("minecraft:armor_toughness", slot) + data.modifier("armor_toughness", slot);
        }
    }
    (points as f32, toughness as f32)
}

/// Vanilla's `CombatRules.getDamageAfterAbsorb`.
pub fn damage_after_armor(damage: f32, armor: f32, toughness: f32) -> f32 {
    let f = 2.0 + toughness / 4.0;
    let g = (armor - damage / f).clamp(armor * 0.2, 20.0);
    damage * (1.0 - g / 25.0)
}

/// Protection from enchantments on armour (`getDamageProtection`), capped at 20.
pub fn enchantment_protection(armor: &[&Slot], damage: &Damage) -> f32 {
    let mut total = 0;
    for item in armor {
        let level = |e: &str| super::loot::enchantment_level(Some(*item), e);
        total += level("minecraft:protection");
        if damage.is("is_fire") {
            total += 2 * level("minecraft:fire_protection");
        }
        if damage.is("is_explosion") {
            total += 2 * level("minecraft:blast_protection");
        }
        if damage.is("is_projectile") {
            total += 2 * level("minecraft:projectile_protection");
        }
        if damage.is("is_fall") {
            total += 3 * level("minecraft:feather_falling");
        }
    }
    total.min(20) as f32
}

// --- experience ----------------------------------------------------------------------------

/// Points needed to get from `level` to the next one.
pub fn xp_needed(level: i32) -> i32 {
    if level >= 30 {
        112 + (level - 30) * 9
    } else if level >= 15 {
        37 + (level - 15) * 5
    } else {
        7 + level * 2
    }
}

/// The value of the next orb when splitting `amount` (vanilla's `getExperienceValue`).
fn orb_value(amount: i32) -> i32 {
    for v in [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3] {
        if amount >= v {
            return v;
        }
    }
    1
}

impl Server {
    /// A player's item in `slot` wears by `amount` uses (`ItemStack.hurtAndBreak`), Unbreaking
    /// first: plugins may change the wear or stop it (`PlayerItemDamageEvent`), and hear when it
    /// breaks (`PlayerItemBreakEvent`), which empties the slot. Returns whether it broke. The
    /// slot is not sent to the player.
    pub(super) fn wear_player_item(&mut self, id: i32, slot: usize, amount: i32, armor: bool) -> bool {
        let Some(item) = self.players.get(&id).and_then(|p| p.inventory.get(slot)).cloned() else { return false };
        let wear = item_wear(&item, amount, armor);
        if wear <= 0 {
            return false;
        }
        let Some(wear) = crate::plugins::events::on_item_damage(self, id, &item, wear, amount) else { return false };
        let Some(p) = self.players.get_mut(&id) else { return false };
        let worn = &mut p.inventory[slot];
        // (Unless plugins changed the item meanwhile.)
        if *worn != item {
            return false;
        }
        let broke = add_wear(worn, wear);
        if broke {
            crate::plugins::events::on_item_break(self, id, &item);
        }
        broke
    }

    // --- sounds ------------------------------------------------------------------------------

    /// Plays a sound at a position for players in hearing range (16 blocks, more when louder),
    /// except `except` (whose client plays it itself).
    pub fn play_sound(&self, world: usize, at: Vec3, sound: &str, category: i32, volume: f32, pitch: f32, except: Option<i32>) {
        let sound = if sound.contains(':') { sound.to_owned() } else { format!("minecraft:{sound}") };
        let sound_ref = match paper_data::registries::id("minecraft:sound_event", &sound) {
            Some(i) => play::SoundRef::Id(i as i32),
            None => play::SoundRef::Named { name: sound, fixed_range: None },
        };
        let packet = play::Sound { sound: sound_ref, category, x: at.x, y: at.y, z: at.z, volume, pitch, seed: rand::random() };
        let payload = Bytes::from(encode_packet(&packet));
        let range = if volume > 1.0 { 16.0 * volume as f64 } else { 16.0 };
        for p in self.players.values() {
            if p.world != world || Some(p.entity_id) == except {
                continue;
            }
            let (dx, dy, dz) = (p.loc.x - at.x, p.loc.y - at.y, p.loc.z - at.z);
            if dx * dx + dy * dy + dz * dz <= range * range {
                p.conn.send_payload(payload.clone());
            }
        }
    }

    fn random_pitch() -> f32 {
        let mut rng = rand::thread_rng();
        (rng.gen::<f32>() - rng.gen::<f32>()) * 0.2 + 1.0
    }

    // --- per tick ----------------------------------------------------------------------------

    pub(super) fn tick_players(&mut self) {
        for id in self.player_order.clone() {
            self.tick_client_load(id);
            self.tick_gliding(id);
            self.tick_player(id);
            self.tick_player_portal(id);
        }
        self.tick_travels();
    }

    /// A client that never says it has loaded is taken as loaded after three seconds
    /// (`ServerGamePacketListenerImpl.tickClientLoadTimeout`).
    fn tick_client_load(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        if p.loaded || p.load_timeout <= 0 {
            return;
        }
        if p.load_timeout > 1 {
            p.load_timeout -= 1;
        } else {
            self.mark_client_loaded(id, true);
        }
    }

    /// The client has loaded the world after joining or respawning, or the server stopped
    /// waiting for it (`timeout`): plugins hear `PlayerClientLoadedWorldEvent` once.
    pub(super) fn mark_client_loaded(&mut self, id: i32, timeout: bool) {
        let Some(p) = self.players.get_mut(&id) else { return };
        p.load_timeout = 0;
        if std::mem::replace(&mut p.loaded, true) {
            return;
        }
        crate::plugins::events::on_client_loaded(self, id, timeout);
    }

    fn tick_player(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        p.warden.tick();
        if p.dead || p.game_mode == GameMode::Spectator {
            return;
        }
        if p.invulnerable_time > 0 {
            p.invulnerable_time -= 1;
        }
        if p.xp_pickup_delay > 0 {
            p.xp_pickup_delay -= 1;
        }
        let world = p.world;
        let pos = Vec3::new(p.loc.x, p.loc.y, p.loc.z);
        let eye = Vec3::new(p.loc.x, p.loc.y + p.eye_height(), p.loc.z);
        let height = if p.sneaking { 1.5 } else { 1.8 };
        let area = Aabb::of_entity(pos, 0.6, height);
        let invulnerable = p.game_mode == GameMode::Creative;
        let (sneaking, on_ground) = (p.sneaking, p.on_ground);
        let moved = p.moved_this_tick;
        p.moved_this_tick = false;
        let w = &self.worlds[world];

        // Outside the world border, past its safe zone (`LivingEntity.baseTick`).
        let outside = w.border.distance_inside(pos.x, pos.z) + w.border.damage_safe_zone;
        let border_damage = (outside < 0.0 && w.border.damage_per_block > 0.0).then(|| (-outside * w.border.damage_per_block).floor().max(1.0) as f32);
        // Falling out of the world.
        if pos.y < (w.min_y - 64) as f64 {
            self.hurt_player(id, Damage::new("minecraft:out_of_world"), 4.0);
        }
        if let Some(amount) = border_damage {
            self.hurt_player(id, Damage::new("minecraft:outside_border"), amount);
        }
        let Some(p) = self.players.get(&id) else { return };
        if p.dead {
            return;
        }

        let w = &self.worlds[world];
        let in_water = box_in_fluid(w, &area, Fluid::Water);
        let in_lava = box_in_fluid(w, &area, Fluid::Lava);
        let eye_in_water = point_in_fluid(w, Vec3::new(eye.x, eye.y - 0.11111111, eye.z), Fluid::Water);
        let inside = blocks_inside(w, &area);
        let below = w.block_at(pos.x.floor() as i32, (pos.y - 0.2).floor() as i32, pos.z.floor() as i32).unwrap_or(blocks::AIR);
        let suffocating = in_wall(w, eye, 0.6);
        if let Some(p) = self.players.get_mut(&id) {
            p.api.fluids = (in_water, in_lava, eye_in_water);
        }

        // Water puts fires out and breaks falls.
        if in_water {
            let p = self.players.get_mut(&id).unwrap();
            if p.fire_ticks > 0 {
                p.fire_ticks = 0;
                p.metadata_dirty = true;
            }
            p.fall_distance = 0.0;
        }
        if in_lava {
            let p = self.players.get_mut(&id).unwrap();
            p.fall_distance *= 0.5;
            // Catching fire asks plugins (`EntityCombustByBlockEvent`); burning on does not.
            if !invulnerable && p.fire_ticks > 0 {
                p.fire_ticks = p.fire_ticks.max(300);
                p.metadata_dirty = true;
            } else if !invulnerable {
                let at = BlockPos::new(pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32);
                self.set_on_fire(world, id, 300, crate::plugins::events::Combuster::Block(at));
            }
            if self.hurt_player(id, Damage::new("minecraft:lava"), 4.0) {
                let pitch = 2.0 + rand::thread_rng().gen::<f32>() * 0.4;
                self.play_sound(world, pos, "minecraft:entity.generic.burn", category::PLAYERS, 0.4, pitch, None);
            }
        }

        // Blocks the player is inside of or standing on.
        for (at, state) in &inside {
            let name = blocks::block_of_state(*state).name;
            match name {
                "minecraft:fire" | "minecraft:soul_fire" => {
                    if !invulnerable && self.players[&id].fire_ticks > 0 {
                        let p = self.players.get_mut(&id).unwrap();
                        p.fire_ticks = (p.fire_ticks + rand::thread_rng().gen_range(1..3)).max(160);
                        p.metadata_dirty = true;
                    } else if !invulnerable {
                        self.set_on_fire(world, id, 160, crate::plugins::events::Combuster::Block(*at));
                    }
                    let amount = if name == "minecraft:soul_fire" { 2.0 } else { 1.0 };
                    self.hurt_player(id, Damage::new("minecraft:in_fire"), amount);
                }
                "minecraft:cactus" => {
                    self.hurt_player(id, Damage::new("minecraft:cactus"), 1.0);
                }
                "minecraft:sweet_berry_bush" if moved && blocks::property(*state, "age").is_some_and(|a| a != "0") => {
                    self.hurt_player(id, Damage::new("minecraft:sweet_berry_bush"), 1.0);
                }
                "minecraft:campfire" | "minecraft:soul_campfire" if blocks::property(*state, "lit") == Some("true") => {
                    let amount = if name == "minecraft:soul_campfire" { 2.0 } else { 1.0 };
                    self.hurt_player(id, Damage::new("minecraft:campfire"), amount);
                }
                _ => {}
            }
        }
        if on_ground && !sneaking && blocks::block_of_state(below).name == "minecraft:magma_block" {
            self.hurt_player(id, Damage::new("minecraft:hot_floor"), 1.0);
        }
        if suffocating {
            self.hurt_player(id, Damage::new("minecraft:in_wall"), 1.0);
        }

        // Burning.
        let Some(p) = self.players.get_mut(&id) else { return };
        if p.fire_ticks > 0 {
            let hurt = p.fire_ticks % 20 == 0 && !in_lava;
            p.fire_ticks -= 1;
            if p.fire_ticks == 0 {
                p.metadata_dirty = true;
            }
            if hurt {
                self.hurt_player(id, Damage::new("minecraft:on_fire"), 1.0);
            }
        }

        // Breath.
        let drowning = self.worlds[world].rules.bool("drowning_damage");
        let Some(p) = self.players.get_mut(&id) else { return };
        let air_before = p.air;
        let water_breathing = p.effects.iter().any(|e| e.name() == "minecraft:water_breathing" || e.name() == "minecraft:conduit_power");
        if eye_in_water && !invulnerable && !water_breathing {
            let respiration = super::loot::enchantment_level(Some(&p.inventory[5]), "minecraft:respiration");
            if respiration == 0 || rand::thread_rng().gen_range(0..respiration + 1) == 0 {
                p.air -= 1;
            }
            if p.air <= -20 {
                p.air = 0;
                if drowning {
                    self.hurt_player(id, Damage::new("minecraft:drown"), 2.0);
                }
            }
        } else if p.air < 300 {
            p.air = (p.air + 4).min(p.api.max_air);
        }
        if let Some(p) = self.players.get_mut(&id) {
            if p.air != air_before {
                p.metadata_dirty = true;
            }
        }

        self.tick_food(id);
        self.tick_using_item(id);
        self.send_health_if_changed(id);
    }

    /// Hunger and natural regeneration (`FoodData.tick`).
    fn tick_food(&mut self, id: i32) {
        let difficulty = self.difficulty;
        let Some(p) = self.players.get(&id) else { return };
        if p.dead {
            return;
        }
        let regen = self.worlds[p.world].rules.bool("natural_health_regeneration");
        let tick = self.tick;
        let p = self.players.get_mut(&id).unwrap();
        if difficulty == Difficulty::Peaceful && regen {
            let heal = p.health < p.max_health() && tick % 20 == 0;
            if p.food < 20 && tick % 10 == 0 {
                p.food += 1;
            }
            if heal {
                self.regain_health(id, 1.0, "REGEN");
            }
        }
        let Some(p) = self.players.get_mut(&id) else { return };
        if p.exhaustion > 4.0 {
            p.exhaustion -= 4.0;
            if p.saturation > 0.0 {
                p.saturation = (p.saturation - 1.0).max(0.0);
            } else if difficulty != Difficulty::Peaceful {
                // Getting hungry: plugins may keep the food bar where it is.
                let level = (p.food - 1).max(0);
                if let Some(level) = crate::plugins::events::on_food_level_change(self, id, level, None) {
                    if let Some(p) = self.players.get_mut(&id) {
                        p.food = level;
                    }
                }
            }
        }
        let Some(p) = self.players.get_mut(&id) else { return };
        let hurt = p.health > 0.0 && p.health < p.max_health();
        if regen && p.saturation > 0.0 && hurt && p.food >= 20 {
            p.food_timer += 1;
            if p.food_timer >= 10 {
                let amount = p.saturation.min(6.0);
                p.exhaustion = (p.exhaustion + amount).min(40.0);
                p.food_timer = 0;
                self.regain_health(id, amount / 6.0, "SATIATED");
            }
        } else if regen && p.food >= 18 && hurt {
            p.food_timer += 1;
            if p.food_timer >= 80 {
                p.exhaustion = (p.exhaustion + 6.0).min(40.0);
                p.food_timer = 0;
                self.regain_health(id, 1.0, "SATIATED");
            }
        } else if p.food <= 0 {
            p.food_timer += 1;
            if p.food_timer >= 80 {
                p.food_timer = 0;
                let health = p.health;
                if health > 10.0 || difficulty == Difficulty::Hard || health > 1.0 && difficulty == Difficulty::Normal {
                    self.hurt_player(id, Damage::new("minecraft:starve"), 1.0);
                }
            }
        } else {
            p.food_timer = 0;
        }
    }

    /// Adds exhaustion (players in creative or spectator mode get none).
    pub fn add_exhaustion(&mut self, id: i32, amount: f32) {
        if let Some(p) = self.players.get_mut(&id) {
            if matches!(p.game_mode, GameMode::Survival | GameMode::Adventure) {
                p.exhaustion = (p.exhaustion + amount).min(40.0);
            }
        }
    }

    /// Sends health, food and saturation when they changed (as `ServerPlayer.doTick`).
    pub fn send_health_if_changed(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        let now = (p.health, p.food, p.saturation == 0.0);
        if p.sent_health != Some(now) {
            p.sent_health = Some(now);
            // `Player.setHealthScale`: the client shows health out of the scale
            // (`CraftPlayer.getScaledHealth`).
            let health = match p.api.health_scale {
                Some(scale) => (p.health as f64 * scale / p.max_health() as f64) as f32,
                None => p.health,
            };
            p.conn.send(&play::SetHealth { health, food: VarInt(p.food), saturation: p.saturation });
            p.metadata_dirty = true;
        }
    }

    /// A player's natural regeneration, unless plugins stop it (`EntityRegainHealthEvent`).
    fn regain_health(&mut self, id: i32, amount: f32, reason: &str) {
        let Some(world) = self.players.get(&id).map(|p| p.world) else { return };
        if let Some(amount) = crate::plugins::events::on_regain_health(self, world, id, amount, reason) {
            self.heal_player(id, amount);
        }
    }

    pub fn heal_player(&mut self, id: i32, amount: f32) {
        if let Some(p) = self.players.get_mut(&id) {
            if !p.dead && p.health > 0.0 {
                p.health = (p.health + amount).min(p.max_health());
            }
        }
    }

    // --- movement ----------------------------------------------------------------------------

    /// After a movement packet: fall distance and fall damage, jumping and walking
    /// exhaustion (vanilla's `doCheckFallDamage` and `checkMovementStatistics`).
    pub(super) fn after_player_moved(&mut self, id: i32, from: Location, was_on_ground: bool) {
        let Some(p) = self.players.get(&id) else { return };
        if p.dead || p.game_mode == GameMode::Spectator {
            return;
        }
        let world = p.world;
        let to = p.loc;
        let (dx, dy, dz) = (to.x - from.x, to.y - from.y, to.z - from.z);
        let on_ground = p.on_ground;
        let w = &self.worlds[world];
        let feet = w.block_at(to.x.floor() as i32, to.y.floor() as i32, to.z.floor() as i32).unwrap_or(blocks::AIR);
        let area = Aabb::of_entity(Vec3::new(to.x, to.y, to.z), 0.6, 1.8);
        let in_water = box_in_fluid(w, &area, Fluid::Water);
        let below_pos = BlockPos::new(to.x.floor() as i32, (to.y - 0.2).floor() as i32, to.z.floor() as i32);
        let below = w.block_at(below_pos.x, below_pos.y, below_pos.z).unwrap_or(blocks::AIR);
        let climbing = is_climbable(feet);
        let eyes_in_water = w.block_at(to.x.floor() as i32, (to.y + 1.62).floor() as i32, to.z.floor() as i32).and_then(fluid_of).is_some_and(|(f, _)| f == Fluid::Water);
        let slows_fall = matches!(blocks::block_of_state(feet).name, "minecraft:cobweb" | "minecraft:powder_snow" | "minecraft:sweet_berry_bush");
        let fall_damage = w.rules.bool("fall_damage");
        let sprinting = p.sprinting;
        let moved = dx * dx + dy * dy + dz * dz > 1.0e-7;

        let slow_falling = p.effects.iter().any(|e| e.name() == "minecraft:slow_falling");
        let p = self.players.get_mut(&id).unwrap();
        if moved {
            p.moved_this_tick = true;
        }
        // As vanilla's `checkFallDamage`: the drop of this move counts, then landing applies.
        let mut landed: Option<f64> = None;
        if p.flying || in_water || climbing || slows_fall || slow_falling && dy <= 0.0 {
            p.fall_distance = 0.0;
        } else {
            if dy < 0.0 {
                p.fall_distance -= dy;
            }
            // A glider that is not diving falls no further than a block
            // (`checkFallDistanceAccumulation`).
            if p.gliding && dy > -0.5 && p.fall_distance > 1.0 {
                p.fall_distance = 1.0;
            }
            if on_ground {
                if p.fall_distance > 0.0 {
                    landed = Some(p.fall_distance);
                }
                p.fall_distance = 0.0;
            }
        }
        // Steps and strokes sculk hears (`Entity.move`): one every 1/0.6 blocks moved, on
        // something; climbing counts the climb.
        let mut step = None;
        if p.vehicle.is_none() && moved {
            let climb = if climbing { dy } else { 0.0 };
            p.move_dist += ((dx * dx + climb * climb + dz * dz).sqrt() * 0.6) as f32;
            if p.move_dist > p.next_step && !blocks::is_air(below) {
                p.next_step = p.move_dist.floor() + 1.0;
                step = Some(if in_water { "minecraft:swim" } else { "minecraft:step" });
            }
        }

        // Exhaustion: jumping and sprinting on the ground, swimming.
        let horizontal = (dx * dx + dz * dz).sqrt();
        let mut exhaustion = 0.0;
        if was_on_ground && !on_ground && dy > 0.0 {
            exhaustion += if sprinting { 0.2 } else { 0.05 };
        }
        if in_water {
            exhaustion += 0.01 * ((dx * dx + dy * dy + dz * dz).sqrt() * 100.0).round() as f32 * 0.01;
        } else if on_ground && sprinting {
            exhaustion += 0.1 * (horizontal * 100.0).round() as f32 * 0.01;
        }
        if exhaustion > 0.0 {
            self.add_exhaustion(id, exhaustion);
        }

        let here = Vec3::new(to.x, to.y, to.z);
        if let Some(event) = step {
            let muffled = event == "minecraft:step" && paper_data::synced::tag("minecraft:block", "minecraft:occludes_vibration_signals").is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(below) as i32)));
            if !muffled {
                self.game_event(world, event, here, Some(id), (event == "minecraft:step").then_some(below));
            }
        }
        if landed.is_some() {
            self.game_event(world, "minecraft:hit_ground", here, Some(id), Some(below));
        }
        // Standing on a shrieker sets it off (`SculkShriekerBlock.stepOn`).
        if on_ground && blocks::block_of_state(below).name == "minecraft:sculk_shrieker" {
            self.try_shriek(world, below_pos, Some(id));
        }
        if let Some(distance) = landed {
            let multiplier = match blocks::block_of_state(below).name {
                "minecraft:hay_block" | "minecraft:honey_block" => 0.2,
                "minecraft:slime_block" if !self.players[&id].sneaking => 0.0,
                name if name.ends_with("_bed") => 0.5,
                "minecraft:pointed_dripstone" if blocks::property(below, "vertical_direction") == Some("up") && blocks::property(below, "thickness") == Some("tip") => 2.0,
                _ => 1.0,
            };
            if fall_damage {
                self.cause_fall_damage(id, distance, multiplier);
            }
            // Jumping on farmland tramples it.
            let world = self.players[&id].world;
            self.landed_on(world, below_pos, distance, id);
        }
        // Statistics: jumps and how far it went (`checkMovementStatistics`).
        if was_on_ground && !on_ground && dy > 0.0 {
            self.award_stat(id, super::stats::CUSTOM, "minecraft:jump", 1);
        }
        self.movement_stats(id, dx, dy, dz, in_water, eyes_in_water, climbing);
    }

    fn cause_fall_damage(&mut self, id: i32, distance: f64, multiplier: f64) {
        // `calculateFallDamage`: past the safe fall distance (3, more with jump boost), times
        // the fall damage multiplier.
        let Some((safe, factor)) = self.players.get(&id).map(|p| (p.attributes.value("safe_fall_distance"), p.attributes.value("fall_damage_multiplier"))) else { return };
        let amount = ((distance - safe) * multiplier * factor).ceil();
        if amount <= 0.0 {
            return;
        }
        let Some(p) = self.players.get(&id) else { return };
        let (world, pos) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z));
        let damage = Damage::new(if blocks::block_of_state(self.worlds[world].block_at(pos.x.floor() as i32, (pos.y - 0.2).floor() as i32, pos.z.floor() as i32).unwrap_or(0)).name == "minecraft:pointed_dripstone" {
            "minecraft:stalagmite"
        } else {
            "minecraft:fall"
        });
        if let Some(p) = self.players.get_mut(&id) {
            p.last_fall = distance;
        }
        if self.hurt_player(id, damage, amount as f32) {
            let sound = if amount > 4.0 { "minecraft:entity.player.big_fall" } else { "minecraft:entity.player.small_fall" };
            self.play_sound(world, pos, sound, category::PLAYERS, 1.0, 1.0, None);
        }
    }

    // --- damage ------------------------------------------------------------------------------

    /// Hurts a player (vanilla's `Player.hurtServer` / `LivingEntity.hurtServer`). Returns
    /// whether the hit landed.
    pub fn hurt_player(&mut self, id: i32, damage: Damage, mut amount: f32) -> bool {
        let difficulty = self.difficulty;
        let Some(p) = self.players.get(&id) else { return false };
        if p.dead || p.health <= 0.0 {
            return false;
        }
        let world = p.world;
        let rules = &self.worlds[world].rules;
        // Creative and spectator players, and those a plugin made invulnerable
        // (`Entity.setInvulnerable`), only take damage that bypasses invulnerability.
        if (matches!(p.game_mode, GameMode::Creative | GameMode::Spectator) || p.api.invulnerable) && !damage.is("bypasses_invulnerability") {
            return false;
        }
        if damage.is("is_drowning") && !rules.bool("drowning_damage")
            || damage.is("is_fall") && !rules.bool("fall_damage")
            || damage.is("is_fire") && !rules.bool("fire_damage")
            || damage.is("is_freezing") && !rules.bool("freeze_damage")
        {
            return false;
        }
        // Player versus player.
        if let Some(attacker) = damage.attacker {
            if attacker != id && self.players.contains_key(&attacker) && !rules.bool("pvp") {
                return false;
            }
        }
        let info = self.damage_types.get(damage.kind).cloned();
        let attacker_is_mob = damage.attacker.is_some_and(|a| !self.players.contains_key(&a));
        let scales = match info.as_ref().map_or("when_caused_by_living_non_player", |i| i.scaling.as_str()) {
            "always" => true,
            "never" => false,
            _ => attacker_is_mob,
        };
        if scales {
            amount = match difficulty {
                Difficulty::Peaceful => 0.0,
                Difficulty::Easy => (amount / 2.0 + 1.0).min(amount),
                Difficulty::Normal => amount,
                Difficulty::Hard => amount * 1.5,
            };
        }
        if amount <= 0.0 {
            return false;
        }
        if damage.is("is_fire") && !damage.is("bypasses_effects") && self.effect_level(world, id, "minecraft:fire_resistance").is_some() {
            return false;
        }
        // A raised shield takes what it blocks (`applyItemBlocking`); a hit it takes whole
        // does not land.
        let blocked = self.shield_blocks(id, &damage, amount);
        if blocked > 0.0 {
            amount -= blocked;
            if amount <= 0.0 {
                return false;
            }
        }
        let resistance = if damage.is("bypasses_resistance") { None } else { self.effect_level(world, id, "minecraft:resistance") };
        if !crate::plugins::events::on_entity_damage(self, id, &damage, &mut amount) {
            return false;
        }

        // A falling block on the head.
        if damage.is("damages_helmet") && !self.players[&id].inventory[5].is_empty() {
            let wear = (amount / 4.0).max(1.0) as i32;
            if self.wear_player_item(id, 5, wear, true) {
                self.players.get_mut(&id).unwrap().equipment_dirty = true;
            }
            amount *= 0.75;
        }
        let p = self.players.get_mut(&id).unwrap();
        // Within the invulnerability window only damage above the last hit lands.
        let full = if p.invulnerable_time > 10 && !damage.is("bypasses_cooldown") {
            if amount <= p.last_hurt {
                return false;
            }
            let extra = amount - p.last_hurt;
            p.last_hurt = amount;
            amount = extra;
            false
        } else {
            p.last_hurt = amount;
            p.invulnerable_time = 20;
            true
        };

        // Armour, then protection enchantments.
        let armor_slots = [p.inventory[8].clone(), p.inventory[7].clone(), p.inventory[6].clone(), p.inventory[5].clone()];
        let armor_refs: Vec<&Slot> = armor_slots.iter().collect();
        if !damage.is("bypasses_armor") {
            let (points, toughness) = (p.attributes.value("armor") as f32, p.attributes.value("armor_toughness") as f32);
            // Each armour piece wears by a quarter of the damage (at least one point).
            let wear = (amount / 4.0).max(1.0) as i32;
            for slot in 5..9 {
                self.wear_player_item(id, slot, wear, true);
            }
            self.players.get_mut(&id).unwrap().equipment_dirty = true;
            amount = damage_after_armor(amount, points, toughness);
        }
        let p = self.players.get_mut(&id).unwrap();
        // Resistance: 20% less per level.
        // What resistance and absorption took, and what is left, count as statistics.
        let mut awards: Vec<(&str, i32)> = Vec::new();
        if let Some(level) = resistance {
            let before = amount;
            amount = (amount * (25 - (level + 1) * 5) as f32 / 25.0).max(0.0);
            if before - amount > 0.0 && before - amount < 3.4028235e37 {
                awards.push(("minecraft:damage_resisted", ((before - amount) * 10.0).round() as i32));
            }
        }
        if !damage.is("bypasses_enchantments") {
            let protection = enchantment_protection(&armor_refs, &damage);
            if protection > 0.0 {
                amount *= 1.0 - protection / 25.0;
            }
        }
        amount = amount.max(0.0);
        // Absorption hearts go first.
        let absorbed = amount.min(p.absorption);
        if absorbed > 0.0 {
            p.absorption -= absorbed;
            amount -= absorbed;
            p.metadata_dirty = true;
            if absorbed < 3.4028235e37 {
                awards.push(("minecraft:damage_absorbed", (absorbed * 10.0).round() as i32));
            }
        }
        // Vanilla leaves out absurd amounts (`/kill`).
        if amount > 0.0 && amount < 3.4028235e37 {
            awards.push(("minecraft:damage_taken", (amount * 10.0).round() as i32));
        }
        p.health = (p.health - amount).max(0.0);
        if let Some(info) = &info {
            if amount > 0.0 && info.exhaustion > 0.0 && matches!(p.game_mode, GameMode::Survival | GameMode::Adventure) {
                p.exhaustion = (p.exhaustion + info.exhaustion).min(40.0);
            }
        }
        if let Some(attacker) = damage.attacker {
            p.last_attacker = Some((attacker, self.tick));
        }
        p.last_damage = Some(damage.clone());
        let (pos, loc) = (Vec3::new(p.loc.x, p.loc.y, p.loc.z), p.loc);
        let dead = p.health <= 0.0;

        if full {
            // Red flash and hurt sound for everyone, camera tilt for the player.
            let event = play::DamageEvent { entity_id: id, source_type: damage.network_id(), cause_id: damage.attacker, direct_id: damage.direct, source_position: damage.position.map(|v| (v.x, v.y, v.z)) };
            let payload = Bytes::from(encode_packet(&event));
            p.conn.send_payload(payload.clone());
            for viewer in p.seen_by.clone() {
                if let Some(v) = self.players.get(&viewer) {
                    v.conn.send_payload(payload.clone());
                }
            }
            // Knockback away from the source.
            if !damage.is("no_knockback") {
                if let Some(from) = damage.position {
                    let (kx, kz) = (from.x - loc.x, from.z - loc.z);
                    self.knockback_player(id, 0.4, kx, kz, damage.direct, if damage.direct.is_some() { "ENTITY_ATTACK" } else { "DAMAGE" });
                    let yaw = (kz.atan2(kx).to_degrees() as f32) - 90.0 - loc.yaw;
                    self.send(id, &play::HurtAnimation { entity_id: VarInt(id), yaw });
                }
            }
            if !dead {
                let effects = info.as_ref().map_or("hurt", |i| i.effects.as_str());
                let sound = match effects {
                    "drowning" => "minecraft:entity.player.hurt_drown",
                    "burning" => "minecraft:entity.player.hurt_on_fire",
                    "freezing" => "minecraft:entity.player.hurt_freeze",
                    "poking" => "minecraft:entity.player.hurt_sweet_berry_bush",
                    _ => "minecraft:entity.player.hurt",
                };
                self.play_sound(world, pos, sound, category::PLAYERS, 1.0, Self::random_pitch(), None);
            }
        }
        for (stat, amount) in awards {
            self.award_stat(id, super::stats::CUSTOM, stat, amount);
        }
        if self.players.get(&id).is_some_and(|p| p.sleeping.is_some()) {
            self.wake_up(id);
        }
        self.game_event(world, "minecraft:entity_damage", pos, damage.attacker, None);
        if dead {
            self.kill_player(id, damage);
        }
        self.send_health_if_changed(id);
        true
    }

    /// Pushes a player away (`LivingEntity.knockback`); the client applies the motion.
    /// A player's knockback (see `knockback_entity`), sent for its client to carry out. The
    /// server does not know how fast the client moves, so plugins see it as standing still.
    pub fn knockback_player(&mut self, id: i32, strength: f64, dx: f64, dz: f64, attacker: Option<i32>, cause: &str) {
        let Some(p) = self.players.get(&id) else { return };
        let resistance = p.attributes.value("knockback_resistance");
        let strength = strength * (1.0 - resistance.clamp(0.0, 1.0));
        if dx * dx + dz * dz < 1.0e-5 {
            return;
        }
        let len = (dx * dx + dz * dz).sqrt();
        let (nx, nz) = (dx / len * strength, dz / len * strength);
        let vy = if p.on_ground { 0.4f64.min(strength) } else { 0.0 };
        let world = p.world;
        let pushed = Vec3::new(-nx, vy, -nz);
        let Some(knock) = crate::plugins::events::on_knockback(self, world, id, attacker, cause, strength, pushed, Vec3::new(0.0, 0.0, 0.0)) else { return };
        if knock.x == 0.0 && knock.y == 0.0 && knock.z == 0.0 {
            return;
        }
        self.send(id, &play::SetEntityMotion { entity_id: VarInt(id), velocity: LpVec3(knock.x, knock.y, knock.z) });
    }

    // --- death and respawn -----------------------------------------------------------------

    /// The translated death message (vanilla's `DamageSource.getLocalizedDeathMessage` and
    /// `CombatTracker`).
    pub fn death_message(&self, id: i32, damage: &Damage) -> Component {
        let Some(p) = self.players.get(&id) else { return Component::text("") };
        let victim = p.chat_name();
        let info = self.damage_types.get(damage.kind);
        let msg = info.map_or("generic", |i| i.message_id.as_str());
        let killer = damage.attacker.or_else(|| p.last_attacker.filter(|(_, t)| self.tick.saturating_sub(*t) < 100).map(|(a, _)| a)).filter(|a| *a != id);
        let killer_name = killer.and_then(|k| self.entity_display_name(p.world, k));
        if info.is_some_and(|i| i.death_message_type == "fall_variants") {
            return match killer_name {
                Some(name) => Component::translatable("death.fell.assist", vec![victim, name]),
                None if p.last_fall > 5.0 => Component::translatable("death.fell.accident.generic", vec![victim]),
                None => Component::translatable(format!("death.attack.{msg}"), vec![victim]),
            };
        }
        match (damage.attacker, killer_name) {
            (Some(_), Some(name)) => Component::translatable(format!("death.attack.{msg}"), vec![victim, name]),
            (None, Some(name)) => Component::translatable(format!("death.attack.{msg}.player"), vec![victim, name]),
            _ => Component::translatable(format!("death.attack.{msg}"), vec![victim]),
        }
    }

    /// The name of a player or other entity for messages.
    pub fn entity_display_name(&self, world: usize, id: i32) -> Option<Component> {
        if let Some(p) = self.players.get(&id) {
            return Some(p.chat_name());
        }
        let e = self.worlds.get(world)?.entities.get(&id)?;
        Some(e.custom_name.clone().unwrap_or_else(|| {
            let name = e.info.name.replacen(':', ".", 1);
            Component::translatable(format!("entity.{name}"), vec![])
        }))
    }

    pub(crate) fn kill_player(&mut self, id: i32, damage: Damage) {
        let Some(p) = self.players.get(&id) else { return };
        let world = p.world;
        let message = self.death_message(id, &damage);
        // Plugin hook: PlayerDeathEvent may cancel the death, change the message, keep the
        // inventory or level, and change the drops (it applies the game rules without plugins).
        let Some(death) = crate::plugins::events::on_player_death(self, id, &damage, message) else { return };
        // Statistics and the kill and death criteria (`ServerPlayer.die`, `awardKillScore`).
        self.award_stat(id, super::stats::CUSTOM, "minecraft:deaths", 1);
        self.reset_stat(id, super::stats::CUSTOM, "minecraft:time_since_death");
        self.reset_stat(id, super::stats::CUSTOM, "minecraft:time_since_rest");
        self.bump_criteria(id, "deathCount");
        if let Some(attacker) = damage.attacker.filter(|a| *a != id) {
            if self.players.contains_key(&attacker) {
                self.award_stat(id, "minecraft:killed_by", "minecraft:player", 1);
                self.award_stat(attacker, super::stats::CUSTOM, "minecraft:player_kills", 1);
                self.bump_criteria(attacker, "totalKillCount");
                self.bump_criteria(attacker, "playerKillCount");
            } else if let Some(kind) = self.worlds[world].entities.get(&attacker).map(|e| e.info.name.clone()) {
                self.award_stat(id, "minecraft:killed_by", &kind, 1);
            }
        }
        let show = self.worlds[world].rules.bool("show_death_messages") && death.message.is_some();
        let message = death.message.clone().unwrap_or_else(|| Component::text(""));
        self.close_menu(id, true);
        self.clear_effects(world, id, "DEATH");
        // Dying wakes (`LivingEntity.die`) and calls off a portal trip on its way.
        if self.players.get(&id).is_some_and(|p| p.sleeping.is_some()) {
            self.wake_up(id);
        }
        self.cancel_travel(id);
        let Some(p) = self.players.get_mut(&id) else { return };
        p.dead = true;
        p.last_death = Some((world, paper_protocol::codec::BlockPos::new(p.loc.x.floor() as i32, p.loc.y.floor() as i32, p.loc.z.floor() as i32)));
        p.absorption = 0.0;
        p.health = 0.0;
        p.fire_ticks = 0;
        p.using_item = None;
        p.metadata_dirty = true;
        let pos = Vec3::new(p.loc.x, p.loc.y, p.loc.z);
        let combat_message = if show { message.clone() } else { Component::text("") };
        p.conn.send(&play::PlayerCombatKill { player_id: VarInt(id), message: combat_message });
        if show {
            log::info!("{}", message.to_plain());
            self.broadcast_message(&message);
        }
        self.play_sound(world, pos, "minecraft:entity.player.death", category::PLAYERS, 1.0, Self::random_pitch(), None);

        // Drop everything (unless the inventory is kept) and some of the experience.
        // (`death` holds what PlayerDeathEvent decided; the drops fall even with a kept
        // inventory, as in Paper.)
        {
            let p = self.players.get_mut(&id).unwrap();
            if !death.keep_inventory {
                p.inventory.iter_mut().for_each(|s| *s = Slot::EMPTY);
            }
            let items = death.drops;
            let xp = death.xp;
            if !death.keep_level {
                p.xp_level = 0;
                p.xp_progress = 0.0;
                p.xp_total = 0;
            }
            p.equipment_dirty = true;
            let mut rng = rand::thread_rng();
            for stack in items {
                // Vanilla drops from the eyes with a random horizontal throw.
                let (f, g) = (rng.gen::<f64>() * 0.5, rng.gen::<f64>() * std::f64::consts::TAU);
                let vel = Vec3::new(-g.sin() * f, 0.2, g.cos() * f);
                self.spawn_item(world, Vec3::new(pos.x, pos.y + 1.32, pos.z), stack, vel, 40);
            }
            // A catalyst near takes the experience to spread sculk with.
            let left = self.death_game_event(world, id, pos, xp);
            self.award_experience(world, pos, left);
            self.resync_inventory(id);
            self.send_experience(id);
        }
        self.send_health_if_changed(id);
    }

    /// The client asked to respawn after the death screen, or to come back from the end
    /// credits (`ServerGamePacketListenerImpl.handleClientCommand`, `PlayerList.respawn`).
    pub(crate) fn respawn_player(&mut self, id: i32) {
        self.respawn_player_as(id, None);
    }

    /// [`Self::respawn_player`] for `reason` (Paper's `RespawnReason`; by default `DEATH`, or
    /// `END_PORTAL` after the credits).
    pub(crate) fn respawn_player_as(&mut self, id: i32, reason: Option<&'static str>) {
        let Some(p) = self.players.get(&id) else { return };
        // After the end credits the player comes back with everything it had.
        let after_credits = p.won_game && !p.dead;
        // Otherwise vanilla only asks that no health is left: a player that died here, but
        // also one that left on the death screen and came back, or that a plugin set to zero.
        if !after_credits && !p.dead && p.health > 0.0 {
            return;
        }
        let overworld = self.world_of_dimension("minecraft:overworld").unwrap_or(0);
        // Respawn next to the bed (or at a forced point), or at the world spawn with some spread.
        let point = self.respawn_point(id);
        let (world, target) = match point.at {
            Some(found) => found,
            None => (overworld, self.world_spawn_location(overworld, true)),
        };
        let reason = reason.unwrap_or(if after_credits { "END_PORTAL" } else { "DEATH" });
        let (world, target) = crate::plugins::events::on_respawn(self, id, world, target, point.bed, point.missing, reason);
        let hardcore = self.config.properties.hardcore && !after_credits;
        // A plugin may have sent the player away meanwhile.
        let Some(old_world) = self.players.get(&id).map(|p| p.world) else { return };
        self.cancel_travel(id);

        // A new player takes the old one's place, as in vanilla, where a new `ServerPlayer`
        // does: others see the old body go and the new player come, and the client forgets
        // what it saw and gets the chunks and entities around the new place again, even in
        // the same world. Old clients keep chunks until told: the ones the new view leaves
        // out are forgotten (in another world the client drops them all itself).
        let server_view = self.config.properties.view_distance;
        if old_world == world {
            let p = &self.players[&id];
            let mut seen: Vec<VarInt> = p.visible.iter().map(|v| VarInt(*v)).collect();
            seen.extend(self.worlds[world].entities.values().filter(|e| e.viewers.contains(&id)).map(|e| VarInt(e.id)));
            if !seen.is_empty() {
                p.conn.send(&play::RemoveEntities { entity_ids: seen });
            }
            let kept: std::collections::HashSet<_> = super::player::ChunkView::area(target.chunk(), p.view_distance(server_view)).collect();
            for pos in p.view.sent.iter().filter(|c| !kept.contains(c)) {
                p.conn.send(&play::ForgetLevelChunk { x: pos.x, z: pos.z });
            }
        }
        self.leave_world(id);
        let Some(p) = self.players.get(&id) else { return };
        let mut spawn_info = self.spawn_info(world, p.game_mode, p.previous_game_mode);
        spawn_info.death_location = self.death_location(p);
        let p = self.players.get_mut(&id).unwrap();
        p.seen_by.clear();
        if !after_credits {
            p.health = p.max_health();
            p.food = 20;
            p.saturation = 5.0;
            p.exhaustion = 0.0;
            p.food_timer = 0;
            p.air = 300;
            p.last_attacker = None;
            p.last_damage = None;
            // The client starts the new player from the attributes' base values
            // (`assignBaseValues`): their modifiers are sent again.
            p.attributes.mark_all_dirty();
        }
        p.dead = false;
        p.won_game = false;
        p.sleeping = None;
        // The client loads the world again (`restartClientLoadTimerAfterRespawn`).
        p.loaded = false;
        p.load_timeout = 60;
        p.fire_ticks = 0;
        p.fall_distance = 0.0;
        p.invulnerable_time = 0;
        p.portal_time = 0;
        p.sent_health = None;
        p.metadata_dirty = true;
        p.equipment_dirty = true;
        p.world = world;
        // A fresh view, sent from scratch; how fast the client takes chunks belongs to the
        // connection (vanilla's `ChunkSender`), so that carries over.
        let radius = p.view_distance(server_view);
        let old = std::mem::replace(&mut p.view, super::player::ChunkView::new(target.chunk(), radius));
        p.view.desired_per_tick = old.desired_per_tick;
        p.view.unacknowledged_batches = old.unacknowledged_batches;
        p.view.max_unacknowledged_batches = old.max_unacknowledged_batches;
        p.conn.send(&play::Respawn { spawn: spawn_info, data_kept: if after_credits { 3 } else { 0 } });
        let w = &self.worlds[overworld];
        let p = &self.players[&id];
        p.conn.send(&play::SetDefaultSpawnPosition { dimension: w.key.clone(), pos: w.spawn, yaw: w.spawn_yaw, pitch: 0.0 });
        p.conn.send(&play::ChangeDifficulty { difficulty: self.difficulty.id(), locked: false });
        p.conn.send(&p.abilities_packet());
        p.conn.send(&play::SetHeldSlot { slot: VarInt(p.held_slot as i32) });
        p.conn.send(&play::EntityEvent { entity_id: id, status: 24 + p.op_level.min(4) as i8 });
        p.conn.send_payload(self.time_payload_for(world, p));
        p.conn.send(&self.border_packet_for(id, world));
        self.send_weather(id);
        p.conn.send(&play::SetChunkCacheCenter { x: VarInt(p.view.center.x), z: VarInt(p.view.center.z) });
        self.teleport(id, target);
        // As vanilla's `sendLevelInfo`: the client waits for its chunks once it knows where
        // it is.
        self.send(id, &play::GameEvent { event: play::game_event::START_WAITING_FOR_CHUNKS, value: 0.0 });
        self.resync_inventory(id);
        self.send_experience(id);
        self.send_health_if_changed(id);
        // Effects outlive the credits; the new player on the client has none.
        self.resend_effects(id);
        self.refresh_chunk_view(id);
        // Hardcore: a spectator from now on, told to everyone (and to plugins).
        if hardcore {
            self.set_game_mode(id, GameMode::Spectator);
        }
        if old_world != world && self.players.contains_key(&id) {
            crate::plugins::events::on_changed_world(self, id, old_world);
        }
        if self.players.contains_key(&id) {
            crate::plugins::events::on_post_respawn(self, id, point.bed, point.missing, reason);
        }
    }

    /// The world spawn, spread over `respawn_radius` like vanilla when `spread` is set.
    pub fn world_spawn_location(&self, world: usize, spread: bool) -> Location {
        let w = &self.worlds[world];
        let spawn = w.spawn;
        let base = Location { x: spawn.x as f64 + 0.5, y: spawn.y as f64, z: spawn.z as f64 + 0.5, yaw: w.spawn_yaw, pitch: 0.0 };
        let radius = w.rules.int("respawn_radius").max(0);
        if !spread || radius == 0 || !w.has_skylight {
            return base;
        }
        let mut rng = rand::thread_rng();
        for _ in 0..10 {
            let (x, z) = (spawn.x + rng.gen_range(-radius..=radius), spawn.z + rng.gen_range(-radius..=radius));
            let Some(top) = w.top_y(x, z) else { continue };
            let ground = w.block_at(x, top - 1, z).unwrap_or(blocks::AIR);
            if fluid_of(ground).is_some() || !blocks::has_collision(ground) {
                continue;
            }
            return Location { x: x as f64 + 0.5, y: top as f64, z: z as f64 + 0.5, ..base };
        }
        base
    }

    // --- eating and using items --------------------------------------------------------------

    /// Right-click with an item in the air.
    pub(super) fn handle_use_item(&mut self, id: i32, hand: i32, sequence: i32, yaw: f32, pitch: f32) {
        self.send(id, &play::BlockChangedAck { sequence: VarInt(sequence) });
        // Plugins may deny the use of the item (`PlayerInteractEvent`): as they decided for
        // the block click this follows, or for a click in the air.
        let denied = match self.last_interact.remove(&id) {
            Some((h, tick, denied)) if h == hand && self.tick.saturating_sub(tick) <= 1 => denied,
            _ => self.players.get(&id).is_some_and(|p| !p.dead && p.game_mode != GameMode::Spectator) && crate::plugins::events::on_interact(self, id, crate::plugins::events::Click::RightAir, None, hand).item == crate::plugins::events::Use::Deny,
        };
        if denied {
            self.resync_inventory(id);
            return;
        }
        if self.players.get(&id).is_some_and(|p| p.dead) || self.use_bucket(id, hand, yaw, pitch) || self.use_bottle(id, hand, yaw, pitch) {
            return;
        }
        let Some(p) = self.players.get_mut(&id) else { return };
        if p.dead || p.game_mode == GameMode::Spectator {
            return;
        }
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return };
        let data = item_data::by_id(stack.item as u32);
        let name = items::get(stack.item as u32).map_or("", |i| i.name);
        if name == "minecraft:ender_eye" {
            self.throw_ender_eye(id, slot);
            return;
        }
        if name.ends_with("_spawn_egg") {
            self.use_spawn_egg_on_fluid(id, hand, yaw, pitch);
            return;
        }
        if super::boats::is_boat(name) {
            self.use_boat(id, hand, yaw, pitch);
            return;
        }
        // Snowballs, eggs, ender pearls, bottles o' enchanting and splash potions are thrown.
        if super::throwing::throwable(name).is_some() {
            self.throw_held(id, hand);
            return;
        }
        // A crossbow fires what it is loaded with, or starts charging.
        if name == "minecraft:crossbow" {
            self.use_crossbow(id, hand);
            return;
        }
        // A shield is raised.
        if name == "minecraft:shield" {
            self.raise_shield(id, hand);
            return;
        }
        // A firework rocket boosts a glider.
        if name == "minecraft:firework_rocket" {
            self.use_firework(id, hand);
            return;
        }
        // A fishing rod casts a bobber, or reels it in.
        if name == "minecraft:fishing_rod" {
            // The player turns the way the use says first (`absSnapRotationTo`).
            if let Some(p) = self.players.get_mut(&id) {
                (p.loc.yaw, p.loc.pitch) = (yaw, pitch);
            }
            self.use_fishing_rod(id, hand);
            return;
        }
        // A carrot or warped fungus on a stick boosts the pig or strider being ridden.
        if matches!(name, "minecraft:carrot_on_a_stick" | "minecraft:warped_fungus_on_a_stick") {
            self.boost_ridden(id, hand);
            return;
        }
        if let Some(seconds) = data.consume_seconds {
            let can_eat = match &data.food {
                Some(food) => p.game_mode == GameMode::Creative || food.can_always_eat || p.food < 20,
                None => true,
            };
            if can_eat {
                let ticks = (seconds * 20.0).round() as i32;
                if ticks <= 0 {
                    p.using_item = Some((hand as u8, 0));
                    self.finish_using_item(id);
                } else {
                    p.using_item = Some((hand as u8, ticks));
                    p.metadata_dirty = true;
                }
            }
            return;
        }
        if name == "minecraft:bow" {
            let has_arrows = p.game_mode == GameMode::Creative || p.inventory.iter().any(|s| s.0.as_ref().is_some_and(|s| is_arrow(s.item)));
            if has_arrows {
                p.using_item = Some((hand as u8, 72000));
                p.metadata_dirty = true;
            }
        }
    }

    /// The client stopped holding right-click (`releaseUsingItem`).
    pub(super) fn release_using_item(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        let Some((hand, remaining)) = p.using_item.take() else { return };
        p.metadata_dirty = true;
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return };
        match items::get(stack.item as u32).map(|i| i.name) {
            Some("minecraft:bow") => self.shoot_bow(id, slot, 72000 - remaining),
            Some("minecraft:crossbow") => self.release_crossbow(id, slot, 72000 - remaining),
            _ => {}
        }
    }

    fn tick_using_item(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        let Some((hand, remaining)) = p.using_item else { return };
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else {
            p.using_item = None;
            p.metadata_dirty = true;
            return;
        };
        let data = item_data::by_id(stack.item as u32);
        let Some(seconds) = data.consume_seconds else {
            // Bows and other held items just count up; a charging crossbow makes its sounds.
            p.using_item = Some((hand, remaining - 1));
            if items::get(stack.item as u32).is_some_and(|i| i.name == "minecraft:crossbow") {
                self.crossbow_charging(id, slot, 72000 - (remaining - 1));
            }
            return;
        };
        let total = (seconds * 20.0).round() as i32;
        let remaining = remaining - 1;
        p.using_item = Some((hand, remaining));
        let (world, pos) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z));
        // Eating noises after the first few ticks, every four ticks.
        if remaining <= total - 7 && remaining % 4 == 0 && remaining > 0 {
            let sound = if data.food.is_some() { "minecraft:entity.generic.eat" } else { "minecraft:entity.generic.drink" };
            let volume = 0.5 + 0.5 * rand::thread_rng().gen_range(0..2) as f32;
            self.play_sound(world, pos, sound, category::PLAYERS, volume, Self::random_pitch(), Some(id));
        }
        if remaining <= 0 {
            self.finish_using_item(id);
        }
    }

    /// Eats or drinks the item being used.
    fn finish_using_item(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        let Some((hand, _)) = p.using_item.take() else { return };
        p.metadata_dirty = true;
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(held) = p.inventory[slot].0.clone() else { return };
        // Plugins may stop it, or have another item eaten (`PlayerItemConsumeEvent`).
        let Some((consumed, replacement)) = crate::plugins::events::on_item_consume(self, id, &Slot(Some(held.clone())), hand as i32) else {
            self.resync_inventory(id);
            if let Some(p) = self.players.get(&id) {
                p.conn.send(&play::SetHealth { health: p.health, food: VarInt(p.food), saturation: p.saturation });
            }
            return;
        };
        let stack = consumed.0.unwrap_or_else(|| held.clone());
        let data = item_data::by_id(stack.item as u32);
        if let Some(p) = self.players.get(&id) {
            let (world, at) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z));
            self.game_event(world, if data.food.is_some() { "minecraft:eat" } else { "minecraft:drink" }, at, Some(id), None);
            self.game_event(world, "minecraft:item_interact_finish", at, Some(id), None);
        }
        // Eating fills the food bar, as far as plugins let it (`FoodLevelChangeEvent`).
        let food_level = match &data.food {
            Some(food) => {
                let level = (self.players[&id].food + food.nutrition).clamp(0, 20);
                crate::plugins::events::on_food_level_change(self, id, level, Some(&Slot(Some(stack.clone()))))
            }
            None => None,
        };
        let Some(p) = self.players.get_mut(&id) else { return };
        let creative = p.game_mode == GameMode::Creative;
        p.conn.send(&play::EntityEvent { entity_id: id, status: 9 });
        if let (Some(food), Some(level)) = (&data.food, food_level) {
            p.food = level;
            p.saturation = (p.saturation + food.saturation).clamp(0.0, p.food as f32);
        }
        let (world, pos) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z));
        if !creative || replacement.is_some() {
            let rest = if let Some(replacement) = &replacement {
                replacement.clone()
            } else if held.count > 1 {
                Slot(Some(held.with_count(held.count - 1)))
            } else {
                Slot::EMPTY
            };
            p.inventory[slot] = rest;
            p.equipment_dirty = true;
            if let Some(remainder) = data.use_remainder.as_deref().and_then(items::id).filter(|_| replacement.is_none()) {
                let leftover = ItemStack::new(remainder as i32, 1);
                if p.inventory[slot].is_empty() {
                    p.inventory[slot] = Slot(Some(leftover));
                } else {
                    let left = self.add_to_inventory(id, &leftover);
                    if left > 0 {
                        self.player_drop(id, Slot(Some(leftover)), false);
                    }
                }
            }
            self.resync_inventory(id);
        }
        if data.food.is_some() {
            let pitch = rand::thread_rng().gen::<f32>() * 0.1 + 0.9;
            self.play_sound(world, pos, "minecraft:entity.player.burp", category::PLAYERS, 0.5, pitch, None);
        }
        self.consume_effects(id, &stack);
        self.send_health_if_changed(id);
        if let Some(item) = items::get(stack.item as u32) {
            self.award_stat(id, "minecraft:used", item.name, 1);
        }
    }

    // --- experience ----------------------------------------------------------------------------

    /// Spawns experience orbs worth `amount` in total.
    pub fn award_experience(&mut self, world: usize, pos: Vec3, mut amount: i32) {
        let Some(info) = paper_data::entity_types::by_name("minecraft:experience_orb") else { return };
        let mut rng = rand::thread_rng();
        while amount > 0 {
            let value = orb_value(amount);
            amount -= value;
            let mut e = Entity::new(self.next_entity_id(), info, pos, EntityKind::ExperienceOrb { value });
            e.vel = Vec3::new((rng.gen::<f64>() * 0.2 - 0.1) * 2.0, rng.gen::<f64>() * 0.2 * 2.0, (rng.gen::<f64>() * 0.2 - 0.1) * 2.0);
            e.yaw = rng.gen_range(0.0..360.0);
            let id = self.spawn_entity(world, e);
            self.keep_spawned(world, id);
        }
    }

    /// Adds experience points (`giveExperiencePoints`).
    pub fn give_experience(&mut self, id: i32, amount: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        let level_before = p.xp_level;
        p.xp_progress += amount as f32 / xp_needed(p.xp_level) as f32;
        p.xp_total = (p.xp_total + amount).max(0);
        while p.xp_progress >= 1.0 {
            p.xp_progress = (p.xp_progress - 1.0) * xp_needed(p.xp_level) as f32;
            p.xp_level += 1;
            p.xp_progress /= xp_needed(p.xp_level) as f32;
        }
        let (world, pos, level) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z), p.xp_level);
        if level > level_before && level % 5 == 0 {
            let volume = (level as f32 / 30.0).min(1.0) * 0.75;
            self.play_sound(world, pos, "minecraft:entity.player.levelup", category::PLAYERS, volume, 1.0, None);
        }
        self.send_experience(id);
    }

    pub fn send_experience(&mut self, id: i32) {
        if let Some(p) = self.players.get(&id) {
            p.conn.send(&play::SetExperience { bar: p.xp_progress.clamp(0.0, 1.0), level: VarInt(p.xp_level), total: VarInt(p.xp_total) });
        }
    }

    /// Experience orbs drift towards the nearest player and are absorbed on contact.
    pub(super) fn tick_experience_orb(&mut self, world: usize, e: &mut Entity) {
        let EntityKind::ExperienceOrb { value } = e.kind else { return };
        let target = self
            .players
            .values()
            .filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator)
            .map(|p| (p.entity_id, Vec3::new(p.loc.x, p.loc.y + p.eye_height() / 2.0, p.loc.z)))
            .filter(|(_, at)| at.sub(e.pos).length_squared() < 64.0)
            .min_by(|a, b| a.1.sub(e.pos).length_squared().total_cmp(&b.1.sub(e.pos).length_squared()));
        if let Some((player, at)) = target {
            let to = at.sub(e.pos);
            let pull = 1.0 - to.length() / 8.0;
            if pull > 0.0 {
                e.vel = e.vel.add(to.normalize().scale(pull * pull * 0.1));
            }
            // Touching the player: absorbed.
            let player_box = Aabb::of_entity(Vec3::new(at.x, at.y - self.players[&player].eye_height() / 2.0, at.z), 0.6, 1.8);
            let p = self.players.get_mut(&player).unwrap();
            if e.bounding_box().intersects(&player_box) && p.xp_pickup_delay == 0 {
                p.xp_pickup_delay = 2;
                let packet = play::TakeItemEntity { collected_id: VarInt(e.id), collector_id: VarInt(player), count: VarInt(1) };
                let payload = Bytes::from(encode_packet(&packet));
                p.conn.send_payload(payload.clone());
                for v in &e.viewers {
                    if *v != player {
                        if let Some(viewer) = self.players.get(v) {
                            viewer.conn.send_payload(payload.clone());
                        }
                    }
                }
                e.removed = true;
                self.repair_with_experience(player, value);
            }
        }
    }

    /// Mending repairs items with orb experience first; the rest goes to the player.
    fn repair_with_experience(&mut self, id: i32, value: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        let mut left = value;
        for slot in [HOTBAR_START + p.held_slot as usize, OFFHAND_SLOT, 5, 6, 7, 8] {
            if left <= 0 {
                break;
            }
            let Some(stack) = p.inventory[slot].0.as_mut() else { continue };
            if super::loot::enchantment_level(Some(&Slot(Some(stack.clone()))), "minecraft:mending") == 0 {
                continue;
            }
            let damage = item_damage(stack);
            if damage == 0 {
                continue;
            }
            let repaired = (left * 2).min(damage);
            set_item_damage(stack, damage - repaired);
            left -= (repaired + 1) / 2;
            p.equipment_dirty = true;
        }
        if left != value {
            self.resync_inventory(id);
        }
        if left > 0 {
            self.give_experience(id, left);
        }
    }
}

pub fn is_arrow(item: i32) -> bool {
    items::get(item as u32).is_some_and(|i| matches!(i.name, "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:tipped_arrow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_formula_matches_vanilla() {
        // Full diamond (20 armour, 8 toughness) against a 10-point hit:
        // 20 - 10 / (2 + 8 / 4) = 17.5 points, so 70% is absorbed.
        let reduced = damage_after_armor(10.0, 20.0, 8.0);
        assert!((reduced - 3.0).abs() < 0.01, "{reduced}");
        // No armour: unchanged.
        assert_eq!(damage_after_armor(7.0, 0.0, 0.0), 7.0);
    }

    #[test]
    fn experience_levels() {
        assert_eq!(xp_needed(0), 7);
        assert_eq!(xp_needed(15), 37);
        assert_eq!(xp_needed(30), 112);
        assert_eq!(orb_value(10), 7);
        assert_eq!(orb_value(2), 1);
    }

    #[test]
    fn fluid_heights() {
        let water = blocks::default_state("minecraft:water").unwrap();
        assert_eq!(fluid_of(water), Some((Fluid::Water, 8)));
        let flowing = blocks::with_property(water, "level", "3").unwrap();
        assert_eq!(fluid_of(flowing), Some((Fluid::Water, 5)));
        let falling = blocks::with_property(water, "level", "9").unwrap();
        assert_eq!(fluid_of(falling), Some((Fluid::Water, 8)));
        assert!(damage_tag("minecraft:fall", "is_fall"));
        assert!(damage_tag("minecraft:out_of_world", "bypasses_invulnerability"));
        assert!(!damage_tag("minecraft:cactus", "bypasses_armor"));
    }
}
