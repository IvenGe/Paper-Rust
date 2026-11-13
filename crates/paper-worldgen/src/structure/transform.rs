//! Rotations and mirrors of positions and block states.
//!
//! Written to vanilla 26.3's `Rotation`, `Mirror` and the `rotate` / `mirror` overrides of
//! the block classes: facing, axis, sign rotation, rail shapes, stair shapes, door hinges,
//! the four side properties of fences, walls, panes, vines, redstone and multiface blocks,
//! and the front/top orientation of jigsaws and crafters. Vanilla quirks kept: fire and
//! chorus plants are never rotated, anvils are not mirrored, mirroring a stair along the
//! x axis keeps inner corners as they are, and chests keep their left/right type.

use crate::feature::world::{Dir, Pos};
use crate::random::Random;
use paper_data::blocks;
use std::sync::OnceLock;

/// `Rotation`, in vanilla's declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    #[default]
    None,
    Clockwise90,
    Clockwise180,
    CounterClockwise90,
}

impl Rotation {
    pub const ALL: [Rotation; 4] = [Rotation::None, Rotation::Clockwise90, Rotation::Clockwise180, Rotation::CounterClockwise90];

    pub fn ordinal(self) -> usize {
        self as usize
    }

    pub fn from_ordinal(i: usize) -> Self {
        Self::ALL[i & 3]
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" | "NONE" => Rotation::None,
            "clockwise_90" | "CLOCKWISE_90" => Rotation::Clockwise90,
            "180" | "clockwise_180" | "CLOCKWISE_180" => Rotation::Clockwise180,
            "counterclockwise_90" | "COUNTERCLOCKWISE_90" => Rotation::CounterClockwise90,
            _ => return None,
        })
    }

    /// `Rotation.getRandom`.
    pub fn random(r: &mut impl Random) -> Self {
        Self::ALL[r.next_bounded(4) as usize]
    }

    /// `Rotation.getShuffled`: `Util.shuffledCopy(values(), random)`.
    pub fn shuffled(r: &mut impl Random) -> [Rotation; 4] {
        let mut v = Self::ALL;
        for i in (2..=4).rev() {
            let j = r.next_bounded(i as i32) as usize;
            v.swap(i - 1, j);
        }
        v
    }

    /// `getRotated`: this rotation followed by `other`.
    pub fn then(self, other: Rotation) -> Rotation {
        Self::from_ordinal(self.ordinal() + other.ordinal())
    }

    /// `rotate(Direction)`.
    pub fn rotate(self, d: Dir) -> Dir {
        if !d.is_horizontal() {
            return d;
        }
        match self {
            Rotation::None => d,
            Rotation::Clockwise90 => d.clockwise(),
            Rotation::Clockwise180 => d.opposite(),
            Rotation::CounterClockwise90 => d.counter_clockwise(),
        }
    }

    /// `rotate(int value, int maxValue)`.
    pub fn rotate_segment(self, value: i32, max: i32) -> i32 {
        match self {
            Rotation::None => value,
            Rotation::Clockwise90 => (value + max / 4) % max,
            Rotation::Clockwise180 => (value + max / 2) % max,
            Rotation::CounterClockwise90 => (value + max * 3 / 4) % max,
        }
    }
}

/// `Mirror`, in vanilla's declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Mirror {
    #[default]
    None,
    /// Flips z.
    LeftRight,
    /// Flips x.
    FrontBack,
}

impl Mirror {
    pub fn ordinal(self) -> usize {
        self as usize
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" | "NONE" => Mirror::None,
            "left_right" | "LEFT_RIGHT" => Mirror::LeftRight,
            "front_back" | "FRONT_BACK" => Mirror::FrontBack,
            _ => return None,
        })
    }

    /// `mirror(Direction)`.
    pub fn mirror(self, d: Dir) -> Dir {
        match (self, d) {
            (Mirror::LeftRight, Dir::North | Dir::South) | (Mirror::FrontBack, Dir::East | Dir::West) => d.opposite(),
            _ => d,
        }
    }

    /// `getRotation(Direction)`.
    pub fn rotation_for(self, d: Dir) -> Rotation {
        match (self, d) {
            (Mirror::LeftRight, Dir::North | Dir::South) | (Mirror::FrontBack, Dir::East | Dir::West) => Rotation::Clockwise180,
            _ => Rotation::None,
        }
    }

    /// `mirror(int value, int maxValue)`.
    pub fn mirror_segment(self, value: i32, max: i32) -> i32 {
        let half = max / 2;
        let local = if value > half { value - max } else { value };
        match self {
            Mirror::None => value,
            Mirror::LeftRight => (half - local + max) % max,
            Mirror::FrontBack => (max - local) % max,
        }
    }
}

/// `StructureTemplate.transform(pos, mirror, rotation, pivot)`.
pub fn transform_pos(p: Pos, mirror: Mirror, rotation: Rotation, pivot: Pos) -> Pos {
    let (mut x, y, mut z) = (p.x, p.y, p.z);
    let mirrored = match mirror {
        Mirror::LeftRight => {
            z = -z;
            true
        }
        Mirror::FrontBack => {
            x = -x;
            true
        }
        Mirror::None => false,
    };
    let (px, pz) = (pivot.x, pivot.z);
    match rotation {
        Rotation::CounterClockwise90 => Pos::new(px - pz + z, y, px + pz - x),
        Rotation::Clockwise90 => Pos::new(px + pz - z, y, pz - px + x),
        Rotation::Clockwise180 => Pos::new(px + px - x, y, pz + pz - z),
        Rotation::None => {
            if mirrored {
                Pos::new(x, y, z)
            } else {
                p
            }
        }
    }
}

/// `StructureTemplate.transform(Vec3, mirror, rotation, pivot)` for entity positions.
pub fn transform_vec(p: (f64, f64, f64), mirror: Mirror, rotation: Rotation, pivot: Pos) -> (f64, f64, f64) {
    let (mut x, y, mut z) = p;
    let mirrored = match mirror {
        Mirror::LeftRight => {
            z = 1.0 - z;
            true
        }
        Mirror::FrontBack => {
            x = 1.0 - x;
            true
        }
        Mirror::None => false,
    };
    let (px, pz) = (pivot.x as f64, pivot.z as f64);
    match rotation {
        Rotation::CounterClockwise90 => (px - pz + z, y, px + pz + 1.0 - x),
        Rotation::Clockwise90 => (px + pz + 1.0 - z, y, pz - px + x),
        Rotation::Clockwise180 => (px + px + 1.0 - x, y, pz + pz + 1.0 - z),
        Rotation::None => {
            if mirrored {
                (x, y, z)
            } else {
                p
            }
        }
    }
}

struct Tables {
    /// `rotate` for CLOCKWISE_90, CLOCKWISE_180, COUNTERCLOCKWISE_90.
    rotate: [Vec<u32>; 3],
    /// `mirror` for LEFT_RIGHT, FRONT_BACK.
    mirror: [Vec<u32>; 2],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let n = blocks::STATE_COUNT;
        let rot = |r: Rotation| (0..n).map(|s| transform_state(s, Mirror::None, r)).collect::<Vec<u32>>();
        let mir = |m: Mirror| (0..n).map(|s| transform_state(s, m, Rotation::None)).collect::<Vec<u32>>();
        Tables { rotate: [rot(Rotation::Clockwise90), rot(Rotation::Clockwise180), rot(Rotation::CounterClockwise90)], mirror: [mir(Mirror::LeftRight), mir(Mirror::FrontBack)] }
    })
}

/// `BlockState.rotate(rotation)`.
#[inline]
pub fn rotate_state(state: u32, rotation: Rotation) -> u32 {
    match rotation {
        Rotation::None => state,
        r => tables().rotate[r.ordinal() - 1].get(state as usize).copied().unwrap_or(state),
    }
}

/// `BlockState.mirror(mirror)`.
#[inline]
pub fn mirror_state(state: u32, mirror: Mirror) -> u32 {
    match mirror {
        Mirror::None => state,
        m => tables().mirror[m.ordinal() - 1].get(state as usize).copied().unwrap_or(state),
    }
}

/// `state.mirror(mirror).rotate(rotation)`.
#[inline]
pub fn transform(state: u32, mirror: Mirror, rotation: Rotation) -> u32 {
    rotate_state(mirror_state(state, mirror), rotation)
}

fn dir_of(s: &str) -> Option<Dir> {
    Dir::parse(s)
}

fn transform_dir(d: Dir, mirror: Mirror, rotation: Rotation) -> Dir {
    rotation.rotate(mirror.mirror(d))
}

/// Computes one transformed state from the block's properties (mirror first, then rotation).
fn transform_state(state: u32, mirror: Mirror, rotation: Rotation) -> u32 {
    let block = blocks::block_of_state(state);
    if block.properties.is_empty() {
        return state;
    }
    let name = block.name;
    if matches!(name, "minecraft:fire" | "minecraft:chorus_plant") {
        return state;
    }
    let props = blocks::properties_of(state);
    let get = |k: &str| props.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
    let mut out = state;
    let mut set = |k: &str, v: &str| {
        if let Some(s) = blocks::with_property(out, k, v) {
            out = s;
        }
    };
    let anvil = name.ends_with("anvil");
    let mirror = if anvil { Mirror::None } else { mirror };
    if mirror == Mirror::None && rotation == Rotation::None {
        return state;
    }

    if name.ends_with("_stairs") {
        if let (Some(facing), Some(shape)) = (get("facing").and_then(dir_of), get("shape")) {
            let mut f = facing;
            let mut sh = shape;
            let along = match mirror {
                Mirror::LeftRight => matches!(f, Dir::North | Dir::South),
                Mirror::FrontBack => matches!(f, Dir::East | Dir::West),
                Mirror::None => false,
            };
            if along {
                f = f.opposite();
                sh = match (mirror, shape) {
                    (_, "outer_left") => "outer_right",
                    (_, "outer_right") => "outer_left",
                    (Mirror::LeftRight, "inner_left") => "inner_right",
                    (Mirror::LeftRight, "inner_right") => "inner_left",
                    (_, s) => s,
                };
            }
            f = rotation.rotate(f);
            set("facing", f.name());
            set("shape", sh);
        }
        return out;
    }

    let mut new_sides: Vec<(&'static str, &'static str)> = Vec::new();
    for (k, v) in &props {
        match *k {
            "facing" => {
                if let Some(d) = dir_of(v) {
                    new_sides.push(("facing", transform_dir(d, mirror, rotation).name()));
                }
            }
            "axis" => {
                if matches!(rotation, Rotation::Clockwise90 | Rotation::CounterClockwise90) {
                    let swapped = match *v {
                        "x" => "z",
                        "z" => "x",
                        other => other,
                    };
                    new_sides.push(("axis", swapped));
                }
            }
            "rotation" => {
                if let Ok(r) = v.parse::<i32>() {
                    let r = rotation.rotate_segment(mirror.mirror_segment(r, 16), 16);
                    new_sides.push(("rotation", rotation_str(r)));
                }
            }
            "hinge" if mirror != Mirror::None => {
                new_sides.push(("hinge", if *v == "left" { "right" } else { "left" }));
            }
            "shape" if name.ends_with("rail") => {
                new_sides.push(("shape", transform_rail(v, mirror, rotation)));
            }
            "orientation" => {
                if let Some((front, top)) = v.split_once('_') {
                    if let (Some(f), Some(t)) = (dir_of(front), dir_of(top)) {
                        let (f, t) = (transform_dir(f, mirror, rotation), transform_dir(t, mirror, rotation));
                        if let Some(n) = orientation_str(f, t) {
                            new_sides.push(("orientation", n));
                        }
                    }
                }
            }
            "north" | "east" | "south" | "west" => {
                let d = dir_of(k).expect("side name");
                let nd = transform_dir(d, mirror, rotation);
                new_sides.push((nd.name(), v));
            }
            _ => {}
        }
    }
    for (k, v) in new_sides {
        set(k, v);
    }
    out
}

fn rotation_str(r: i32) -> &'static str {
    const S: [&str; 16] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"];
    S[r.rem_euclid(16) as usize]
}

fn orientation_str(front: Dir, top: Dir) -> Option<&'static str> {
    const ALL: [&str; 12] = ["down_east", "down_north", "down_south", "down_west", "up_east", "up_north", "up_south", "up_west", "west_up", "east_up", "north_up", "south_up"];
    ALL.iter().copied().find(|n| n.split_once('_').map(|(f, t)| (dir_of(f), dir_of(t))) == Some((Some(front), Some(top))))
}

/// `BaseRailBlock.rotate` / `mirror` on rail shapes (geometric: each named side is turned).
fn transform_rail(shape: &str, mirror: Mirror, rotation: Rotation) -> &'static str {
    const ALL: [&str; 10] = ["north_south", "east_west", "ascending_east", "ascending_west", "ascending_north", "ascending_south", "south_east", "south_west", "north_west", "north_east"];
    let t = |d: Dir| transform_dir(d, mirror, rotation);
    if let Some(d) = shape.strip_prefix("ascending_").and_then(dir_of) {
        let nd = t(d);
        return ALL.iter().copied().find(|n| *n == format!("ascending_{}", nd.name())).unwrap_or("north_south");
    }
    if shape == "north_south" || shape == "east_west" {
        let d = if shape == "north_south" { Dir::North } else { Dir::East };
        return if matches!(t(d), Dir::North | Dir::South) { "north_south" } else { "east_west" };
    }
    let (a, b) = shape.split_once('_').and_then(|(a, b)| Some((dir_of(a)?, dir_of(b)?))).unwrap_or((Dir::North, Dir::East));
    let (a, b) = (t(a), t(b));
    let (ns, ew) = if matches!(a, Dir::North | Dir::South) { (a, b) } else { (b, a) };
    ALL.iter().copied().find(|n| *n == format!("{}_{}", ns.name(), ew.name())).unwrap_or("north_south")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(s: &str) -> u32 {
        blocks::parse_state(s).unwrap()
    }

    #[test]
    fn rails_match_vanilla_tables() {
        // Spot checks against BaseRailBlock's switch tables.
        let r = |s: &str, m: Mirror, rot: Rotation| transform_rail(s, m, rot);
        assert_eq!(r("south_east", Mirror::None, Rotation::Clockwise90), "south_west");
        assert_eq!(r("north_east", Mirror::None, Rotation::CounterClockwise90), "north_west");
        assert_eq!(r("ascending_east", Mirror::None, Rotation::Clockwise90), "ascending_south");
        assert_eq!(r("south_east", Mirror::LeftRight, Rotation::None), "north_east");
        assert_eq!(r("north_west", Mirror::FrontBack, Rotation::None), "north_east");
        assert_eq!(r("ascending_north", Mirror::FrontBack, Rotation::None), "ascending_north");
        assert_eq!(r("north_south", Mirror::None, Rotation::Clockwise180), "north_south");
    }

    #[test]
    fn states() {
        let stair = st("minecraft:oak_stairs[facing=east,shape=inner_left]");
        assert_eq!(mirror_state(stair, Mirror::FrontBack), st("minecraft:oak_stairs[facing=west,shape=inner_left]"));
        assert_eq!(mirror_state(stair, Mirror::LeftRight), stair);
        let door = st("minecraft:oak_door[facing=east,hinge=left]");
        assert_eq!(mirror_state(door, Mirror::LeftRight), st("minecraft:oak_door[facing=east,hinge=right]"));
        let fence = st("minecraft:oak_fence[north=true,east=false,south=false,west=false]");
        assert_eq!(rotate_state(fence, Rotation::Clockwise90), st("minecraft:oak_fence[north=false,east=true,south=false,west=false]"));
        let log = st("minecraft:oak_log[axis=x]");
        assert_eq!(rotate_state(log, Rotation::Clockwise90), st("minecraft:oak_log[axis=z]"));
        let sign = st("minecraft:oak_sign[rotation=3]");
        assert_eq!(mirror_state(sign, Mirror::LeftRight), st("minecraft:oak_sign[rotation=5]"));
        let anvil = st("minecraft:anvil[facing=north]");
        assert_eq!(mirror_state(anvil, Mirror::LeftRight), anvil);
        assert_eq!(rotate_state(anvil, Rotation::Clockwise90), st("minecraft:anvil[facing=east]"));
        let jig = st("minecraft:jigsaw[orientation=north_up]");
        assert_eq!(rotate_state(jig, Rotation::Clockwise90), st("minecraft:jigsaw[orientation=east_up]"));
    }

    #[test]
    fn shuffle_and_positions() {
        let p = transform_pos(Pos::new(3, 1, 2), Mirror::None, Rotation::Clockwise90, Pos::new(0, 0, 0));
        assert_eq!(p, Pos::new(-2, 1, 3));
    }
}
