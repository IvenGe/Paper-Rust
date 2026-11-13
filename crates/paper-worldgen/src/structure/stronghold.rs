//! Strongholds: corridors, crossings, prisons, libraries and the portal room, grown from a
//! spiral staircase by weighted piece choice.
//!
//! Written to vanilla 26.3's `StrongholdStructure` and `StrongholdPieces`; the layout
//! algorithm also follows `pumpkin-world/src/generation/structure/structures/stronghold/`
//! of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, whose layouts match vanilla's.

use super::piece::{st, with, with_bool, with_dir, PieceBase, PlaceCtx, Selector};
use super::{BoundingBox, GenCtx, Piece, StructureGen};
use crate::feature::world::{Dir, Pos};
use crate::random::{Random, WorldgenRandom};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct Stronghold;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Door {
    Opening,
    WoodDoor,
    Grates,
    IronDoor,
}

/// `randomSmallDoor`.
fn random_door(r: &mut WorldgenRandom) -> Door {
    match r.next_bounded(5) {
        2 => Door::WoodDoor,
        3 => Door::Grates,
        4 => Door::IronDoor,
        _ => Door::Opening,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Type {
    Straight,
    PrisonHall,
    LeftTurn,
    RightTurn,
    RoomCrossing,
    StraightStairsDown,
    StairsDown,
    FiveCrossing,
    ChestCorridor,
    Library,
    PortalRoom,
}

enum Kind {
    StairsDown { source: bool },
    Straight { left: bool, right: bool },
    ChestCorridor { chest: AtomicBool },
    StraightStairsDown,
    LeftTurn,
    RightTurn,
    RoomCrossing { kind: i32 },
    PrisonHall,
    Library { tall: bool },
    FiveCrossing { left_low: bool, left_high: bool, right_low: bool, right_high: bool },
    PortalRoom { spawner: AtomicBool },
    FillerCorridor { steps: i32 },
}

struct ShPiece {
    base: PieceBase,
    entry: Door,
    kind: Kind,
}

/// `PieceWeight`.
struct Weight {
    t: Type,
    weight: i32,
    max: i32,
    count: i32,
}

impl Weight {
    fn do_place(&self, depth: i32) -> bool {
        let base = self.max == 0 || self.count < self.max;
        match self.t {
            Type::Library => base && depth > 4,
            Type::PortalRoom => base && depth > 5,
            _ => base,
        }
    }

    fn is_valid(&self) -> bool {
        self.max == 0 || self.count < self.max
    }
}

fn weights() -> Vec<Weight> {
    let w = |t, weight, max| Weight { t, weight, max, count: 0 };
    vec![
        w(Type::Straight, 40, 0),
        w(Type::PrisonHall, 5, 5),
        w(Type::LeftTurn, 20, 0),
        w(Type::RightTurn, 20, 0),
        w(Type::RoomCrossing, 10, 6),
        w(Type::StraightStairsDown, 5, 5),
        w(Type::StairsDown, 5, 5),
        w(Type::FiveCrossing, 5, 4),
        w(Type::ChestCorridor, 5, 4),
        w(Type::Library, 10, 2),
        w(Type::PortalRoom, 20, 1),
    ]
}

/// The state vanilla keeps in statics and the start piece while a stronghold grows.
struct Builder {
    pieces: Vec<ShPiece>,
    pending: Vec<usize>,
    weights: Vec<Weight>,
    total_weight: i32,
    imposed: Option<Type>,
    /// Index into `weights` of the previous piece's weight (by type).
    previous: Option<Type>,
    portal_room: bool,
}

impl Builder {
    fn collides(&self, b: &BoundingBox) -> Option<usize> {
        self.pieces.iter().position(|p| p.base.bbox.intersects(b))
    }

    fn update_piece_weight(&mut self) -> bool {
        let mut any = false;
        self.total_weight = 0;
        for w in &self.weights {
            if w.max > 0 && w.count < w.max {
                any = true;
            }
            self.total_weight += w.weight;
        }
        any
    }

    fn ok(b: &BoundingBox) -> bool {
        b.min_y > 10
    }

    /// `findAndCreatePieceFactory`: the piece's `createPiece`.
    #[allow(clippy::too_many_arguments)]
    fn create(&self, t: Type, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, d: Dir, depth: i32) -> Option<ShPiece> {
        let make = |kind_id, b: BoundingBox| PieceBase::new(kind_id, depth, b).with_orientation(Some(d));
        let free = |b: &BoundingBox| Self::ok(b) && self.collides(b).is_none();
        Some(match t {
            Type::Straight => {
                let b = BoundingBox::orient(x, y, z, -1, -1, 0, 5, 5, 7, d);
                if !free(&b) {
                    return None;
                }
                let entry = random_door(r);
                let left = r.next_bounded(2) == 0;
                let right = r.next_bounded(2) == 0;
                ShPiece { base: make("minecraft:shs", b), entry, kind: Kind::Straight { left, right } }
            }
            Type::PrisonHall => {
                let b = BoundingBox::orient(x, y, z, -1, -1, 0, 9, 5, 11, d);
                if !free(&b) {
                    return None;
                }
                ShPiece { base: make("minecraft:shph", b), entry: random_door(r), kind: Kind::PrisonHall }
            }
            Type::LeftTurn | Type::RightTurn => {
                let b = BoundingBox::orient(x, y, z, -1, -1, 0, 5, 5, 5, d);
                if !free(&b) {
                    return None;
                }
                let (id, kind) = if t == Type::LeftTurn { ("minecraft:shlt", Kind::LeftTurn) } else { ("minecraft:shrt", Kind::RightTurn) };
                ShPiece { base: make(id, b), entry: random_door(r), kind }
            }
            Type::RoomCrossing => {
                let b = BoundingBox::orient(x, y, z, -4, -1, 0, 11, 7, 11, d);
                if !free(&b) {
                    return None;
                }
                let entry = random_door(r);
                let kind = r.next_bounded(5);
                ShPiece { base: make("minecraft:shrc", b), entry, kind: Kind::RoomCrossing { kind } }
            }
            Type::StraightStairsDown => {
                let b = BoundingBox::orient(x, y, z, -1, -7, 0, 5, 11, 8, d);
                if !free(&b) {
                    return None;
                }
                ShPiece { base: make("minecraft:shssd", b), entry: random_door(r), kind: Kind::StraightStairsDown }
            }
            Type::StairsDown => {
                let b = BoundingBox::orient(x, y, z, -1, -7, 0, 5, 11, 5, d);
                if !free(&b) {
                    return None;
                }
                ShPiece { base: make("minecraft:shsd", b), entry: random_door(r), kind: Kind::StairsDown { source: false } }
            }
            Type::FiveCrossing => {
                let b = BoundingBox::orient(x, y, z, -4, -3, 0, 10, 9, 11, d);
                if !free(&b) {
                    return None;
                }
                let entry = random_door(r);
                let left_low = r.next_bool();
                let left_high = r.next_bool();
                let right_low = r.next_bool();
                let right_high = r.next_bounded(3) > 0;
                ShPiece { base: make("minecraft:sh5c", b), entry, kind: Kind::FiveCrossing { left_low, left_high, right_low, right_high } }
            }
            Type::ChestCorridor => {
                let b = BoundingBox::orient(x, y, z, -1, -1, 0, 5, 5, 7, d);
                if !free(&b) {
                    return None;
                }
                ShPiece { base: make("minecraft:shcc", b), entry: random_door(r), kind: Kind::ChestCorridor { chest: AtomicBool::new(false) } }
            }
            Type::Library => {
                let mut b = BoundingBox::orient(x, y, z, -4, -1, 0, 14, 11, 15, d);
                if !free(&b) {
                    b = BoundingBox::orient(x, y, z, -4, -1, 0, 14, 6, 15, d);
                    if !free(&b) {
                        return None;
                    }
                }
                let entry = random_door(r);
                ShPiece { base: make("minecraft:shli", b), entry, kind: Kind::Library { tall: b.y_span() > 6 } }
            }
            Type::PortalRoom => {
                let b = BoundingBox::orient(x, y, z, -4, -1, 0, 11, 8, 16, d);
                if !free(&b) {
                    return None;
                }
                ShPiece { base: make("minecraft:shpr", b), entry: Door::Opening, kind: Kind::PortalRoom { spawner: AtomicBool::new(false) } }
            }
        })
    }

    /// `generatePieceFromSmallDoor`.
    #[allow(clippy::too_many_arguments)]
    fn from_small_door(&mut self, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, d: Dir, depth: i32) -> Option<ShPiece> {
        if !self.update_piece_weight() {
            return None;
        }
        if let Some(t) = self.imposed.take() {
            if let Some(p) = self.create(t, r, x, y, z, d, depth) {
                return Some(p);
            }
        }
        for _ in 0..5 {
            let mut selection = r.next_bounded(self.total_weight);
            let mut i = 0;
            while i < self.weights.len() {
                selection -= self.weights[i].weight;
                if selection < 0 {
                    if !self.weights[i].do_place(depth) || Some(self.weights[i].t) == self.previous {
                        break;
                    }
                    let t = self.weights[i].t;
                    if let Some(p) = self.create(t, r, x, y, z, d, depth) {
                        self.weights[i].count += 1;
                        self.previous = Some(t);
                        if !self.weights[i].is_valid() {
                            self.weights.remove(i);
                        }
                        return Some(p);
                    }
                }
                i += 1;
            }
        }
        // FillerCorridor.findPieceBox
        let b = BoundingBox::orient(x, y, z, -1, -1, 0, 5, 5, 4, d);
        let c = self.collides(&b)?;
        let cb = self.pieces[c].base.bbox;
        if cb.min_y != b.min_y {
            return None;
        }
        for len in (1..=2).rev() {
            let shorter = BoundingBox::orient(x, y, z, -1, -1, 0, 5, 5, len, d);
            if !cb.intersects(&shorter) {
                let b = BoundingBox::orient(x, y, z, -1, -1, 0, 5, 5, len + 1, d);
                if b.min_y <= 1 {
                    return None;
                }
                let steps = if matches!(d, Dir::North | Dir::South) { b.z_span() } else { b.x_span() };
                return Some(ShPiece { base: PieceBase::new("minecraft:shfc", depth, b).with_orientation(Some(d)), entry: Door::Opening, kind: Kind::FillerCorridor { steps } });
            }
        }
        None
    }

    /// `generateAndAddPiece`.
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, d: Dir, depth: i32) {
        if depth > 50 {
            return;
        }
        let sb = self.pieces[0].base.bbox;
        if (x - sb.min_x).abs() > 112 || (z - sb.min_z).abs() > 112 {
            return;
        }
        if let Some(p) = self.from_small_door(r, x, y, z, d, depth + 1) {
            self.pieces.push(p);
            self.pending.push(self.pieces.len() - 1);
        }
    }

    fn forward(&mut self, i: usize, r: &mut WorldgenRandom, x_off: i32, y_off: i32) {
        let b = self.pieces[i].base.bbox;
        let depth = self.pieces[i].base.gen_depth;
        match self.pieces[i].base.orientation {
            Some(Dir::North) => self.add(r, b.min_x + x_off, b.min_y + y_off, b.min_z - 1, Dir::North, depth),
            Some(Dir::South) => self.add(r, b.min_x + x_off, b.min_y + y_off, b.max_z + 1, Dir::South, depth),
            Some(Dir::West) => self.add(r, b.min_x - 1, b.min_y + y_off, b.min_z + x_off, Dir::West, depth),
            Some(Dir::East) => self.add(r, b.max_x + 1, b.min_y + y_off, b.min_z + x_off, Dir::East, depth),
            _ => {}
        }
    }

    fn left(&mut self, i: usize, r: &mut WorldgenRandom, y_off: i32, z_off: i32) {
        let b = self.pieces[i].base.bbox;
        let depth = self.pieces[i].base.gen_depth;
        match self.pieces[i].base.orientation {
            Some(Dir::North | Dir::South) => self.add(r, b.min_x - 1, b.min_y + y_off, b.min_z + z_off, Dir::West, depth),
            Some(Dir::West | Dir::East) => self.add(r, b.min_x + z_off, b.min_y + y_off, b.min_z - 1, Dir::North, depth),
            _ => {}
        }
    }

    fn right(&mut self, i: usize, r: &mut WorldgenRandom, y_off: i32, z_off: i32) {
        let b = self.pieces[i].base.bbox;
        let depth = self.pieces[i].base.gen_depth;
        match self.pieces[i].base.orientation {
            Some(Dir::North | Dir::South) => self.add(r, b.max_x + 1, b.min_y + y_off, b.min_z + z_off, Dir::East, depth),
            Some(Dir::West | Dir::East) => self.add(r, b.min_x + z_off, b.min_y + y_off, b.max_z + 1, Dir::South, depth),
            _ => {}
        }
    }

    /// `addChildren` of piece `i`.
    fn add_children(&mut self, i: usize, r: &mut WorldgenRandom) {
        let o = self.pieces[i].base.orientation;
        let north_or_east = matches!(o, Some(Dir::North | Dir::East));
        match &self.pieces[i].kind {
            Kind::StairsDown { source } => {
                if *source {
                    self.imposed = Some(Type::FiveCrossing);
                }
                self.forward(i, r, 1, 1);
            }
            Kind::Straight { left, right } => {
                let (left, right) = (*left, *right);
                self.forward(i, r, 1, 1);
                if left {
                    self.left(i, r, 1, 2);
                }
                if right {
                    self.right(i, r, 1, 2);
                }
            }
            Kind::ChestCorridor { .. } | Kind::StraightStairsDown | Kind::PrisonHall => self.forward(i, r, 1, 1),
            Kind::LeftTurn => {
                if north_or_east {
                    self.left(i, r, 1, 1);
                } else {
                    self.right(i, r, 1, 1);
                }
            }
            Kind::RightTurn => {
                if north_or_east {
                    self.right(i, r, 1, 1);
                } else {
                    self.left(i, r, 1, 1);
                }
            }
            Kind::RoomCrossing { .. } => {
                self.forward(i, r, 4, 1);
                self.left(i, r, 1, 4);
                self.right(i, r, 1, 4);
            }
            Kind::FiveCrossing { left_low, left_high, right_low, right_high } => {
                let (ll, lh, rl, rh) = (*left_low, *left_high, *right_low, *right_high);
                let (mut a, mut b) = (3, 5);
                if matches!(o, Some(Dir::West | Dir::North)) {
                    a = 8 - a;
                    b = 8 - b;
                }
                self.forward(i, r, 5, 1);
                if ll {
                    self.left(i, r, a, 1);
                }
                if lh {
                    self.left(i, r, b, 7);
                }
                if rl {
                    self.right(i, r, a, 1);
                }
                if rh {
                    self.right(i, r, b, 7);
                }
            }
            Kind::PortalRoom { .. } => self.portal_room = true,
            Kind::Library { .. } | Kind::FillerCorridor { .. } => {}
        }
    }
}

impl StructureGen for Stronghold {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        if !ctx.valid_biome(Pos::new(ctx.min_block_x(), 0, ctx.min_block_z())) {
            return None;
        }
        let (cx, cz) = (ctx.chunk_x, ctx.chunk_z);
        let mut tries = 0i64;
        loop {
            let r = &mut ctx.r;
            r.set_large_feature_seed(ctx.seed.wrapping_add(tries), cx, cz);
            tries += 1;
            let mut b = Builder { pieces: Vec::new(), pending: Vec::new(), weights: weights(), total_weight: 0, imposed: None, previous: None, portal_room: false };
            let d = Dir::random_horizontal(r);
            let bbox = BoundingBox::make((cx << 4) + 2, 64, (cz << 4) + 2, d, 5, 11, 5);
            b.pieces.push(ShPiece { base: PieceBase::new("minecraft:shstart", 0, bbox).with_orientation(Some(d)), entry: Door::Opening, kind: Kind::StairsDown { source: true } });
            b.add_children(0, r);
            while !b.pending.is_empty() {
                let i = r.next_bounded(b.pending.len() as i32) as usize;
                let p = b.pending.remove(i);
                b.add_children(p, r);
            }
            // moveBelowSeaLevel(seaLevel, minY, random, 10)
            let all = BoundingBox::encapsulating(b.pieces.iter().map(|p| p.base.bbox)).expect("start piece");
            let max_y = ctx.gen.sea_level - 10;
            let mut y1 = all.y_span() + ctx.gen.gen_min_y + 1;
            if y1 < max_y {
                y1 += r.next_bounded(max_y - y1);
            }
            let dy = y1 - all.max_y;
            for p in &mut b.pieces {
                p.base.bbox.move_by(0, dy, 0);
            }
            if !b.pieces.is_empty() && b.portal_room {
                return Some(b.pieces.into_iter().map(|p| Box::new(p) as Box<dyn Piece>).collect());
            }
        }
    }
}

/// `SmoothStoneSelector`.
struct SmoothStone {
    cracked: u32,
    mossy: u32,
    infested: u32,
    bricks: u32,
    cave_air: u32,
}

impl SmoothStone {
    fn new() -> Self {
        Self { cracked: st("minecraft:cracked_stone_bricks"), mossy: st("minecraft:mossy_stone_bricks"), infested: st("minecraft:infested_stone_bricks"), bricks: st("minecraft:stone_bricks"), cave_air: st("minecraft:cave_air") }
    }
}

impl Selector for SmoothStone {
    fn next(&mut self, r: &mut WorldgenRandom, _x: i32, _y: i32, _z: i32, edge: bool) -> u32 {
        if !edge {
            return self.cave_air;
        }
        let f = r.next_f32();
        if f < 0.2 {
            self.cracked
        } else if f < 0.5 {
            self.mossy
        } else if f < 0.55 {
            self.infested
        } else {
            self.bricks
        }
    }
}

impl ShPiece {
    /// `generateSmallDoor`.
    fn small_door(&self, ctx: &mut PlaceCtx, door: Door, x: i32, y: i32, z: i32) {
        let b = &self.base;
        let bricks = st("minecraft:stone_bricks");
        let cave_air = st("minecraft:cave_air");
        match door {
            Door::Opening => b.generate_box(ctx, x, y, z, x + 2, y + 2, z, cave_air, cave_air, false),
            Door::WoodDoor | Door::IronDoor => {
                for (dx, dy) in [(0, 0), (0, 1), (0, 2), (1, 2), (2, 2), (2, 1), (2, 0)] {
                    b.place_block(ctx, bricks, x + dx, y + dy, z);
                }
                let door_block = st(if door == Door::WoodDoor { "minecraft:oak_door" } else { "minecraft:iron_door" });
                b.place_block(ctx, door_block, x + 1, y, z);
                b.place_block(ctx, with(door_block, "half", "upper"), x + 1, y + 1, z);
                if door == Door::IronDoor {
                    let button = st("minecraft:stone_button");
                    b.place_block(ctx, with_dir(button, "facing", Dir::North), x + 2, y + 1, z + 1);
                    b.place_block(ctx, with_dir(button, "facing", Dir::South), x + 2, y + 1, z - 1);
                }
            }
            Door::Grates => {
                let bars = st("minecraft:iron_bars");
                let w = with_bool(bars, "west", true);
                let e = with_bool(bars, "east", true);
                let ew = with_bool(w, "east", true);
                b.place_block(ctx, cave_air, x + 1, y, z);
                b.place_block(ctx, cave_air, x + 1, y + 1, z);
                b.place_block(ctx, w, x, y, z);
                b.place_block(ctx, w, x, y + 1, z);
                b.place_block(ctx, ew, x, y + 2, z);
                b.place_block(ctx, ew, x + 1, y + 2, z);
                b.place_block(ctx, ew, x + 2, y + 2, z);
                b.place_block(ctx, e, x + 2, y + 1, z);
                b.place_block(ctx, e, x + 2, y, z);
            }
        }
    }
}

impl Piece for ShPiece {
    fn base(&self) -> &PieceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PieceBase {
        &mut self.base
    }

    fn post_process(&self, ctx: &mut PlaceCtx) {
        let b = &self.base;
        let mut sel = SmoothStone::new();
        let bricks = st("minecraft:stone_bricks");
        let cave_air = st("minecraft:cave_air");
        let slab = st("minecraft:smooth_stone_slab");
        let torch = |d: Dir| with_dir(st("minecraft:wall_torch"), "facing", d);
        match &self.kind {
            Kind::StairsDown { .. } => {
                b.generate_box_sel(ctx, 0, 0, 0, 4, 10, 4, true, &mut sel);
                self.small_door(ctx, self.entry, 1, 7, 0);
                self.small_door(ctx, Door::Opening, 1, 1, 4);
                for (s, x, y, z) in [
                    (bricks, 2, 6, 1),
                    (bricks, 1, 5, 1),
                    (slab, 1, 6, 1),
                    (bricks, 1, 5, 2),
                    (bricks, 1, 4, 3),
                    (slab, 1, 5, 3),
                    (bricks, 2, 4, 3),
                    (bricks, 3, 3, 3),
                    (slab, 3, 4, 3),
                    (bricks, 3, 3, 2),
                    (bricks, 3, 2, 1),
                    (slab, 3, 3, 1),
                    (bricks, 2, 2, 1),
                    (bricks, 1, 1, 1),
                    (slab, 1, 2, 1),
                    (bricks, 1, 1, 2),
                    (slab, 1, 1, 3),
                ] {
                    b.place_block(ctx, s, x, y, z);
                }
            }
            Kind::Straight { left, right } => {
                b.generate_box_sel(ctx, 0, 0, 0, 4, 4, 6, true, &mut sel);
                self.small_door(ctx, self.entry, 1, 1, 0);
                self.small_door(ctx, Door::Opening, 1, 1, 6);
                let (e, w) = (torch(Dir::East), torch(Dir::West));
                b.maybe_generate_block(ctx, 0.1, 1, 2, 1, e);
                b.maybe_generate_block(ctx, 0.1, 3, 2, 1, w);
                b.maybe_generate_block(ctx, 0.1, 1, 2, 5, e);
                b.maybe_generate_block(ctx, 0.1, 3, 2, 5, w);
                if *left {
                    b.generate_box(ctx, 0, 1, 2, 0, 3, 4, cave_air, cave_air, false);
                }
                if *right {
                    b.generate_box(ctx, 4, 1, 2, 4, 3, 4, cave_air, cave_air, false);
                }
            }
            Kind::ChestCorridor { chest } => {
                b.generate_box_sel(ctx, 0, 0, 0, 4, 4, 6, true, &mut sel);
                self.small_door(ctx, self.entry, 1, 1, 0);
                self.small_door(ctx, Door::Opening, 1, 1, 6);
                b.generate_box(ctx, 3, 1, 2, 3, 1, 4, bricks, bricks, false);
                let brick_slab = st("minecraft:stone_brick_slab");
                b.place_block(ctx, brick_slab, 3, 1, 1);
                b.place_block(ctx, brick_slab, 3, 1, 5);
                b.place_block(ctx, brick_slab, 3, 2, 2);
                b.place_block(ctx, brick_slab, 3, 2, 4);
                for z in 2..=4 {
                    b.place_block(ctx, brick_slab, 2, 1, z);
                }
                if !chest.load(Ordering::Relaxed) && ctx.chunk_box.is_inside(b.world_pos(3, 2, 3)) {
                    chest.store(true, Ordering::Relaxed);
                    b.create_chest(ctx, 3, 2, 3, "minecraft:chests/stronghold_corridor");
                }
            }
            Kind::StraightStairsDown => {
                b.generate_box_sel(ctx, 0, 0, 0, 4, 10, 7, true, &mut sel);
                self.small_door(ctx, self.entry, 1, 7, 0);
                self.small_door(ctx, Door::Opening, 1, 1, 7);
                let stairs = with_dir(st("minecraft:cobblestone_stairs"), "facing", Dir::South);
                for i in 0..6 {
                    for x in 1..=3 {
                        b.place_block(ctx, stairs, x, 6 - i, 1 + i);
                    }
                    if i < 5 {
                        for x in 1..=3 {
                            b.place_block(ctx, bricks, x, 5 - i, 1 + i);
                        }
                    }
                }
            }
            Kind::LeftTurn | Kind::RightTurn => {
                b.generate_box_sel(ctx, 0, 0, 0, 4, 4, 4, true, &mut sel);
                self.small_door(ctx, self.entry, 1, 1, 0);
                let north_or_east = matches!(b.orientation, Some(Dir::North | Dir::East));
                let west_side = matches!(self.kind, Kind::LeftTurn) == north_or_east;
                if west_side {
                    b.generate_box(ctx, 0, 1, 1, 0, 3, 3, cave_air, cave_air, false);
                } else {
                    b.generate_box(ctx, 4, 1, 1, 4, 3, 3, cave_air, cave_air, false);
                }
            }
            Kind::RoomCrossing { kind } => self.room_crossing(ctx, *kind, &mut sel),
            Kind::PrisonHall => {
                b.generate_box_sel(ctx, 0, 0, 0, 8, 4, 10, true, &mut sel);
                self.small_door(ctx, self.entry, 1, 1, 0);
                b.generate_box(ctx, 1, 1, 10, 3, 3, 10, cave_air, cave_air, false);
                b.generate_box_sel(ctx, 4, 1, 1, 4, 3, 1, false, &mut sel);
                b.generate_box_sel(ctx, 4, 1, 3, 4, 3, 3, false, &mut sel);
                b.generate_box_sel(ctx, 4, 1, 7, 4, 3, 7, false, &mut sel);
                b.generate_box_sel(ctx, 4, 1, 9, 4, 3, 9, false, &mut sel);
                let bars = st("minecraft:iron_bars");
                let ns = with_bool(with_bool(bars, "north", true), "south", true);
                let nse = with_bool(ns, "east", true);
                let we = with_bool(with_bool(bars, "west", true), "east", true);
                for y in 1..=3 {
                    b.place_block(ctx, ns, 4, y, 4);
                    b.place_block(ctx, nse, 4, y, 5);
                    b.place_block(ctx, ns, 4, y, 6);
                    b.place_block(ctx, we, 5, y, 5);
                    b.place_block(ctx, we, 6, y, 5);
                    b.place_block(ctx, we, 7, y, 5);
                }
                b.place_block(ctx, ns, 4, 3, 2);
                b.place_block(ctx, ns, 4, 3, 8);
                let bottom = with_dir(st("minecraft:iron_door"), "facing", Dir::West);
                let top = with(bottom, "half", "upper");
                b.place_block(ctx, bottom, 4, 1, 2);
                b.place_block(ctx, top, 4, 2, 2);
                b.place_block(ctx, bottom, 4, 1, 8);
                b.place_block(ctx, top, 4, 2, 8);
            }
            Kind::Library { tall } => self.library(ctx, *tall, &mut sel),
            Kind::FiveCrossing { left_low, left_high, right_low, right_high } => {
                b.generate_box_sel(ctx, 0, 0, 0, 9, 8, 10, true, &mut sel);
                self.small_door(ctx, self.entry, 4, 3, 0);
                if *left_low {
                    b.generate_box(ctx, 0, 3, 1, 0, 5, 3, cave_air, cave_air, false);
                }
                if *right_low {
                    b.generate_box(ctx, 9, 3, 1, 9, 5, 3, cave_air, cave_air, false);
                }
                if *left_high {
                    b.generate_box(ctx, 0, 5, 7, 0, 7, 9, cave_air, cave_air, false);
                }
                if *right_high {
                    b.generate_box(ctx, 9, 5, 7, 9, 7, 9, cave_air, cave_air, false);
                }
                b.generate_box(ctx, 5, 1, 10, 7, 3, 10, cave_air, cave_air, false);
                b.generate_box_sel(ctx, 1, 2, 1, 8, 2, 6, false, &mut sel);
                b.generate_box_sel(ctx, 4, 1, 5, 4, 4, 9, false, &mut sel);
                b.generate_box_sel(ctx, 8, 1, 5, 8, 4, 9, false, &mut sel);
                b.generate_box_sel(ctx, 1, 4, 7, 3, 4, 9, false, &mut sel);
                b.generate_box_sel(ctx, 1, 3, 5, 3, 3, 6, false, &mut sel);
                b.generate_box(ctx, 1, 3, 4, 3, 3, 4, slab, slab, false);
                b.generate_box(ctx, 1, 4, 6, 3, 4, 6, slab, slab, false);
                b.generate_box_sel(ctx, 5, 1, 7, 7, 1, 8, false, &mut sel);
                b.generate_box(ctx, 5, 1, 9, 7, 1, 9, slab, slab, false);
                b.generate_box(ctx, 5, 2, 7, 7, 2, 7, slab, slab, false);
                b.generate_box(ctx, 4, 5, 7, 4, 5, 9, slab, slab, false);
                b.generate_box(ctx, 8, 5, 7, 8, 5, 9, slab, slab, false);
                let double = with(slab, "type", "double");
                b.generate_box(ctx, 5, 5, 7, 7, 5, 9, double, double, false);
                b.place_block(ctx, torch(Dir::South), 6, 5, 6);
            }
            Kind::PortalRoom { spawner } => self.portal_room(ctx, spawner, &mut sel),
            Kind::FillerCorridor { steps } => {
                for i in 0..*steps {
                    for x in 0..=4 {
                        b.place_block(ctx, bricks, x, 0, i);
                    }
                    for y in 1..=3 {
                        b.place_block(ctx, bricks, 0, y, i);
                        b.place_block(ctx, cave_air, 1, y, i);
                        b.place_block(ctx, cave_air, 2, y, i);
                        b.place_block(ctx, cave_air, 3, y, i);
                        b.place_block(ctx, bricks, 4, y, i);
                    }
                    for x in 0..=4 {
                        b.place_block(ctx, bricks, x, 4, i);
                    }
                }
            }
        }
    }
}

impl ShPiece {
    fn room_crossing(&self, ctx: &mut PlaceCtx, kind: i32, sel: &mut SmoothStone) {
        let b = &self.base;
        let bricks = st("minecraft:stone_bricks");
        let cave_air = st("minecraft:cave_air");
        let slab = st("minecraft:smooth_stone_slab");
        let cobble = st("minecraft:cobblestone");
        let planks = st("minecraft:oak_planks");
        let wall_torch = st("minecraft:wall_torch");
        b.generate_box_sel(ctx, 0, 0, 0, 10, 6, 10, true, sel);
        self.small_door(ctx, self.entry, 4, 1, 0);
        b.generate_box(ctx, 4, 1, 10, 6, 3, 10, cave_air, cave_air, false);
        b.generate_box(ctx, 0, 1, 4, 0, 3, 6, cave_air, cave_air, false);
        b.generate_box(ctx, 10, 1, 4, 10, 3, 6, cave_air, cave_air, false);
        match kind {
            0 => {
                b.place_block(ctx, bricks, 5, 1, 5);
                b.place_block(ctx, bricks, 5, 2, 5);
                b.place_block(ctx, bricks, 5, 3, 5);
                b.place_block(ctx, with_dir(wall_torch, "facing", Dir::West), 4, 3, 5);
                b.place_block(ctx, with_dir(wall_torch, "facing", Dir::East), 6, 3, 5);
                b.place_block(ctx, with_dir(wall_torch, "facing", Dir::South), 5, 3, 4);
                b.place_block(ctx, with_dir(wall_torch, "facing", Dir::North), 5, 3, 6);
                for (x, z) in [(4, 4), (4, 5), (4, 6), (6, 4), (6, 5), (6, 6), (5, 4), (5, 6)] {
                    b.place_block(ctx, slab, x, 1, z);
                }
            }
            1 => {
                for i in 0..5 {
                    b.place_block(ctx, bricks, 3, 1, 3 + i);
                    b.place_block(ctx, bricks, 7, 1, 3 + i);
                    b.place_block(ctx, bricks, 3 + i, 1, 3);
                    b.place_block(ctx, bricks, 3 + i, 1, 7);
                }
                b.place_block(ctx, bricks, 5, 1, 5);
                b.place_block(ctx, bricks, 5, 2, 5);
                b.place_block(ctx, bricks, 5, 3, 5);
                b.place_block(ctx, st("minecraft:water"), 5, 4, 5);
            }
            2 => {
                for z in 1..=9 {
                    b.place_block(ctx, cobble, 1, 3, z);
                    b.place_block(ctx, cobble, 9, 3, z);
                }
                for x in 1..=9 {
                    b.place_block(ctx, cobble, x, 3, 1);
                    b.place_block(ctx, cobble, x, 3, 9);
                }
                for (x, y, z) in [(5, 1, 4), (5, 1, 6), (5, 3, 4), (5, 3, 6), (4, 1, 5), (6, 1, 5), (4, 3, 5), (6, 3, 5)] {
                    b.place_block(ctx, cobble, x, y, z);
                }
                for y in 1..=3 {
                    b.place_block(ctx, cobble, 4, y, 4);
                    b.place_block(ctx, cobble, 6, y, 4);
                    b.place_block(ctx, cobble, 4, y, 6);
                    b.place_block(ctx, cobble, 6, y, 6);
                }
                b.place_block(ctx, wall_torch, 5, 3, 5);
                for z in 2..=8 {
                    b.place_block(ctx, planks, 2, 3, z);
                    b.place_block(ctx, planks, 3, 3, z);
                    if z <= 3 || z >= 7 {
                        b.place_block(ctx, planks, 4, 3, z);
                        b.place_block(ctx, planks, 5, 3, z);
                        b.place_block(ctx, planks, 6, 3, z);
                    }
                    b.place_block(ctx, planks, 7, 3, z);
                    b.place_block(ctx, planks, 8, 3, z);
                }
                let ladder = with_dir(st("minecraft:ladder"), "facing", Dir::West);
                b.place_block(ctx, ladder, 9, 1, 3);
                b.place_block(ctx, ladder, 9, 2, 3);
                b.place_block(ctx, ladder, 9, 3, 3);
                b.create_chest(ctx, 3, 4, 8, "minecraft:chests/stronghold_crossing");
            }
            _ => {}
        }
    }

    fn library(&self, ctx: &mut PlaceCtx, tall: bool, sel: &mut SmoothStone) {
        let b = &self.base;
        let planks = st("minecraft:oak_planks");
        let shelf = st("minecraft:bookshelf");
        let cobweb = st("minecraft:cobweb");
        let fence = st("minecraft:oak_fence");
        let wall_torch = st("minecraft:wall_torch");
        let height = if tall { 11 } else { 6 };
        b.generate_box_sel(ctx, 0, 0, 0, 13, height - 1, 14, true, sel);
        self.small_door(ctx, self.entry, 4, 1, 0);
        b.generate_maybe_box(ctx, 0.07, 2, 1, 1, 11, 4, 13, cobweb, cobweb, false, false);
        for d in 1..=13 {
            if (d - 1) % 4 == 0 {
                b.generate_box(ctx, 1, 1, d, 1, 4, d, planks, planks, false);
                b.generate_box(ctx, 12, 1, d, 12, 4, d, planks, planks, false);
                b.place_block(ctx, with_dir(wall_torch, "facing", Dir::East), 2, 3, d);
                b.place_block(ctx, with_dir(wall_torch, "facing", Dir::West), 11, 3, d);
                if tall {
                    b.generate_box(ctx, 1, 6, d, 1, 9, d, planks, planks, false);
                    b.generate_box(ctx, 12, 6, d, 12, 9, d, planks, planks, false);
                }
            } else {
                b.generate_box(ctx, 1, 1, d, 1, 4, d, shelf, shelf, false);
                b.generate_box(ctx, 12, 1, d, 12, 4, d, shelf, shelf, false);
                if tall {
                    b.generate_box(ctx, 1, 6, d, 1, 9, d, shelf, shelf, false);
                    b.generate_box(ctx, 12, 6, d, 12, 9, d, shelf, shelf, false);
                }
            }
        }
        let mut d = 3;
        while d < 12 {
            b.generate_box(ctx, 3, 1, d, 4, 3, d, shelf, shelf, false);
            b.generate_box(ctx, 6, 1, d, 7, 3, d, shelf, shelf, false);
            b.generate_box(ctx, 9, 1, d, 10, 3, d, shelf, shelf, false);
            d += 2;
        }
        if tall {
            b.generate_box(ctx, 1, 5, 1, 3, 5, 13, planks, planks, false);
            b.generate_box(ctx, 10, 5, 1, 12, 5, 13, planks, planks, false);
            b.generate_box(ctx, 4, 5, 1, 9, 5, 2, planks, planks, false);
            b.generate_box(ctx, 4, 5, 12, 9, 5, 13, planks, planks, false);
            b.place_block(ctx, planks, 9, 5, 11);
            b.place_block(ctx, planks, 8, 5, 11);
            b.place_block(ctx, planks, 9, 5, 10);
            let f = |pairs: &[(&str, bool)]| pairs.iter().fold(fence, |s, (k, v)| with_bool(s, k, *v));
            let we = f(&[("west", true), ("east", true)]);
            let ns = f(&[("north", true), ("south", true)]);
            b.generate_box(ctx, 3, 6, 3, 3, 6, 11, ns, ns, false);
            b.generate_box(ctx, 10, 6, 3, 10, 6, 9, ns, ns, false);
            b.generate_box(ctx, 4, 6, 2, 9, 6, 2, we, we, false);
            b.generate_box(ctx, 4, 6, 12, 7, 6, 12, we, we, false);
            b.place_block(ctx, f(&[("north", true), ("east", true)]), 3, 6, 2);
            b.place_block(ctx, f(&[("south", true), ("east", true)]), 3, 6, 12);
            b.place_block(ctx, f(&[("north", true), ("west", true)]), 10, 6, 2);
            for i in 0..=2 {
                b.place_block(ctx, f(&[("south", true), ("west", true)]), 8 + i, 6, 12 - i);
                if i != 2 {
                    b.place_block(ctx, f(&[("north", true), ("east", true)]), 8 + i, 6, 11 - i);
                }
            }
            let ladder = with_dir(st("minecraft:ladder"), "facing", Dir::South);
            for y in 1..=7 {
                b.place_block(ctx, ladder, 10, y, 13);
            }
            let e = f(&[("east", true)]);
            let w = f(&[("west", true)]);
            b.place_block(ctx, e, 6, 9, 7);
            b.place_block(ctx, w, 7, 9, 7);
            b.place_block(ctx, e, 6, 8, 7);
            b.place_block(ctx, w, 7, 8, 7);
            let nswe = with_bool(with_bool(ns, "west", true), "east", true);
            b.place_block(ctx, nswe, 6, 7, 7);
            b.place_block(ctx, nswe, 7, 7, 7);
            b.place_block(ctx, e, 5, 7, 7);
            b.place_block(ctx, w, 8, 7, 7);
            b.place_block(ctx, with_bool(e, "north", true), 6, 7, 6);
            b.place_block(ctx, with_bool(e, "south", true), 6, 7, 8);
            b.place_block(ctx, with_bool(w, "north", true), 7, 7, 6);
            b.place_block(ctx, with_bool(w, "south", true), 7, 7, 8);
            let torch = st("minecraft:torch");
            for (x, z) in [(5, 7), (8, 7), (6, 6), (6, 8), (7, 6), (7, 8)] {
                b.place_block(ctx, torch, x, 8, z);
            }
        }
        b.create_chest(ctx, 3, 3, 5, "minecraft:chests/stronghold_library");
        if tall {
            b.place_block(ctx, st("minecraft:cave_air"), 12, 9, 1);
            b.create_chest(ctx, 12, 8, 1, "minecraft:chests/stronghold_library");
        }
    }

    fn portal_room(&self, ctx: &mut PlaceCtx, spawner: &AtomicBool, sel: &mut SmoothStone) {
        let b = &self.base;
        let lava = st("minecraft:lava");
        b.generate_box_sel(ctx, 0, 0, 0, 10, 7, 15, false, sel);
        self.small_door(ctx, Door::Grates, 4, 1, 0);
        b.generate_box_sel(ctx, 1, 6, 1, 1, 6, 14, false, sel);
        b.generate_box_sel(ctx, 9, 6, 1, 9, 6, 14, false, sel);
        b.generate_box_sel(ctx, 2, 6, 1, 8, 6, 2, false, sel);
        b.generate_box_sel(ctx, 2, 6, 14, 8, 6, 14, false, sel);
        b.generate_box_sel(ctx, 1, 1, 1, 2, 1, 4, false, sel);
        b.generate_box_sel(ctx, 8, 1, 1, 9, 1, 4, false, sel);
        b.generate_box(ctx, 1, 1, 1, 1, 1, 3, lava, lava, false);
        b.generate_box(ctx, 9, 1, 1, 9, 1, 3, lava, lava, false);
        b.generate_box_sel(ctx, 3, 1, 8, 7, 1, 12, false, sel);
        b.generate_box(ctx, 4, 1, 9, 6, 1, 11, lava, lava, false);
        let bars = st("minecraft:iron_bars");
        let ns = with_bool(with_bool(bars, "north", true), "south", true);
        let we = with_bool(with_bool(bars, "west", true), "east", true);
        let mut z = 3;
        while z < 14 {
            b.generate_box(ctx, 0, 3, z, 0, 4, z, ns, ns, false);
            b.generate_box(ctx, 10, 3, z, 10, 4, z, ns, ns, false);
            z += 2;
        }
        let mut x = 2;
        while x < 9 {
            b.generate_box(ctx, x, 3, 15, x, 4, 15, we, we, false);
            x += 2;
        }
        let stairs = with_dir(st("minecraft:stone_brick_stairs"), "facing", Dir::North);
        b.generate_box_sel(ctx, 4, 1, 5, 6, 1, 7, false, sel);
        b.generate_box_sel(ctx, 4, 2, 6, 6, 2, 7, false, sel);
        b.generate_box_sel(ctx, 4, 3, 7, 6, 3, 7, false, sel);
        for x in 4..=6 {
            b.place_block(ctx, stairs, x, 1, 4);
            b.place_block(ctx, stairs, x, 2, 5);
            b.place_block(ctx, stairs, x, 3, 6);
        }
        let frame = st("minecraft:end_portal_frame");
        let frames = [Dir::North, Dir::South, Dir::East, Dir::West].map(|d| with_dir(frame, "facing", d));
        let mut eyes = [false; 12];
        let mut all = true;
        for e in eyes.iter_mut() {
            *e = ctx.r.next_f32() > 0.9;
            all &= *e;
        }
        let spots = [(0, 4, 8), (0, 5, 8), (0, 6, 8), (1, 4, 12), (1, 5, 12), (1, 6, 12), (2, 3, 9), (2, 3, 10), (2, 3, 11), (3, 7, 9), (3, 7, 10), (3, 7, 11)];
        for (i, (f, x, z)) in spots.into_iter().enumerate() {
            b.place_block(ctx, with_bool(frames[f], "eye", eyes[i]), x, 3, z);
        }
        if all {
            let portal = st("minecraft:end_portal");
            for z in 9..=11 {
                for x in 4..=6 {
                    b.place_block(ctx, portal, x, 3, z);
                }
            }
        }
        if !spawner.load(Ordering::Relaxed) {
            let p = b.world_pos(5, 3, 6);
            if ctx.chunk_box.is_inside(p) {
                spawner.store(true, Ordering::Relaxed);
                ctx.set(p, st("minecraft:spawner"));
                ctx.set_spawner(p, "minecraft:silverfish");
            }
        }
    }
}
