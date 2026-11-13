//! Positions, directions and the view of the 3x3 chunk region features write into.
//!
//! The region semantics (writes limited to the center chunk and its neighbours, reads of
//! unknown chunks returning air) follow vanilla's `WorldGenRegion` as modelled by the test
//! cache in `pumpkin-world/src/generation/proto_chunk_test/stage_parity.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::biome::FuzzyZoom;
use crate::chunk::{Heightmap, ProtoChunk};
use crate::generator::WorldGenerator;
use std::cell::RefCell;

use super::blockinfo::Blocks;
use super::{BlockEntity, RegionAccess};

/// A block position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Pos {
    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub fn add(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self::new(self.x.wrapping_add(dx), self.y.wrapping_add(dy), self.z.wrapping_add(dz))
    }

    #[inline]
    pub fn up(self) -> Self {
        self.add(0, 1, 0)
    }

    #[inline]
    pub fn down(self) -> Self {
        self.add(0, -1, 0)
    }

    #[inline]
    pub fn up_n(self, n: i32) -> Self {
        self.add(0, n, 0)
    }

    #[inline]
    pub fn rel(self, d: Dir) -> Self {
        let (dx, dy, dz) = d.offset();
        self.add(dx, dy, dz)
    }

    #[inline]
    pub fn rel_n(self, d: Dir, n: i32) -> Self {
        let (dx, dy, dz) = d.offset();
        self.add(dx * n, dy * n, dz * n)
    }

    #[inline]
    pub fn with_y(self, y: i32) -> Self {
        Self::new(self.x, y, self.z)
    }

    pub fn manhattan(self, o: Pos) -> i32 {
        (self.x - o.x).abs() + (self.y - o.y).abs() + (self.z - o.z).abs()
    }

    pub fn dist_sq(self, o: Pos) -> f64 {
        let dx = (self.x - o.x) as f64;
        let dy = (self.y - o.y) as f64;
        let dz = (self.z - o.z) as f64;
        dx * dx + dy * dy + dz * dz
    }

    /// `Vec3i.hashCode`.
    pub fn java_hash(self) -> i32 {
        self.y.wrapping_add(self.z.wrapping_mul(31)).wrapping_mul(31).wrapping_add(self.x)
    }
}

/// `Direction`, in vanilla's declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    /// `Direction.values()`.
    pub const ALL: [Dir; 6] = [Dir::Down, Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East];
    /// `Direction.Plane.HORIZONTAL`.
    pub const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    #[inline]
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

    /// `getClockWise` (around the y axis).
    pub fn clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            d => d,
        }
    }

    pub fn counter_clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::West,
            Dir::West => Dir::South,
            Dir::South => Dir::East,
            Dir::East => Dir::North,
            d => d,
        }
    }

    pub fn is_horizontal(self) -> bool {
        !matches!(self, Dir::Up | Dir::Down)
    }

    /// The axis name used by block properties (`x`, `y`, `z`).
    pub fn axis(self) -> &'static str {
        match self {
            Dir::Down | Dir::Up => "y",
            Dir::North | Dir::South => "z",
            Dir::West | Dir::East => "x",
        }
    }

    /// Whether the direction points along its axis (`AxisDirection.POSITIVE`).
    pub fn positive(self) -> bool {
        matches!(self, Dir::Up | Dir::South | Dir::East)
    }

    /// The name used by block properties (`north`, `up`, ...).
    pub fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }

    pub fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "down" => Dir::Down,
            "up" => Dir::Up,
            "north" => Dir::North,
            "south" => Dir::South,
            "west" => Dir::West,
            "east" => Dir::East,
            _ => return None,
        })
    }

    /// `Direction.getRandom`: one of all six.
    pub fn random(r: &mut impl crate::random::Random) -> Dir {
        Dir::ALL[r.next_bounded(6) as usize]
    }

    /// `Direction.Plane.HORIZONTAL.getRandomDirection`.
    pub fn random_horizontal(r: &mut impl crate::random::Random) -> Dir {
        Dir::HORIZONTAL[r.next_bounded(4) as usize]
    }
}

/// The world as features see it: the 3x3 chunks around the chunk being decorated.
pub struct World<'a> {
    pub gen: &'a WorldGenerator,
    pub b: &'a Blocks,
    region: &'a mut dyn RegionAccess,
    pub center_x: i32,
    pub center_z: i32,
    /// The dimension's build limits.
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    zoom: RefCell<FuzzyZoom>,
    region_random: RefCell<Option<crate::random::RandomSource>>,
}

impl<'a> World<'a> {
    pub fn new(gen: &'a WorldGenerator, b: &'a Blocks, region: &'a mut dyn RegionAccess, center_x: i32, center_z: i32) -> Self {
        Self {
            gen,
            b,
            region,
            center_x,
            center_z,
            min_y: gen.min_y,
            height: gen.height,
            sea_level: gen.sea_level,
            zoom: RefCell::new(FuzzyZoom::new(gen.biome_zoom_seed, gen.min_y, gen.height)),
            region_random: RefCell::new(None),
        }
    }

    #[inline]
    pub fn max_y(&self) -> i32 {
        self.min_y + self.height - 1
    }

    #[inline]
    pub fn out_of_height(&self, y: i32) -> bool {
        y < self.min_y || y >= self.min_y + self.height
    }

    #[inline]
    fn chunk(&self, x: i32, z: i32) -> Option<&ProtoChunk> {
        self.region.chunk(x >> 4, z >> 4)
    }

    /// The state at a position: void air outside the build height, air in chunks the region
    /// does not have.
    #[inline]
    pub fn get(&self, p: Pos) -> u32 {
        if self.out_of_height(p.y) {
            return self.b.known.void_air;
        }
        match self.chunk(p.x, p.z) {
            Some(c) => c.get(p.x, p.y, p.z),
            None => self.b.known.air,
        }
    }

    /// `WorldGenRegion.ensureCanWrite`: the center chunk and its neighbours.
    #[inline]
    pub fn can_write(&self, p: Pos) -> bool {
        ((p.x >> 4) - self.center_x).abs() <= 1 && ((p.z >> 4) - self.center_z).abs() <= 1
    }

    /// `setBlock`: updates the post-feature heightmaps. Returns whether the write happened.
    pub fn set(&mut self, p: Pos, state: u32) -> bool {
        if !self.can_write(p) || self.out_of_height(p.y) {
            return false;
        }
        let flags = &self.gen.flags;
        match self.region.chunk_mut(p.x >> 4, p.z >> 4) {
            Some(c) => {
                c.set(p.x, p.y, p.z, state, flags);
                true
            }
            None => false,
        }
    }

    /// A write straight into the section, without heightmap updates (as the ore feature does).
    pub fn set_raw(&mut self, p: Pos, state: u32) {
        if !self.can_write(p) || self.out_of_height(p.y) {
            return;
        }
        if let Some(c) = self.region.chunk_mut(p.x >> 4, p.z >> 4) {
            c.set_raw(p.x, p.y, p.z, state);
        }
    }

    pub fn add_block_entity(&mut self, p: Pos, entity: BlockEntity) {
        if self.can_write(p) && !self.out_of_height(p.y) {
            self.region.add_block_entity(p.x, p.y, p.z, entity);
        }
    }

    #[inline]
    pub fn is_air(&self, p: Pos) -> bool {
        self.b.is_air(self.get(p))
    }

    #[inline]
    pub fn fluid(&self, p: Pos) -> super::blockinfo::Fluid {
        self.b.fluid(self.get(p))
    }

    /// `isWaterAt`: the fluid at the position is water (source or flowing).
    pub fn is_water_at(&self, p: Pos) -> bool {
        self.fluid(p).is_water()
    }

    /// `getHeight(type, x, z)`: the first free y above the heightmap.
    pub fn height_at(&self, hm: Heightmap, x: i32, z: i32) -> i32 {
        match self.chunk(x, z) {
            Some(c) => c.top_y(hm, x, z),
            None => self.min_y,
        }
    }

    /// The biome at a quart position, from the region's chunks or computed directly.
    pub fn noise_biome(&self, qx: i32, qy: i32, qz: i32) -> u8 {
        match self.region.chunk(qx >> 2, qz >> 2) {
            Some(c) => c.biome(qx, qy, qz),
            None => self.gen.biome_at_quart(qx, qy, qz),
        }
    }

    /// `getBiome(pos)`: the fuzzy (zoomed) biome lookup.
    pub fn biome(&self, p: Pos) -> u8 {
        let (qx, qy, qz) = self.zoom.borrow_mut().quart(p.x, p.y, p.z);
        self.noise_biome(qx, qy, qz)
    }

    pub fn seed(&self) -> i64 {
        self.gen.seed
    }

    /// `WorldGenRegion.getRandom()`: one random per decorated chunk, positioned at the
    /// chunk's origin (used where vanilla takes the level's random instead of the
    /// feature's).
    pub fn region_random(&self) -> crate::random::RandomSource {
        self.region_random.borrow_mut().take().unwrap_or_else(|| {
            self.gen.base_random.from_hash_of("minecraft:worldgen_region_random").fork_positional().at(self.center_x << 4, 0, self.center_z << 4)
        })
    }

    pub fn store_region_random(&self, r: crate::random::RandomSource) {
        *self.region_random.borrow_mut() = Some(r);
    }
}
