//! Sounds, particles, explosions, the command tree, statistics and teams.

use super::{Modern, V1_21_6, V1_21_9, V26_1, V26_2};
use crate::io::{Result, Rw};
use crate::Outcome;
use anyhow::bail;
use paper_protocol::codec::{Decode, Reader, WriteExt};
use paper_protocol::packets::play::{Commands, NodeKind};

/// A sound event holder: a registry id (plus one), or 0 and an inline sound.
fn sound_holder(m: &Modern, rw: &mut Rw<'_>) -> Result<()> {
    let id = rw.r.varint()?;
    if id == 0 {
        rw.w.put_varint(0);
        rw.copy(|r| r.string(32767))?;
        rw.copy(|r| r.option(|r| r.f32()))?;
        return Ok(());
    }
    let old = m.registry_id("minecraft:sound_event", id - 1);
    if old >= 0 {
        rw.w.put_varint(old + 1);
    } else {
        // A sound the old version lacks: by name, which it ignores if it cannot play it.
        let name = paper_data::registries::entries("minecraft:sound_event").and_then(|e| e.get((id - 1) as usize).copied()).unwrap_or("minecraft:intentionally_empty");
        rw.w.put_varint(0);
        rw.w.put_string(name);
        rw.w.put_bool(false);
    }
    Ok(())
}

pub(super) fn sound(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    sound_holder(m, &mut rw)?;
    // The user interface's sounds (10) were the master volume's (0) before 1.21.6.
    rw.map_varint(|source| if source == 10 && m.protocol() < V1_21_6 { 0 } else { source })?;
    Ok(rw.finish())
}

/// Stopping sounds of a source (flag 1): the user interface's are the master volume's before
/// 1.21.6.
pub(super) fn stop_sound(old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    let flags = rw.u8()?;
    if flags & 1 != 0 {
        rw.map_varint(|source| if source == 10 { 0 } else { source })?;
    }
    Ok(rw.finish())
}

/// The time: 26.1 sends clocks (each with a rate); before, the time of day and whether it
/// runs. The world's age counts the client's ticks for anger (before 1.21.11).
pub(super) fn set_time(m: &mut Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(body);
    let age = r.i64()?;
    m.game_time = age;
    if m.protocol() >= V26_1 {
        return Ok(crate::io::with_id(old, body));
    }
    let clocks = r.list(64, |r| Ok((r.varint()?, r.varlong()?, r.f32()?, r.f32()?)))?;
    if let Some((_, time, _, rate)) = clocks.first() {
        m.day_time = (*time, *rate != 0.0);
    }
    let mut w = Vec::new();
    w.put_varint(old);
    w.put_i64(age);
    w.put_i64(m.day_time.0);
    w.put_bool(m.day_time.1);
    Ok(w)
}

/// The world border: before 1.21.11 its changes take milliseconds rather than ticks, and
/// before 1.21.9 its centre is scaled by the dimension's coordinate scale.
pub(super) fn border(m: &Modern, id: i32, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    use paper_data::packets::play::clientbound as cb;
    let mut rw = Rw::new(body, old);
    if id == cb::INITIALIZE_BORDER || id == cb::SET_BORDER_CENTER {
        let (x, z) = (rw.r.f64()?, rw.r.f64()?);
        let scale = if m.protocol() < V1_21_9 { m.coordinate_scale } else { 1.0 };
        rw.w.put_f64(x * scale);
        rw.w.put_f64(z * scale);
    }
    if id == cb::INITIALIZE_BORDER || id == cb::SET_BORDER_LERP_SIZE {
        rw.f64()?;
        rw.f64()?;
        let ticks = rw.r.varlong()?;
        rw.w.put_varlong(ticks.saturating_mul(50));
    }
    Ok(rw.finish())
}

/// A teleport: where the player then looks is kept, for rotations relative to it (before
/// 1.21.9 they are absolute).
pub(super) fn teleport(m: &mut Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(body);
    r.varint()?;
    r.take(6 * 8)?;
    let (yaw, pitch) = (r.f32()?, r.f32()?);
    let flags = r.i32()?;
    let relative = |bit: i32, base: f32, v: f32| if flags & bit != 0 { base + v } else { v };
    m.rotation = (relative(0x08, m.rotation.0, yaw), relative(0x10, m.rotation.1, pitch));
    Ok(crate::io::with_id(old, body))
}

/// A rotation: 1.21.9 may make either angle relative; before, both are absolute.
pub(super) fn rotation(m: &mut Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(body);
    let (yaw, yaw_relative, pitch, pitch_relative) = (r.f32()?, r.bool()?, r.f32()?, r.bool()?);
    let yaw = if yaw_relative { m.rotation.0 + yaw } else { yaw };
    let pitch = if pitch_relative { m.rotation.1 + pitch } else { pitch };
    m.rotation = (yaw, pitch);
    if m.protocol() >= V1_21_9 {
        return Ok(crate::io::with_id(old, body));
    }
    let mut w = Vec::new();
    w.put_varint(old);
    w.put_f32(yaw);
    w.put_f32(pitch);
    Ok(w)
}

/// What follows the type of a particle, by particle name.
#[derive(Clone, Copy)]
enum Options {
    None,
    BlockState,
    /// An RGB colour (int) and a scale (float).
    Dust,
    /// Two colours and a scale.
    DustTransition,
    /// An ARGB colour.
    Color,
    /// A colour and a power (float).
    ColorPower,
    Float,
    /// A 4-byte int (geysers: how many water blocks).
    Int,
    /// An int and a float.
    IntFloat,
    VarInt,
    Item,
    Vibration,
    /// A target position (3 doubles), a colour and a duration.
    Trail,
}

fn options(name: &str) -> Options {
    match name {
        "minecraft:block" | "minecraft:block_marker" | "minecraft:falling_dust" | "minecraft:dust_pillar" | "minecraft:block_crumble" => Options::BlockState,
        "minecraft:dust" => Options::Dust,
        "minecraft:dust_color_transition" => Options::DustTransition,
        "minecraft:entity_effect" | "minecraft:tinted_leaves" | "minecraft:flash" => Options::Color,
        "minecraft:effect" | "minecraft:instant_effect" => Options::ColorPower,
        "minecraft:sculk_charge" | "minecraft:dragon_breath" => Options::Float,
        "minecraft:geyser" | "minecraft:geyser_plume" => Options::Int,
        "minecraft:geyser_base" | "minecraft:geyser_poof" => Options::IntFloat,
        "minecraft:shriek" => Options::VarInt,
        "minecraft:item" => Options::Item,
        "minecraft:vibration" => Options::Vibration,
        "minecraft:trail" => Options::Trail,
        _ => Options::None,
    }
}

/// A particle (its type, then its options) in the old version's terms.
pub(super) fn particle(m: &Modern, rw: &mut Rw<'_>) -> Result<()> {
    let kind = rw.r.varint()?;
    let name = paper_data::registries::entries("minecraft:particle_type").and_then(|e| e.get(kind as usize).copied()).unwrap_or("");
    let old = m.registry_id("minecraft:particle_type", kind);
    // The poplar leaves of 26.3 are tinted leaves of their colour before.
    let poplar = match name {
        "minecraft:orange_poplar_leaves" => Some(0xFFDB_7A2Au32 as i32),
        "minecraft:yellow_poplar_leaves" => Some(0xFFE5_C13Au32 as i32),
        _ => None,
    };
    rw.w.put_varint(old.max(0));
    // 1.21.9 gave dragon breath a power, flashes a colour and effects a colour and power.
    if m.protocol() < V1_21_9 && matches!(name, "minecraft:dragon_breath" | "minecraft:flash" | "minecraft:effect" | "minecraft:instant_effect") {
        let mark = rw.w.len();
        match options(name) {
            Options::Float | Options::Color => rw.bytes(4)?,
            _ => rw.bytes(8)?,
        }
        rw.w.truncate(mark);
        return Ok(());
    }
    match options(name) {
        Options::None => {}
        Options::BlockState => {
            rw.map_varint(|state| m.registry_id_blockstate(state))?;
        }
        Options::Dust => rw.bytes(8)?,
        Options::DustTransition => rw.bytes(12)?,
        Options::Color | Options::Float | Options::Int => rw.bytes(4)?,
        Options::ColorPower | Options::IntFloat => rw.bytes(8)?,
        Options::VarInt => {
            rw.varint()?;
        }
        Options::Item => super::items::template(m, rw)?,
        Options::Vibration => {
            match rw.varint()? {
                0 => {
                    rw.i64()?;
                }
                1 => {
                    rw.varint()?;
                    rw.f32()?;
                }
                other => bail!("position source type {other}"),
            }
            rw.varint()?;
        }
        Options::Trail => {
            rw.bytes(24)?;
            rw.i32()?;
            rw.varint()?;
        }
    }
    if let Some(color) = poplar {
        rw.w.put_i32(color);
    }
    Ok(())
}

/// 26.3: the particle first, a speed per axis, a VarInt count and a randomization; before:
/// the particle last, one speed and an int count.
pub(super) fn particles(m: &Modern, old: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
    let mut rw = Rw::new(body, old);
    particle(m, &mut rw)?;
    let particle_bytes = rw.w.split_off(crate::io::varint_len(old));
    let r = &mut rw.r;
    let (override_limiter, always_show) = (r.bool()?, r.bool()?);
    let (x, y, z) = (r.f64()?, r.f64()?, r.f64()?);
    let (ox, oy, oz) = (r.f32()?, r.f32()?, r.f32()?);
    let (sx, sy, sz) = (r.f32()?, r.f32()?, r.f32()?);
    let count = r.varint()?;
    let randomization = r.varint()?;
    let write = |x: f64, y: f64, z: f64, ox: f32, oy: f32, oz: f32, speed: f32, count: i32| {
        let mut w = Vec::new();
        w.put_varint(old);
        w.put_bool(override_limiter);
        w.put_bool(always_show);
        w.put_f64(x);
        w.put_f64(y);
        w.put_f64(z);
        w.put_f32(ox);
        w.put_f32(oy);
        w.put_f32(oz);
        w.put_f32(speed);
        w.put_i32(count);
        w.extend_from_slice(&particle_bytes);
        w
    };
    let same_speed = sx == sy && sy == sz;
    if count <= 0 {
        // The offsets are a direction (or a colour) scaled by the speed.
        let payload = if same_speed { write(x, y, z, ox, oy, oz, sx, count) } else { write(x, y, z, sx * ox, sy * oy, sz * oz, 1.0, count) };
        out.to_client.push(payload);
        return Ok(());
    }
    let speed = sx.max(sy).max(sz);
    let payload = if randomization == 0 {
        write(x, y, z, ox, oy, oz, speed, count)
    } else {
        // Spread over the box from the position instead of around it.
        write(x + ox as f64 / 2.0, y + oy as f64 / 2.0, z + oz as f64 / 2.0, ox / 2.0, oy / 2.0, oz / 2.0, speed, count)
    };
    out.to_client.push(payload);
    Ok(())
}

/// 26.3 says whether to play the sound with a flag at the end; before, the sound always
/// plays, so a silent explosion gets an empty sound.
pub(super) fn explode(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.bytes(24)?;
    // 1.21.9 added the radius, the count of blocks hit and the block particles.
    let v1_21_9 = m.protocol() >= V1_21_9;
    if v1_21_9 {
        rw.f32()?;
        rw.i32()?;
    } else {
        rw.r.take(8)?;
    }
    if rw.bool()? {
        rw.bytes(24)?;
    }
    particle(m, &mut rw)?;
    let mark = rw.w.len();
    sound_holder(m, &mut rw)?;
    let before_particles = rw.w.len();
    let count = rw.varint()?;
    for _ in 0..count {
        particle(m, &mut rw)?;
        rw.f32()?;
        rw.f32()?;
        rw.varint()?;
    }
    if !v1_21_9 {
        rw.w.truncate(before_particles);
    }
    let play_sound = rw.r.bool()?;
    if !play_sound {
        let mut rest = rw.w.split_off(mark);
        // Replace the sound (the start of `rest`) by an empty one.
        let mut r = Reader::new(&rest);
        let id = r.varint()?;
        if id == 0 {
            r.string(32767)?;
            r.option(|r| r.f32())?;
        }
        let tail = r.rest().to_vec();
        rest.clear();
        rw.w.put_varint(0);
        rw.w.put_string("minecraft:intentionally_empty");
        rw.w.put_bool(false);
        rw.w.extend_from_slice(&tail);
    }
    Ok(rw.w)
}

/// The command tree with the old argument type ids; argument types the old version lacks
/// are quotable strings.
pub(super) fn commands(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut tree = Commands::decode(&mut Reader::new(body))?;
    let string = paper_data::registries::id("minecraft:command_argument_type", "brigadier:string").unwrap_or(5) as i32;
    for node in &mut tree.nodes {
        if let NodeKind::Argument { parser, .. } = &mut node.kind {
            let id = m.registry_id("minecraft:command_argument_type", parser.id);
            if id >= 0 {
                parser.id = id;
            } else {
                parser.id = m.registry_id("minecraft:command_argument_type", string);
                parser.properties = vec![1];
            }
        }
    }
    let mut w = Vec::new();
    w.put_varint(old);
    paper_protocol::codec::Encode::encode(&tree, &mut w);
    Ok(w)
}

/// Statistics with the old ids of their blocks, items, entities and custom statistics.
pub(super) fn award_stats(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(body);
    let stats = r.list(1 << 16, |r| Ok((r.varint()?, r.varint()?, r.varint()?)))?;
    let registry = |kind: i32| match paper_data::registries::entries("minecraft:stat_type").and_then(|e| e.get(kind as usize).copied()).unwrap_or("") {
        "minecraft:mined" => "minecraft:block",
        "minecraft:killed" | "minecraft:killed_by" => "minecraft:entity_type",
        "minecraft:custom" => "minecraft:custom_stat",
        _ => "minecraft:item",
    };
    let kept: Vec<(i32, i32, i32)> = stats.into_iter().filter_map(|(kind, key, value)| Some((kind, m.registry_id(registry(kind), key), value)).filter(|s| s.1 >= 0)).collect();
    let mut w = Vec::new();
    w.put_varint(old);
    w.put_varint(kept.len() as i32);
    for (kind, key, value) in kept {
        w.put_varint(kind);
        w.put_varint(key);
        w.put_varint(value);
    }
    Ok(w)
}

/// 26.1 orders a team's settings differently and has a colour (21: none) where 26.2 has an
/// optional one.
pub(super) fn player_team(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.string()?;
    let method = rw.i8()?;
    if (method == 0 || method == 2) && m.protocol() < V26_2 {
        rw.nbt()?;
        let prefix = rw.r.nbt()?;
        let suffix = rw.r.nbt()?;
        let nametags = rw.r.varint()?;
        let collision = rw.r.varint()?;
        let color = rw.r.option(|r| r.varint())?;
        let options = rw.r.i8()?;
        rw.w.put_i8(options);
        rw.w.put_varint(nametags);
        rw.w.put_varint(collision);
        rw.w.put_varint(color.unwrap_or(21));
        for tag in [prefix, suffix] {
            match tag {
                Some(tag) => paper_nbt::write_network(&mut rw.w, &tag),
                None => paper_nbt::write_network_end(&mut rw.w),
            }
        }
    }
    Ok(rw.finish())
}

/// A painting variant holder: a registry id (plus one), or 0 and an inline variant.
pub(super) fn painting_variant(m: &Modern, rw: &mut Rw<'_>) -> Result<()> {
    let id = rw.r.varint()?;
    if id == 0 {
        rw.w.put_varint(0);
        rw.varint()?;
        rw.varint()?;
        rw.string()?;
        if rw.bool()? {
            rw.nbt()?;
        }
        if rw.bool()? {
            rw.nbt()?;
        }
        return Ok(());
    }
    rw.w.put_varint(m.registry_id("minecraft:painting_variant", id - 1).max(0) + 1);
    Ok(())
}
