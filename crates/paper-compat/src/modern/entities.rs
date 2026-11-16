//! Entities: their types, data and movement as the older versions encode them.

use super::{items, misc, Modern, V1_21_11, V1_21_6, V1_21_9, V26_1, V26_2};
use crate::io::{self, Result, Rw};
use crate::Outcome;
use anyhow::bail;
use paper_data::packets::play;
use paper_protocol::codec::{Reader, WriteExt};

/// Entity data serializers, as 26.3 numbers them.
mod serializer {
    pub const BYTE: i32 = 0;
    pub const INT: i32 = 1;
    pub const LONG: i32 = 2;
    pub const FLOAT: i32 = 3;
    pub const STRING: i32 = 4;
    pub const COMPONENT: i32 = 5;
    pub const OPTIONAL_COMPONENT: i32 = 6;
    pub const ITEM_STACK: i32 = 7;
    pub const BOOLEAN: i32 = 8;
    pub const ROTATIONS: i32 = 9;
    pub const BLOCK_POS: i32 = 10;
    pub const OPTIONAL_BLOCK_POS: i32 = 11;
    pub const DIRECTION: i32 = 12;
    pub const OPTIONAL_LIVING_ENTITY_REFERENCE: i32 = 13;
    pub const BLOCK_STATE: i32 = 14;
    pub const OPTIONAL_BLOCK_STATE: i32 = 15;
    pub const PARTICLE: i32 = 16;
    pub const PARTICLES: i32 = 17;
    pub const VILLAGER_DATA: i32 = 18;
    pub const OPTIONAL_UNSIGNED_INT: i32 = 19;
    pub const POSE: i32 = 20;
    /// 21 to 32: variants of animals, by registry id.
    pub const FIRST_VARIANT: i32 = 21;
    pub const LAST_VARIANT: i32 = 32;
    pub const OPTIONAL_GLOBAL_POS: i32 = 33;
    pub const PAINTING_VARIANT: i32 = 34;
    /// 35 to 38: sniffer, armadillo, copper golem and weathering copper states.
    pub const FIRST_STATE: i32 = 35;
    pub const LAST_STATE: i32 = 38;
    pub const VECTOR3: i32 = 39;
    pub const QUATERNION: i32 = 40;
    pub const RESOLVABLE_PROFILE: i32 = 41;
    pub const HUMANOID_ARM: i32 = 42;
    /// New in 26.3 (cushions); older versions have none.
    pub const DYE_COLOR: i32 = 43;
}

fn entity_name(kind: i32) -> &'static str {
    paper_data::registries::entries("minecraft:entity_type").and_then(|e| e.get(kind as usize).copied()).unwrap_or("")
}

/// The mobs that grow up (`AgeableMob`), which 26.1 gave an age lock at 17.
const AGEABLE: &[&str] = &[
    "villager", "wandering_trader", "axolotl", "chicken", "panda", "pig", "polar_bear", "rabbit", "sheep", "bee", "turtle", "fox", "frog", "goat", "hoglin", "strider", "sniffer",
    "armadillo", "happy_ghast", "ocelot", "cow", "mooshroom", "cat", "wolf", "parrot", "nautilus", "zombie_nautilus", "horse", "skeleton_horse", "zombie_horse", "camel", "camel_husk",
    "donkey", "mule", "llama", "trader_llama", "dolphin", "squid", "glow_squid",
];

/// Removes the value at `removed` (if any), moving those after it down by one.
fn remove_index(index: u8, removed: Option<u8>) -> Option<u8> {
    match removed {
        Some(r) if index == r => None,
        Some(r) if index > r => Some(index - 1),
        _ => Some(index),
    }
}

/// How a value changes on its way to an older version, besides its serializer's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Keep,
    /// The main arm: its own serializer from 1.21.11, a byte before.
    ArmToByte,
    /// Wolves' and bees' anger: the game time it ends from 1.21.11, the ticks left before.
    AngerToTicks,
    /// A shoulder parrot: its variant from 1.21.9, the parrot as NBT before.
    ShoulderToNbt,
}

impl Modern {
    /// Which entity data of an entity of 26.3 type `kind` the old version has, and at which
    /// index (`None`: drop it). Each version's changes apply in turn, newest first, as the
    /// indexes they name are those of the version before.
    fn data_index(&self, kind: i32, index: u8) -> Option<u8> {
        let name = entity_name(kind);
        let short = name.strip_prefix("minecraft:").unwrap_or(name);
        let p = self.protocol();
        // Entities the old version does not have are shown as another type: keep only what
        // every entity has.
        let replaced = match short {
            "cushion" => true,
            "sulfur_cube" => p < V26_2,
            _ => false,
        };
        if replaced && index > 7 {
            return None;
        }
        let mut index = index;
        if p < V26_2 && matches!(short, "slime" | "magma_cube" | "sulfur_cube") {
            // 26.2 gave cube mobs a baby flag (16) and an age lock (17).
            index = match index {
                16 | 17 => return None,
                i if i > 17 => i - 2,
                i => i,
            };
        }
        if p < V26_1 {
            // 26.1: villagers' "data finalized" flags, the animals' sound variants, and age
            // locks (the type's own first: they come after the age lock).
            let own = match short {
                "zombie_villager" => Some(21),
                "villager" => Some(20),
                "cat" => Some(24),
                "chicken" | "cow" => Some(19),
                "pig" => Some(20),
                "tadpole" => Some(17),
                _ => None,
            };
            index = remove_index(index, own)?;
            if AGEABLE.contains(&short) {
                index = remove_index(index, Some(17))?;
            }
        }
        // 1.21.11's nautiluses are squids before: no taming, dashing or variant.
        if p < V1_21_11 && matches!(short, "nautilus" | "zombie_nautilus") && index >= 17 {
            return None;
        }
        if p < V1_21_9 {
            // 1.21.9's copper golems are frogs before; players' arm, skin, absorption and
            // score moved.
            if short == "copper_golem" && matches!(index, 16 | 17) {
                return None;
            }
            if short == "player" {
                index = match index {
                    15 => 18,
                    16 => 17,
                    17 => 15,
                    18 => 16,
                    i => i,
                };
            }
        }
        if p < V1_21_6 {
            // 1.21.6 gave hanging entities their direction (8); happy ghasts are ghasts before.
            if matches!(short, "item_frame" | "glow_item_frame" | "painting" | "leash_knot") {
                index = remove_index(index, Some(8))?;
            }
            if short == "happy_ghast" && (16..=18).contains(&index) {
                return None;
            }
        }
        Some(index)
    }

    /// The old id of a 26.3 entity data serializer (`None`: the old version has none).
    fn serializer(&self, ty: i32) -> Option<i32> {
        // From ViaVersion's `EntityDataTypes` of each version, by name.
        const TO_1_21_11: [i8; 44] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, -1, 22, -1, 23, 24, 25, 26, -1, 27, -1, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, -1];
        const TO_1_21_9: [i8; 44] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, -1, 22, -1, 23, 24, 25, 26, -1, 27, -1, -1, 28, 29, 30, 31, 32, 33, 34, 35, 36, -1, -1];
        const TO_1_21_5: [i8; 44] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 17, 18, 19, 20, 21, 22, -1, 23, -1, 24, 25, 26, 27, -1, 28, -1, -1, 29, 30, 31, 32, -1, -1, 33, 34, -1, -1, -1];
        let p = self.protocol();
        let table = if p >= V26_1 {
            return (ty != serializer::DYE_COLOR).then_some(ty);
        } else if p >= V1_21_11 {
            &TO_1_21_11
        } else if p >= V1_21_9 {
            &TO_1_21_9
        } else {
            &TO_1_21_5
        };
        table.get(ty as usize).map(|&t| t as i32).filter(|t| *t >= 0)
    }

    /// How a value of an entity of 26.3 type `kind` changes besides its serializer.
    fn change(&self, kind: Option<i32>, ty: i32) -> Change {
        let p = self.protocol();
        let short = kind.map(entity_name).map_or("", |n| n.strip_prefix("minecraft:").unwrap_or(n));
        match ty {
            serializer::HUMANOID_ARM if p < V1_21_9 => Change::ArmToByte,
            serializer::LONG if p < V1_21_11 && matches!(short, "wolf" | "bee") => Change::AngerToTicks,
            serializer::OPTIONAL_UNSIGNED_INT if p < V1_21_9 && short == "player" => Change::ShoulderToNbt,
            _ => Change::Keep,
        }
    }
}

/// Login and respawn: the game modes as bytes (26.3: VarInts), and 26.1 has no `online
/// mode` flag.
pub(super) fn spawn(m: &mut Modern, login: bool, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    if login {
        // The player's own entity: its data is a player's too.
        let player = rw.i32()?;
        if let Some(kind) = paper_data::registries::id("minecraft:entity_type", "minecraft:player") {
            m.entity_types.insert(player, kind as i32);
        }
        rw.bool()?;
        rw.copy(|r| r.list(64, |r| r.string(256)))?;
        rw.varint()?;
        rw.varint()?;
        rw.varint()?;
        rw.bool()?;
        rw.bool()?;
        rw.bool()?;
    }
    let dimension_type = rw.r.varint()?;
    rw.w.put_varint(m.registry_id("minecraft:dimension_type", dimension_type).max(0));
    m.enter_dimension(dimension_type);
    rw.string()?;
    rw.i64()?;
    let game_mode = rw.r.varint()?;
    m.game_mode = game_mode;
    rw.w.put_i8(game_mode as i8);
    let previous = rw.r.varint()?;
    rw.w.put_i8(if previous == 0 { -1 } else { (previous - 1) as i8 });
    rw.bool()?;
    rw.bool()?;
    rw.copy(|r| r.option(|r| Ok((r.string(256)?, r.position()?))))?;
    rw.varint()?;
    rw.varint()?;
    if login {
        let online_mode = rw.r.bool()?;
        if m.protocol() >= V26_2 {
            rw.w.put_bool(online_mode);
        }
    }
    Ok(rw.finish())
}

pub(super) fn add_entity(m: &mut Modern, old: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
    let mut rw = Rw::new(body, old);
    let id = rw.varint()?;
    rw.uuid()?;
    let kind = rw.r.varint()?;
    let old_kind = m.registry_id("minecraft:entity_type", kind);
    if old_kind < 0 {
        return Ok(());
    }
    // Mannequins are players before 1.21.9, which then need a tab list entry of their own.
    if m.protocol() < V1_21_9 && entity_name(kind) == "minecraft:mannequin" {
        return Ok(());
    }
    m.entity_types.insert(id, kind);
    rw.w.put_varint(old_kind);
    rw.f64()?;
    rw.f64()?;
    rw.f64()?;
    // Before 1.21.9 the velocity comes last, as shorts.
    let velocity = if m.protocol() < V1_21_9 { Some(rw.r.lp_vec3()?) } else {
        rw.copy(|r| r.lp_vec3())?;
        None
    };
    rw.u8()?;
    rw.u8()?;
    rw.u8()?;
    let data = rw.r.varint()?;
    let data = match entity_name(kind) {
        "minecraft:falling_block" => m.registry_id_blockstate(data),
        "minecraft:cushion" => m.registry_id_blockstate(paper_data::blocks::default_state("minecraft:white_carpet").unwrap_or(0) as i32),
        _ => data,
    };
    rw.w.put_varint(data);
    if let Some((vx, vy, vz)) = velocity {
        let shorts = [legacy_velocity(vx), legacy_velocity(vy), legacy_velocity(vz)];
        for v in shorts {
            rw.w.put_i16(v);
        }
        // 1.21.5 sets only living things moving from their spawn: others get it apart.
        if m.protocol() < V1_21_6 && shorts != [0, 0, 0] && !is_living(kind) {
            out.to_client.push(rw.finish());
            let mut motion = Vec::new();
            motion.put_varint(m.old_id(play::clientbound::SET_ENTITY_MOTION));
            motion.put_varint(id);
            for v in shorts {
                motion.put_i16(v);
            }
            out.to_client.push(motion);
            return Ok(());
        }
    }
    out.to_client.push(rw.finish());
    Ok(())
}

/// A velocity (blocks per tick) as the shorts of before 1.21.9 (8000ths, at most 3.9).
fn legacy_velocity(v: f64) -> i16 {
    (v.clamp(-3.9, 3.9) * 8000.0) as i16
}

/// Whether entities of a 26.3 type are living (mobs, players, armor stands).
fn is_living(kind: i32) -> bool {
    let name = entity_name(kind);
    matches!(name, "minecraft:player" | "minecraft:armor_stand" | "minecraft:mannequin") || paper_data::entity_types::by_name(name).is_some_and(|t| t.mob)
}

/// 26.3: an entity's velocity as a low-precision vector; before 1.21.9: three shorts.
pub(super) fn entity_motion(old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    let (x, y, z) = rw.r.lp_vec3()?;
    for v in [x, y, z] {
        rw.w.put_i16(legacy_velocity(v));
    }
    Ok(rw.finish())
}

impl Modern {
    pub(super) fn registry_id_blockstate(&self, state: i32) -> i32 {
        self.data.mapping("blockstates").old(state).max(0)
    }
}

pub(super) fn remove_entities(m: &mut Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(body);
    for id in r.list(1 << 16, |r| r.varint())? {
        m.entity_types.remove(&id);
    }
    Ok(io::with_id(old, body))
}

/// Entity data: serializer ids, indices and the ids inside values in the old version's
/// terms. `None` if nothing is left to send.
pub(super) fn entity_data(m: &mut Modern, old: i32, body: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut rw = Rw::new(body, old);
    let id = rw.varint()?;
    let kind = m.entity_types.get(&id).copied();
    let mut entries = 0;
    loop {
        let index = rw.r.u8()?;
        if index == 0xFF {
            break;
        }
        let ty = rw.r.varint()?;
        let keep = match kind {
            Some(kind) => m.data_index(kind, index),
            // One the client was not told about.
            None => Some(index),
        };
        let change = m.change(kind, ty);
        // What the value becomes, by its 26.3 serializer.
        let as_ty = match change {
            Change::Keep => ty,
            Change::ArmToByte => serializer::BYTE,
            Change::AngerToTicks => serializer::INT,
            Change::ShoulderToNbt => serializer::OPTIONAL_UNSIGNED_INT,
        };
        let old_ty = match change {
            // Compound tags are 16 in the versions that have shoulder parrots as NBT.
            Change::ShoulderToNbt => Some(16),
            _ => m.serializer(as_ty),
        };
        match (keep, old_ty) {
            (Some(index), Some(old_ty)) => {
                rw.w.put_u8(index);
                rw.w.put_varint(old_ty);
                match change {
                    Change::Keep => value(m, &mut rw, ty)?,
                    Change::ArmToByte => rw.w.put_u8(rw.r.varint()? as u8),
                    Change::AngerToTicks => {
                        let end = rw.r.varlong()?;
                        rw.w.put_varint((end - m.game_time).clamp(0, i32::MAX as i64) as i32);
                    }
                    Change::ShoulderToNbt => {
                        let mut parrot = paper_nbt::Compound::default();
                        if rw.r.bool()? {
                            parrot.insert("id", paper_nbt::Tag::String("minecraft:parrot".into()));
                            parrot.insert("Variant", paper_nbt::Tag::Int(rw.r.varint()?));
                        }
                        paper_nbt::write_network_compound(&mut rw.w, &parrot);
                    }
                }
                entries += 1;
            }
            _ => {
                // Read past the value without writing it.
                let mark = rw.w.len();
                value(m, &mut rw, ty)?;
                rw.w.truncate(mark);
            }
        }
    }
    rw.w.put_u8(0xFF);
    Ok((entries > 0).then_some(rw.w))
}

/// One entity data value, copied or rewritten.
fn value(m: &Modern, rw: &mut Rw<'_>, ty: i32) -> Result<()> {
    use serializer as s;
    match ty {
        s::BYTE => {
            rw.u8()?;
        }
        s::INT | s::DIRECTION | s::OPTIONAL_UNSIGNED_INT | s::POSE | s::HUMANOID_ARM | s::DYE_COLOR => {
            rw.varint()?;
        }
        s::FIRST_STATE..=s::LAST_STATE => {
            rw.varint()?;
        }
        s::LONG => {
            rw.varlong()?;
        }
        s::FLOAT => {
            rw.f32()?;
        }
        s::STRING => {
            rw.string()?;
        }
        s::COMPONENT => rw.nbt()?,
        s::OPTIONAL_COMPONENT => {
            if rw.bool()? {
                rw.nbt()?;
            }
        }
        s::ITEM_STACK => items::slot(m, rw)?,
        s::BOOLEAN => {
            rw.bool()?;
        }
        s::ROTATIONS | s::VECTOR3 => rw.bytes(12)?,
        s::QUATERNION => rw.bytes(16)?,
        s::BLOCK_POS => {
            rw.i64()?;
        }
        s::OPTIONAL_BLOCK_POS => {
            if rw.bool()? {
                rw.i64()?;
            }
        }
        s::OPTIONAL_LIVING_ENTITY_REFERENCE => {
            if rw.bool()? {
                rw.uuid()?;
            }
        }
        s::BLOCK_STATE => {
            rw.map_varint(|state| m.registry_id_blockstate(state))?;
        }
        s::OPTIONAL_BLOCK_STATE => {
            rw.map_varint(|state| if state == 0 { 0 } else { m.registry_id_blockstate(state) })?;
        }
        s::PARTICLE => misc::particle(m, rw)?,
        s::PARTICLES => {
            let count = rw.varint()?;
            for _ in 0..count {
                misc::particle(m, rw)?;
            }
        }
        s::VILLAGER_DATA => {
            rw.varint()?;
            rw.varint()?;
            rw.varint()?;
        }
        s::FIRST_VARIANT..=s::LAST_VARIANT => {
            rw.varint()?;
        }
        s::OPTIONAL_GLOBAL_POS => {
            if rw.bool()? {
                rw.string()?;
                rw.i64()?;
            }
        }
        s::PAINTING_VARIANT => misc::painting_variant(m, rw)?,
        s::RESOLVABLE_PROFILE => items::resolvable_profile(rw)?,
        other => bail!("entity data of serializer {other}"),
    }
    Ok(())
}

/// 26.3 moves an entity by a properties VarInt (on ground, steps) and one or more deltas;
/// before: one delta, then rotation, then on ground.
pub(super) fn move_entity(old: i32, body: &[u8], rotates: bool) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    let properties = rw.r.varint()?;
    let on_ground = properties & 1 != 0;
    let steps = (properties as u32) >> 1;
    let (mut dx, mut dy, mut dz) = (0i16, 0i16, 0i16);
    if steps == 0 {
        (dx, dy, dz) = (rw.r.i16()?, rw.r.i16()?, rw.r.i16()?);
    } else {
        for _ in 0..steps.min(1024) {
            rw.r.varint()?;
            dx = dx.wrapping_add(rw.r.i16()?);
            dy = dy.wrapping_add(rw.r.i16()?);
            dz = dz.wrapping_add(rw.r.i16()?);
        }
    }
    rw.w.put_i16(dx);
    rw.w.put_i16(dy);
    rw.w.put_i16(dz);
    if rotates {
        rw.u8()?;
        rw.u8()?;
    }
    rw.w.put_bool(on_ground);
    Ok(rw.w)
}

pub(super) fn move_entity_rot(old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    let on_ground = rw.r.bool()?;
    rw.u8()?;
    rw.u8()?;
    rw.w.put_bool(on_ground);
    Ok(rw.w)
}

/// 26.3: a path (one position, or steps); before: a position and a velocity.
pub(super) fn position_sync(old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    let (pos, velocity) = match rw.r.varint()? {
        0 => ((rw.r.f64()?, rw.r.f64()?, rw.r.f64()?), (0.0, 0.0, 0.0)),
        1 => {
            let steps = rw.r.list(1024, |r| Ok((r.f64()?, r.f64()?, r.f64()?, r.varint()?)))?;
            let last = *steps.last().ok_or_else(|| anyhow::anyhow!("an empty position path"))?;
            let velocity = match steps.len() {
                n if n > 1 && last.3 > 0 => {
                    let before = steps[n - 2];
                    ((last.0 - before.0) / last.3 as f64, (last.1 - before.1) / last.3 as f64, (last.2 - before.2) / last.3 as f64)
                }
                _ => (0.0, 0.0, 0.0),
            };
            ((last.0, last.1, last.2), velocity)
        }
        other => bail!("position path type {other}"),
    };
    for v in [pos.0, pos.1, pos.2, velocity.0, velocity.1, velocity.2] {
        rw.w.put_f64(v);
    }
    Ok(rw.finish())
}

/// 26.3 numbers the animations anew (the swings went to their own packet).
pub(super) fn animate(old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    let action = rw.r.u8()?;
    rw.w.put_u8(match action {
        0 => 2,
        1 => 4,
        2 => 5,
        other => other,
    });
    Ok(rw.finish())
}

/// 26.3's `swing_animation`: before, `animate` with 0 (main hand) or 3 (off hand).
pub(super) fn swing(m: &mut Modern, body: &[u8], out: &mut Outcome) -> Result<()> {
    let mut r = Reader::new(body);
    let entity = r.varint()?;
    let hand = r.varint()?;
    let animation = r.varint()?;
    if animation == 0 {
        return Ok(());
    }
    let mut w = Vec::new();
    w.put_varint(m.old_id_named("minecraft:animate")?);
    w.put_varint(entity);
    w.put_u8(if hand == 0 { 0 } else { 3 });
    out.to_client.push(w);
    Ok(())
}

/// The old `swing` (a hand) is 26.3's `punch` for the main hand; an off-hand swing has no
/// counterpart.
pub(super) fn swing_serverbound(_m: &mut Modern, body: &[u8], out: &mut Outcome) -> Result<()> {
    let hand = Reader::new(body).varint()?;
    if hand == 0 {
        let mut w = Vec::new();
        w.put_varint(play::serverbound::PUNCH);
        out.to_server.push(w);
    }
    Ok(())
}

/// Before 26.1 one `interact` packet attacks (1), interacts (0) or interacts at a point (2);
/// 26.3 has `attack` (or `spectator_action` for a spectator) and `interact` with the point.
/// Clients send the interaction at a point first and the plain one after it: the latter is
/// left out.
pub(super) fn interact_serverbound(m: &mut Modern, body: &[u8], out: &mut Outcome) -> Result<()> {
    use paper_protocol::packets::play as packets;
    use paper_protocol::types::LpVec3;
    let mut r = Reader::new(body);
    let entity = r.varint()?;
    let payload = match r.varint()? {
        0 => return Ok(()),
        1 if m.game_mode == 3 => {
            let mut w = Vec::new();
            w.put_varint(play::serverbound::SPECTATOR_ACTION);
            w.put_varint(entity + 1);
            w
        }
        1 => paper_protocol::packets::encode_packet(&packets::Attack { entity_id: paper_protocol::codec::VarInt(entity) }),
        2 => {
            let target = LpVec3(r.f32()? as f64, r.f32()? as f64, r.f32()? as f64);
            let hand = r.varint()?;
            let sneaking = r.bool()?;
            paper_protocol::packets::encode_packet(&packets::Interact { entity_id: paper_protocol::codec::VarInt(entity), hand: paper_protocol::codec::VarInt(hand), target, sneaking })
        }
        other => bail!("interaction type {other}"),
    };
    out.to_server.push(payload);
    Ok(())
}

/// The old `spectate_entity` is 26.3's `spectator_action`; 26.1 sends a plain entity id,
/// 26.2 an optional one.
pub(super) fn spectate(m: &mut Modern, body: &[u8], out: &mut Outcome) -> Result<()> {
    let mut w = Vec::new();
    w.put_varint(play::serverbound::SPECTATOR_ACTION);
    if m.protocol() < V26_2 {
        let id = Reader::new(body).varint()?;
        w.put_varint(id + 1);
    } else {
        w.extend_from_slice(body);
    }
    out.to_server.push(w);
    Ok(())
}

pub(super) fn attributes(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(body);
    let entity = r.varint()?;
    let attributes = r.list(64, |r| Ok((r.varint()?, r.f64()?, r.list(256, |r| Ok((r.string(256)?, r.f64()?, r.i8()?)))?)))?;
    let mut w = Vec::new();
    w.put_varint(old);
    w.put_varint(entity);
    let kept: Vec<_> = attributes.into_iter().filter_map(|(id, base, modifiers)| Some((m.registry_id("minecraft:attribute", id).max(-1), base, modifiers)).filter(|a| a.0 >= 0)).collect();
    w.put_varint(kept.len() as i32);
    for (id, base, modifiers) in kept {
        w.put_varint(id);
        w.put_f64(base);
        w.put_varint(modifiers.len() as i32);
        for (name, amount, operation) in modifiers {
            w.put_string(&name);
            w.put_f64(amount);
            w.put_i8(operation);
        }
    }
    Ok(w)
}

pub(super) fn damage_event(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    rw.map_varint(|id| m.registry_id("minecraft:damage_type", id).max(0))?;
    Ok(rw.finish())
}
