//! Small helpers the block behaviours share: directions, block state properties without
//! allocation, cached block ids and tags, and the behaviour family of each block.

use paper_data::blocks;
use paper_protocol::codec::BlockPos;
use std::sync::OnceLock;

/// Vanilla's `Direction`, in its ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

/// Horizontal or vertical axis of a direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Dir {
    /// `Direction.values()`.
    pub const ALL: [Dir; 6] = [Dir::Down, Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East];
    /// `Direction.Plane.HORIZONTAL`.
    pub const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];
    /// `Direction.Plane.VERTICAL`.
    pub const VERTICAL: [Dir; 2] = [Dir::Up, Dir::Down];
    /// `NeighborUpdater.UPDATE_ORDER`: the order neighbours are told about a change.
    pub const UPDATE_ORDER: [Dir; 6] = [Dir::West, Dir::East, Dir::Down, Dir::Up, Dir::North, Dir::South];
    /// `BlockBehaviour.UPDATE_SHAPE_ORDER`: the order neighbours re-check their shape.
    pub const SHAPE_UPDATE_ORDER: [Dir; 6] = [Dir::West, Dir::East, Dir::North, Dir::South, Dir::Down, Dir::Up];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn offset(self) -> (i32, i32, i32) {
        match self {
            Dir::Down => (0, -1, 0),
            Dir::Up => (0, 1, 0),
            Dir::North => (0, 0, -1),
            Dir::South => (0, 0, 1),
            Dir::West => (-1, 0, 0),
            Dir::East => (1, 0, 0),
        }
    }

    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }

    pub fn axis(self) -> Axis {
        match self {
            Dir::Down | Dir::Up => Axis::Y,
            Dir::North | Dir::South => Axis::Z,
            Dir::West | Dir::East => Axis::X,
        }
    }

    /// Clockwise seen from above (north, east, south, west); vertical directions stay.
    pub fn clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            other => other,
        }
    }

    pub fn counter_clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::West,
            Dir::West => Dir::South,
            Dir::South => Dir::East,
            Dir::East => Dir::North,
            other => other,
        }
    }

    pub fn name(self) -> &'static str {
        ["down", "up", "north", "south", "west", "east"][self as usize]
    }

    pub fn from_name(name: &str) -> Option<Dir> {
        Some(match name {
            "down" => Dir::Down,
            "up" => Dir::Up,
            "north" => Dir::North,
            "south" => Dir::South,
            "west" => Dir::West,
            "east" => Dir::East,
            _ => return None,
        })
    }

    /// The horizontal direction an entity with this yaw faces (`Direction.fromYRot`).
    pub fn from_yaw(yaw: f32) -> Dir {
        [Dir::South, Dir::West, Dir::North, Dir::East][((yaw / 90.0 + 0.5).floor() as i32 & 3) as usize]
    }
}

/// Neighbour arithmetic on block positions.
pub trait PosExt {
    fn rel(self, dir: Dir) -> BlockPos;
    fn above(self) -> BlockPos;
    fn below(self) -> BlockPos;
}

impl PosExt for BlockPos {
    #[inline]
    fn rel(self, dir: Dir) -> BlockPos {
        let (x, y, z) = dir.offset();
        BlockPos::new(self.x + x, self.y + y, self.z + z)
    }
    #[inline]
    fn above(self) -> BlockPos {
        BlockPos::new(self.x, self.y + 1, self.z)
    }
    #[inline]
    fn below(self) -> BlockPos {
        BlockPos::new(self.x, self.y - 1, self.z)
    }
}

// --- properties ---------------------------------------------------------------------------

/// The value of a property, without allocating (unlike `blocks::property`).
pub fn prop(state: u32, name: &str) -> Option<&'static str> {
    let block = blocks::block_of_state(state);
    let mut index = state.wrapping_sub(block.min_state);
    for (prop, values) in block.properties.iter().rev() {
        let n = values.len() as u32;
        if *prop == name {
            return values.get((index % n) as usize).copied();
        }
        index /= n;
    }
    None
}

pub fn has_prop(state: u32, name: &str) -> bool {
    blocks::block_of_state(state).properties.iter().any(|(p, _)| *p == name)
}

/// The state with `name` set to `value`; unchanged if the block has no such value.
pub fn with(state: u32, name: &str, value: &str) -> u32 {
    blocks::with_property(state, name, value).unwrap_or(state)
}

pub fn get_bool(state: u32, name: &str) -> bool {
    prop(state, name) == Some("true")
}

pub fn set_bool(state: u32, name: &str, value: bool) -> u32 {
    with(state, name, if value { "true" } else { "false" })
}

pub fn get_int(state: u32, name: &str) -> i32 {
    prop(state, name).and_then(|v| v.parse().ok()).unwrap_or(0)
}

pub fn set_int(state: u32, name: &str, value: i32) -> u32 {
    with(state, name, &value.to_string())
}

pub fn get_dir(state: u32, name: &str) -> Option<Dir> {
    prop(state, name).and_then(Dir::from_name)
}

pub fn set_dir(state: u32, name: &str, dir: Dir) -> u32 {
    with(state, name, dir.name())
}

/// Vanilla's `cycle`: the next value of the property in its declared order.
pub fn cycle(state: u32, name: &str) -> u32 {
    let block = blocks::block_of_state(state);
    let Some((_, values)) = block.properties.iter().find(|(p, _)| *p == name) else { return state };
    let Some(current) = prop(state, name) else { return state };
    let i = values.iter().position(|v| *v == current).unwrap_or(0);
    with(state, name, values[(i + 1) % values.len()])
}

#[inline]
pub fn block_id(state: u32) -> u32 {
    blocks::block_id_of_state(state)
}

pub fn block_name(state: u32) -> &'static str {
    blocks::block_of_state(state).name
}

#[inline]
pub fn is_air(state: u32) -> bool {
    blocks::is_air(state)
}

/// The default state of a block id.
pub fn default_of(block: u32) -> u32 {
    blocks::BLOCKS[block as usize].default_state
}

/// Block id of a `minecraft:` block, looked up once per call site.
macro_rules! block {
    ($name:literal) => {{
        static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        *ID.get_or_init(|| paper_data::blocks::id(concat!("minecraft:", $name)).expect(concat!("unknown block ", $name)))
    }};
}
pub(crate) use block;

/// Default state of a `minecraft:` block, looked up once per call site.
macro_rules! default_state {
    ($name:literal) => {{
        static STATE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        *STATE.get_or_init(|| paper_data::blocks::default_state(concat!("minecraft:", $name)).expect(concat!("unknown block ", $name)))
    }};
}
pub(crate) use default_state;

/// Membership of block ids in the block tags sent to clients.
pub struct TagSet(Vec<bool>);

impl TagSet {
    fn load(tag: &str) -> TagSet {
        let mut set = vec![false; blocks::BLOCKS.len()];
        for id in paper_data::synced::tag("minecraft:block", tag).unwrap_or(&[]) {
            if let Some(slot) = set.get_mut(*id as usize) {
                *slot = true;
            }
        }
        TagSet(set)
    }

    #[inline]
    pub fn contains(&self, state: u32) -> bool {
        self.0.get(block_id(state) as usize).copied().unwrap_or(false)
    }
}

/// Whether a state's block is in a `minecraft:` block tag (cached per call site).
macro_rules! in_tag {
    ($state:expr, $tag:literal) => {{
        static SET: std::sync::OnceLock<$crate::world::block_updates::state::TagSet> = std::sync::OnceLock::new();
        SET.get_or_init(|| $crate::world::block_updates::state::TagSet::load_tag(concat!("minecraft:", $tag))).contains($state)
    }};
}
pub(crate) use in_tag;

impl TagSet {
    pub fn load_tag(tag: &str) -> TagSet {
        TagSet::load(tag)
    }
}

// --- behaviour families --------------------------------------------------------------------

/// The behaviour family of a block, decided once per block from its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Other,
    Water,
    Lava,
    RedstoneWire,
    RedstoneTorch,
    RedstoneWallTorch,
    /// Plain torches (torch, soul, copper): only need support.
    Torch,
    WallTorch,
    Lever,
    /// Buttons stay pressed 30 ticks (wooden) or 20 ticks (stone).
    Button { wooden: bool },
    RedstoneBlock,
    Repeater,
    Comparator,
    RedstoneLamp,
    Observer,
    NoteBlock,
    /// `by_hand`: players can open it (everything but iron).
    Door { by_hand: bool },
    Trapdoor { by_hand: bool },
    FenceGate,
    Bed,
    Ladder,
    Carpet,
    PressurePlate { weighted: bool },
    TargetBlock,
    DaylightDetector,
    LightningRod,
    TripwireHook,
    SculkSensor,
    /// Small plants that need a block from a support tag below.
    Vegetation,
    /// Two-block-tall plants.
    TallPlant,
    SnowyDirt,
    Fence,
    Pane,
    Stairs,
    Lectern,
    DetectorRail,
    /// Signal sources whose power needs simulation not done here (trapped chests, playing
    /// jukeboxes, calibrated sculk sensors): wires connect to them.
    PassiveSource,
    Piston { sticky: bool },
    PistonHead,
    MovingPiston,
    /// Primed by redstone power.
    Tnt,
    /// Fire and soul fire.
    Fire,
    NetherPortal,
    /// Dispensers and droppers: fired by a rising redstone signal.
    Dispenser,
    /// Hoppers: locked while powered.
    Hopper,
    // Plants and blocks with random ticks (see `plants`).
    Crop,
    Farmland,
    SugarCane,
    Cactus,
    Stem,
    AttachedStem,
    SweetBerryBush,
    NetherWart,
    Cocoa,
    Leaves,
    Ice,
    SnowLayer,
}

fn classify(name: &str) -> Kind {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    match n {
        "water" => return Kind::Water,
        "lava" => return Kind::Lava,
        "redstone_wire" => return Kind::RedstoneWire,
        "redstone_torch" => return Kind::RedstoneTorch,
        "redstone_wall_torch" => return Kind::RedstoneWallTorch,
        "torch" | "soul_torch" | "copper_torch" => return Kind::Torch,
        "wall_torch" | "soul_wall_torch" | "copper_wall_torch" => return Kind::WallTorch,
        "lever" => return Kind::Lever,
        "redstone_block" => return Kind::RedstoneBlock,
        "repeater" => return Kind::Repeater,
        "comparator" => return Kind::Comparator,
        "redstone_lamp" => return Kind::RedstoneLamp,
        "observer" => return Kind::Observer,
        "note_block" => return Kind::NoteBlock,
        "ladder" => return Kind::Ladder,
        "target" => return Kind::TargetBlock,
        "daylight_detector" => return Kind::DaylightDetector,
        "tripwire_hook" => return Kind::TripwireHook,
        "sculk_sensor" => return Kind::SculkSensor,
        "piston" => return Kind::Piston { sticky: false },
        "sticky_piston" => return Kind::Piston { sticky: true },
        "piston_head" => return Kind::PistonHead,
        "moving_piston" => return Kind::MovingPiston,
        "tnt" => return Kind::Tnt,
        "fire" | "soul_fire" => return Kind::Fire,
        "dispenser" | "dropper" => return Kind::Dispenser,
        "hopper" => return Kind::Hopper,
        _ if super::plants::classify(n).is_some() => return super::plants::classify(n).unwrap(),
        "nether_portal" => return Kind::NetherPortal,
        "grass_block" | "podzol" | "mycelium" => return Kind::SnowyDirt,
        "iron_bars" | "glass_pane" => return Kind::Pane,
        "moss_carpet" | "pale_moss_carpet" => return Kind::Carpet,
        "lectern" => return Kind::Lectern,
        "detector_rail" => return Kind::DetectorRail,
        "trapped_chest" | "jukebox" | "calibrated_sculk_sensor" => return Kind::PassiveSource,
        _ => {}
    }
    if n.ends_with("_stairs") {
        return Kind::Stairs;
    }
    if n.ends_with("_button") {
        return Kind::Button { wooden: !matches!(n, "stone_button" | "polished_blackstone_button") };
    }
    if n.ends_with("_door") {
        return Kind::Door { by_hand: n != "iron_door" };
    }
    if n.ends_with("_trapdoor") {
        return Kind::Trapdoor { by_hand: n != "iron_trapdoor" };
    }
    if n.ends_with("_fence_gate") {
        return Kind::FenceGate;
    }
    if n.ends_with("_bed") {
        return Kind::Bed;
    }
    if n.ends_with("_carpet") {
        return Kind::Carpet;
    }
    if n.ends_with("_pressure_plate") {
        return Kind::PressurePlate { weighted: n.contains("weighted") };
    }
    if n.ends_with("lightning_rod") {
        return Kind::LightningRod;
    }
    if n.ends_with("_stained_glass_pane") || n.ends_with("copper_bars") {
        return Kind::Pane;
    }
    if n.ends_with("_fence") {
        return Kind::Fence;
    }
    if matches!(n, "tall_grass" | "large_fern" | "sunflower" | "lilac" | "rose_bush" | "peony") {
        return Kind::TallPlant;
    }
    if matches!(n, "short_grass" | "fern" | "bush" | "firefly_bush")
        || n.ends_with("_sapling") && n != "bamboo_sapling"
        || n.ends_with("_tulip")
        || matches!(n, "dandelion" | "golden_dandelion" | "poppy" | "blue_orchid" | "allium" | "azure_bluet" | "oxeye_daisy" | "cornflower" | "lily_of_the_valley" | "torchflower" | "open_eyeblossom" | "closed_eyeblossom" | "pink_petals" | "wildflowers" | "leaf_litter")
    {
        return Kind::Vegetation;
    }
    Kind::Other
}

/// The behaviour family of a state's block.
pub fn kind(state: u32) -> Kind {
    static TABLE: OnceLock<Vec<Kind>> = OnceLock::new();
    let table = TABLE.get_or_init(|| blocks::BLOCKS.iter().map(|b| classify(b.name)).collect());
    table.get(block_id(state) as usize).copied().unwrap_or(Kind::Other)
}

/// A family of door-like blocks for picking open/close and click sounds, like vanilla's
/// `BlockSetType` and `WoodType`.
pub fn block_set(state: u32) -> &'static str {
    let n = block_name(state).strip_prefix("minecraft:").unwrap_or("");
    if n.starts_with("iron_") {
        "iron"
    } else if n.contains("copper") {
        "copper"
    } else if n.starts_with("cherry_") {
        "cherry_wood"
    } else if n.starts_with("bamboo_") {
        "bamboo_wood"
    } else if n.starts_with("crimson_") || n.starts_with("warped_") {
        "nether_wood"
    } else if n.starts_with("stone_") || n.starts_with("polished_blackstone_") {
        "stone"
    } else {
        "wooden"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_without_allocation() {
        let wire = blocks::parse_state("redstone_wire[power=7,north=side]").unwrap();
        assert_eq!(prop(wire, "power"), Some("7"));
        assert_eq!(get_int(wire, "power"), 7);
        assert_eq!(prop(wire, "north"), Some("side"));
        assert_eq!(prop(wire, "nope"), None);
        assert_eq!(get_int(set_int(wire, "power", 15), "power"), 15);
        let repeater = blocks::parse_state("repeater[delay=4]").unwrap();
        assert_eq!(get_int(cycle(repeater, "delay"), "delay"), 1, "cycle wraps around");
        let lever = blocks::parse_state("lever[powered=false]").unwrap();
        assert!(get_bool(cycle(lever, "powered"), "powered"));
        assert_eq!(get_dir(blocks::parse_state("repeater[facing=west]").unwrap(), "facing"), Some(Dir::West));
    }

    #[test]
    fn kinds_and_tags() {
        let s = |n: &str| blocks::parse_state(n).unwrap();
        assert_eq!(kind(s("redstone_wire")), Kind::RedstoneWire);
        assert_eq!(kind(s("oak_button")), Kind::Button { wooden: true });
        assert_eq!(kind(s("stone_button")), Kind::Button { wooden: false });
        assert_eq!(kind(s("iron_door")), Kind::Door { by_hand: false });
        assert_eq!(kind(s("oxidized_copper_door")), Kind::Door { by_hand: true });
        assert_eq!(kind(s("oak_trapdoor")), Kind::Trapdoor { by_hand: true });
        assert_eq!(kind(s("birch_fence_gate")), Kind::FenceGate);
        assert_eq!(kind(s("oak_fence")), Kind::Fence);
        assert_eq!(kind(s("poppy")), Kind::Vegetation);
        assert_eq!(kind(s("stone")), Kind::Other);
        assert!(in_tag!(s("torch"), "washed_away_by_fluids"));
        assert!(!in_tag!(s("stone"), "washed_away_by_fluids"));
        assert_eq!(block!("redstone_wire"), block_id(s("redstone_wire")));
        assert_eq!(default_state!("stone"), s("stone"));
        assert_eq!(block_set(s("crimson_door")), "nether_wood");
        assert_eq!(Dir::from_yaw(0.0), Dir::South);
        assert_eq!(Dir::from_yaw(90.0), Dir::West);
    }
}
