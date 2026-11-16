//! Entities: their types, data and movement as the older versions encode them.

use super::{items, misc, Modern, V26_2};
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

impl Modern {
    /// Which entity data of an entity of 26.3 type `kind` the old version has, and at which
    /// index (`None`: drop it).
    fn data_index(&self, kind: i32, index: u8) -> Option<u8> {
        let name = entity_name(kind);
        // Entities the old version does not have are shown as another type: keep only what
        // every entity has.
        let replaced = match name {
            "minecraft:cushion" => true,
            "minecraft:sulfur_cube" => self.protocol() < V26_2,
            _ => false,
        };
        if replaced && index > 7 {
            return None;
        }
        if self.protocol() < V26_2 && matches!(name, "minecraft:slime" | "minecraft:magma_cube" | "minecraft:sulfur_cube") {
            // 26.2 gave cube mobs a baby flag (16) and an age lock (17).
            return match index {
                16 | 17 => None,
                i if i > 17 => Some(i - 2),
                i => Some(i),
            };
        }
        Some(index)
    }
}

/// Login and respawn: the game modes as bytes (26.3: VarInts), and 26.1 has no `online
/// mode` flag.
pub(super) fn spawn(m: &mut Modern, login: bool, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    if login {
        rw.i32()?;
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

pub(super) fn add_entity(m: &mut Modern, old: i32, body: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut rw = Rw::new(body, old);
    let id = rw.varint()?;
    rw.uuid()?;
    let kind = rw.r.varint()?;
    let old_kind = m.registry_id("minecraft:entity_type", kind);
    if old_kind < 0 {
        return Ok(None);
    }
    m.entity_types.insert(id, kind);
    rw.w.put_varint(old_kind);
    rw.f64()?;
    rw.f64()?;
    rw.f64()?;
    rw.copy(|r| r.lp_vec3())?;
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
    Ok(Some(rw.finish()))
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
            // The player's own entity, or one the client was not told about.
            None => Some(index),
        };
        let keep = keep.filter(|_| ty != serializer::DYE_COLOR);
        match keep {
            Some(index) => {
                rw.w.put_u8(index);
                rw.w.put_varint(ty);
                value(m, &mut rw, ty)?;
                entries += 1;
            }
            None => {
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
