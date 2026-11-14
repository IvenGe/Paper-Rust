//! Swamp huts and jungle temples (`ScatteredFeaturePiece`s).
//!
//! Written to vanilla 26.3's `ScatteredFeaturePiece`, `SwampHutStructure`/`SwampHutPiece` and
//! `JungleTempleStructure`/`JungleTemplePiece`. The pieces settle their height when first
//! placed (from the heightmaps of that chunk), as vanilla does. The witch and cat of a swamp
//! hut are returned as entity NBT; vanilla's `finalizeSpawn` is left to the caller.

use super::bbox::BoundingBox;
use super::piece::{st, with, with_bool, with_dir, PieceBase, PlaceCtx, Selector};
use super::{GenCtx, Piece, StructureGen};
use crate::chunk::Heightmap;
use crate::feature::world::{Dir, Pos};
use crate::random::{Random, WorldgenRandom};
use paper_nbt::{Compound, Tag};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

enum Kind {
    SwampHut { witch: AtomicBool, cat: AtomicBool },
    JungleTemple(JungleFlags),
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
    /// The vertical move applied when the height was settled (`heightPosition`).
    settled: Mutex<Option<i32>>,
    kind: Kind,
}

impl Scattered {
    fn new(kind_id: &'static str, x: i32, y: i32, z: i32, width: i32, height: i32, depth: i32, dir: Dir, kind: Kind) -> Self {
        let base = PieceBase::new(kind_id, 0, BoundingBox::make(x, y, z, dir, width, height, depth)).with_orientation(Some(dir));
        Self { base, settled: Mutex::new(None), kind }
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
            *settled = Some(total / count - bb.min_y + offset);
        }
        let mut base = self.base.clone();
        base.bbox.move_by(0, settled.unwrap_or(0), 0);
        Some(base)
    }
}

/// A mob spawned by a structure, as entity NBT (`setPersistenceRequired`, at the block's
/// bottom center).
fn mob(id: &str, p: Pos) -> Compound {
    let mut nbt = Compound::new();
    nbt.insert("id", id);
    nbt.insert("Pos", Tag::List(vec![(p.x as f64 + 0.5).into(), (p.y as f64).into(), (p.z as f64 + 0.5).into()]));
    nbt.insert("Rotation", Tag::List(vec![0.0f32.into(), 0.0f32.into()]));
    nbt.insert("PersistenceRequired", true);
    nbt
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
        let dy = self.settled.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or(0);
        self.base.bbox.moved(0, dy, 0)
    }

    fn post_process(&self, ctx: &mut PlaceCtx) {
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
            b.place_block(ctx, south_stairs, 5, (0 - i), (6 + i));
            b.place_block(ctx, south_stairs, 6, (0 - i), (6 + i));
            b.generate_air_box(ctx, 5, (0 - i), (7 + i), 6, (0 - i), (9 + i));
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
