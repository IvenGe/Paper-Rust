//! Swamp huts, jungle temples and desert pyramids (`ScatteredFeaturePiece`s).
//!
//! Written to vanilla 26.3's `ScatteredFeaturePiece`, `SwampHutStructure`/`SwampHutPiece`,
//! `JungleTempleStructure`/`JungleTemplePiece` and `DesertPyramidStructure`/
//! `DesertPyramidPiece`. The pieces settle their height when first
//! placed (from the heightmaps of that chunk), as vanilla does. The witch and cat of a swamp
//! hut are returned as entity NBT; vanilla's `finalizeSpawn` is left to the caller.

use super::bbox::BoundingBox;
use super::piece::{mob, st, with, with_bool, with_dir, PieceBase, PlaceCtx, Selector};
use super::persist::{get_bool, get_i32, read_base};
use super::{GenCtx, Piece, StructureGen};
use crate::chunk::Heightmap;
use crate::generator::WorldGenerator;
use anyhow::Result;
use paper_nbt::Compound;
use crate::feature::world::{Dir, Pos};
use crate::random::{Random, WorldgenRandom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

enum Kind {
    SwampHut { witch: AtomicBool, cat: AtomicBool },
    JungleTemple(JungleFlags),
    /// `hasPlacedChest`, by `get2DDataValue`.
    DesertPyramid([AtomicBool; 4]),
}

struct JungleFlags {
    placed_main_chest: AtomicBool,
    placed_hidden_chest: AtomicBool,
    placed_trap1: AtomicBool,
    placed_trap2: AtomicBool,
}

/// `ScatteredFeaturePiece`.
struct Scattered {
    base: PieceBase,
    /// `width`, `height`, `depth`.
    size: (i32, i32, i32),
    /// Once the height is settled: the vertical move applied to `base`'s box and the ground
    /// height found (`heightPosition`).
    settled: Mutex<Option<(i32, i32)>>,
    kind: Kind,
}

impl Scattered {
    #[allow(clippy::too_many_arguments)]
    fn new(kind_id: &'static str, x: i32, y: i32, z: i32, width: i32, height: i32, depth: i32, dir: Dir, kind: Kind) -> Self {
        let base = PieceBase::new(kind_id, 0, BoundingBox::make(x, y, z, dir, width, height, depth)).with_orientation(Some(dir));
        Self { base, size: (width, height, depth), settled: Mutex::new(None), kind }
    }

    /// The NBT constructor: the saved box is already moved when `HPos` is set.
    fn load(t: &Compound, id: &'static str, kind: Kind) -> Result<Box<dyn Piece>> {
        let base = read_base(t, &[id])?;
        let hpos = t.get("HPos").and_then(paper_nbt::Tag::as_i64).unwrap_or(-1) as i32;
        let size = (get_i32(t, "Width"), get_i32(t, "Height"), get_i32(t, "Depth"));
        Ok(Box::new(Self { base, size, settled: Mutex::new((hpos >= 0).then_some((0, hpos))), kind }))
    }

    /// `updateHeightPositionToLowestGroundHeight`: the piece's base with its settled height.
    fn lowest_ground_height(&self, ctx: &PlaceCtx, offset: i32) -> PieceBase {
        let mut settled = self.settled.lock().unwrap_or_else(|e| e.into_inner());
        if settled.is_none() {
            let bb = self.base.bbox;
            let mut lowest = ctx.w.max_y() + 1;
            for z in bb.min_z..=bb.max_z {
                for x in bb.min_x..=bb.max_x {
                    lowest = lowest.min(ctx.w.height_at(Heightmap::MotionBlockingNoLeaves, x, z));
                }
            }
            *settled = Some((lowest - bb.min_y + offset, lowest));
        }
        let mut base = self.base.clone();
        base.bbox.move_by(0, settled.map_or(0, |s| s.0), 0);
        base
    }

    /// `updateAverageGroundHeight`: the piece's base with its settled height, if it could be
    /// settled.
    fn average_ground_height(&self, ctx: &PlaceCtx, offset: i32) -> Option<PieceBase> {
        let mut settled = self.settled.lock().unwrap_or_else(|e| e.into_inner());
        if settled.is_none() {
            let bb = self.base.bbox;
            let (mut total, mut count) = (0, 0);
            for z in bb.min_z..=bb.max_z {
                for x in bb.min_x..=bb.max_x {
                    if ctx.chunk_box.contains(x, 64, z) {
                        total += ctx.w.height_at(Heightmap::MotionBlockingNoLeaves, x, z);
                        count += 1;
                    }
                }
            }
            if count == 0 {
                return None;
            }
            *settled = Some((total / count - bb.min_y + offset, total / count));
        }
        let mut base = self.base.clone();
        base.bbox.move_by(0, settled.map_or(0, |s| s.0), 0);
        Some(base)
    }
}

/// `JungleTemplePiece.MossStoneSelector`.
struct MossStone;

impl Selector for MossStone {
    fn next(&mut self, r: &mut WorldgenRandom, _x: i32, _y: i32, _z: i32, _edge: bool) -> u32 {
        if r.next_f32() < 0.4 {
            st("minecraft:cobblestone")
        } else {
            st("minecraft:mossy_cobblestone")
        }
    }
}

impl Piece for Scattered {
    fn base(&self) -> &PieceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PieceBase {
        &mut self.base
    }

    fn bbox(&self) -> BoundingBox {
        let dy = self.settled.lock().unwrap_or_else(|e| e.into_inner()).map_or(0, |s| s.0);
        self.base.bbox.moved(0, dy, 0)
    }

    fn save(&self, t: &mut Compound) {
        t.insert("Width", self.size.0);
        t.insert("Height", self.size.1);
        t.insert("Depth", self.size.2);
        t.insert("HPos", self.settled.lock().unwrap_or_else(|e| e.into_inner()).map_or(-1, |s| s.1));
        let flag = |b: &AtomicBool| b.load(Ordering::Relaxed);
        match &self.kind {
            Kind::SwampHut { witch, cat } => {
                t.insert("Witch", flag(witch));
                t.insert("Cat", flag(cat));
            }
            Kind::JungleTemple(f) => {
                t.insert("placedMainChest", flag(&f.placed_main_chest));
                t.insert("placedHiddenChest", flag(&f.placed_hidden_chest));
                t.insert("placedTrap1", flag(&f.placed_trap1));
                t.insert("placedTrap2", flag(&f.placed_trap2));
            }
            Kind::DesertPyramid(chests) => {
                for (i, c) in chests.iter().enumerate() {
                    t.insert(format!("hasPlacedChest{i}"), flag(c));
                }
            }
        }
    }

    fn post_process(&self, ctx: &mut PlaceCtx) {
        if let Kind::DesertPyramid(chests) = &self.kind {
            let offset = -ctx.r.next_bounded(3);
            let b = self.lowest_ground_height(ctx, offset);
            desert_pyramid(&b, ctx, chests);
            return;
        }
        let Some(b) = self.average_ground_height(ctx, 0) else { return };
        match &self.kind {
            Kind::SwampHut { witch, cat } => {
                swamp_hut(&b, ctx);
                let p = b.world_pos(2, 2, 5);
                for (flag, id) in [(witch, "minecraft:witch"), (cat, "minecraft:cat")] {
                    if !flag.load(Ordering::Relaxed) && ctx.chunk_box.is_inside(p) {
                        flag.store(true, Ordering::Relaxed);
                        ctx.add_entity_nbt(mob(id, p));
                    }
                }
            }
            Kind::JungleTemple(flags) => jungle_temple(&b, ctx, flags),
            Kind::DesertPyramid(_) => {}
        }
    }
}

/// `SwampHutStructure`.
pub(crate) struct SwampHut;

impl StructureGen for SwampHut {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        let stub = ctx.on_top_of_chunk_center(Heightmap::WorldSurfaceWg)?;
        if !ctx.valid_biome(stub) {
            return None;
        }
        let dir = Dir::random_horizontal(&mut ctx.r);
        let kind = Kind::SwampHut { witch: AtomicBool::new(false), cat: AtomicBool::new(false) };
        Some(vec![Box::new(Scattered::new("minecraft:tesh", ctx.min_block_x(), 64, ctx.min_block_z(), 7, 7, 9, dir, kind))])
    }

    fn load_pieces(&self, _gen: &WorldGenerator, _x: i32, _z: i32, children: &[&Compound]) -> Result<Vec<Box<dyn Piece>>> {
        children
            .iter()
            .map(|t| Scattered::load(t, "minecraft:tesh", Kind::SwampHut { witch: AtomicBool::new(get_bool(t, "Witch")), cat: AtomicBool::new(get_bool(t, "Cat")) }))
            .collect()
    }
}

/// `SinglePieceStructure.findGenerationPoint`'s checks: the stub on top of the chunk center.
fn single_piece_stub(ctx: &mut GenCtx, width: i32, depth: i32) -> Option<Pos> {
    if !ctx.could_valid_biome_exist_on_top_of_chunk_center() {
        return None;
    }
    let (x, z) = (ctx.min_block_x(), ctx.min_block_z());
    if ctx.cols.lowest_y(x, z, width, depth) < ctx.gen.sea_level {
        return None;
    }
    let (mx, mz) = (ctx.middle_block_x(), ctx.middle_block_z());
    let p = Pos::new(mx, ctx.cols.first_occupied(mx, mz, Heightmap::WorldSurfaceWg), mz);
    ctx.valid_biome(p).then_some(p)
}

/// `JungleTempleStructure`.
pub(crate) struct JungleTemple;

impl StructureGen for JungleTemple {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        single_piece_stub(ctx, 12, 15)?;
        let dir = Dir::random_horizontal(&mut ctx.r);
        let flags = JungleFlags {
            placed_main_chest: AtomicBool::new(false),
            placed_hidden_chest: AtomicBool::new(false),
            placed_trap1: AtomicBool::new(false),
            placed_trap2: AtomicBool::new(false),
        };
        Some(vec![Box::new(Scattered::new("minecraft:tejp", ctx.min_block_x(), 64, ctx.min_block_z(), 12, 10, 15, dir, Kind::JungleTemple(flags)))])
    }

    fn load_pieces(&self, _gen: &WorldGenerator, _x: i32, _z: i32, children: &[&Compound]) -> Result<Vec<Box<dyn Piece>>> {
        children
            .iter()
            .map(|t| {
                let flags = JungleFlags {
                    placed_main_chest: AtomicBool::new(get_bool(t, "placedMainChest")),
                    placed_hidden_chest: AtomicBool::new(get_bool(t, "placedHiddenChest")),
                    placed_trap1: AtomicBool::new(get_bool(t, "placedTrap1")),
                    placed_trap2: AtomicBool::new(get_bool(t, "placedTrap2")),
                };
                Scattered::load(t, "minecraft:tejp", Kind::JungleTemple(flags))
            })
            .collect()
    }
}

fn swamp_hut(b: &PieceBase, ctx: &mut PlaceCtx) {
        b.generate_box(ctx, 1, 1, 1, 5, 1, 7, st("minecraft:spruce_planks"), st("minecraft:spruce_planks"), false);
        b.generate_box(ctx, 1, 4, 2, 5, 4, 7, st("minecraft:spruce_planks"), st("minecraft:spruce_planks"), false);
        b.generate_box(ctx, 2, 1, 0, 4, 1, 0, st("minecraft:spruce_planks"), st("minecraft:spruce_planks"), false);
        b.generate_box(ctx, 2, 2, 2, 3, 3, 2, st("minecraft:spruce_planks"), st("minecraft:spruce_planks"), false);
        b.generate_box(ctx, 1, 2, 3, 1, 3, 6, st("minecraft:spruce_planks"), st("minecraft:spruce_planks"), false);
        b.generate_box(ctx, 5, 2, 3, 5, 3, 6, st("minecraft:spruce_planks"), st("minecraft:spruce_planks"), false);
        b.generate_box(ctx, 2, 2, 7, 4, 3, 7, st("minecraft:spruce_planks"), st("minecraft:spruce_planks"), false);
        b.generate_box(ctx, 1, 0, 2, 1, 3, 2, st("minecraft:oak_log"), st("minecraft:oak_log"), false);
        b.generate_box(ctx, 5, 0, 2, 5, 3, 2, st("minecraft:oak_log"), st("minecraft:oak_log"), false);
        b.generate_box(ctx, 1, 0, 7, 1, 3, 7, st("minecraft:oak_log"), st("minecraft:oak_log"), false);
        b.generate_box(ctx, 5, 0, 7, 5, 3, 7, st("minecraft:oak_log"), st("minecraft:oak_log"), false);
        b.place_block(ctx, st("minecraft:oak_fence"), 2, 3, 2);
        b.place_block(ctx, st("minecraft:oak_fence"), 3, 3, 7);
        b.place_block(ctx, st("minecraft:air"), 1, 3, 4);
        b.place_block(ctx, st("minecraft:air"), 5, 3, 4);
        b.place_block(ctx, st("minecraft:air"), 5, 3, 5);
        b.place_block(ctx, st("minecraft:potted_red_mushroom"), 1, 3, 5);
        b.place_block(ctx, st("minecraft:crafting_table"), 3, 2, 6);
        b.place_block(ctx, st("minecraft:cauldron"), 4, 2, 6);
        b.place_block(ctx, st("minecraft:oak_fence"), 1, 2, 1);
        b.place_block(ctx, st("minecraft:oak_fence"), 5, 2, 1);
        let north = with_dir(st("minecraft:spruce_stairs"), "facing", Dir::North);
        let east = with_dir(st("minecraft:spruce_stairs"), "facing", Dir::East);
        let west = with_dir(st("minecraft:spruce_stairs"), "facing", Dir::West);
        let south = with_dir(st("minecraft:spruce_stairs"), "facing", Dir::South);
        b.generate_box(ctx, 0, 4, 1, 6, 4, 1, north, north, false);
        b.generate_box(ctx, 0, 4, 2, 0, 4, 7, east, east, false);
        b.generate_box(ctx, 6, 4, 2, 6, 4, 7, west, west, false);
        b.generate_box(ctx, 0, 4, 8, 6, 4, 8, south, south, false);
        b.place_block(ctx, with(north, "shape", "outer_right"), 0, 4, 1);
        b.place_block(ctx, with(north, "shape", "outer_left"), 6, 4, 1);
        b.place_block(ctx, with(south, "shape", "outer_left"), 0, 4, 8);
        b.place_block(ctx, with(south, "shape", "outer_right"), 6, 4, 8);
        for z in (2..=7).step_by(5) {
            for x in (1..=5).step_by(4) {
                b.fill_column_down(ctx, st("minecraft:oak_log"), x, -1, z);
            }
        }
}

fn jungle_temple(b: &PieceBase, ctx: &mut PlaceCtx, flags: &JungleFlags) {
    let mut sel = MossStone;
        b.generate_box_sel(ctx, 2, 1, 2, 9, 2, 2, false, &mut sel);
        b.generate_box_sel(ctx, 2, 1, 12, 9, 2, 12, false, &mut sel);
        b.generate_box_sel(ctx, 2, 1, 3, 2, 2, 11, false, &mut sel);
        b.generate_box_sel(ctx, 9, 1, 3, 9, 2, 11, false, &mut sel);
        b.generate_box_sel(ctx, 1, 3, 1, 10, 6, 1, false, &mut sel);
        b.generate_box_sel(ctx, 1, 3, 13, 10, 6, 13, false, &mut sel);
        b.generate_box_sel(ctx, 1, 3, 2, 1, 6, 12, false, &mut sel);
        b.generate_box_sel(ctx, 10, 3, 2, 10, 6, 12, false, &mut sel);
        b.generate_box_sel(ctx, 2, 3, 2, 9, 3, 12, false, &mut sel);
        b.generate_box_sel(ctx, 2, 6, 2, 9, 6, 12, false, &mut sel);
        b.generate_box_sel(ctx, 3, 7, 3, 8, 7, 11, false, &mut sel);
        b.generate_box_sel(ctx, 4, 8, 4, 7, 8, 10, false, &mut sel);
        b.generate_air_box(ctx, 3, 1, 3, 8, 2, 11);
        b.generate_air_box(ctx, 4, 3, 6, 7, 3, 9);
        b.generate_air_box(ctx, 2, 4, 2, 9, 5, 12);
        b.generate_air_box(ctx, 4, 6, 5, 7, 6, 9);
        b.generate_air_box(ctx, 5, 7, 6, 6, 7, 8);
        b.generate_air_box(ctx, 5, 1, 2, 6, 2, 2);
        b.generate_air_box(ctx, 5, 2, 12, 6, 2, 12);
        b.generate_air_box(ctx, 5, 5, 1, 6, 5, 1);
        b.generate_air_box(ctx, 5, 5, 13, 6, 5, 13);
        b.place_block(ctx, st("minecraft:air"), 1, 5, 5);
        b.place_block(ctx, st("minecraft:air"), 10, 5, 5);
        b.place_block(ctx, st("minecraft:air"), 1, 5, 9);
        b.place_block(ctx, st("minecraft:air"), 10, 5, 9);
        for x_z in (0..=14).step_by(14) {
            b.generate_box_sel(ctx, 2, 4, x_z, 2, 5, x_z, false, &mut sel);
            b.generate_box_sel(ctx, 4, 4, x_z, 4, 5, x_z, false, &mut sel);
            b.generate_box_sel(ctx, 7, 4, x_z, 7, 5, x_z, false, &mut sel);
            b.generate_box_sel(ctx, 9, 4, x_z, 9, 5, x_z, false, &mut sel);
        }
        b.generate_box_sel(ctx, 5, 6, 0, 6, 6, 0, false, &mut sel);
        for x_z in (0..=11).step_by(11) {
            for z2 in (2..=12).step_by(2) {
                b.generate_box_sel(ctx, x_z, 4, z2, x_z, 5, z2, false, &mut sel);
            }
            b.generate_box_sel(ctx, x_z, 6, 5, x_z, 6, 5, false, &mut sel);
            b.generate_box_sel(ctx, x_z, 6, 9, x_z, 6, 9, false, &mut sel);
        }
        b.generate_box_sel(ctx, 2, 7, 2, 2, 9, 2, false, &mut sel);
        b.generate_box_sel(ctx, 9, 7, 2, 9, 9, 2, false, &mut sel);
        b.generate_box_sel(ctx, 2, 7, 12, 2, 9, 12, false, &mut sel);
        b.generate_box_sel(ctx, 9, 7, 12, 9, 9, 12, false, &mut sel);
        b.generate_box_sel(ctx, 4, 9, 4, 4, 9, 4, false, &mut sel);
        b.generate_box_sel(ctx, 7, 9, 4, 7, 9, 4, false, &mut sel);
        b.generate_box_sel(ctx, 4, 9, 10, 4, 9, 10, false, &mut sel);
        b.generate_box_sel(ctx, 7, 9, 10, 7, 9, 10, false, &mut sel);
        b.generate_box_sel(ctx, 5, 9, 7, 6, 9, 7, false, &mut sel);
        let x_z = with_dir(st("minecraft:cobblestone_stairs"), "facing", Dir::East);
        let z2 = with_dir(st("minecraft:cobblestone_stairs"), "facing", Dir::West);
        let south_stairs = with_dir(st("minecraft:cobblestone_stairs"), "facing", Dir::South);
        let north_stairs = with_dir(st("minecraft:cobblestone_stairs"), "facing", Dir::North);
        b.place_block(ctx, north_stairs, 5, 9, 6);
        b.place_block(ctx, north_stairs, 6, 9, 6);
        b.place_block(ctx, south_stairs, 5, 9, 8);
        b.place_block(ctx, south_stairs, 6, 9, 8);
        b.place_block(ctx, north_stairs, 4, 0, 0);
        b.place_block(ctx, north_stairs, 5, 0, 0);
        b.place_block(ctx, north_stairs, 6, 0, 0);
        b.place_block(ctx, north_stairs, 7, 0, 0);
        b.place_block(ctx, north_stairs, 4, 1, 8);
        b.place_block(ctx, north_stairs, 4, 2, 9);
        b.place_block(ctx, north_stairs, 4, 3, 10);
        b.place_block(ctx, north_stairs, 7, 1, 8);
        b.place_block(ctx, north_stairs, 7, 2, 9);
        b.place_block(ctx, north_stairs, 7, 3, 10);
        b.generate_box_sel(ctx, 4, 1, 9, 4, 1, 9, false, &mut sel);
        b.generate_box_sel(ctx, 7, 1, 9, 7, 1, 9, false, &mut sel);
        b.generate_box_sel(ctx, 4, 1, 10, 7, 2, 10, false, &mut sel);
        b.generate_box_sel(ctx, 5, 4, 5, 6, 4, 5, false, &mut sel);
        b.place_block(ctx, x_z, 4, 4, 5);
        b.place_block(ctx, z2, 7, 4, 5);
        for i in 0..4 {
            b.place_block(ctx, south_stairs, 5, 0 - i, 6 + i);
            b.place_block(ctx, south_stairs, 6, 0 - i, 6 + i);
            b.generate_air_box(ctx, 5, 0 - i, 7 + i, 6, 0 - i, 9 + i);
        }
        b.generate_air_box(ctx, 1, -3, 12, 10, -1, 13);
        b.generate_air_box(ctx, 1, -3, 1, 3, -1, 13);
        b.generate_air_box(ctx, 1, -3, 1, 9, -1, 5);
        for i in (1..=13).step_by(2) {
            b.generate_box_sel(ctx, 1, -3, i, 1, -2, i, false, &mut sel);
        }
        for i in (2..=12).step_by(2) {
            b.generate_box_sel(ctx, 1, -1, i, 3, -1, i, false, &mut sel);
        }
        b.generate_box_sel(ctx, 2, -2, 1, 5, -2, 1, false, &mut sel);
        b.generate_box_sel(ctx, 7, -2, 1, 9, -2, 1, false, &mut sel);
        b.generate_box_sel(ctx, 6, -3, 1, 6, -3, 1, false, &mut sel);
        b.generate_box_sel(ctx, 6, -1, 1, 6, -1, 1, false, &mut sel);
        b.place_block(ctx, with_bool(with_dir(st("minecraft:tripwire_hook"), "facing", Dir::East), "attached", true), 1, -3, 8);
        b.place_block(ctx, with_bool(with_dir(st("minecraft:tripwire_hook"), "facing", Dir::West), "attached", true), 4, -3, 8);
        b.place_block(ctx, with_bool(with_bool(with_bool(st("minecraft:tripwire"), "attached", true), "east", true), "west", true), 2, -3, 8);
        b.place_block(ctx, with_bool(with_bool(with_bool(st("minecraft:tripwire"), "attached", true), "east", true), "west", true), 3, -3, 8);
        let i = with(with(st("minecraft:redstone_wire"), "north", "side"), "south", "side");
        b.place_block(ctx, i, 5, -3, 7);
        b.place_block(ctx, i, 5, -3, 6);
        b.place_block(ctx, i, 5, -3, 5);
        b.place_block(ctx, i, 5, -3, 4);
        b.place_block(ctx, i, 5, -3, 3);
        b.place_block(ctx, i, 5, -3, 2);
        b.place_block(ctx, with(with(st("minecraft:redstone_wire"), "north", "side"), "west", "side"), 5, -3, 1);
        b.place_block(ctx, with(with(st("minecraft:redstone_wire"), "east", "side"), "west", "side"), 4, -3, 1);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 3, -3, 1);
        if !flags.placed_trap1.load(Ordering::Relaxed) {
            flags.placed_trap1.store(b.create_dispenser(ctx, 3, -2, 1, Dir::North, "minecraft:chests/jungle_temple_dispenser"), Ordering::Relaxed);
        }
        b.place_block(ctx, with_bool(st("minecraft:vine"), "south", true), 3, -2, 2);
        b.place_block(ctx, with_bool(with_dir(st("minecraft:tripwire_hook"), "facing", Dir::North), "attached", true), 7, -3, 1);
        b.place_block(ctx, with_bool(with_dir(st("minecraft:tripwire_hook"), "facing", Dir::South), "attached", true), 7, -3, 5);
        b.place_block(ctx, with_bool(with_bool(with_bool(st("minecraft:tripwire"), "attached", true), "north", true), "south", true), 7, -3, 2);
        b.place_block(ctx, with_bool(with_bool(with_bool(st("minecraft:tripwire"), "attached", true), "north", true), "south", true), 7, -3, 3);
        b.place_block(ctx, with_bool(with_bool(with_bool(st("minecraft:tripwire"), "attached", true), "north", true), "south", true), 7, -3, 4);
        b.place_block(ctx, with(with(st("minecraft:redstone_wire"), "east", "side"), "west", "side"), 8, -3, 6);
        b.place_block(ctx, with(with(st("minecraft:redstone_wire"), "west", "side"), "south", "side"), 9, -3, 6);
        b.place_block(ctx, with(with(st("minecraft:redstone_wire"), "north", "side"), "south", "up"), 9, -3, 5);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 9, -3, 4);
        b.place_block(ctx, i, 9, -2, 4);
        if !flags.placed_trap2.load(Ordering::Relaxed) {
            flags.placed_trap2.store(b.create_dispenser(ctx, 9, -2, 3, Dir::West, "minecraft:chests/jungle_temple_dispenser"), Ordering::Relaxed);
        }
        b.place_block(ctx, with_bool(st("minecraft:vine"), "east", true), 8, -1, 3);
        b.place_block(ctx, with_bool(st("minecraft:vine"), "east", true), 8, -2, 3);
        if !flags.placed_main_chest.load(Ordering::Relaxed) {
            flags.placed_main_chest.store(b.create_chest(ctx, 8, -3, 3, "minecraft:chests/jungle_temple"), Ordering::Relaxed);
        }
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 9, -3, 2);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 8, -3, 1);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 4, -3, 5);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 5, -2, 5);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 5, -1, 5);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 6, -3, 5);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 7, -2, 5);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 7, -1, 5);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 8, -3, 5);
        b.generate_box_sel(ctx, 9, -1, 1, 9, -1, 5, false, &mut sel);
        b.generate_air_box(ctx, 8, -3, 8, 10, -1, 10);
        b.place_block(ctx, st("minecraft:chiseled_stone_bricks"), 8, -2, 11);
        b.place_block(ctx, st("minecraft:chiseled_stone_bricks"), 9, -2, 11);
        b.place_block(ctx, st("minecraft:chiseled_stone_bricks"), 10, -2, 11);
        let lever = with(with_dir(st("minecraft:lever"), "facing", Dir::North), "face", "wall");
        b.place_block(ctx, lever, 8, -2, 12);
        b.place_block(ctx, lever, 9, -2, 12);
        b.place_block(ctx, lever, 10, -2, 12);
        b.generate_box_sel(ctx, 8, -3, 8, 8, -3, 10, false, &mut sel);
        b.generate_box_sel(ctx, 10, -3, 8, 10, -3, 10, false, &mut sel);
        b.place_block(ctx, st("minecraft:mossy_cobblestone"), 10, -2, 9);
        b.place_block(ctx, i, 8, -2, 9);
        b.place_block(ctx, i, 8, -2, 10);
        b.place_block(ctx, with(with(with(with(st("minecraft:redstone_wire"), "north", "side"), "south", "side"), "east", "side"), "west", "side"), 10, -1, 9);
        b.place_block(ctx, with_dir(st("minecraft:sticky_piston"), "facing", Dir::Up), 9, -2, 8);
        b.place_block(ctx, with_dir(st("minecraft:sticky_piston"), "facing", Dir::West), 10, -2, 8);
        b.place_block(ctx, with_dir(st("minecraft:sticky_piston"), "facing", Dir::West), 10, -1, 8);
        b.place_block(ctx, with_dir(st("minecraft:repeater"), "facing", Dir::North), 10, -2, 10);
        if !flags.placed_hidden_chest.load(Ordering::Relaxed) {
            flags.placed_hidden_chest.store(b.create_chest(ctx, 9, -3, 10, "minecraft:chests/jungle_temple"), Ordering::Relaxed);
        }
}

fn desert_pyramid(b: &PieceBase, ctx: &mut PlaceCtx, chests: &[AtomicBool; 4]) {
    let orange = st("minecraft:orange_terracotta");
        b.generate_box(ctx, 0, -4, 0, 20, 0, 20, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        for n in 1..=9 {
            b.generate_box(ctx, n, n, n, 20 - n, n, 20 - n, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
            b.generate_box(ctx, n + 1, n, n + 1, 19 - n, n, 19 - n, st("minecraft:air"), st("minecraft:air"), false);
        }
        for n in 0..21 {
            for s in 0..21 {
                b.fill_column_down(ctx, st("minecraft:sandstone"), n, -5, s);
            }
        }
        let n = with_dir(st("minecraft:sandstone_stairs"), "facing", Dir::North);
        let s = with_dir(st("minecraft:sandstone_stairs"), "facing", Dir::South);
        let e = with_dir(st("minecraft:sandstone_stairs"), "facing", Dir::East);
        let w = with_dir(st("minecraft:sandstone_stairs"), "facing", Dir::West);
        b.generate_box(ctx, 0, 0, 0, 4, 9, 4, st("minecraft:sandstone"), st("minecraft:air"), false);
        b.generate_box(ctx, 1, 10, 1, 3, 10, 3, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.place_block(ctx, n, 2, 10, 0);
        b.place_block(ctx, s, 2, 10, 4);
        b.place_block(ctx, e, 0, 10, 2);
        b.place_block(ctx, w, 4, 10, 2);
        b.generate_box(ctx, 16, 0, 0, 20, 9, 4, st("minecraft:sandstone"), st("minecraft:air"), false);
        b.generate_box(ctx, 17, 10, 1, 19, 10, 3, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.place_block(ctx, n, 18, 10, 0);
        b.place_block(ctx, s, 18, 10, 4);
        b.place_block(ctx, e, 16, 10, 2);
        b.place_block(ctx, w, 20, 10, 2);
        b.generate_box(ctx, 8, 0, 0, 12, 4, 4, st("minecraft:sandstone"), st("minecraft:air"), false);
        b.generate_box(ctx, 9, 1, 0, 11, 3, 4, st("minecraft:air"), st("minecraft:air"), false);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 9, 1, 1);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 9, 2, 1);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 9, 3, 1);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 10, 3, 1);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 11, 3, 1);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 11, 2, 1);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 11, 1, 1);
        b.generate_box(ctx, 4, 1, 1, 8, 3, 3, st("minecraft:sandstone"), st("minecraft:air"), false);
        b.generate_box(ctx, 4, 1, 2, 8, 2, 2, st("minecraft:air"), st("minecraft:air"), false);
        b.generate_box(ctx, 12, 1, 1, 16, 3, 3, st("minecraft:sandstone"), st("minecraft:air"), false);
        b.generate_box(ctx, 12, 1, 2, 16, 2, 2, st("minecraft:air"), st("minecraft:air"), false);
        b.generate_box(ctx, 5, 4, 5, 15, 4, 15, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.generate_box(ctx, 9, 4, 9, 11, 4, 11, st("minecraft:air"), st("minecraft:air"), false);
        b.generate_box(ctx, 8, 1, 8, 8, 3, 8, st("minecraft:cut_sandstone"), st("minecraft:cut_sandstone"), false);
        b.generate_box(ctx, 12, 1, 8, 12, 3, 8, st("minecraft:cut_sandstone"), st("minecraft:cut_sandstone"), false);
        b.generate_box(ctx, 8, 1, 12, 8, 3, 12, st("minecraft:cut_sandstone"), st("minecraft:cut_sandstone"), false);
        b.generate_box(ctx, 12, 1, 12, 12, 3, 12, st("minecraft:cut_sandstone"), st("minecraft:cut_sandstone"), false);
        b.generate_box(ctx, 1, 1, 5, 4, 4, 11, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.generate_box(ctx, 16, 1, 5, 19, 4, 11, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.generate_box(ctx, 6, 7, 9, 6, 7, 11, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.generate_box(ctx, 14, 7, 9, 14, 7, 11, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.generate_box(ctx, 5, 5, 9, 5, 7, 11, st("minecraft:cut_sandstone"), st("minecraft:cut_sandstone"), false);
        b.generate_box(ctx, 15, 5, 9, 15, 7, 11, st("minecraft:cut_sandstone"), st("minecraft:cut_sandstone"), false);
        b.place_block(ctx, st("minecraft:air"), 5, 5, 10);
        b.place_block(ctx, st("minecraft:air"), 5, 6, 10);
        b.place_block(ctx, st("minecraft:air"), 6, 6, 10);
        b.place_block(ctx, st("minecraft:air"), 15, 5, 10);
        b.place_block(ctx, st("minecraft:air"), 15, 6, 10);
        b.place_block(ctx, st("minecraft:air"), 14, 6, 10);
        b.generate_box(ctx, 2, 4, 4, 2, 6, 4, st("minecraft:air"), st("minecraft:air"), false);
        b.generate_box(ctx, 18, 4, 4, 18, 6, 4, st("minecraft:air"), st("minecraft:air"), false);
        b.place_block(ctx, n, 2, 4, 5);
        b.place_block(ctx, n, 2, 3, 4);
        b.place_block(ctx, n, 18, 4, 5);
        b.place_block(ctx, n, 18, 3, 4);
        b.generate_box(ctx, 1, 1, 3, 2, 2, 3, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.generate_box(ctx, 18, 1, 3, 19, 2, 3, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.place_block(ctx, st("minecraft:sandstone"), 1, 1, 2);
        b.place_block(ctx, st("minecraft:sandstone"), 19, 1, 2);
        b.place_block(ctx, st("minecraft:sandstone_slab"), 1, 2, 2);
        b.place_block(ctx, st("minecraft:sandstone_slab"), 19, 2, 2);
        b.place_block(ctx, w, 2, 1, 2);
        b.place_block(ctx, e, 18, 1, 2);
        b.generate_box(ctx, 4, 3, 5, 4, 3, 17, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.generate_box(ctx, 16, 3, 5, 16, 3, 17, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.generate_box(ctx, 3, 1, 5, 4, 2, 16, st("minecraft:air"), st("minecraft:air"), false);
        b.generate_box(ctx, 15, 1, 5, 16, 2, 16, st("minecraft:air"), st("minecraft:air"), false);
        for x_z in (5..=17).step_by(2) {
            b.place_block(ctx, st("minecraft:cut_sandstone"), 4, 1, x_z);
            b.place_block(ctx, st("minecraft:chiseled_sandstone"), 4, 2, x_z);
            b.place_block(ctx, st("minecraft:cut_sandstone"), 16, 1, x_z);
            b.place_block(ctx, st("minecraft:chiseled_sandstone"), 16, 2, x_z);
        }
        b.place_block(ctx, orange, 10, 0, 7);
        b.place_block(ctx, orange, 10, 0, 8);
        b.place_block(ctx, orange, 9, 0, 9);
        b.place_block(ctx, orange, 11, 0, 9);
        b.place_block(ctx, orange, 8, 0, 10);
        b.place_block(ctx, orange, 12, 0, 10);
        b.place_block(ctx, orange, 7, 0, 10);
        b.place_block(ctx, orange, 13, 0, 10);
        b.place_block(ctx, orange, 9, 0, 11);
        b.place_block(ctx, orange, 11, 0, 11);
        b.place_block(ctx, orange, 10, 0, 12);
        b.place_block(ctx, orange, 10, 0, 13);
        b.place_block(ctx, st("minecraft:blue_terracotta"), 10, 0, 10);
        let mut x_z = 0;
        while x_z <= 20 {
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 2, 1);
            b.place_block(ctx, orange, x_z, 2, 2);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 2, 3);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 3, 1);
            b.place_block(ctx, orange, x_z, 3, 2);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 3, 3);
            b.place_block(ctx, orange, x_z, 4, 1);
            b.place_block(ctx, st("minecraft:chiseled_sandstone"), x_z, 4, 2);
            b.place_block(ctx, orange, x_z, 4, 3);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 5, 1);
            b.place_block(ctx, orange, x_z, 5, 2);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 5, 3);
            b.place_block(ctx, orange, x_z, 6, 1);
            b.place_block(ctx, st("minecraft:chiseled_sandstone"), x_z, 6, 2);
            b.place_block(ctx, orange, x_z, 6, 3);
            b.place_block(ctx, orange, x_z, 7, 1);
            b.place_block(ctx, orange, x_z, 7, 2);
            b.place_block(ctx, orange, x_z, 7, 3);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 8, 1);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 8, 2);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 8, 3);
            x_z += 20;
        }
        let mut x_z = 2;
        while x_z <= 18 {
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z - 1, 2, 0);
            b.place_block(ctx, orange, x_z, 2, 0);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z + 1, 2, 0);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z - 1, 3, 0);
            b.place_block(ctx, orange, x_z, 3, 0);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z + 1, 3, 0);
            b.place_block(ctx, orange, x_z - 1, 4, 0);
            b.place_block(ctx, st("minecraft:chiseled_sandstone"), x_z, 4, 0);
            b.place_block(ctx, orange, x_z + 1, 4, 0);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z - 1, 5, 0);
            b.place_block(ctx, orange, x_z, 5, 0);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z + 1, 5, 0);
            b.place_block(ctx, orange, x_z - 1, 6, 0);
            b.place_block(ctx, st("minecraft:chiseled_sandstone"), x_z, 6, 0);
            b.place_block(ctx, orange, x_z + 1, 6, 0);
            b.place_block(ctx, orange, x_z - 1, 7, 0);
            b.place_block(ctx, orange, x_z, 7, 0);
            b.place_block(ctx, orange, x_z + 1, 7, 0);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z - 1, 8, 0);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z, 8, 0);
            b.place_block(ctx, st("minecraft:cut_sandstone"), x_z + 1, 8, 0);
            x_z += 18 - 2;
        }
        b.generate_box(ctx, 8, 4, 0, 12, 6, 0, st("minecraft:cut_sandstone"), st("minecraft:cut_sandstone"), false);
        b.place_block(ctx, st("minecraft:air"), 8, 6, 0);
        b.place_block(ctx, st("minecraft:air"), 12, 6, 0);
        b.place_block(ctx, orange, 9, 5, 0);
        b.place_block(ctx, st("minecraft:chiseled_sandstone"), 10, 5, 0);
        b.place_block(ctx, orange, 11, 5, 0);
        b.generate_box(ctx, 8, -14, 8, 12, -11, 12, st("minecraft:cut_sandstone"), st("minecraft:cut_sandstone"), false);
        b.generate_box(ctx, 8, -10, 8, 12, -10, 12, st("minecraft:chiseled_sandstone"), st("minecraft:chiseled_sandstone"), false);
        b.generate_box(ctx, 8, -9, 8, 12, -9, 12, st("minecraft:cut_sandstone"), st("minecraft:cut_sandstone"), false);
        b.generate_box(ctx, 8, -8, 8, 12, -1, 12, st("minecraft:sandstone"), st("minecraft:sandstone"), false);
        b.generate_box(ctx, 9, -11, 9, 11, -1, 11, st("minecraft:air"), st("minecraft:air"), false);
        b.place_block(ctx, st("minecraft:stone_pressure_plate"), 10, -11, 10);
        b.generate_box(ctx, 9, -13, 9, 11, -13, 11, st("minecraft:tnt"), st("minecraft:air"), false);
        b.place_block(ctx, st("minecraft:air"), 8, -11, 10);
        b.place_block(ctx, st("minecraft:air"), 8, -10, 10);
        b.place_block(ctx, st("minecraft:chiseled_sandstone"), 7, -10, 10);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 7, -11, 10);
        b.place_block(ctx, st("minecraft:air"), 12, -11, 10);
        b.place_block(ctx, st("minecraft:air"), 12, -10, 10);
        b.place_block(ctx, st("minecraft:chiseled_sandstone"), 13, -10, 10);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 13, -11, 10);
        b.place_block(ctx, st("minecraft:air"), 10, -11, 8);
        b.place_block(ctx, st("minecraft:air"), 10, -10, 8);
        b.place_block(ctx, st("minecraft:chiseled_sandstone"), 10, -10, 7);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 10, -11, 7);
        b.place_block(ctx, st("minecraft:air"), 10, -11, 12);
        b.place_block(ctx, st("minecraft:air"), 10, -10, 12);
        b.place_block(ctx, st("minecraft:chiseled_sandstone"), 10, -10, 13);
        b.place_block(ctx, st("minecraft:cut_sandstone"), 10, -11, 13);
        for direction in Dir::HORIZONTAL {
            let flag = &chests[data_2d(direction)];
            if !flag.load(Ordering::Relaxed) {
                let (dx, _, dz) = direction.offset();
                flag.store(b.create_chest(ctx, 10 + dx * 2, -11, 10 + dz * 2, "minecraft:chests/desert_pyramid"), Ordering::Relaxed);
            }
        }
        cellar_stairs(b, ctx);
        cellar_room(b, ctx);
}

/// The center of the cellar room (`addCellar`).
const CELLAR: (i32, i32, i32) = (16, -4, 13);

/// `addCellarStairs`.
fn cellar_stairs(b: &PieceBase, ctx: &mut PlaceCtx) {
    let (x, y, z) = CELLAR;
    let stairs = super::transform::rotate_state(st("minecraft:sandstone_stairs"), super::transform::Rotation::CounterClockwise90);
    b.place_block(ctx, stairs, 13, -1, 17);
    b.place_block(ctx, stairs, 14, -2, 17);
    b.place_block(ctx, stairs, 15, -3, 17);
    let (sand, sandstone) = (st("minecraft:sand"), st("minecraft:sandstone"));
    let mut r = ctx.w.region_random();
    let variant = r.next_bool();
    ctx.w.store_region_random(r);
    for dx in [-4, -3, -2, -1, 0] {
        b.place_block(ctx, sand, x + dx, y + 4, z + 4);
    }
    b.place_block(ctx, sand, x - 2, y + 3, z + 4);
    b.place_block(ctx, if variant { sand } else { sandstone }, x - 1, y + 3, z + 4);
    b.place_block(ctx, if !variant { sand } else { sandstone }, x, y + 3, z + 4);
    b.place_block(ctx, sand, x - 1, y + 2, z + 4);
    b.place_block(ctx, sandstone, x, y + 2, z + 4);
    b.place_block(ctx, sand, x, y + 1, z + 4);
}

/// `addCellarRoom`.
fn cellar_room(b: &PieceBase, ctx: &mut PlaceCtx) {
    let (x, y, z) = CELLAR;
    let _cut = st("minecraft:cut_sandstone");
    let _chiseled = st("minecraft:chiseled_sandstone");
    let orange = st("minecraft:orange_terracotta");
    let _blue = st("minecraft:blue_terracotta");
        let cut = st("minecraft:cut_sandstone");
        let chiseled = st("minecraft:chiseled_sandstone");
        b.generate_box(ctx, x - 3, y + 1, z - 3, x - 3, y + 1, z + 2, cut, cut, true);
        b.generate_box(ctx, x + 3, y + 1, z - 3, x + 3, y + 1, z + 2, cut, cut, true);
        b.generate_box(ctx, x - 3, y + 1, z - 3, x + 3, y + 1, z - 2, cut, cut, true);
        b.generate_box(ctx, x - 3, y + 1, z + 3, x + 3, y + 1, z + 3, cut, cut, true);
        b.generate_box(ctx, x - 3, y + 2, z - 3, x - 3, y + 2, z + 2, chiseled, chiseled, true);
        b.generate_box(ctx, x + 3, y + 2, z - 3, x + 3, y + 2, z + 2, chiseled, chiseled, true);
        b.generate_box(ctx, x - 3, y + 2, z - 3, x + 3, y + 2, z - 2, chiseled, chiseled, true);
        b.generate_box(ctx, x - 3, y + 2, z + 3, x + 3, y + 2, z + 3, chiseled, chiseled, true);
        b.generate_box(ctx, x - 3, -1, z - 3, x - 3, -1, z + 2, cut, cut, true);
        b.generate_box(ctx, x + 3, -1, z - 3, x + 3, -1, z + 2, cut, cut, true);
        b.generate_box(ctx, x - 3, -1, z - 3, x + 3, -1, z - 2, cut, cut, true);
        b.generate_box(ctx, x - 3, -1, z + 3, x + 3, -1, z + 3, cut, cut, true);
        // The sand box and collapsed roof (`placeSandBox`, `placeCollapsedRoof`).
        collapsed_roof(b, ctx, x - 2, y + 4, z - 2, x + 2, z + 2);
                let blue = st("minecraft:blue_terracotta");
        b.place_block(ctx, blue, x, y, z);
        b.place_block(ctx, orange, x + 1, y, z - 1);
        b.place_block(ctx, orange, x + 1, y, z + 1);
        b.place_block(ctx, orange, x - 1, y, z - 1);
        b.place_block(ctx, orange, x - 1, y, z + 1);
        b.place_block(ctx, orange, x + 2, y, z);
        b.place_block(ctx, orange, x - 2, y, z);
        b.place_block(ctx, orange, x, y, z + 2);
        b.place_block(ctx, orange, x, y, z - 2);
        b.place_block(ctx, orange, x + 3, y, z);
        b.place_block(ctx, cut, x + 4, y + 1, z);
        b.place_block(ctx, chiseled, x + 4, y + 2, z);
        b.place_block(ctx, orange, x - 3, y, z);
        b.place_block(ctx, cut, x - 4, y + 1, z);
        b.place_block(ctx, chiseled, x - 4, y + 2, z);
        b.place_block(ctx, orange, x, y, z + 3);
        b.place_block(ctx, orange, x, y, z - 3);
        b.place_block(ctx, cut, x, y + 1, z - 4);
        b.place_block(ctx, chiseled, x, -2, z - 4);
}

/// `placeCollapsedRoof`: sandstone or sand from the region's random.
fn collapsed_roof(b: &PieceBase, ctx: &mut PlaceCtx, x0: i32, y0: i32, z0: i32, x1: i32, z1: i32) {
    let mut r = ctx.w.region_random();
    for x in x0..=x1 {
        for z in z0..=z1 {
            let state = if r.next_f32() < 0.33 { st("minecraft:sandstone") } else { st("minecraft:sand") };
            b.place_block(ctx, state, x, y0, z);
        }
    }
    ctx.w.store_region_random(r);
}

/// The positions `placeSand` records for the suspicious sand (`placeSandBox` and the sand
/// next to the cellar room), in order.
fn sand_positions(b: &PieceBase) -> Vec<Pos> {
    let (x, y, z) = CELLAR;
    let mut out = Vec::new();
    for yy in y + 1..=y + 3 {
        for xx in x - 2..=x + 2 {
            for zz in z - 2..=z + 2 {
                out.push(b.world_pos(xx, yy, zz));
            }
        }
    }
    for (dx, dz) in [(3, 0), (-3, 0), (0, 3), (0, -3)] {
        out.push(b.world_pos(x + dx, y + 1, z + dz));
        out.push(b.world_pos(x + dx, y + 2, z + dz));
    }
    out
}

/// `randomCollapsedRoofPos`.
fn collapsed_roof_pos(b: &PieceBase, seed: i64) -> Pos {
    let (x, y, z) = CELLAR;
    let (x0, y0, z0, x1, z1) = (x - 2, y + 4, z - 2, x + 2, z + 2);
    let mut r = WorldgenRandom::legacy(seed);
    let f = r.fork_positional();
    let p = b.world_pos(x0, y0, z0);
    let mut r = WorldgenRandom::new(f.at(p.x, p.y, p.z));
    let rx = r.next_bounded(x1 - x0 + 1) + x0;
    let rz = r.next_bounded(z1 - z0 + 1) + z0;
    b.world_pos(rx, y0, rz)
}

/// `Direction.get2DDataValue`.
fn data_2d(d: Dir) -> usize {
    match d {
        Dir::South => 0,
        Dir::West => 1,
        Dir::North => 2,
        _ => 3,
    }
}

/// `BlockPos.asLong`.
fn pos_as_long(p: Pos) -> i64 {
    ((p.x as i64 & 0x3FF_FFFF) << 38) | ((p.z as i64 & 0x3FF_FFFF) << 12) | (p.y as i64 & 0xFFF)
}

/// `DesertPyramidStructure`.
pub(crate) struct DesertPyramid;

impl StructureGen for DesertPyramid {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        single_piece_stub(ctx, 21, 21)?;
        let dir = Dir::random_horizontal(&mut ctx.r);
        let kind = Kind::DesertPyramid(std::array::from_fn(|_| AtomicBool::new(false)));
        Some(vec![Box::new(Scattered::new("minecraft:tedp", ctx.min_block_x(), 64, ctx.min_block_z(), 21, 15, 21, dir, kind))])
    }

    fn load_pieces(&self, _gen: &WorldGenerator, _x: i32, _z: i32, children: &[&Compound]) -> Result<Vec<Box<dyn Piece>>> {
        children
            .iter()
            .map(|t| {
                let chests = std::array::from_fn(|i| AtomicBool::new(get_bool(t, &format!("hasPlacedChest{i}"))));
                Scattered::load(t, "minecraft:tedp", Kind::DesertPyramid(chests))
            })
            .collect()
    }

    /// `DesertPyramidStructure.afterPlace`: suspicious sand at 5 to 7 of the sand positions
    /// (and at the collapsed roof), plain sand at the others.
    fn after_place(&self, start: &super::StructureStart, ctx: &mut PlaceCtx) {
        let mut unique = std::collections::BTreeSet::new();
        let mut boxes = Vec::new();
        for p in &start.pieces {
            let mut base = p.base().clone();
            base.bbox = p.bbox();
            boxes.push(base.bbox);
            if base.kind != "minecraft:tedp" {
                continue;
            }
            for s in sand_positions(&base) {
                unique.insert((s.y, s.z, s.x));
            }
            let roof = collapsed_roof_pos(&base, ctx.w.seed());
            suspicious_sand(ctx, roof);
        }
        let Some(all) = BoundingBox::encapsulating(boxes) else { return };
        let mut list: Vec<Pos> = unique.into_iter().map(|(y, z, x)| Pos::new(x, y, z)).collect();
        let mut r = WorldgenRandom::legacy(ctx.w.seed());
        let f = r.fork_positional();
        let c = all.center();
        let mut r = WorldgenRandom::new(f.at(c.x, c.y, c.z));
        super::jigsaw::shuffle(&mut list, &mut r);
        let mut count = (list.len() as i32).min(5 + r.next_bounded(3));
        for p in list {
            if count > 0 {
                count -= 1;
                suspicious_sand(ctx, p);
            } else if ctx.chunk_box.is_inside(p) {
                ctx.w.set(p, st("minecraft:sand"));
            }
        }
    }
}

/// `DesertPyramidStructure.placeSuspiciousSand`.
fn suspicious_sand(ctx: &mut PlaceCtx, p: Pos) {
    if ctx.chunk_box.is_inside(p) {
        ctx.w.set(p, st("minecraft:suspicious_sand"));
        ctx.add_block_entity(p, crate::feature::BlockEntity::Loot { table: "minecraft:archaeology/desert_pyramid".into(), seed: pos_as_long(p) });
    }
}
