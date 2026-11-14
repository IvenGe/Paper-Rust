//! Structure pieces: the common state of vanilla's `StructurePiece` and its block placement
//! helpers, and the context pieces are placed in.
//!
//! Written to vanilla 26.3's `StructurePiece` (`placeBlock`, `generateBox` and friends,
//! `createChest`, `reorient`, `fillColumnDown`).

use super::bbox::BoundingBox;
use super::transform::{self, Mirror, Rotation};
use crate::chunk::Heightmap;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::BlockEntity;
use crate::random::{Random, WorldgenRandom};
use paper_data::blocks;
use paper_nbt::Compound;

/// A block state by name, cached.
pub fn st(name: &str) -> u32 {
    use std::collections::HashMap;
    use std::sync::{OnceLock, RwLock};
    static CACHE: OnceLock<RwLock<HashMap<String, u32>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    if let Some(s) = cache.read().ok().and_then(|c| c.get(name).copied()) {
        return s;
    }
    let s = blocks::parse_state(name).unwrap_or_else(|| panic!("unknown block state {name}"));
    if let Ok(mut c) = cache.write() {
        c.insert(name.to_owned(), s);
    }
    s
}

/// `state.setValue(property, value)`; unchanged if the block has no such property.
pub fn with(state: u32, property: &str, value: &str) -> u32 {
    blocks::with_property(state, property, value).unwrap_or(state)
}

pub fn with_bool(state: u32, property: &str, value: bool) -> u32 {
    with(state, property, if value { "true" } else { "false" })
}

pub fn with_dir(state: u32, property: &str, d: Dir) -> u32 {
    with(state, property, d.name())
}

/// Where a piece is placed: the chunk being decorated, inside the 3x3 region of the
/// decoration step, and the random of the structure's step.
pub struct PlaceCtx<'c, 'w> {
    pub w: &'c mut World<'w>,
    pub r: &'c mut WorldgenRandom,
    /// The blocks the piece may write (`getWritableArea` of the chunk).
    pub chunk_box: BoundingBox,
    /// The center of the first piece at its bottom (`StructureStart.placeInChunk`).
    pub reference: Pos,
}

impl PlaceCtx<'_, '_> {
    #[inline]
    pub fn get(&self, p: Pos) -> u32 {
        self.w.get(p)
    }

    /// `level.setBlock(pos, state, 2)`.
    #[inline]
    pub fn set(&mut self, p: Pos, state: u32) -> bool {
        self.w.set(p, state)
    }

    pub fn is_air(&self, p: Pos) -> bool {
        self.w.b.is_air(self.get(p))
    }

    /// `level.getHeight(heightmap, x, z)`.
    pub fn height(&self, hm: Heightmap, x: i32, z: i32) -> i32 {
        self.w.height_at(hm, x, z)
    }

    pub fn min_y(&self) -> i32 {
        self.w.min_y
    }

    pub fn add_block_entity(&mut self, p: Pos, be: BlockEntity) {
        self.w.add_block_entity(p, be);
    }

    pub fn add_entity_nbt(&mut self, nbt: Compound) {
        self.w.add_entity_nbt(nbt);
    }

    /// A container with a loot table and a seed from the step random (`setLootTable(table,
    /// random.nextLong())`).
    pub fn set_loot(&mut self, p: Pos, table: &str) {
        let seed = self.r.next_i64();
        self.add_block_entity(p, BlockEntity::Loot { table: table.to_owned(), seed });
    }

    /// A spawner for `entity` (`SpawnerBlockEntity.setEntityId`, which draws no random
    /// number for a spawner without spawn potentials).
    pub fn set_spawner(&mut self, p: Pos, entity: &'static str) {
        self.add_block_entity(p, BlockEntity::Spawner { entity });
    }

    /// `StructurePiece.isReplaceableByStructures`.
    pub fn replaceable_by_structures(&self, state: u32) -> bool {
        let b = self.w.b;
        b.is_air(state) || b.is_liquid(state) || b.is_block(state, b.known.glow_lichen) || b.is_block(state, b.known.seagrass) || b.is_block(state, b.known.tall_seagrass)
    }
}

/// `StructurePiece.BlockSelector`.
pub trait Selector {
    fn next(&mut self, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, edge: bool) -> u32;
}

/// The state shared by all pieces (`StructurePiece`'s fields).
#[derive(Clone, Debug)]
pub struct PieceBase {
    /// The `structure_piece` registry id of the piece type (`minecraft:shstart`, ...).
    pub kind: &'static str,
    pub gen_depth: i32,
    pub bbox: BoundingBox,
    pub orientation: Option<Dir>,
    pub mirror: Mirror,
    pub rotation: Rotation,
    /// Blocks `placeBlock` must not replace (mineshafts' `canBeReplaced`): block ids.
    pub protect: Option<[u32; 4]>,
}

impl PieceBase {
    pub fn new(kind: &'static str, gen_depth: i32, bbox: BoundingBox) -> Self {
        Self { kind, gen_depth, bbox, orientation: None, mirror: Mirror::None, rotation: Rotation::None, protect: None }
    }

    /// `setOrientation`.
    pub fn set_orientation(&mut self, d: Option<Dir>) {
        self.orientation = d;
        let (m, r) = match d {
            None => (Mirror::None, Rotation::None),
            Some(Dir::South) => (Mirror::LeftRight, Rotation::None),
            Some(Dir::West) => (Mirror::LeftRight, Rotation::Clockwise90),
            Some(Dir::East) => (Mirror::None, Rotation::Clockwise90),
            Some(_) => (Mirror::None, Rotation::None),
        };
        self.mirror = m;
        self.rotation = r;
    }

    pub fn with_orientation(mut self, d: Option<Dir>) -> Self {
        self.set_orientation(d);
        self
    }

    #[inline]
    pub fn world_x(&self, x: i32, z: i32) -> i32 {
        match self.orientation {
            None => x,
            Some(Dir::North | Dir::South) => self.bbox.min_x + x,
            Some(Dir::West) => self.bbox.max_x - z,
            Some(Dir::East) => self.bbox.min_x + z,
            Some(_) => x,
        }
    }

    #[inline]
    pub fn world_y(&self, y: i32) -> i32 {
        match self.orientation {
            None => y,
            Some(_) => y + self.bbox.min_y,
        }
    }

    #[inline]
    pub fn world_z(&self, x: i32, z: i32) -> i32 {
        match self.orientation {
            None => z,
            Some(Dir::North) => self.bbox.max_z - z,
            Some(Dir::South) => self.bbox.min_z + z,
            Some(Dir::West | Dir::East) => self.bbox.min_z + x,
            Some(_) => z,
        }
    }

    #[inline]
    pub fn world_pos(&self, x: i32, y: i32, z: i32) -> Pos {
        Pos::new(self.world_x(x, z), self.world_y(y), self.world_z(x, z))
    }

    fn can_be_replaced(&self, ctx: &PlaceCtx, x: i32, y: i32, z: i32) -> bool {
        match self.protect {
            None => true,
            Some(ids) => {
                let s = self.get_block(ctx, x, y, z);
                let id = blocks::block_id_of_state(s);
                !ids.contains(&id)
            }
        }
    }

    /// `placeBlock`: mirrored and rotated with the piece, clipped to the chunk.
    pub fn place_block(&self, ctx: &mut PlaceCtx, state: u32, x: i32, y: i32, z: i32) {
        let p = self.world_pos(x, y, z);
        if !ctx.chunk_box.is_inside(p) || !self.can_be_replaced(ctx, x, y, z) {
            return;
        }
        let s = transform::transform(state, self.mirror, self.rotation);
        ctx.set(p, s);
    }

    /// `getBlock`: air outside the chunk.
    pub fn get_block(&self, ctx: &PlaceCtx, x: i32, y: i32, z: i32) -> u32 {
        let p = self.world_pos(x, y, z);
        if !ctx.chunk_box.is_inside(p) {
            return blocks::AIR;
        }
        ctx.get(p)
    }

    /// `isInterior`: below the ocean floor heightmap of the column, one block up.
    pub fn is_interior(&self, ctx: &PlaceCtx, x: i32, y: i32, z: i32) -> bool {
        let p = self.world_pos(x, y + 1, z);
        ctx.chunk_box.is_inside(p) && p.y < ctx.height(Heightmap::OceanFloorWg, p.x, p.z)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn generate_air_box(&self, ctx: &mut PlaceCtx, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    self.place_block(ctx, blocks::AIR, x, y, z);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn generate_box(&self, ctx: &mut PlaceCtx, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, edge: u32, fill: u32, skip_air: bool) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if skip_air && ctx.w.b.is_air(self.get_block(ctx, x, y, z)) {
                        continue;
                    }
                    let on_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    self.place_block(ctx, if on_edge { edge } else { fill }, x, y, z);
                }
            }
        }
    }

    /// `generateBox` with a block selector (which draws for every position of the box).
    #[allow(clippy::too_many_arguments)]
    pub fn generate_box_sel(&self, ctx: &mut PlaceCtx, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, skip_air: bool, sel: &mut impl Selector) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if skip_air && ctx.w.b.is_air(self.get_block(ctx, x, y, z)) {
                        continue;
                    }
                    let on_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    let s = sel.next(ctx.r, x, y, z, on_edge);
                    self.place_block(ctx, s, x, y, z);
                }
            }
        }
    }

    /// `generateMaybeBox`.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_maybe_box(&self, ctx: &mut PlaceCtx, probability: f32, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, edge: u32, fill: u32, skip_air: bool, inside: bool) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if ctx.r.next_f32() > probability {
                        continue;
                    }
                    if skip_air && ctx.w.b.is_air(self.get_block(ctx, x, y, z)) {
                        continue;
                    }
                    if inside && !self.is_interior(ctx, x, y, z) {
                        continue;
                    }
                    let on_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    self.place_block(ctx, if on_edge { edge } else { fill }, x, y, z);
                }
            }
        }
    }

    /// `maybeGenerateBlock`.
    pub fn maybe_generate_block(&self, ctx: &mut PlaceCtx, probability: f32, x: i32, y: i32, z: i32, state: u32) {
        if ctx.r.next_f32() < probability {
            self.place_block(ctx, state, x, y, z);
        }
    }

    /// `generateUpperHalfSphere`.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_upper_half_sphere(&self, ctx: &mut PlaceCtx, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, fill: u32, skip_air: bool) {
        let diag_x = (x1 - x0 + 1) as f32;
        let diag_y = (y1 - y0 + 1) as f32;
        let diag_z = (z1 - z0 + 1) as f32;
        let cx = x0 as f32 + diag_x / 2.0;
        let cz = z0 as f32 + diag_z / 2.0;
        for y in y0..=y1 {
            let ny = (y - y0) as f32 / diag_y;
            for x in x0..=x1 {
                let nx = (x as f32 - cx) / (diag_x * 0.5);
                for z in z0..=z1 {
                    let nz = (z as f32 - cz) / (diag_z * 0.5);
                    if skip_air && ctx.w.b.is_air(self.get_block(ctx, x, y, z)) {
                        continue;
                    }
                    let dist = nx * nx + ny * ny + nz * nz;
                    if dist <= 1.05 {
                        self.place_block(ctx, fill, x, y, z);
                    }
                }
            }
        }
    }

    /// `fillColumnDown`: not rotated, down through air, fluids and water plants.
    pub fn fill_column_down(&self, ctx: &mut PlaceCtx, state: u32, x: i32, y: i32, z: i32) {
        let mut p = self.world_pos(x, y, z);
        if !ctx.chunk_box.is_inside(p) {
            return;
        }
        while ctx.replaceable_by_structures(ctx.get(p)) && p.y > ctx.min_y() + 1 {
            ctx.set(p, state);
            p = p.down();
        }
    }

    /// `createChest(level, chunkBB, random, x, y, z, lootTable)`.
    pub fn create_chest(&self, ctx: &mut PlaceCtx, x: i32, y: i32, z: i32, loot: &str) -> bool {
        create_chest_at(ctx, self.world_pos(x, y, z), loot, None)
    }

    /// `createDispenser`.
    pub fn create_dispenser(&self, ctx: &mut PlaceCtx, x: i32, y: i32, z: i32, facing: Dir, loot: &str) -> bool {
        let p = self.world_pos(x, y, z);
        let dispenser = st("minecraft:dispenser");
        if !ctx.chunk_box.is_inside(p) || ctx.w.b.is_block(ctx.get(p), dispenser) {
            return false;
        }
        self.place_block(ctx, with_dir(dispenser, "facing", facing), x, y, z);
        if blocks::block_id_of_state(ctx.get(p)) == blocks::block_id_of_state(dispenser) {
            ctx.set_loot(p, loot);
        }
        true
    }
}

/// `StructurePiece.createChest(level, chunkBB, random, pos, lootTable, state)`.
pub fn create_chest_at(ctx: &mut PlaceCtx, p: Pos, loot: &str, state: Option<u32>) -> bool {
    let chest = st("minecraft:chest");
    if !ctx.chunk_box.is_inside(p) || ctx.w.b.is_block(ctx.get(p), chest) {
        return false;
    }
    let state = state.unwrap_or_else(|| reorient(ctx, p, chest));
    ctx.set(p, state);
    if blocks::block_id_of_state(ctx.get(p)) == blocks::block_id_of_state(chest) {
        ctx.set_loot(p, loot);
    }
    true
}

/// `StructurePiece.reorient`: turns a chest away from the one solid neighbour, or to a
/// side that is free.
pub fn reorient(ctx: &PlaceCtx, p: Pos, state: u32) -> u32 {
    let b = ctx.w.b;
    let chest = blocks::block_id_of_state(st("minecraft:chest"));
    let mut solid = None;
    for d in Dir::HORIZONTAL {
        let s = ctx.get(p.rel(d));
        if blocks::block_id_of_state(s) == chest {
            return state;
        }
        if b.is_solid_render(s) {
            if solid.is_none() {
                solid = Some(d);
            } else {
                solid = None;
                break;
            }
        }
    }
    if let Some(d) = solid {
        return with_dir(state, "facing", d.opposite());
    }
    let mut lock = blocks::property(state, "facing").and_then(Dir::parse).unwrap_or(Dir::North);
    let mut rel = p.rel(lock);
    if b.is_solid_render(ctx.get(rel)) {
        lock = lock.opposite();
        rel = p.rel(lock);
    }
    if b.is_solid_render(ctx.get(rel)) {
        lock = lock.clockwise();
        rel = p.rel(lock);
    }
    if b.is_solid_render(ctx.get(rel)) {
        lock = lock.opposite();
    }
    with_dir(state, "facing", lock)
}
