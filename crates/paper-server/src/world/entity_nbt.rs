//! Entities in vanilla's saved form (the `Entities` list of `entities/r.x.z.mca` chunks):
//! `id`, `Pos`, `Motion`, `Rotation`, `UUID`, ... and the fields of each kind.

use super::entity::{Entity, EntityFlags, EntityKind, LivingState, Mob, MobBrain, Vec3};
use super::storage::{state_from_nbt, state_nbt};
use crate::server::playerdata::{item_from_nbt, item_to_nbt};
use paper_nbt::{Compound, Tag};
use paper_protocol::text::Component;
use paper_protocol::types::Slot;
use uuid::Uuid;

/// Equipment keys in the order of `Mob::equipment`.
const EQUIPMENT: [&str; 8] = ["mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle"];

pub(crate) fn uuid_tag(uuid: Uuid) -> Tag {
    let v = uuid.as_u128();
    Tag::IntArray(vec![(v >> 96) as i32, (v >> 64) as i32, (v >> 32) as i32, v as i32])
}

pub(crate) fn uuid_from(tag: Option<&Tag>) -> Option<Uuid> {
    let Some(Tag::IntArray(v)) = tag else { return None };
    if v.len() != 4 {
        return None;
    }
    let n = v.iter().fold(0u128, |acc, part| (acc << 32) | (*part as u32 as u128));
    Some(Uuid::from_u128(n))
}

fn doubles(v: Vec3) -> Tag {
    Tag::List(vec![Tag::Double(v.x), Tag::Double(v.y), Tag::Double(v.z)])
}

fn vec3(tag: Option<&[Tag]>) -> Option<Vec3> {
    let l = tag?;
    let v = Vec3::new(l.first()?.as_f64()?, l.get(1)?.as_f64()?, l.get(2)?.as_f64()?);
    (v.x.is_finite() && v.y.is_finite() && v.z.is_finite()).then_some(v)
}

fn short(v: i32) -> Tag {
    Tag::Short(v.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
}

/// Whether an entity type keeps its babies in the `Age` field (animals) rather than `IsBaby`.
/// Whether a mob type has an age (`AgeableMob`: animals and the like; not zombies).
pub fn ages(name: &str) -> bool {
    !matches!(name, "minecraft:zombie" | "minecraft:husk" | "minecraft:drowned" | "minecraft:zombie_villager" | "minecraft:piglin" | "minecraft:zombified_piglin")
}

/// Slots in vanilla's `ContainerHelper.saveAllItems` form: the filled ones, each with its
/// `Slot`.
pub fn items_to_nbt(items: &[Slot]) -> Vec<Tag> {
    items
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let mut entry = item_to_nbt(s)?;
            entry.insert("Slot", Tag::Byte(i as i8));
            Some(Tag::Compound(entry))
        })
        .collect()
}

/// `size` slots read back from the `Items` list of `c` (`ContainerHelper.loadAllItems`).
pub fn items_from_nbt(c: &Compound, size: usize) -> Vec<Slot> {
    let mut items = vec![Slot::EMPTY; size];
    for entry in c.list("Items").unwrap_or(&[]).iter().filter_map(|t| t.as_compound()) {
        if let Some(slot) = entry.i32("Slot").filter(|s| (0..size as i32).contains(s)) {
            items[slot as usize] = item_from_nbt(entry);
        }
    }
    items
}

/// The saved form of an entity, or `None` for entities that are not saved (dead ones).
pub fn entity_to_nbt(e: &Entity) -> Option<Compound> {
    entity_to_nbt_in(e, "minecraft:overworld")
}

/// The saved form of an entity in the world of `dimension` (which positions it remembers,
/// such as a villager's workstation, are in).
pub fn entity_to_nbt_in(e: &Entity, dimension: &str) -> Option<Compound> {
    // Thrown eyes of ender live for four seconds and are not kept.
    // (Thrown items and potion clouds are short-lived too.)
    if e.removed || !e.is_alive() || matches!(e.kind, EntityKind::EyeOfEnder { .. } | EntityKind::Thrown { .. } | EntityKind::Cloud { .. } | EntityKind::Fishing(_) | EntityKind::Firework(_)) {
        return None;
    }
    let mut c = Compound::new()
        .with("id", Tag::String(e.info.name.clone()))
        .with("Pos", doubles(e.pos))
        .with("Motion", doubles(e.vel))
        .with("Rotation", Tag::List(vec![Tag::Float(e.yaw), Tag::Float(e.pitch)]))
        .with("OnGround", Tag::Byte(e.on_ground as i8))
        .with("UUID", uuid_tag(e.uuid));
    if let Some(pdc) = &e.pdc {
        c.insert("BukkitValues", Tag::Compound(pdc.clone()));
    }
    if let Some(name) = &e.custom_name {
        c.insert("CustomName", name.to_nbt());
        if e.custom_name_visible {
            c.insert("CustomNameVisible", Tag::Byte(1));
        }
    }
    if e.no_gravity {
        c.insert("NoGravity", Tag::Byte(1));
    }
    let f = &e.flags;
    for (key, set) in [("Glowing", f.glowing), ("Silent", f.silent), ("Invulnerable", f.invulnerable), ("HasVisualFire", f.visual_fire), ("Bukkit.invisible", f.invisible)] {
        if set {
            c.insert(key, Tag::Byte(1));
        }
    }
    if f.portal_cooldown > 0 {
        c.insert("PortalCooldown", Tag::Int(f.portal_cooldown));
    }
    if f.freeze_ticks > 0 {
        c.insert("TicksFrozen", Tag::Int(f.freeze_ticks));
    }
    if !f.tags.is_empty() {
        c.insert("Tags", Tag::List(f.tags.iter().map(|t| Tag::String(t.clone())).collect()));
    }
    match &e.kind {
        EntityKind::Item { stack, pickup_delay, thrower, owner } => {
            c.insert("Item", Tag::Compound(item_to_nbt(stack)?));
            c.insert("PickupDelay", short(*pickup_delay));
            c.insert("Age", short(e.age));
            if let Some(t) = thrower {
                c.insert("Thrower", uuid_tag(*t));
            }
            if let Some(o) = owner {
                c.insert("Owner", uuid_tag(*o));
            }
        }
        EntityKind::ExperienceOrb { value } => {
            c.insert("Value", short(*value));
            c.insert("Age", short(e.age));
            c.insert("Count", Tag::Int(1));
        }
        EntityKind::Arrow { damage, in_ground, life, pickup, critical, .. } => {
            c.insert("damage", Tag::Double(*damage));
            c.insert("inGround", Tag::Byte(*in_ground as i8));
            c.insert("life", short(*life));
            c.insert("pickup", Tag::Byte(*pickup as i8));
            c.insert("crit", Tag::Byte(*critical as i8));
        }
        EntityKind::FallingBlock { state, time, fall } => {
            c.insert("fall_distance", Tag::Double(f64::from(*fall)));
            c.insert("BlockState", state_nbt(*state));
            c.insert("Time", Tag::Int(*time));
        }
        EntityKind::Tnt { fuse } => {
            c.insert("fuse", short(*fuse));
        }
        EntityKind::Mob(mob) => {
            let l = &mob.living;
            c.insert("Health", Tag::Float(l.health));
            c.insert("HurtTime", short(l.hurt_time));
            c.insert("Fire", short(l.fire_ticks));
            c.insert("Air", short(l.air));
            c.insert("fall_distance", Tag::Double(l.fall_distance));
            c.insert("PersistenceRequired", Tag::Byte(mob.persistent as i8));
            for (key, set) in [("NoAI", f.no_ai), ("LeftHanded", f.left_handed), ("CanPickUpLoot", f.can_pick_up_loot), ("AgeLocked", f.age_locked)] {
                if set {
                    c.insert(key, Tag::Byte(1));
                }
            }
            if f.unaware {
                c.insert("Bukkit.Aware", Tag::Byte(0));
            }
            if let Some(reason) = &f.spawn_reason {
                c.insert("Paper.SpawnReason", Tag::String(reason.clone()));
            }
            c.insert("attributes", mob.attributes.to_nbt());
            let mut equipment = Compound::new();
            for (key, slot) in EQUIPMENT.iter().zip(mob.equipment.iter()) {
                if let Some(item) = item_to_nbt(slot) {
                    equipment.insert(*key, Tag::Compound(item));
                }
            }
            c.insert("equipment", Tag::Compound(equipment));
            if ages(&e.info.name) {
                let age = if mob.baby { mob.age.min(-1) } else { mob.age.max(0) };
                c.insert("Age", Tag::Int(age));
                c.insert("InLove", Tag::Int(mob.in_love));
                if e.info.name == "minecraft:chicken" {
                    c.insert("EggLayTime", Tag::Int(mob.egg_time));
                }
            } else {
                c.insert("IsBaby", Tag::Byte(mob.baby as i8));
            }
            if e.info.name == "minecraft:sheep" {
                c.insert("Color", Tag::Byte(mob.variant as i8));
                c.insert("Sheared", Tag::Byte(mob.sheared as i8));
            }
            if e.info.name == "minecraft:creeper" && mob.powered {
                c.insert("powered", Tag::Byte(1));
            }
            // Variants: an entry of a variant registry by key, or a number (horses and
            // llamas keep theirs with the rest of their data below).
            if let Some(key) = crate::server::variants::registry_of(&e.info.name).and_then(|r| crate::server::variants::key_of(r, mob.variant)) {
                c.insert("variant", Tag::String(key.to_owned()));
            }
            match e.info.name.as_str() {
                "minecraft:rabbit" => {
                    c.insert("RabbitType", Tag::Int(mob.variant));
                }
                "minecraft:parrot" | "minecraft:axolotl" => {
                    c.insert("Variant", Tag::Int(mob.variant));
                }
                name => {
                    if let Some(t) = crate::server::variants::type_name(name, mob.variant) {
                        c.insert("Type", Tag::String(t.to_owned()));
                    }
                }
            }
            if let Some(merchant) = &mob.merchant {
                merchant.save(&mut c, dimension, e.info.name == "minecraft:villager");
            }
            if let Some(carried) = mob.carried {
                c.insert("carriedBlockState", state_nbt(carried));
            }
            // `TamableAnimal` and `AbstractHorse`.
            if let Some(owner) = mob.owner {
                c.insert("Owner", uuid_tag(owner));
            }
            if let Some(dragon) = &mob.dragon {
                c.insert("DragonPhase", Tag::Int(dragon.phase as i32));
                c.insert("DragonDeathTime", Tag::Int(dragon.death_time));
            }
            if crate::server::riding::is_horse(&e.info.name) {
                c.insert("Tame", Tag::Byte(mob.tamed as i8));
                c.insert("Temper", Tag::Int(mob.temper));
                c.insert("Variant", Tag::Int(mob.variant));
                // Donkeys', mules' and llamas' chests (slots count from 0 within the chest).
                if let Some(chest) = &mob.chest {
                    c.insert("ChestedHorse", Tag::Byte(1));
                    c.insert("Items", Tag::List(items_to_nbt(chest)));
                }
                if e.info.name.contains("llama") {
                    c.insert("Strength", Tag::Int(mob.strength));
                }
            } else if crate::server::taming::is_tameable(&e.info.name) {
                c.insert("Sitting", Tag::Byte(mob.sitting as i8));
                if matches!(e.info.name.as_str(), "minecraft:wolf" | "minecraft:cat") {
                    c.insert("CollarColor", Tag::Byte(mob.collar as i8));
                }
            }
        }
        // `addChestVehicleSaveData` (Paper keeps the slots even while a loot table is left).
        EntityKind::Boat { chest: Some(chest), .. } => {
            if let Some(table) = &chest.loot_table {
                c.insert("LootTable", Tag::String(table.clone()));
                if chest.loot_seed != 0 {
                    c.insert("LootTableSeed", Tag::Long(chest.loot_seed));
                }
            }
            c.insert("Items", Tag::List(items_to_nbt(&chest.items)));
        }
        // `BlockAttachedEntity`'s `block_pos`; a painting's `facing` (2D) and `variant`, a
        // frame's `Facing` (3D), item, drop chance, turn and flags.
        EntityKind::Hanging(h) => {
            c.insert("block_pos", Tag::IntArray(vec![h.block.x, h.block.y, h.block.z]));
            if e.info.name == super::hanging::PAINTING {
                c.insert("facing", Tag::Byte(FACING_2D[h.facing as usize % 6]));
                c.insert("variant", Tag::String(h.variant.clone()));
            } else {
                c.insert("Facing", Tag::Byte(h.facing as i8));
                if let Some(item) = item_to_nbt(&h.item).filter(|_| !h.item.is_empty()) {
                    c.insert("Item", Tag::Compound(item));
                }
                c.insert("ItemDropChance", Tag::Float(h.drop_chance));
                c.insert("ItemRotation", Tag::Byte(h.rotation as i8));
                c.insert("Invisible", Tag::Byte(e.flags.invisible as i8));
                c.insert("Fixed", Tag::Byte(h.fixed as i8));
            }
        }
        // `EndCrystal.addAdditionalSaveData`.
        EntityKind::Crystal { beam_target, show_bottom } => {
            if let Some(p) = beam_target {
                c.insert("beam_target", Tag::IntArray(vec![p.x, p.y, p.z]));
            }
            c.insert("ShowBottom", Tag::Byte(*show_bottom as i8));
        }
        EntityKind::EyeOfEnder { .. } | EntityKind::Thrown { .. } | EntityKind::Cloud { .. } | EntityKind::Boat { .. } | EntityKind::Fishing(_) | EntityKind::Firework(_) | EntityKind::Other => {}
    }
    Some(c)
}

/// `Direction`'s 2D data values (south 0, west 1, north 2, east 3) by 3D value; up and down
/// have none.
const FACING_2D: [i8; 6] = [-1, -1, 2, 0, 1, 3];

/// A hanging entity's saved form: its block (`block_pos`, or the older `TileX`/`TileY`/`TileZ`)
/// and the rest as `entity_to_nbt` writes it.
fn hanging_from_nbt(name: &str, c: &Compound) -> Option<super::hanging::Hanging> {
    let block = match c.get("block_pos") {
        Some(Tag::IntArray(v)) if v.len() == 3 => paper_protocol::codec::BlockPos::new(v[0], v[1], v[2]),
        _ => paper_protocol::codec::BlockPos::new(c.i32("TileX")?, c.i32("TileY")?, c.i32("TileZ")?),
    };
    if name == super::hanging::PAINTING {
        let facing = c.i32("facing").and_then(|f| FACING_2D.iter().position(|&v| v as i32 == f)).unwrap_or(3) as u8;
        let mut h = super::hanging::Hanging::new(block, facing);
        h.variant = c.string("variant").unwrap_or("minecraft:kebab").to_owned();
        return Some(h);
    }
    let mut h = super::hanging::Hanging::new(block, c.i32("Facing").unwrap_or(3).clamp(0, 5) as u8);
    h.item = c.compound("Item").map(item_from_nbt).unwrap_or(Slot::EMPTY);
    h.drop_chance = c.f64("ItemDropChance").unwrap_or(1.0) as f32;
    h.rotation = c.i32("ItemRotation").unwrap_or(0).rem_euclid(8) as u8;
    h.fixed = c.bool("Fixed").unwrap_or(false);
    Some(h)
}

/// An entity from its saved form, with a new runtime `id`.
/// A mob's saved variant: a key of its variant registry (its default if none is saved), a
/// rabbit's `RabbitType`, a fox's or mooshroom's `Type`; a sheep's colour is its `Color`,
/// and horses, llamas, parrots and axolotls keep a `Variant`.
fn saved_variant(name: &str, c: &Compound) -> i32 {
    use crate::server::variants;
    if let Some(registry) = variants::registry_of(name) {
        return c.string("variant").and_then(|k| variants::id_of(registry, k)).unwrap_or_else(|| variants::default_id(registry));
    }
    match name {
        "minecraft:rabbit" => c.i32("RabbitType"),
        "minecraft:fox" | "minecraft:mooshroom" => c.string("Type").map(|t| variants::type_from_name(name, t)),
        _ => None,
    }
    .or_else(|| c.i32("Color"))
    .or_else(|| c.i32("Variant"))
    .unwrap_or(0)
}

pub fn entity_from_nbt(id: i32, c: &Compound) -> Option<Entity> {
    let info = paper_data::entity_types::by_name(c.string("id")?)?;
    let pos = vec3(c.list("Pos"))?;
    let kind = match info.name.as_str() {
        "minecraft:item" => {
            let stack = c.compound("Item").map(item_from_nbt).unwrap_or(Slot::EMPTY);
            if stack.is_empty() {
                return None;
            }
            EntityKind::Item { stack, pickup_delay: c.i32("PickupDelay").unwrap_or(0), thrower: uuid_from(c.get("Thrower")), owner: uuid_from(c.get("Owner")) }
        }
        "minecraft:experience_orb" => EntityKind::ExperienceOrb { value: c.i32("Value").unwrap_or(1) },
        "minecraft:arrow" | "minecraft:spectral_arrow" => EntityKind::Arrow {
            damage: c.f64("damage").unwrap_or(2.0),
            owner: None,
            in_ground: c.bool("inGround").unwrap_or(false),
            life: c.i32("life").unwrap_or(0),
            pickup: c.i32("pickup").unwrap_or(1) == 1,
            critical: c.bool("crit").unwrap_or(false),
        },
        "minecraft:falling_block" => EntityKind::FallingBlock { state: c.get("BlockState").and_then(state_from_nbt)?, time: c.i32("Time").unwrap_or(0) , fall: c.f64("fall_distance").unwrap_or(0.0) as f32 },
        "minecraft:tnt" => EntityKind::Tnt { fuse: c.i32("fuse").unwrap_or(80) },
        n if crate::server::boats::is_boat(n) => {
            let mut kind = crate::server::boats::boat_kind(n);
            if let EntityKind::Boat { chest: Some(chest), .. } = &mut kind {
                chest.items = items_from_nbt(c, chest.items.len());
                chest.loot_table = c.string("LootTable").map(str::to_owned);
                chest.loot_seed = c.i64("LootTableSeed").unwrap_or(0);
            }
            kind
        }
        _ if info.mob => {
            let mut attributes = crate::world::attributes::Attributes::of_type(&info.name);
            attributes.load_nbt(c.list("attributes").unwrap_or(&[]));
            attributes.take_dirty();
            let max_health = if attributes.has("max_health") { attributes.value("max_health") as f32 } else { info.max_health.max(1.0) };
            let mut living = LivingState::new(max_health);
            living.health = c.f64("Health").map_or(max_health, |h| h as f32).clamp(0.0, max_health);
            if living.health <= 0.0 {
                return None;
            }
            living.fire_ticks = c.i32("Fire").unwrap_or(0).max(0);
            living.air = c.i32("Air").unwrap_or(300);
            living.fall_distance = c.f64("fall_distance").unwrap_or(0.0);
            let mut equipment: [Slot; 8] = Default::default();
            if let Some(eq) = c.compound("equipment") {
                for (i, key) in EQUIPMENT.iter().enumerate() {
                    if let Some(item) = eq.compound(key) {
                        equipment[i] = item_from_nbt(item);
                    }
                }
            }
            let age = if ages(&info.name) { c.i32("Age").unwrap_or(0) } else { 0 };
            let baby = if ages(&info.name) { age < 0 } else { c.bool("IsBaby").unwrap_or(false) };
            EntityKind::Mob(Box::new(Mob {
                attributes,
                living,
                equipment,
                brain: MobBrain { speed_factor: 1.0, idle_ticks: 60, ..Default::default() },
                persistent: c.bool("PersistenceRequired").unwrap_or(false),
                baby,
                variant: saved_variant(&info.name, c),
                sheared: c.bool("Sheared").unwrap_or(false),
                powered: c.bool("powered").unwrap_or(false),
                no_player_ticks: 0,
                age,
                in_love: c.i32("InLove").unwrap_or(0),
                love_cause: None,
                egg_time: c.i32("EggLayTime").unwrap_or(6000),
                merchant: crate::server::villagers::is_merchant(&info.name).then(|| Box::new(crate::world::merchant::Merchant::load(c, info.name == "minecraft:villager"))),
                carried: c.get("carriedBlockState").and_then(state_from_nbt),
                creepy: false,
                stared_at: false,
                tamed: c.bool("Tame").unwrap_or(false) || uuid_from(c.get("Owner")).is_some() && !crate::server::riding::is_horse(&info.name),
                owner: uuid_from(c.get("Owner")),
                sitting: c.bool("Sitting").unwrap_or(false),
                collar: c.i32("CollarColor").unwrap_or(14),
                temper: c.i32("Temper").unwrap_or(0),
                boost: 0,
                chest: c.bool("ChestedHorse").unwrap_or(false).then(|| {
                    let size = crate::server::riding::chest_columns(&info.name, c.i32("Strength").unwrap_or(3)) as usize * 3;
                    items_from_nbt(c, size)
                }),
                strength: c.i32("Strength").unwrap_or(3).clamp(1, 5),
                dragon: (info.name == crate::server::dragon::DRAGON).then(|| {
                    let mut brain = crate::server::dragon::DragonBrain::new(crate::server::dragon::Phase::from_id(c.i32("DragonPhase").unwrap_or(0)));
                    brain.death_time = c.i32("DragonDeathTime").unwrap_or(0);
                    Box::new(brain)
                }),
            }))
        }
        name if super::hanging::is_hanging(name) => EntityKind::Hanging(Box::new(hanging_from_nbt(name, c)?)),
        // Its beam's target (`beam_target`, or the `BeamTarget` compound of older worlds).
        "minecraft:end_crystal" => EntityKind::Crystal {
            beam_target: match c.get("beam_target").or_else(|| c.get("BeamTarget")) {
                Some(Tag::IntArray(v)) if v.len() == 3 => Some(paper_protocol::codec::BlockPos::new(v[0], v[1], v[2])),
                Some(Tag::Compound(p)) => match (p.i32("X"), p.i32("Y"), p.i32("Z")) {
                    (Some(x), Some(y), Some(z)) => Some(paper_protocol::codec::BlockPos::new(x, y, z)),
                    _ => None,
                },
                _ => None,
            },
            show_bottom: c.bool("ShowBottom").unwrap_or(true),
        },
        _ => EntityKind::Other,
    };
    let mut e = Entity::new(id, info, pos, kind);
    if let Some(uuid) = uuid_from(c.get("UUID")) {
        e.uuid = uuid;
    }
    e.pdc = c.compound("BukkitValues").cloned();
    e.vel = vec3(c.list("Motion")).unwrap_or(Vec3::ZERO);
    if let Some(rotation) = c.list("Rotation") {
        e.yaw = rotation.first().and_then(Tag::as_f64).unwrap_or(0.0) as f32;
        e.pitch = rotation.get(1).and_then(Tag::as_f64).unwrap_or(0.0) as f32;
        e.head_yaw = e.yaw;
    }
    e.on_ground = c.bool("OnGround").unwrap_or(false);
    e.no_gravity = c.bool("NoGravity").unwrap_or(false);
    e.age = c.i32("Age").filter(|_| matches!(e.kind, EntityKind::Item { .. } | EntityKind::ExperienceOrb { .. })).unwrap_or(0);
    e.custom_name = c.get("CustomName").map(|t| match t {
        Tag::String(s) => Component::from_json_str(s).unwrap_or_else(|_| Component::text(s.clone())),
        other => Component::from_nbt(other),
    });
    e.custom_name_visible = c.bool("CustomNameVisible").unwrap_or(false);
    let flag = |key: &str| c.bool(key).unwrap_or(false);
    e.flags = EntityFlags {
        glowing: flag("Glowing"),
        silent: flag("Silent"),
        invisible: flag("Bukkit.invisible") || flag("Invisible") && super::hanging::is_frame(&info.name),
        invulnerable: flag("Invulnerable"),
        visual_fire: flag("HasVisualFire"),
        portal_cooldown: c.i32("PortalCooldown").unwrap_or(0).max(0),
        freeze_ticks: c.i32("TicksFrozen").unwrap_or(0).max(0),
        tags: c.list("Tags").unwrap_or(&[]).iter().filter_map(Tag::as_str).map(str::to_owned).collect(),
        no_ai: flag("NoAI"),
        unaware: c.bool("Bukkit.Aware") == Some(false),
        left_handed: flag("LeftHanded"),
        can_pick_up_loot: flag("CanPickUpLoot"),
        age_locked: flag("AgeLocked"),
        spawn_reason: c.string("Paper.SpawnReason").map(str::to_owned),
        visual_only: false,
    };
    if let EntityKind::Hanging(h) = &mut e.kind {
        h.check = id.rem_euclid(super::hanging::CHECK_INTERVAL);
    }
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_paintings_round_trip() {
        use crate::world::hanging::{new_entity, Hanging, ITEM_FRAME, PAINTING};
        let block = paper_protocol::codec::BlockPos::new(4, 70, -3);
        let mut frame = Hanging::new(block, 4);
        frame.item = Slot::of(paper_data::items::id("minecraft:diamond").unwrap() as i32, 1);
        frame.rotation = 5;
        frame.fixed = true;
        frame.drop_chance = 0.5;
        let mut e = new_entity(9, ITEM_FRAME, frame).unwrap();
        e.flags.invisible = true;
        let nbt = entity_to_nbt(&e).unwrap();
        assert_eq!(nbt.get("block_pos"), Some(&Tag::IntArray(vec![4, 70, -3])));
        assert_eq!(nbt.get("Facing"), Some(&Tag::Byte(4)));
        let back = entity_from_nbt(10, &nbt).unwrap();
        let EntityKind::Hanging(h) = &back.kind else { panic!("not hanging: {:?}", back.kind) };
        assert_eq!((h.block, h.facing, h.rotation, h.fixed, h.drop_chance), (block, 4, 5, true, 0.5));
        assert!(!h.item.is_empty() && back.flags.invisible);

        let mut painting = Hanging::new(block, 5);
        painting.variant = "minecraft:pointer".into();
        let nbt = entity_to_nbt(&new_entity(11, PAINTING, painting).unwrap()).unwrap();
        // East is 3 among the horizontal directions.
        assert_eq!(nbt.get("facing"), Some(&Tag::Byte(3)));
        let back = entity_from_nbt(12, &nbt).unwrap();
        let EntityKind::Hanging(h) = &back.kind else { panic!() };
        assert_eq!((h.facing, h.variant.as_str()), (5, "minecraft:pointer"));
    }

    #[test]
    fn mobs_and_items_round_trip() {
        let zombie = paper_data::entity_types::by_name("minecraft:zombie").unwrap();
        let mut mob = Mob {
            attributes: crate::world::attributes::Attributes::of_type("minecraft:zombie"),
            living: LivingState::new(20.0),
            equipment: Default::default(),
            brain: MobBrain::default(),
            persistent: true,
            baby: true,
            variant: 0,
            sheared: false,
            powered: false,
            no_player_ticks: 0,
            age: 0,
            in_love: 0,
            love_cause: None,
            egg_time: 0,
            merchant: None,
            carried: None,
            creepy: false,
            stared_at: false,
            tamed: false,
            owner: None,
            sitting: false,
            collar: 14,
            temper: 0,
            boost: 0,
            chest: None,
            strength: 3,
            dragon: None,
        };
        mob.living.health = 13.5;
        mob.equipment[5] = Slot::of(paper_data::items::id("minecraft:iron_helmet").unwrap() as i32, 1);
        let mut e = Entity::new(7, zombie, Vec3::new(1.5, -60.0, 2.5), EntityKind::Mob(Box::new(mob)));
        e.yaw = 45.0;
        e.custom_name = Some(Component::text("Bob"));
        let nbt = entity_to_nbt(&e).unwrap();
        let back = entity_from_nbt(99, &nbt).unwrap();
        assert_eq!((back.id, back.uuid, back.pos, back.yaw), (99, e.uuid, e.pos, 45.0));
        let m = back.mob().unwrap();
        assert_eq!((m.living.health, m.baby, m.persistent), (13.5, true, true));
        assert_eq!(m.equipment[5], e.mob().unwrap().equipment[5]);
        assert_eq!(back.custom_name.as_ref().map(|c| c.to_plain()), Some("Bob".to_owned()));

        let item_type = paper_data::entity_types::by_name("minecraft:item").unwrap();
        let stick = Slot::of(paper_data::items::id("minecraft:stick").unwrap() as i32, 5);
        let owner = Uuid::from_u128(0x1234_5678);
        let item = Entity::new(3, item_type, Vec3::new(0.0, 0.0, 0.0), EntityKind::Item { stack: stick.clone(), pickup_delay: 10, thrower: None, owner: Some(owner) });
        let back = entity_from_nbt(4, &entity_to_nbt(&item).unwrap()).unwrap();
        assert!(matches!(back.kind, EntityKind::Item { ref stack, pickup_delay: 10, thrower: None, owner: Some(o) } if *stack == stick && o == owner));

        // A chest boat keeps its slots and the loot table still to roll.
        let boat_type = paper_data::entity_types::by_name("minecraft:birch_chest_boat").unwrap();
        let mut boat = Entity::new(5, boat_type, Vec3::new(0.5, -60.0, 0.5), crate::server::boats::boat_kind("minecraft:birch_chest_boat"));
        let chest = boat.chest_mut().unwrap();
        chest.items[26] = stick.clone();
        chest.loot_table = Some("minecraft:chests/shipwreck_supply".to_owned());
        chest.loot_seed = 42;
        let back = entity_from_nbt(6, &entity_to_nbt(&boat).unwrap()).unwrap();
        let chest = back.chest().expect("a chest boat again");
        assert_eq!((chest.items.len(), &chest.items[26], chest.loot_table.as_deref(), chest.loot_seed), (27, &stick, Some("minecraft:chests/shipwreck_supply"), 42));
        let plain = paper_data::entity_types::by_name("minecraft:oak_boat").unwrap();
        let back = entity_from_nbt(8, &entity_to_nbt(&Entity::new(7, plain, Vec3::new(0.0, 0.0, 0.0), crate::server::boats::boat_kind("minecraft:oak_boat"))).unwrap()).unwrap();
        assert!(matches!(back.kind, EntityKind::Boat { chest: None, .. }), "a plain boat has no chest");
    }
}
