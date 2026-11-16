//! Particles as 26.3 sends them: a `minecraft:particle_type` id, then options that depend on
//! the type (ViaVersion's 26.3 particle filler).

use crate::io::Result;
use anyhow::bail;
use paper_protocol::codec::Reader;

/// What follows the type of a particle, by particle name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    None,
    /// A block state id.
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
    /// An item template.
    Item,
    /// A position source and the ticks to arrive.
    Vibration,
    /// A target position (3 doubles), a colour and a duration.
    Trail,
}

pub fn kind(name: &str) -> Kind {
    match name {
        "minecraft:block" | "minecraft:block_marker" | "minecraft:falling_dust" | "minecraft:dust_pillar" | "minecraft:block_crumble" => Kind::BlockState,
        "minecraft:dust" => Kind::Dust,
        "minecraft:dust_color_transition" => Kind::DustTransition,
        "minecraft:entity_effect" | "minecraft:tinted_leaves" | "minecraft:flash" => Kind::Color,
        "minecraft:effect" | "minecraft:instant_effect" => Kind::ColorPower,
        "minecraft:sculk_charge" | "minecraft:dragon_breath" => Kind::Float,
        "minecraft:geyser" | "minecraft:geyser_plume" => Kind::Int,
        "minecraft:geyser_base" | "minecraft:geyser_poof" => Kind::IntFloat,
        "minecraft:shriek" => Kind::VarInt,
        "minecraft:item" => Kind::Item,
        "minecraft:vibration" => Kind::Vibration,
        "minecraft:trail" => Kind::Trail,
        _ => Kind::None,
    }
}

/// The 26.3 name of a particle type.
pub fn name(id: i32) -> &'static str {
    paper_data::registries::entries("minecraft:particle_type").and_then(|e| e.get(id as usize).copied()).unwrap_or("")
}

/// What a particle's options hold that an older client may need.
#[derive(Clone, Debug, PartialEq)]
pub enum Options {
    BlockState(u32),
    /// The item of an item particle (its count and components are read past).
    Item(i32),
    Other,
}

/// Reads a particle: its type id and options.
pub fn read(r: &mut Reader<'_>) -> Result<(i32, Options)> {
    let id = r.varint()?;
    let options = match kind(name(id)) {
        Kind::None => Options::Other,
        Kind::BlockState => Options::BlockState(r.varint()? as u32),
        Kind::Dust | Kind::ColorPower | Kind::IntFloat => {
            r.take(8)?;
            Options::Other
        }
        Kind::DustTransition => {
            r.take(12)?;
            Options::Other
        }
        Kind::Color | Kind::Float | Kind::Int => {
            r.take(4)?;
            Options::Other
        }
        Kind::VarInt => {
            r.varint()?;
            Options::Other
        }
        Kind::Item => {
            let mut rw = crate::io::Rw { r: r.clone(), w: Vec::new() };
            let item = rw.r.clone().varint()?;
            crate::components::template(&crate::components::Same, &mut rw)?;
            *r = rw.r;
            Options::Item(item)
        }
        Kind::Vibration => {
            match r.varint()? {
                0 => {
                    r.i64()?;
                }
                1 => {
                    r.varint()?;
                    r.f32()?;
                }
                other => bail!("position source type {other}"),
            }
            r.varint()?;
            Options::Other
        }
        Kind::Trail => {
            r.take(28)?;
            r.varint()?;
            Options::Other
        }
    };
    Ok((id, options))
}
