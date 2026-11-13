//! Data tables for one Minecraft version: packet ids, registry orders, block states, items,
//! the registries synchronised to clients and their tags.
//!
//! The tables are generated at build time from `data/<version>/*.json`; see `build.rs` and
//! `tools/datagen/README.md` for where the JSON comes from.

use std::collections::HashMap;
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/version.rs"));

/// Packet ids, as `packets::<state>::<direction>::<NAME>`.
pub mod packets {
    include!(concat!(env!("OUT_DIR"), "/packets.rs"));
}

/// Static description of a block and the range of state ids it owns.
#[derive(Debug)]
pub struct BlockInfo {
    /// Namespaced id, e.g. `minecraft:oak_stairs`.
    pub name: &'static str,
    /// First state id of this block.
    pub min_state: u32,
    pub state_count: u32,
    pub default_state: u32,
    /// Properties sorted by name, each with its values in state-id order. The first
    /// property varies slowest across the block's state range.
    pub properties: &'static [(&'static str, &'static [&'static str])],
    /// The item that places this block, if any.
    pub item: Option<&'static str>,
    /// Mining hardness (`-1` unbreakable, NaN unknown).
    pub hardness: f32,
    pub blast_resistance: f32,
}

/// Static description of an item.
#[derive(Debug)]
pub struct ItemInfo {
    pub name: &'static str,
    pub max_stack_size: u32,
    /// The block this item places when used on a block, if any.
    pub block: Option<&'static str>,
}

/// The parts of a dimension type the server itself needs.
#[derive(Debug)]
pub struct DimensionTypeInfo {
    pub name: &'static str,
    pub min_y: i32,
    pub height: i32,
    pub logical_height: i32,
    pub has_skylight: bool,
    pub has_ceiling: bool,
    /// The `minecraft:gameplay/fast_lava` attribute: lava flows faster and further.
    pub fast_lava: bool,
}

fn name_index(names: &'static [&'static str]) -> HashMap<&'static str, u32> {
    names.iter().enumerate().map(|(i, n)| (*n, i as u32)).collect()
}

/// Accepts `stone` as shorthand for `minecraft:stone`.
fn normalize(name: &str) -> std::borrow::Cow<'_, str> {
    if name.contains(':') {
        std::borrow::Cow::Borrowed(name)
    } else {
        std::borrow::Cow::Owned(format!("minecraft:{name}"))
    }
}

/// Built-in registries (`minecraft:item`, `minecraft:entity_type`, ...) by protocol id.
pub mod registries {
    use super::*;

    include!(concat!(env!("OUT_DIR"), "/registries.rs"));

    fn indexes() -> &'static HashMap<&'static str, (&'static [&'static str], HashMap<&'static str, u32>)> {
        static INDEX: OnceLock<HashMap<&'static str, (&'static [&'static str], HashMap<&'static str, u32>)>> = OnceLock::new();
        INDEX.get_or_init(|| REGISTRIES.iter().map(|(registry, names)| (*registry, (*names, name_index(names)))).collect())
    }

    /// Entry names of `registry`, indexed by protocol id.
    pub fn entries(registry: &str) -> Option<&'static [&'static str]> {
        indexes().get(normalize(registry).as_ref()).map(|(names, _)| *names)
    }

    /// Protocol id of `name` in `registry`. Names without a namespace are tried as given
    /// first (pseudo-registries use bare names) and then in the `minecraft` namespace.
    pub fn id(registry: &str, name: &str) -> Option<u32> {
        let (_, index) = indexes().get(normalize(registry).as_ref())?;
        index.get(name).or_else(|| index.get(normalize(name).as_ref())).copied()
    }

    /// Name of the entry with protocol id `id` in `registry`.
    pub fn name(registry: &str, id: u32) -> Option<&'static str> {
        entries(registry)?.get(id as usize).copied()
    }
}

/// Blocks and block states.
pub mod blocks {
    use super::*;

    include!(concat!(env!("OUT_DIR"), "/blocks.rs"));

    /// State flag: air (`air`, `cave_air`, `void_air`).
    pub const FLAG_AIR: u8 = 1;
    /// State flag: has a collision shape.
    pub const FLAG_COLLISION: u8 = 2;
    /// State flag: the collision shape is exactly the full cube.
    pub const FLAG_FULL_CUBE: u8 = 4;
    /// State flag: contains fluid (water, lava, or waterlogged).
    pub const FLAG_FLUID: u8 = 8;
    /// State flag: replaced when another block is placed into it.
    pub const FLAG_REPLACEABLE: u8 = 16;
    /// State flag: has a block entity.
    pub const FLAG_BLOCK_ENTITY: u8 = 32;

    /// The state id of air.
    pub const AIR: u32 = 0;

    fn by_name_index() -> &'static HashMap<&'static str, u32> {
        static INDEX: OnceLock<HashMap<&'static str, u32>> = OnceLock::new();
        INDEX.get_or_init(|| BLOCKS.iter().enumerate().map(|(i, b)| (b.name, i as u32)).collect())
    }

    /// Block protocol id for a name.
    pub fn id(name: &str) -> Option<u32> {
        by_name_index().get(normalize(name).as_ref()).copied()
    }

    pub fn by_name(name: &str) -> Option<&'static BlockInfo> {
        id(name).map(|i| &BLOCKS[i as usize])
    }

    /// Default state id of a block.
    pub fn default_state(name: &str) -> Option<u32> {
        by_name(name).map(|b| b.default_state)
    }

    /// Block protocol id that owns `state`. Out-of-range states map to air.
    pub fn block_id_of_state(state: u32) -> u32 {
        let i = state as usize * 2;
        match STATE_TO_BLOCK_RAW.get(i..i + 2) {
            Some(b) => u16::from_le_bytes([b[0], b[1]]) as u32,
            None => 0,
        }
    }

    pub fn block_of_state(state: u32) -> &'static BlockInfo {
        &BLOCKS[block_id_of_state(state) as usize]
    }

    pub fn is_valid_state(state: u32) -> bool {
        state < STATE_COUNT
    }

    #[inline]
    pub fn flags(state: u32) -> u8 {
        STATE_FLAGS.get(state as usize).copied().unwrap_or(FLAG_AIR | FLAG_REPLACEABLE)
    }
    #[inline]
    pub fn is_air(state: u32) -> bool {
        flags(state) & FLAG_AIR != 0
    }
    #[inline]
    pub fn has_collision(state: u32) -> bool {
        flags(state) & FLAG_COLLISION != 0
    }
    #[inline]
    pub fn has_fluid(state: u32) -> bool {
        flags(state) & FLAG_FLUID != 0
    }
    #[inline]
    pub fn is_replaceable(state: u32) -> bool {
        flags(state) & FLAG_REPLACEABLE != 0
    }
    /// Light emitted by the state (0-15).
    #[inline]
    pub fn luminance(state: u32) -> u8 {
        STATE_LUMINANCE.get(state as usize).copied().unwrap_or(0)
    }
    /// How much light the state absorbs (0 transparent - 15 opaque).
    #[inline]
    pub fn opacity(state: u32) -> u8 {
        STATE_OPACITY.get(state as usize).copied().unwrap_or(0)
    }

    /// The property values of a state, in the block's property order.
    pub fn properties_of(state: u32) -> Vec<(&'static str, &'static str)> {
        let block = block_of_state(state);
        let mut index = (state - block.min_state) as usize;
        let mut out = vec![("", ""); block.properties.len()];
        for (slot, (name, values)) in out.iter_mut().zip(block.properties.iter()).rev() {
            *slot = (*name, values[index % values.len()]);
            index /= values.len();
        }
        out
    }

    /// The value of one property of a state.
    pub fn property(state: u32, name: &str) -> Option<&'static str> {
        properties_of(state).into_iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    /// The state of the same block with `name` set to `value`; `None` if the block has no
    /// such property or value.
    pub fn with_property(state: u32, name: &str, value: &str) -> Option<u32> {
        let block = block_of_state(state);
        let mut stride = 1u32;
        for (prop, values) in block.properties.iter().rev() {
            if *prop == name {
                let target = values.iter().position(|v| *v == value)? as u32;
                let current = (state - block.min_state) / stride % values.len() as u32;
                return Some(state - current * stride + target * stride);
            }
            stride *= values.len() as u32;
        }
        None
    }

    /// `minecraft:oak_stairs[facing=north,half=bottom,...]`, or just the id for blocks
    /// without properties.
    pub fn state_to_string(state: u32) -> String {
        let block = block_of_state(state);
        if block.properties.is_empty() {
            return block.name.to_owned();
        }
        let props: Vec<String> = properties_of(state).iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{}[{}]", block.name, props.join(","))
    }

    /// Parses `id` or `id[prop=value,...]`; unspecified properties keep their default.
    pub fn parse_state(text: &str) -> Option<u32> {
        let text = text.trim();
        let (name, props) = match text.split_once('[') {
            Some((name, rest)) => (name, Some(rest.strip_suffix(']')?)),
            None => (text, None),
        };
        let mut state = by_name(name.trim())?.default_state;
        if let Some(props) = props {
            for pair in props.split(',').filter(|p| !p.trim().is_empty()) {
                let (k, v) = pair.split_once('=')?;
                state = with_property(state, k.trim(), v.trim())?;
            }
        }
        Some(state)
    }
}

/// Items.
pub mod items {
    use super::*;

    include!(concat!(env!("OUT_DIR"), "/items.rs"));

    fn by_name_index() -> &'static HashMap<&'static str, u32> {
        static INDEX: OnceLock<HashMap<&'static str, u32>> = OnceLock::new();
        INDEX.get_or_init(|| ITEMS.iter().enumerate().map(|(i, item)| (item.name, i as u32)).collect())
    }

    pub fn id(name: &str) -> Option<u32> {
        by_name_index().get(normalize(name).as_ref()).copied()
    }

    pub fn get(id: u32) -> Option<&'static ItemInfo> {
        ITEMS.get(id as usize)
    }

    pub fn by_name(name: &str) -> Option<&'static ItemInfo> {
        id(name).and_then(get)
    }

    /// Number of items in the registry.
    pub fn count() -> usize {
        ITEMS.len()
    }
}

/// Registries synchronised to clients, their tags, and related configuration data.
pub mod synced {
    use super::*;

    include!(concat!(env!("OUT_DIR"), "/synced.rs"));

    /// Entries of a synchronised registry in network id order.
    pub fn entries(registry: &str) -> Option<&'static [&'static str]> {
        let registry = normalize(registry);
        SYNCED_REGISTRIES.iter().find(|(r, _)| *r == registry.as_ref()).map(|(_, e)| *e)
    }

    /// Network id of an entry of a synchronised registry.
    pub fn id(registry: &str, entry: &str) -> Option<i32> {
        let entry = normalize(entry);
        entries(registry)?.iter().position(|e| *e == entry.as_ref()).map(|i| i as i32)
    }

    /// Numeric ids in a tag, e.g. `tag("minecraft:block", "minecraft:mineable/pickaxe")`.
    pub fn tag(registry: &str, tag: &str) -> Option<&'static [i32]> {
        let registry = normalize(registry);
        let tag = normalize(tag);
        let (_, tags) = TAGS.iter().find(|(r, _)| *r == registry.as_ref())?;
        tags.iter().find(|(t, _)| *t == tag.as_ref()).map(|(_, ids)| *ids)
    }

    pub fn dimension_type(name: &str) -> Option<&'static DimensionTypeInfo> {
        let name = normalize(name);
        DIMENSION_TYPES.iter().find(|d| d.name == name.as_ref())
    }
}

mod extra;
pub use extra::{entity_types, game_rules, item_data};

/// Per-state block physics that block updates, fluids and redstone need: which faces give
/// support, vanilla's legacy "solid" flag, redstone conductors, comparator outputs, piston
/// push reactions, full collision faces and note block instruments.
///
/// Faces are numbered like vanilla's `Direction`: 0 down, 1 up, 2 north, 3 south, 4 west,
/// 5 east. Generated from `data/<version>/block_physics.json` (see `tools/datagen/physics.py`).
pub mod physics {
    include!(concat!(env!("OUT_DIR"), "/physics.rs"));

    // Collision shapes, behaviour flags, friction and loot tables per block (derived from
    // Pumpkin's extracted assets; see `extra.rs`).
    pub use crate::extra::physics::*;

    /// How a block reacts to a piston (`PushReaction`).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PushReaction {
        Normal,
        /// Broken (and dropped) when pushed.
        Destroy,
        /// Cannot be moved and stops the piston.
        Block,
        /// Can be pushed but not pulled.
        PushOnly,
    }

    #[inline]
    fn byte(table: &[u8], state: u32) -> u8 {
        table.get(state as usize).copied().unwrap_or(0)
    }

    /// `isFaceSturdy(face, SupportType.FULL)`: the face can hold wall torches, levers, wire...
    #[inline]
    pub fn is_face_sturdy(state: u32, face: usize) -> bool {
        byte(SUPPORT, state) & (1 << face) != 0
    }

    /// `isFaceSturdy(face, SupportType.CENTER)` for the up and down faces (torches on top,
    /// lanterns below); other faces fall back to full support.
    #[inline]
    pub fn has_center_support(state: u32, face: usize) -> bool {
        let support = byte(SUPPORT, state);
        support & (1 << face) != 0 || (face < 2 && support & (1 << (6 + face)) != 0)
    }

    /// Vanilla's legacy `isSolid()`: a large enough collision shape. Fluids form sources on
    /// it and cannot flow into it (via `blocksMotion`).
    #[inline]
    pub fn is_solid(state: u32) -> bool {
        byte(FLAGS, state) & 1 != 0
    }

    /// `isRedstoneConductor`: the block can be powered and passes power on to components.
    #[inline]
    pub fn is_redstone_conductor(state: u32) -> bool {
        byte(FLAGS, state) & 2 != 0
    }

    /// `hasAnalogOutputSignal`: a comparator reads a value from the block.
    #[inline]
    pub fn has_analog_output(state: u32) -> bool {
        byte(FLAGS, state) & 4 != 0
    }

    pub fn push_reaction(state: u32) -> PushReaction {
        match (byte(FLAGS, state) >> 3) & 3 {
            0 => PushReaction::Normal,
            1 => PushReaction::Destroy,
            2 => PushReaction::Block,
            _ => PushReaction::PushOnly,
        }
    }

    /// Whether the collision shape covers the whole face (used for fluid flow between
    /// partial blocks).
    #[inline]
    pub fn collision_face_full(state: u32, face: usize) -> bool {
        byte(COLLISION_FACES, state) & (1 << face) != 0
    }

    /// The note block instrument the block gives (a value of `note_block[instrument]`).
    pub fn instrument(state: u32) -> &'static str {
        INSTRUMENTS.get(byte(INSTRUMENT, state) as usize).copied().unwrap_or("harp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physics_tables() {
        let state = |name: &str| blocks::parse_state(name).unwrap();
        let stone = state("stone");
        assert!((0..6).all(|f| physics::is_face_sturdy(stone, f) && physics::collision_face_full(stone, f)));
        assert!(physics::is_redstone_conductor(stone) && physics::is_solid(stone));
        let glass = state("glass");
        assert!(!physics::is_redstone_conductor(glass) && physics::is_face_sturdy(glass, 1));
        assert!(!physics::is_redstone_conductor(state("redstone_block")));
        assert!(physics::is_redstone_conductor(state("soul_sand")));
        let slab = state("stone_slab[type=bottom]");
        assert!(physics::is_face_sturdy(slab, 0) && !physics::is_face_sturdy(slab, 1));
        assert!(physics::collision_face_full(slab, 0) && !physics::collision_face_full(slab, 2));
        let fence = state("oak_fence");
        assert!(physics::has_center_support(fence, 1) && !physics::is_face_sturdy(fence, 1));
        assert!(physics::has_analog_output(state("chest")));
        assert_eq!(physics::push_reaction(state("obsidian")), physics::PushReaction::Block);
        assert_eq!(physics::push_reaction(state("redstone_wire")), physics::PushReaction::Destroy);
        assert_eq!(physics::instrument(stone), "basedrum");
        assert_eq!(physics::instrument(state("glass")), "hat");
        assert_eq!(physics::instrument(state("zombie_head")), "zombie");
    }

    #[test]
    fn version_constants() {
        assert_eq!(PROTOCOL_VERSION, 777);
        assert_eq!(VERSION_NAME, "26.3");
    }

    #[test]
    fn packet_ids_are_dense() {
        assert_eq!(packets::handshake::serverbound::INTENTION, 0);
        assert_eq!(packets::status::clientbound::NAMES.len(), 2);
        assert_eq!(packets::play::clientbound::NAMES[packets::play::clientbound::LOGIN as usize], "minecraft:login");
        assert_eq!(packets::play::clientbound::BUNDLE_DELIMITER, 0);
    }
}
