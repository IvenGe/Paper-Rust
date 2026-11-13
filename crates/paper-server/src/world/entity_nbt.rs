//! Entities in vanilla's saved form (the `Entities` list of `entities/r.x.z.mca` chunks):
//! `id`, `Pos`, `Motion`, `Rotation`, `UUID`, ... and the fields of each kind.

use super::entity::{Entity, EntityKind, LivingState, Mob, MobBrain, Vec3};
use super::storage::{state_from_nbt, state_nbt};
use crate::server::playerdata::{item_from_nbt, item_to_nbt};
use paper_nbt::{Compound, Tag};
use paper_protocol::text::Component;
use paper_protocol::types::Slot;
use uuid::Uuid;

/// Equipment keys in the order of `Mob::equipment`.
const EQUIPMENT: [&str; 6] = ["mainhand", "offhand", "feet", "legs", "chest", "head"];

fn uuid_tag(uuid: Uuid) -> Tag {
    let v = uuid.as_u128();
    Tag::IntArray(vec![(v >> 96) as i32, (v >> 64) as i32, (v >> 32) as i32, v as i32])
}

fn uuid_from(tag: Option<&Tag>) -> Option<Uuid> {
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

/// The saved form of an entity, or `None` for entities that are not saved (dead ones).
pub fn entity_to_nbt(e: &Entity) -> Option<Compound> {
    if e.removed || !e.is_alive() {
        return None;
    }
    let mut c = Compound::new()
        .with("id", Tag::String(e.info.name.clone()))
        .with("Pos", doubles(e.pos))
        .with("Motion", doubles(e.vel))
        .with("Rotation", Tag::List(vec![Tag::Float(e.yaw), Tag::Float(e.pitch)]))
        .with("OnGround", Tag::Byte(e.on_ground as i8))
        .with("UUID", uuid_tag(e.uuid));
    if let Some(name) = &e.custom_name {
        c.insert("CustomName", name.to_nbt());
        if e.custom_name_visible {
            c.insert("CustomNameVisible", Tag::Byte(1));
        }
    }
    if e.no_gravity {
        c.insert("NoGravity", Tag::Byte(1));
    }
    match &e.kind {
        EntityKind::Item { stack, pickup_delay, thrower } => {
            c.insert("Item", Tag::Compound(item_to_nbt(stack)?));
            c.insert("PickupDelay", short(*pickup_delay));
            c.insert("Age", short(e.age));
            if let Some(t) = thrower {
                c.insert("Thrower", uuid_tag(*t));
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
        EntityKind::FallingBlock { state, time } => {
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
        }
        EntityKind::Other => {}
    }
    Some(c)
}

/// An entity from its saved form, with a new runtime `id`.
pub fn entity_from_nbt(id: i32, c: &Compound) -> Option<Entity> {
    let info = paper_data::entity_types::by_name(c.string("id")?)?;
    let pos = vec3(c.list("Pos"))?;
    let kind = match info.name.as_str() {
        "minecraft:item" => {
            let stack = c.compound("Item").map(item_from_nbt).unwrap_or(Slot::EMPTY);
            if stack.is_empty() {
                return None;
            }
            EntityKind::Item { stack, pickup_delay: c.i32("PickupDelay").unwrap_or(0), thrower: uuid_from(c.get("Thrower")) }
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
        "minecraft:falling_block" => EntityKind::FallingBlock { state: c.get("BlockState").and_then(state_from_nbt)?, time: c.i32("Time").unwrap_or(0) },
        "minecraft:tnt" => EntityKind::Tnt { fuse: c.i32("fuse").unwrap_or(80) },
        _ if info.mob => {
            let max_health = info.attribute("max_health").map_or(info.max_health, |h| h as f32).max(1.0);
            let mut living = LivingState::new(max_health);
            living.health = c.f64("Health").map_or(max_health, |h| h as f32).clamp(0.0, max_health);
            if living.health <= 0.0 {
                return None;
            }
            living.fire_ticks = c.i32("Fire").unwrap_or(0).max(0);
            living.air = c.i32("Air").unwrap_or(300);
            living.fall_distance = c.f64("fall_distance").unwrap_or(0.0);
            let mut equipment: [Slot; 6] = Default::default();
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
                living,
                equipment,
                brain: MobBrain { speed_factor: 1.0, idle_ticks: 60, ..Default::default() },
                persistent: c.bool("PersistenceRequired").unwrap_or(false),
                baby,
                variant: c.i32("Color").unwrap_or(0),
                sheared: c.bool("Sheared").unwrap_or(false),
                no_player_ticks: 0,
                age,
                in_love: c.i32("InLove").unwrap_or(0),
                love_cause: None,
                egg_time: c.i32("EggLayTime").unwrap_or(6000),
            }))
        }
        _ => EntityKind::Other,
    };
    let mut e = Entity::new(id, info, pos, kind);
    if let Some(uuid) = uuid_from(c.get("UUID")) {
        e.uuid = uuid;
    }
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
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobs_and_items_round_trip() {
        let zombie = paper_data::entity_types::by_name("minecraft:zombie").unwrap();
        let mut mob = Mob {
            living: LivingState::new(20.0),
            equipment: Default::default(),
            brain: MobBrain::default(),
            persistent: true,
            baby: true,
            variant: 0,
            sheared: false,
            no_player_ticks: 0,
            age: 0,
            in_love: 0,
            love_cause: None,
            egg_time: 0,
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
        let item = Entity::new(3, item_type, Vec3::new(0.0, 0.0, 0.0), EntityKind::Item { stack: stick.clone(), pickup_delay: 10, thrower: None });
        let back = entity_from_nbt(4, &entity_to_nbt(&item).unwrap()).unwrap();
        assert!(matches!(back.kind, EntityKind::Item { ref stack, pickup_delay: 10, .. } if *stack == stick));
    }
}
