//! Mineshafts: a room with corridors, crossings and stairs branching from it, supports,
//! rails, cobwebs, cave spider spawners and chest minecarts.
//!
//! Written to vanilla 26.3's `MineshaftStructure` and `MineshaftPieces`.

use super::piece::{st, with, with_bool, with_dir, PieceBase, PlaceCtx};
use super::{BoundingBox, GenCtx, Piece, StructureGen};
use crate::chunk::Heightmap;
use crate::feature::world::{Dir, Pos};
use crate::random::{Random, WorldgenRandom};
use paper_data::blocks;
use paper_nbt::{Compound, Tag};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct Mineshaft {
    pub mesa: bool,
    /// `#minecraft:mineshaft_blocking`.
    pub blocking: u128,
}

#[derive(Clone, Copy)]
struct Wood {
    log: u32,
    planks: u32,
    fence: u32,
}

impl Wood {
    fn of(mesa: bool) -> Self {
        if mesa {
            Self { log: st("minecraft:dark_oak_log"), planks: st("minecraft:dark_oak_planks"), fence: st("minecraft:dark_oak_fence") }
        } else {
            Self { log: st("minecraft:oak_log"), planks: st("minecraft:oak_planks"), fence: st("minecraft:oak_fence") }
        }
    }
}

enum Kind {
    Room { entrances: Vec<BoundingBox> },
    Corridor { rails: bool, spider: bool, spider_placed: AtomicBool, sections: i32 },
    /// Not oriented: its coordinates are world coordinates.
    Crossing { direction: Dir, two_floors: bool },
    Stairs,
}

struct MsPiece {
    base: PieceBase,
    wood: Wood,
    blocking: u128,
    kind: Kind,
}

fn protect(wood: Wood) -> [u32; 4] {
    [wood.planks, wood.log, wood.fence, st("minecraft:iron_chain")].map(blocks::block_id_of_state)
}

struct Builder {
    pieces: Vec<MsPiece>,
    wood: Wood,
    blocking: u128,
}

impl Builder {
    fn collides(&self, b: &BoundingBox) -> Option<usize> {
        self.pieces.iter().position(|p| p.base.bbox.intersects(b))
    }

    fn piece(&self, kind_id: &'static str, depth: i32, bbox: BoundingBox, orientation: Option<Dir>, kind: Kind) -> MsPiece {
        let mut base = PieceBase::new(kind_id, depth, bbox).with_orientation(orientation);
        base.protect = Some(protect(self.wood));
        MsPiece { base, wood: self.wood, blocking: self.blocking, kind }
    }

    /// `createRandomShaftPiece`.
    #[allow(clippy::too_many_arguments)]
    fn random_piece(&self, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, d: Dir, depth: i32) -> Option<MsPiece> {
        let selection = r.next_bounded(100);
        if selection >= 80 {
            // MineShaftCrossing.findCrossing
            let y1 = if r.next_bounded(4) == 0 { 6 } else { 2 };
            let b = match d {
                Dir::South => BoundingBox::new(-1, 0, 0, 3, y1, 4),
                Dir::West => BoundingBox::new(-4, 0, -1, 0, y1, 3),
                Dir::East => BoundingBox::new(0, 0, -1, 4, y1, 3),
                _ => BoundingBox::new(-1, 0, -4, 3, y1, 0),
            }
            .moved(x, y, z);
            if self.collides(&b).is_some() {
                return None;
            }
            return Some(self.piece("minecraft:mscrossing", depth, b, None, Kind::Crossing { direction: d, two_floors: b.y_span() > 3 }));
        }
        if selection >= 70 {
            // MineShaftStairs.findStairs
            let b = match d {
                Dir::South => BoundingBox::new(0, -5, 0, 2, 2, 8),
                Dir::West => BoundingBox::new(-8, -5, 0, 0, 2, 2),
                Dir::East => BoundingBox::new(0, -5, 0, 8, 2, 2),
                _ => BoundingBox::new(0, -5, -8, 2, 2, 0),
            }
            .moved(x, y, z);
            if self.collides(&b).is_some() {
                return None;
            }
            return Some(self.piece("minecraft:msstairs", depth, b, Some(d), Kind::Stairs));
        }
        // MineShaftCorridor.findCorridorSize
        let mut len = r.next_bounded(3) + 2;
        while len > 0 {
            let l = len * 5;
            let b = match d {
                Dir::South => BoundingBox::new(0, 0, 0, 2, 2, l - 1),
                Dir::West => BoundingBox::new(-(l - 1), 0, 0, 0, 2, 2),
                Dir::East => BoundingBox::new(0, 0, 0, l - 1, 2, 2),
                _ => BoundingBox::new(0, 0, -(l - 1), 2, 2, 0),
            }
            .moved(x, y, z);
            if self.collides(&b).is_some() {
                len -= 1;
                continue;
            }
            let rails = r.next_bounded(3) == 0;
            let spider = !rails && r.next_bounded(23) == 0;
            let sections = if matches!(d, Dir::North | Dir::South) { b.z_span() / 5 } else { b.x_span() / 5 };
            return Some(self.piece("minecraft:mscorridor", depth, b, Some(d), Kind::Corridor { rails, spider, spider_placed: AtomicBool::new(false), sections }));
        }
        None
    }

    /// `MineshaftPieces.generateAndAddPiece`: children are generated depth first.
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, d: Dir, depth: i32) -> Option<BoundingBox> {
        if depth > 8 {
            return None;
        }
        let sb = self.pieces[0].base.bbox;
        if (x - sb.min_x).abs() > 80 || (z - sb.min_z).abs() > 80 {
            return None;
        }
        let p = self.random_piece(r, x, y, z, d, depth + 1)?;
        let b = p.base.bbox;
        self.pieces.push(p);
        let i = self.pieces.len() - 1;
        self.add_children(i, r);
        Some(b)
    }

    fn add_children(&mut self, i: usize, r: &mut WorldgenRandom) {
        let b = self.pieces[i].base.bbox;
        let depth = self.pieces[i].base.gen_depth;
        match &self.pieces[i].kind {
            Kind::Room { .. } => {
                let height = (b.y_span() - 3 - 1).max(1);
                let mut entrances = Vec::new();
                for side in 0..4 {
                    let span = if side < 2 { b.x_span() } else { b.z_span() };
                    let mut pos = 0;
                    while pos < span {
                        pos += r.next_bounded(span);
                        if pos + 3 > span {
                            break;
                        }
                        let y = b.min_y + r.next_bounded(height) + 1;
                        let child = match side {
                            0 => self.add(r, b.min_x + pos, y, b.min_z - 1, Dir::North, depth),
                            1 => self.add(r, b.min_x + pos, y, b.max_z + 1, Dir::South, depth),
                            2 => self.add(r, b.min_x - 1, y, b.min_z + pos, Dir::West, depth),
                            _ => self.add(r, b.max_x + 1, y, b.min_z + pos, Dir::East, depth),
                        };
                        if let Some(c) = child {
                            entrances.push(match side {
                                0 => BoundingBox::new(c.min_x, c.min_y, b.min_z, c.max_x, c.max_y, b.min_z + 1),
                                1 => BoundingBox::new(c.min_x, c.min_y, b.max_z - 1, c.max_x, c.max_y, b.max_z),
                                2 => BoundingBox::new(b.min_x, c.min_y, c.min_z, b.min_x + 1, c.max_y, c.max_z),
                                _ => BoundingBox::new(b.max_x - 1, c.min_y, c.min_z, b.max_x, c.max_y, c.max_z),
                            });
                        }
                        pos += 4;
                    }
                }
                if let Kind::Room { entrances: e } = &mut self.pieces[i].kind {
                    *e = entrances;
                }
            }
            Kind::Corridor { .. } => {
                let o = self.pieces[i].base.orientation.unwrap_or(Dir::North);
                let end = r.next_bounded(4);
                let y = || b.min_y - 1;
                match o {
                    Dir::South => {
                        if end <= 1 {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.min_x, yy, b.max_z + 1, o, depth);
                        } else if end == 2 {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.min_x - 1, yy, b.max_z - 3, Dir::West, depth);
                        } else {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.max_x + 1, yy, b.max_z - 3, Dir::East, depth);
                        }
                    }
                    Dir::West => {
                        if end <= 1 {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.min_x - 1, yy, b.min_z, o, depth);
                        } else if end == 2 {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.min_x, yy, b.min_z - 1, Dir::North, depth);
                        } else {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.min_x, yy, b.max_z + 1, Dir::South, depth);
                        }
                    }
                    Dir::East => {
                        if end <= 1 {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.max_x + 1, yy, b.min_z, o, depth);
                        } else if end == 2 {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.max_x - 3, yy, b.min_z - 1, Dir::North, depth);
                        } else {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.max_x - 3, yy, b.max_z + 1, Dir::South, depth);
                        }
                    }
                    _ => {
                        if end <= 1 {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.min_x, yy, b.min_z - 1, o, depth);
                        } else if end == 2 {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.min_x - 1, yy, b.min_z, Dir::West, depth);
                        } else {
                            let yy = y() + r.next_bounded(3);
                            self.add(r, b.max_x + 1, yy, b.min_z, Dir::East, depth);
                        }
                    }
                }
                if depth < 8 {
                    if matches!(o, Dir::North | Dir::South) {
                        let mut z = b.min_z + 3;
                        while z + 3 <= b.max_z {
                            match r.next_bounded(5) {
                                0 => {
                                    self.add(r, b.min_x - 1, b.min_y, z, Dir::West, depth + 1);
                                }
                                1 => {
                                    self.add(r, b.max_x + 1, b.min_y, z, Dir::East, depth + 1);
                                }
                                _ => {}
                            }
                            z += 5;
                        }
                    } else {
                        let mut x = b.min_x + 3;
                        while x + 3 <= b.max_x {
                            match r.next_bounded(5) {
                                0 => {
                                    self.add(r, x, b.min_y, b.min_z - 1, Dir::North, depth + 1);
                                }
                                1 => {
                                    self.add(r, x, b.min_y, b.max_z + 1, Dir::South, depth + 1);
                                }
                                _ => {}
                            }
                            x += 5;
                        }
                    }
                }
            }
            Kind::Crossing { direction, two_floors } => {
                let (direction, two_floors) = (*direction, *two_floors);
                match direction {
                    Dir::South => {
                        self.add(r, b.min_x + 1, b.min_y, b.max_z + 1, Dir::South, depth);
                        self.add(r, b.min_x - 1, b.min_y, b.min_z + 1, Dir::West, depth);
                        self.add(r, b.max_x + 1, b.min_y, b.min_z + 1, Dir::East, depth);
                    }
                    Dir::West => {
                        self.add(r, b.min_x + 1, b.min_y, b.min_z - 1, Dir::North, depth);
                        self.add(r, b.min_x + 1, b.min_y, b.max_z + 1, Dir::South, depth);
                        self.add(r, b.min_x - 1, b.min_y, b.min_z + 1, Dir::West, depth);
                    }
                    Dir::East => {
                        self.add(r, b.min_x + 1, b.min_y, b.min_z - 1, Dir::North, depth);
                        self.add(r, b.min_x + 1, b.min_y, b.max_z + 1, Dir::South, depth);
                        self.add(r, b.max_x + 1, b.min_y, b.min_z + 1, Dir::East, depth);
                    }
                    _ => {
                        self.add(r, b.min_x + 1, b.min_y, b.min_z - 1, Dir::North, depth);
                        self.add(r, b.min_x - 1, b.min_y, b.min_z + 1, Dir::West, depth);
                        self.add(r, b.max_x + 1, b.min_y, b.min_z + 1, Dir::East, depth);
                    }
                }
                if two_floors {
                    let y = b.min_y + 3 + 1;
                    if r.next_bool() {
                        self.add(r, b.min_x + 1, y, b.min_z - 1, Dir::North, depth);
                    }
                    if r.next_bool() {
                        self.add(r, b.min_x - 1, y, b.min_z + 1, Dir::West, depth);
                    }
                    if r.next_bool() {
                        self.add(r, b.max_x + 1, y, b.min_z + 1, Dir::East, depth);
                    }
                    if r.next_bool() {
                        self.add(r, b.min_x + 1, y, b.max_z + 1, Dir::South, depth);
                    }
                }
            }
            Kind::Stairs => {
                match self.pieces[i].base.orientation {
                    Some(Dir::South) => self.add(r, b.min_x, b.min_y, b.max_z + 1, Dir::South, depth),
                    Some(Dir::West) => self.add(r, b.min_x - 1, b.min_y, b.min_z, Dir::West, depth),
                    Some(Dir::East) => self.add(r, b.max_x + 1, b.min_y, b.min_z, Dir::East, depth),
                    _ => self.add(r, b.min_x, b.min_y, b.min_z - 1, Dir::North, depth),
                };
            }
        }
    }
}

impl StructureGen for Mineshaft {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        ctx.r.next_f64();
        let (bx, bz) = (ctx.min_block_x() + 2, ctx.min_block_z() + 2);
        let wood = Wood::of(self.mesa);
        let r = &mut ctx.r;
        let max_x = bx + 7 + r.next_bounded(6);
        let max_y = 54 + r.next_bounded(6);
        let max_z = bz + 7 + r.next_bounded(6);
        let mut b = Builder { pieces: Vec::new(), wood, blocking: self.blocking };
        let room = b.piece("minecraft:msroom", 0, BoundingBox::new(bx, 50, bz, max_x, max_y, max_z), None, Kind::Room { entrances: Vec::new() });
        b.pieces.push(room);
        b.add_children(0, r);
        let sea_level = ctx.gen.sea_level;
        let all = BoundingBox::encapsulating(b.pieces.iter().map(|p| p.base.bbox)).expect("room");
        let dy = if self.mesa {
            let c = all.center();
            let surface = ctx.cols.base_height(c.x, c.z, Heightmap::WorldSurfaceWg);
            let target = if surface <= sea_level { sea_level } else { ctx.r.next_bounded(surface - sea_level + 1) + sea_level };
            target - c.y
        } else {
            // moveBelowSeaLevel(seaLevel, minY, random, 10)
            let max_y = sea_level - 10;
            let mut y1 = all.y_span() + ctx.gen.gen_min_y + 1;
            if y1 < max_y {
                y1 += ctx.r.next_bounded(max_y - y1);
            }
            y1 - all.max_y
        };
        for p in &mut b.pieces {
            p.translate_all(dy);
        }
        if !ctx.valid_biome(Pos::new(ctx.middle_block_x(), 50 + dy, ctx.min_block_z())) {
            return None;
        }
        Some(b.pieces.into_iter().map(|p| Box::new(p) as Box<dyn Piece>).collect())
    }
}

impl MsPiece {
    fn translate_all(&mut self, dy: i32) {
        self.base.bbox.move_by(0, dy, 0);
        if let Kind::Room { entrances } = &mut self.kind {
            for e in entrances {
                e.move_by(0, dy, 0);
            }
        }
    }

    /// `isInInvalidLocation`: in a mineshaft blocking biome, or touching liquid.
    fn invalid_location(&self, ctx: &PlaceCtx) -> bool {
        let (b, c) = (self.base.bbox, ctx.chunk_box);
        let x0 = (b.min_x - 1).max(c.min_x);
        let y0 = (b.min_y - 1).max(c.min_y);
        let z0 = (b.min_z - 1).max(c.min_z);
        let x1 = (b.max_x + 1).min(c.max_x);
        let y1 = (b.max_y + 1).min(c.max_y);
        let z1 = (b.max_z + 1).min(c.max_z);
        let center = Pos::new((x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2);
        if self.blocking & (1u128 << ctx.w.biome(center)) != 0 {
            return true;
        }
        let liquid = |x: i32, y: i32, z: i32| ctx.w.b.is_liquid(ctx.get(Pos::new(x, y, z)));
        for x in x0..=x1 {
            for z in z0..=z1 {
                if liquid(x, y0, z) || liquid(x, y1, z) {
                    return true;
                }
            }
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                if liquid(x, y, z0) || liquid(x, y, z1) {
                    return true;
                }
            }
        }
        for z in z0..=z1 {
            for y in y0..=y1 {
                if liquid(x0, y, z) || liquid(x1, y, z) {
                    return true;
                }
            }
        }
        false
    }

    /// `setPlanksBlock`: planks under the corridor where the floor is not sturdy.
    fn set_planks(&self, ctx: &mut PlaceCtx, x: i32, y: i32, z: i32) {
        if !self.base.is_interior(ctx, x, y, z) {
            return;
        }
        let p = self.base.world_pos(x, y, z);
        if !ctx.w.b.sturdy(ctx.get(p), Dir::Up) {
            ctx.set(p, self.wood.planks);
        }
    }

    /// `isSupportingBox`.
    fn supporting(&self, ctx: &PlaceCtx, x0: i32, x1: i32, y1: i32, z: i32) -> bool {
        (x0..=x1).all(|x| !ctx.w.b.is_air(self.base.get_block(ctx, x, y1 + 1, z)))
    }

    /// `placeSupport`.
    #[allow(clippy::too_many_arguments)]
    fn place_support(&self, ctx: &mut PlaceCtx, x0: i32, y0: i32, z: i32, y1: i32, x1: i32) {
        if !self.supporting(ctx, x0, x1, y1, z) {
            return;
        }
        let b = &self.base;
        let cave_air = st("minecraft:cave_air");
        let fence = self.wood.fence;
        b.generate_box(ctx, x0, y0, z, x0, y1 - 1, z, with_bool(fence, "west", true), cave_air, false);
        b.generate_box(ctx, x1, y0, z, x1, y1 - 1, z, with_bool(fence, "east", true), cave_air, false);
        if ctx.r.next_bounded(4) == 0 {
            b.generate_box(ctx, x0, y1, z, x0, y1, z, self.wood.planks, cave_air, false);
            b.generate_box(ctx, x1, y1, z, x1, y1, z, self.wood.planks, cave_air, false);
        } else {
            b.generate_box(ctx, x0, y1, z, x1, y1, z, self.wood.planks, cave_air, false);
            let torch = st("minecraft:wall_torch");
            b.maybe_generate_block(ctx, 0.05, x0 + 1, y1, z - 1, with_dir(torch, "facing", Dir::South));
            b.maybe_generate_block(ctx, 0.05, x0 + 1, y1, z + 1, with_dir(torch, "facing", Dir::North));
        }
    }

    /// `maybePlaceCobWeb`.
    fn maybe_cobweb(&self, ctx: &mut PlaceCtx, probability: f32, x: i32, y: i32, z: i32) {
        if self.base.is_interior(ctx, x, y, z) && ctx.r.next_f32() < probability && self.sturdy_neighbours(ctx, x, y, z, 2) {
            self.base.place_block(ctx, st("minecraft:cobweb"), x, y, z);
        }
    }

    /// `hasSturdyNeighbours`.
    fn sturdy_neighbours(&self, ctx: &PlaceCtx, x: i32, y: i32, z: i32, count: i32) -> bool {
        let p = self.base.world_pos(x, y, z);
        let mut n = 0;
        for d in Dir::ALL {
            let q = p.rel(d);
            if ctx.chunk_box.is_inside(q) && ctx.w.b.sturdy(ctx.get(q), d.opposite()) {
                n += 1;
                if n >= count {
                    return true;
                }
            }
        }
        false
    }

    /// `createChest` of corridors: a rail and a chest minecart entity.
    fn create_minecart(&self, ctx: &mut PlaceCtx, x: i32, y: i32, z: i32, loot: &str) -> bool {
        let p = self.base.world_pos(x, y, z);
        if !ctx.chunk_box.is_inside(p) || !ctx.is_air(p) || ctx.is_air(p.down()) {
            return false;
        }
        let shape = if ctx.r.next_bool() { "north_south" } else { "east_west" };
        self.base.place_block(ctx, with(st("minecraft:rail"), "shape", shape), x, y, z);
        let seed = ctx.r.next_i64();
        let nbt = Compound::new()
            .with("id", Tag::String("minecraft:chest_minecart".into()))
            .with("Pos", Tag::List(vec![Tag::Double(p.x as f64 + 0.5), Tag::Double(p.y as f64 + 0.5), Tag::Double(p.z as f64 + 0.5)]))
            .with("LootTable", Tag::String(loot.to_owned()))
            .with("LootTableSeed", Tag::Long(seed));
        ctx.add_entity_nbt(nbt);
        true
    }

    /// `placeDoubleLowerOrUpperSupport`.
    fn double_support(&self, ctx: &mut PlaceCtx, x: i32, y: i32, z: i32) {
        let planks = blocks::block_id_of_state(self.wood.planks);
        if blocks::block_id_of_state(self.base.get_block(ctx, x, y, z)) == planks {
            self.pillar_down_or_chain_up(ctx, x, y, z);
        }
        if blocks::block_id_of_state(self.base.get_block(ctx, x + 2, y, z)) == planks {
            self.pillar_down_or_chain_up(ctx, x + 2, y, z);
        }
    }

    /// `fillPillarDownOrChainUp`.
    fn pillar_down_or_chain_up(&self, ctx: &mut PlaceCtx, x: i32, y: i32, z: i32) {
        let p = self.base.world_pos(x, y, z);
        if !ctx.chunk_box.is_inside(p) {
            return;
        }
        let lava = blocks::block_id_of_state(st("minecraft:lava"));
        let world_y = p.y;
        let mut dist = 1;
        let (mut below, mut above) = (true, true);
        while below || above {
            if below {
                let q = p.with_y(world_y - dist);
                let s = ctx.get(q);
                let empty = ctx.replaceable_by_structures(s) && blocks::block_id_of_state(s) != lava;
                if !empty && ctx.w.b.sturdy(s, Dir::Up) {
                    for yy in world_y - dist + 1..world_y {
                        ctx.set(p.with_y(yy), self.wood.log);
                    }
                    return;
                }
                below = dist <= 20 && empty && q.y > ctx.min_y() + 1;
            }
            if above {
                let q = p.with_y(world_y + dist);
                let s = ctx.get(q);
                let empty = ctx.replaceable_by_structures(s);
                if !empty && ctx.w.b.center_sturdy(s, Dir::Down) && !falling(s) {
                    ctx.set(p.with_y(world_y + 1), self.wood.fence);
                    let chain = st("minecraft:iron_chain");
                    for yy in world_y + 2..world_y + dist {
                        ctx.set(p.with_y(yy), chain);
                    }
                    return;
                }
                above = dist <= 50 && empty && q.y < ctx.w.max_y();
            }
            dist += 1;
        }
    }
}

/// `state.getBlock() instanceof FallingBlock`.
fn falling(state: u32) -> bool {
    let n = blocks::block_of_state(state).name;
    matches!(n, "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel" | "minecraft:dragon_egg" | "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil") || n.ends_with("_concrete_powder")
}

impl Piece for MsPiece {
    fn base(&self) -> &PieceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PieceBase {
        &mut self.base
    }

    fn translate(&mut self, dx: i32, dy: i32, dz: i32) {
        self.base.bbox.move_by(dx, dy, dz);
        if let Kind::Room { entrances } = &mut self.kind {
            for e in entrances {
                e.move_by(dx, dy, dz);
            }
        }
    }

    fn post_process(&self, ctx: &mut PlaceCtx) {
        if self.invalid_location(ctx) {
            return;
        }
        let b = &self.base;
        let bb = b.bbox;
        let cave_air = st("minecraft:cave_air");
        match &self.kind {
            Kind::Room { entrances } => {
                b.generate_box(ctx, bb.min_x, bb.min_y + 1, bb.min_z, bb.max_x, (bb.min_y + 3).min(bb.max_y), bb.max_z, cave_air, cave_air, false);
                for e in entrances {
                    b.generate_box(ctx, e.min_x, e.max_y - 2, e.min_z, e.max_x, e.max_y, e.max_z, cave_air, cave_air, false);
                }
                b.generate_upper_half_sphere(ctx, bb.min_x, bb.min_y + 4, bb.min_z, bb.max_x, bb.max_y, bb.max_z, cave_air, false);
            }
            Kind::Corridor { rails, spider, spider_placed, sections } => {
                let length = sections * 5 - 1;
                b.generate_box(ctx, 0, 0, 0, 2, 1, length, cave_air, cave_air, false);
                b.generate_maybe_box(ctx, 0.8, 0, 2, 0, 2, 2, length, cave_air, cave_air, false, false);
                if *spider {
                    b.generate_maybe_box(ctx, 0.6, 0, 0, 0, 2, 1, length, st("minecraft:cobweb"), cave_air, false, true);
                }
                for section in 0..*sections {
                    let z = 2 + section * 5;
                    self.place_support(ctx, 0, 0, z, 2, 2);
                    self.maybe_cobweb(ctx, 0.1, 0, 2, z - 1);
                    self.maybe_cobweb(ctx, 0.1, 2, 2, z - 1);
                    self.maybe_cobweb(ctx, 0.1, 0, 2, z + 1);
                    self.maybe_cobweb(ctx, 0.1, 2, 2, z + 1);
                    self.maybe_cobweb(ctx, 0.05, 0, 2, z - 2);
                    self.maybe_cobweb(ctx, 0.05, 2, 2, z - 2);
                    self.maybe_cobweb(ctx, 0.05, 0, 2, z + 2);
                    self.maybe_cobweb(ctx, 0.05, 2, 2, z + 2);
                    if ctx.r.next_bounded(100) == 0 {
                        self.create_minecart(ctx, 2, 0, z - 1, "minecraft:chests/abandoned_mineshaft");
                    }
                    if ctx.r.next_bounded(100) == 0 {
                        self.create_minecart(ctx, 0, 0, z + 1, "minecraft:chests/abandoned_mineshaft");
                    }
                    if *spider && !spider_placed.load(Ordering::Relaxed) {
                        let nz = z - 1 + ctx.r.next_bounded(3);
                        let p = b.world_pos(1, 0, nz);
                        if ctx.chunk_box.is_inside(p) && b.is_interior(ctx, 1, 0, nz) {
                            spider_placed.store(true, Ordering::Relaxed);
                            ctx.set(p, st("minecraft:spawner"));
                            ctx.set_spawner(p, "minecraft:cave_spider");
                        }
                    }
                }
                for x in 0..=2 {
                    for z in 0..=length {
                        self.set_planks(ctx, x, -1, z);
                    }
                }
                self.double_support(ctx, 0, -1, 2);
                if *sections > 1 {
                    self.double_support(ctx, 0, -1, length - 2);
                }
                if *rails {
                    let rail = with(st("minecraft:rail"), "shape", "north_south");
                    for z in 0..=length {
                        let floor = b.get_block(ctx, 1, -1, z);
                        if !ctx.w.b.is_air(floor) && ctx.w.b.is_solid_render(floor) {
                            let p = if b.is_interior(ctx, 1, 0, z) { 0.7 } else { 0.9 };
                            b.maybe_generate_block(ctx, p, 1, 0, z, rail);
                        }
                    }
                }
            }
            Kind::Crossing { two_floors, .. } => {
                if *two_floors {
                    b.generate_box(ctx, bb.min_x + 1, bb.min_y, bb.min_z, bb.max_x - 1, bb.min_y + 3 - 1, bb.max_z, cave_air, cave_air, false);
                    b.generate_box(ctx, bb.min_x, bb.min_y, bb.min_z + 1, bb.max_x, bb.min_y + 3 - 1, bb.max_z - 1, cave_air, cave_air, false);
                    b.generate_box(ctx, bb.min_x + 1, bb.max_y - 2, bb.min_z, bb.max_x - 1, bb.max_y, bb.max_z, cave_air, cave_air, false);
                    b.generate_box(ctx, bb.min_x, bb.max_y - 2, bb.min_z + 1, bb.max_x, bb.max_y, bb.max_z - 1, cave_air, cave_air, false);
                    b.generate_box(ctx, bb.min_x + 1, bb.min_y + 3, bb.min_z + 1, bb.max_x - 1, bb.min_y + 3, bb.max_z - 1, cave_air, cave_air, false);
                } else {
                    b.generate_box(ctx, bb.min_x + 1, bb.min_y, bb.min_z, bb.max_x - 1, bb.max_y, bb.max_z, cave_air, cave_air, false);
                    b.generate_box(ctx, bb.min_x, bb.min_y, bb.min_z + 1, bb.max_x, bb.max_y, bb.max_z - 1, cave_air, cave_air, false);
                }
                for (x, z) in [(bb.min_x + 1, bb.min_z + 1), (bb.min_x + 1, bb.max_z - 1), (bb.max_x - 1, bb.min_z + 1), (bb.max_x - 1, bb.max_z - 1)] {
                    // placeSupportPillar
                    if !ctx.w.b.is_air(b.get_block(ctx, x, bb.max_y + 1, z)) {
                        b.generate_box(ctx, x, bb.min_y, z, x, bb.max_y, z, self.wood.planks, cave_air, false);
                    }
                }
                let y = bb.min_y - 1;
                for x in bb.min_x..=bb.max_x {
                    for z in bb.min_z..=bb.max_z {
                        self.set_planks(ctx, x, y, z);
                    }
                }
            }
            Kind::Stairs => {
                b.generate_box(ctx, 0, 5, 0, 2, 7, 1, cave_air, cave_air, false);
                b.generate_box(ctx, 0, 0, 7, 2, 2, 8, cave_air, cave_air, false);
                for i in 0..5 {
                    b.generate_box(ctx, 0, 5 - i - if i < 4 { 1 } else { 0 }, 2 + i, 2, 7 - i, 2 + i, cave_air, cave_air, false);
                }
            }
        }
    }
}
