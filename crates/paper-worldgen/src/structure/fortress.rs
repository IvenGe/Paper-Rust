//! Nether fortresses: bridges, crossings and corridors with blaze spawners, nether wart
//! and loot chests.
//!
//! Written to vanilla 26.3's `NetherFortressStructure` and `NetherFortressPieces` (the
//! block placement was translated from its code); the layout algorithm also follows
//! `pumpkin-world/src/generation/structure/structures/nether_fortress/` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, whose layouts match vanilla's.

use super::piece::{st, with_bool, with_dir, PieceBase, PlaceCtx};
use super::{BoundingBox, GenCtx, Piece, StructureGen};
use crate::feature::world::{Dir, Pos};
use crate::random::{LegacyRandom, Random, WorldgenRandom};
use paper_data::blocks::AIR;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct Fortress;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Type {
    BridgeStraight,
    BridgeCrossing,
    RoomCrossing,
    StairsRoom,
    MonsterThrone,
    CastleEntrance,
    SmallCorridor,
    SmallCorridorCrossing,
    SmallCorridorRightTurn,
    SmallCorridorLeftTurn,
    CorridorStairs,
    TBalcony,
    StalkRoom,
}

enum Kind {
    BridgeStraight,
    BridgeEndFiller { seed: i32 },
    BridgeCrossing,
    RoomCrossing,
    StairsRoom,
    MonsterThrone { spawner: AtomicBool },
    CastleEntrance,
    StalkRoom,
    SmallCorridor,
    SmallCorridorCrossing,
    SmallCorridorRightTurn { chest: AtomicBool },
    SmallCorridorLeftTurn { chest: AtomicBool },
    CorridorStairs,
    TBalcony,
}

struct FPiece {
    base: PieceBase,
    kind: Kind,
}

/// `NetherFortressPieces.PieceWeight`.
struct Weight {
    t: Type,
    weight: i32,
    max: i32,
    count: i32,
    allow_in_row: bool,
}

impl Weight {
    fn do_place(&self) -> bool {
        self.max == 0 || self.count < self.max
    }
}

fn bridge_weights() -> Vec<Weight> {
    let w = |t, weight, max, allow_in_row| Weight { t, weight, max, count: 0, allow_in_row };
    vec![
        w(Type::BridgeStraight, 30, 0, true),
        w(Type::BridgeCrossing, 10, 4, false),
        w(Type::RoomCrossing, 10, 4, false),
        w(Type::StairsRoom, 10, 3, false),
        w(Type::MonsterThrone, 5, 2, false),
        w(Type::CastleEntrance, 5, 1, false),
    ]
}

fn castle_weights() -> Vec<Weight> {
    let w = |t, weight, max, allow_in_row| Weight { t, weight, max, count: 0, allow_in_row };
    vec![
        w(Type::SmallCorridor, 25, 0, true),
        w(Type::SmallCorridorCrossing, 15, 5, false),
        w(Type::SmallCorridorRightTurn, 5, 10, false),
        w(Type::SmallCorridorLeftTurn, 5, 10, false),
        w(Type::CorridorStairs, 10, 3, true),
        w(Type::TBalcony, 7, 2, false),
        w(Type::StalkRoom, 5, 2, false),
    ]
}

struct Builder {
    pieces: Vec<FPiece>,
    pending: Vec<usize>,
    bridge: Vec<Weight>,
    castle: Vec<Weight>,
    previous: Option<Type>,
}

impl Builder {
    fn free(&self, b: &BoundingBox) -> bool {
        b.min_y > 10 && !self.pieces.iter().any(|p| p.base.bbox.intersects(b))
    }

    /// `BridgeEndFiller.createPiece`.
    #[allow(clippy::too_many_arguments)]
    fn end_filler(&self, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, d: Dir, depth: i32) -> Option<FPiece> {
        let b = BoundingBox::orient(x, y, z, -1, -3, 0, 5, 10, 8, d);
        if !self.free(&b) {
            return None;
        }
        let seed = r.next_i32();
        Some(FPiece { base: PieceBase::new("minecraft:nebef", depth, b).with_orientation(Some(d)), kind: Kind::BridgeEndFiller { seed } })
    }

    /// `findAndCreateBridgePieceFactory`.
    #[allow(clippy::too_many_arguments)]
    fn create(&self, t: Type, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, d: Dir, depth: i32) -> Option<FPiece> {
        let (id, b) = match t {
            Type::BridgeStraight => ("minecraft:nebs", BoundingBox::orient(x, y, z, -1, -3, 0, 5, 10, 19, d)),
            Type::BridgeCrossing => ("minecraft:nebcr", BoundingBox::orient(x, y, z, -8, -3, 0, 19, 10, 19, d)),
            Type::RoomCrossing => ("minecraft:nerc", BoundingBox::orient(x, y, z, -2, 0, 0, 7, 9, 7, d)),
            Type::StairsRoom => ("minecraft:nesr", BoundingBox::orient(x, y, z, -2, 0, 0, 7, 11, 7, d)),
            Type::MonsterThrone => ("minecraft:nemt", BoundingBox::orient(x, y, z, -2, 0, 0, 7, 8, 9, d)),
            Type::CastleEntrance => ("minecraft:nece", BoundingBox::orient(x, y, z, -5, -3, 0, 13, 14, 13, d)),
            Type::SmallCorridor => ("minecraft:nesc", BoundingBox::orient(x, y, z, -1, 0, 0, 5, 7, 5, d)),
            Type::SmallCorridorCrossing => ("minecraft:nescsc", BoundingBox::orient(x, y, z, -1, 0, 0, 5, 7, 5, d)),
            Type::SmallCorridorRightTurn => ("minecraft:nescrt", BoundingBox::orient(x, y, z, -1, 0, 0, 5, 7, 5, d)),
            Type::SmallCorridorLeftTurn => ("minecraft:nesclt", BoundingBox::orient(x, y, z, -1, 0, 0, 5, 7, 5, d)),
            Type::CorridorStairs => ("minecraft:neccs", BoundingBox::orient(x, y, z, -1, -7, 0, 5, 14, 10, d)),
            Type::TBalcony => ("minecraft:nectb", BoundingBox::orient(x, y, z, -3, 0, 0, 9, 7, 9, d)),
            Type::StalkRoom => ("minecraft:necsr", BoundingBox::orient(x, y, z, -5, -3, 0, 13, 14, 13, d)),
        };
        if !self.free(&b) {
            return None;
        }
        let kind = match t {
            Type::BridgeStraight => Kind::BridgeStraight,
            Type::BridgeCrossing => Kind::BridgeCrossing,
            Type::RoomCrossing => Kind::RoomCrossing,
            Type::StairsRoom => Kind::StairsRoom,
            Type::MonsterThrone => Kind::MonsterThrone { spawner: AtomicBool::new(false) },
            Type::CastleEntrance => Kind::CastleEntrance,
            Type::SmallCorridor => Kind::SmallCorridor,
            Type::SmallCorridorCrossing => Kind::SmallCorridorCrossing,
            Type::SmallCorridorRightTurn => Kind::SmallCorridorRightTurn { chest: AtomicBool::new(r.next_bounded(3) == 0) },
            Type::SmallCorridorLeftTurn => Kind::SmallCorridorLeftTurn { chest: AtomicBool::new(r.next_bounded(3) == 0) },
            Type::CorridorStairs => Kind::CorridorStairs,
            Type::TBalcony => Kind::TBalcony,
            Type::StalkRoom => Kind::StalkRoom,
        };
        Some(FPiece { base: PieceBase::new(id, depth, b).with_orientation(Some(d)), kind })
    }

    /// `NetherBridgePiece.generatePiece`.
    #[allow(clippy::too_many_arguments)]
    fn generate_piece(&mut self, castle: bool, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, d: Dir, depth: i32) -> Option<FPiece> {
        let weights = if castle { &self.castle } else { &self.bridge };
        let mut total = 0;
        let mut any = false;
        for w in weights {
            if w.max > 0 && w.count < w.max {
                any = true;
            }
            total += w.weight;
        }
        let total = if any { total } else { -1 };
        let go = total > 0 && depth <= 30;
        let mut attempts = 0;
        while attempts < 5 && go {
            attempts += 1;
            let mut selection = r.next_bounded(total);
            let len = if castle { self.castle.len() } else { self.bridge.len() };
            let mut i = 0;
            while i < len {
                let w = if castle { &self.castle[i] } else { &self.bridge[i] };
                selection -= w.weight;
                if selection < 0 {
                    if !w.do_place() || (Some(w.t) == self.previous && !w.allow_in_row) {
                        break;
                    }
                    let t = w.t;
                    if let Some(p) = self.create(t, r, x, y, z, d, depth) {
                        let list = if castle { &mut self.castle } else { &mut self.bridge };
                        list[i].count += 1;
                        self.previous = Some(t);
                        if !list[i].do_place() {
                            list.remove(i);
                        }
                        return Some(p);
                    }
                }
                i += 1;
            }
        }
        self.end_filler(r, x, y, z, d, depth)
    }

    /// `generateAndAddPiece`.
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, r: &mut WorldgenRandom, x: i32, y: i32, z: i32, d: Dir, depth: i32, castle: bool) {
        let sb = self.pieces[0].base.bbox;
        if (x - sb.min_x).abs() > 112 || (z - sb.min_z).abs() > 112 {
            // Created (drawing its seed) but not added, as vanilla does.
            let _ = self.end_filler(r, x, y, z, d, depth);
            return;
        }
        if let Some(p) = self.generate_piece(castle, r, x, y, z, d, depth + 1) {
            self.pieces.push(p);
            self.pending.push(self.pieces.len() - 1);
        }
    }

    fn forward(&mut self, i: usize, r: &mut WorldgenRandom, x_off: i32, y_off: i32, castle: bool) {
        let b = self.pieces[i].base.bbox;
        let depth = self.pieces[i].base.gen_depth;
        match self.pieces[i].base.orientation {
            Some(Dir::North) => self.add(r, b.min_x + x_off, b.min_y + y_off, b.min_z - 1, Dir::North, depth, castle),
            Some(Dir::South) => self.add(r, b.min_x + x_off, b.min_y + y_off, b.max_z + 1, Dir::South, depth, castle),
            Some(Dir::West) => self.add(r, b.min_x - 1, b.min_y + y_off, b.min_z + x_off, Dir::West, depth, castle),
            Some(Dir::East) => self.add(r, b.max_x + 1, b.min_y + y_off, b.min_z + x_off, Dir::East, depth, castle),
            _ => {}
        }
    }

    fn left(&mut self, i: usize, r: &mut WorldgenRandom, y_off: i32, z_off: i32, castle: bool) {
        let b = self.pieces[i].base.bbox;
        let depth = self.pieces[i].base.gen_depth;
        match self.pieces[i].base.orientation {
            Some(Dir::North | Dir::South) => self.add(r, b.min_x - 1, b.min_y + y_off, b.min_z + z_off, Dir::West, depth, castle),
            Some(Dir::West | Dir::East) => self.add(r, b.min_x + z_off, b.min_y + y_off, b.min_z - 1, Dir::North, depth, castle),
            _ => {}
        }
    }

    fn right(&mut self, i: usize, r: &mut WorldgenRandom, y_off: i32, z_off: i32, castle: bool) {
        let b = self.pieces[i].base.bbox;
        let depth = self.pieces[i].base.gen_depth;
        match self.pieces[i].base.orientation {
            Some(Dir::North | Dir::South) => self.add(r, b.max_x + 1, b.min_y + y_off, b.min_z + z_off, Dir::East, depth, castle),
            Some(Dir::West | Dir::East) => self.add(r, b.min_x + z_off, b.min_y + y_off, b.max_z + 1, Dir::South, depth, castle),
            _ => {}
        }
    }

    /// `addChildren` of piece `i`.
    fn add_children(&mut self, i: usize, r: &mut WorldgenRandom) {
        match self.pieces[i].kind {
            Kind::BridgeStraight => self.forward(i, r, 1, 3, false),
            Kind::BridgeCrossing => {
                self.forward(i, r, 8, 3, false);
                self.left(i, r, 3, 8, false);
                self.right(i, r, 3, 8, false);
            }
            Kind::RoomCrossing => {
                self.forward(i, r, 2, 0, false);
                self.left(i, r, 0, 2, false);
                self.right(i, r, 0, 2, false);
            }
            Kind::StairsRoom => self.right(i, r, 6, 2, false),
            Kind::CastleEntrance => self.forward(i, r, 5, 3, true),
            Kind::StalkRoom => {
                self.forward(i, r, 5, 3, true);
                self.forward(i, r, 5, 11, true);
            }
            Kind::SmallCorridor | Kind::CorridorStairs => self.forward(i, r, 1, 0, true),
            Kind::SmallCorridorCrossing => {
                self.forward(i, r, 1, 0, true);
                self.left(i, r, 0, 1, true);
                self.right(i, r, 0, 1, true);
            }
            Kind::SmallCorridorRightTurn { .. } => self.right(i, r, 0, 1, true),
            Kind::SmallCorridorLeftTurn { .. } => self.left(i, r, 0, 1, true),
            Kind::TBalcony => {
                let z_off = if matches!(self.pieces[i].base.orientation, Some(Dir::West | Dir::North)) { 5 } else { 1 };
                let castle = r.next_bounded(8) > 0;
                self.left(i, r, 0, z_off, castle);
                let castle = r.next_bounded(8) > 0;
                self.right(i, r, 0, z_off, castle);
            }
            Kind::BridgeEndFiller { .. } | Kind::MonsterThrone { .. } => {}
        }
    }
}

impl StructureGen for Fortress {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        if !ctx.valid_biome(Pos::new(ctx.min_block_x(), 64, ctx.min_block_z())) {
            return None;
        }
        let (x, z) = (ctx.min_block_x() + 2, ctx.min_block_z() + 2);
        let r = &mut ctx.r;
        let d = Dir::random_horizontal(r);
        let bbox = BoundingBox::make(x, 64, z, d, 19, 10, 19);
        let mut b = Builder { pieces: Vec::new(), pending: Vec::new(), bridge: bridge_weights(), castle: castle_weights(), previous: None };
        b.pieces.push(FPiece { base: PieceBase::new("minecraft:nebcr", 0, bbox).with_orientation(Some(d)), kind: Kind::BridgeCrossing });
        b.add_children(0, r);
        while !b.pending.is_empty() {
            let i = r.next_bounded(b.pending.len() as i32) as usize;
            let p = b.pending.remove(i);
            b.add_children(p, r);
        }
        // moveInsideHeights(random, 48, 70)
        let all = BoundingBox::encapsulating(b.pieces.iter().map(|p| p.base.bbox)).expect("start piece");
        let span = 70 - 48 + 1 - all.y_span();
        let y0 = if span > 1 { 48 + r.next_bounded(span) } else { 48 };
        let dy = y0 - all.min_y;
        for p in &mut b.pieces {
            p.base.bbox.move_by(0, dy, 0);
        }
        Some(b.pieces.into_iter().map(|p| Box::new(p) as Box<dyn Piece>).collect())
    }
}

impl Piece for FPiece {
    fn base(&self) -> &PieceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PieceBase {
        &mut self.base
    }

    fn post_process(&self, ctx: &mut PlaceCtx) {
        let b = &self.base;
        let nb = st("minecraft:nether_bricks");
        let fence = st("minecraft:nether_brick_fence");
        match &self.kind {
            Kind::BridgeStraight => bridge_straight(b, ctx, nb, fence),
            Kind::BridgeEndFiller { seed } => bridge_end_filler(b, ctx, nb, *seed),
            Kind::BridgeCrossing => bridge_crossing(b, ctx, nb),
            Kind::RoomCrossing => room_crossing(b, ctx, nb, fence),
            Kind::StairsRoom => stairs_room(b, ctx, nb, fence),
            Kind::MonsterThrone { spawner } => monster_throne(b, ctx, nb, fence, spawner),
            Kind::CastleEntrance => castle_entrance(b, ctx, nb, fence),
            Kind::StalkRoom => stalk_room(b, ctx, nb, fence),
            Kind::SmallCorridor => small_corridor(b, ctx, nb, fence),
            Kind::SmallCorridorCrossing => small_corridor_crossing(b, ctx, nb),
            Kind::SmallCorridorRightTurn { chest } => small_corridor_right_turn(b, ctx, nb, fence, chest),
            Kind::SmallCorridorLeftTurn { chest } => small_corridor_left_turn(b, ctx, nb, fence, chest),
            Kind::CorridorStairs => corridor_stairs(b, ctx, nb, fence),
            Kind::TBalcony => t_balcony(b, ctx, nb, fence),
        }
    }
}


fn bridge_straight(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32) {
    b.generate_box(ctx, 0, 3, 0, 4, 4, 18, nb, nb, false);
    b.generate_box(ctx, 1, 5, 0, 3, 7, 18, AIR, AIR, false);
    b.generate_box(ctx, 0, 5, 0, 0, 5, 18, nb, nb, false);
    b.generate_box(ctx, 4, 5, 0, 4, 5, 18, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 4, 2, 5, nb, nb, false);
    b.generate_box(ctx, 0, 2, 13, 4, 2, 18, nb, nb, false);
    b.generate_box(ctx, 0, 0, 0, 4, 1, 3, nb, nb, false);
    b.generate_box(ctx, 0, 0, 15, 4, 1, 18, nb, nb, false);
    for ns_fence_x in 0..=4 {
        for nse_fence_z in 0..=2 {
            b.fill_column_down(ctx, nb, ns_fence_x, -1, nse_fence_z);
            b.fill_column_down(ctx, nb, ns_fence_x, -1, 18 - nse_fence_z);
        }
    }
    let ns_fence_x = with_bool(with_bool(fence, "north", true), "south", true);
    let nse_fence_z = with_bool(ns_fence_x, "east", true);
    let nsw_fence = with_bool(ns_fence_x, "west", true);
    b.generate_box(ctx, 0, 1, 1, 0, 4, 1, nse_fence_z, nse_fence_z, false);
    b.generate_box(ctx, 0, 3, 4, 0, 4, 4, nse_fence_z, nse_fence_z, false);
    b.generate_box(ctx, 0, 3, 14, 0, 4, 14, nse_fence_z, nse_fence_z, false);
    b.generate_box(ctx, 0, 1, 17, 0, 4, 17, nse_fence_z, nse_fence_z, false);
    b.generate_box(ctx, 4, 1, 1, 4, 4, 1, nsw_fence, nsw_fence, false);
    b.generate_box(ctx, 4, 3, 4, 4, 4, 4, nsw_fence, nsw_fence, false);
    b.generate_box(ctx, 4, 3, 14, 4, 4, 14, nsw_fence, nsw_fence, false);
    b.generate_box(ctx, 4, 1, 17, 4, 4, 17, nsw_fence, nsw_fence, false);
}

fn bridge_end_filler(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, seed: i32) {
    // `RandomSource.createThreadLocalInstance(seed)`: a java.util.Random LCG.
    let mut self_random = LegacyRandom::new(seed as i64);
    for x_z in 0..=4 {
        for y_z in 3..=4 {
            let z = self_random.next_bounded(8);
            b.generate_box(ctx, x_z, y_z, 0, x_z, y_z, z, nb, nb, false);
        }
    }
    let x_z = self_random.next_bounded(8);
    b.generate_box(ctx, 0, 5, 0, 0, 5, x_z, nb, nb, false);
    let x_z = self_random.next_bounded(8);
    b.generate_box(ctx, 4, 5, 0, 4, 5, x_z, nb, nb, false);
    for x_z in 0..=4 {
        let y_z = self_random.next_bounded(5);
        b.generate_box(ctx, x_z, 2, 0, x_z, 2, y_z, nb, nb, false);
    }
    for x_z in 0..=4 {
        for y_z in 0..=1 {
            let z = self_random.next_bounded(3);
            b.generate_box(ctx, x_z, y_z, 0, x_z, y_z, z, nb, nb, false);
        }
    }
}

fn bridge_crossing(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32) {
    b.generate_box(ctx, 7, 3, 0, 11, 4, 18, nb, nb, false);
    b.generate_box(ctx, 0, 3, 7, 18, 4, 11, nb, nb, false);
    b.generate_box(ctx, 8, 5, 0, 10, 7, 18, AIR, AIR, false);
    b.generate_box(ctx, 0, 5, 8, 18, 7, 10, AIR, AIR, false);
    b.generate_box(ctx, 7, 5, 0, 7, 5, 7, nb, nb, false);
    b.generate_box(ctx, 7, 5, 11, 7, 5, 18, nb, nb, false);
    b.generate_box(ctx, 11, 5, 0, 11, 5, 7, nb, nb, false);
    b.generate_box(ctx, 11, 5, 11, 11, 5, 18, nb, nb, false);
    b.generate_box(ctx, 0, 5, 7, 7, 5, 7, nb, nb, false);
    b.generate_box(ctx, 11, 5, 7, 18, 5, 7, nb, nb, false);
    b.generate_box(ctx, 0, 5, 11, 7, 5, 11, nb, nb, false);
    b.generate_box(ctx, 11, 5, 11, 18, 5, 11, nb, nb, false);
    b.generate_box(ctx, 7, 2, 0, 11, 2, 5, nb, nb, false);
    b.generate_box(ctx, 7, 2, 13, 11, 2, 18, nb, nb, false);
    b.generate_box(ctx, 7, 0, 0, 11, 1, 3, nb, nb, false);
    b.generate_box(ctx, 7, 0, 15, 11, 1, 18, nb, nb, false);
    for x in 7..=11 {
        for z in 0..=2 {
            b.fill_column_down(ctx, nb, x, -1, z);
            b.fill_column_down(ctx, nb, x, -1, 18 - z);
        }
    }
    b.generate_box(ctx, 0, 2, 7, 5, 2, 11, nb, nb, false);
    b.generate_box(ctx, 13, 2, 7, 18, 2, 11, nb, nb, false);
    b.generate_box(ctx, 0, 0, 7, 3, 1, 11, nb, nb, false);
    b.generate_box(ctx, 15, 0, 7, 18, 1, 11, nb, nb, false);
    for x in 0..=2 {
        for z in 7..=11 {
            b.fill_column_down(ctx, nb, x, -1, z);
            b.fill_column_down(ctx, nb, 18 - x, -1, z);
        }
    }
}

fn room_crossing(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32) {
    b.generate_box(ctx, 0, 0, 0, 6, 1, 6, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 6, 7, 6, AIR, AIR, false);
    b.generate_box(ctx, 0, 2, 0, 1, 6, 0, nb, nb, false);
    b.generate_box(ctx, 0, 2, 6, 1, 6, 6, nb, nb, false);
    b.generate_box(ctx, 5, 2, 0, 6, 6, 0, nb, nb, false);
    b.generate_box(ctx, 5, 2, 6, 6, 6, 6, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 0, 6, 1, nb, nb, false);
    b.generate_box(ctx, 0, 2, 5, 0, 6, 6, nb, nb, false);
    b.generate_box(ctx, 6, 2, 0, 6, 6, 1, nb, nb, false);
    b.generate_box(ctx, 6, 2, 5, 6, 6, 6, nb, nb, false);
    let we_fence = with_bool(with_bool(fence, "west", true), "east", true);
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    b.generate_box(ctx, 2, 6, 0, 4, 6, 0, nb, nb, false);
    b.generate_box(ctx, 2, 5, 0, 4, 5, 0, we_fence, we_fence, false);
    b.generate_box(ctx, 2, 6, 6, 4, 6, 6, nb, nb, false);
    b.generate_box(ctx, 2, 5, 6, 4, 5, 6, we_fence, we_fence, false);
    b.generate_box(ctx, 0, 6, 2, 0, 6, 4, nb, nb, false);
    b.generate_box(ctx, 0, 5, 2, 0, 5, 4, ns_fence, ns_fence, false);
    b.generate_box(ctx, 6, 6, 2, 6, 6, 4, nb, nb, false);
    b.generate_box(ctx, 6, 5, 2, 6, 5, 4, ns_fence, ns_fence, false);
    for x in 0..=6 {
        for z in 0..=6 {
            b.fill_column_down(ctx, nb, x, -1, z);
        }
    }
}

fn stairs_room(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32) {
    b.generate_box(ctx, 0, 0, 0, 6, 1, 6, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 6, 10, 6, AIR, AIR, false);
    b.generate_box(ctx, 0, 2, 0, 1, 8, 0, nb, nb, false);
    b.generate_box(ctx, 5, 2, 0, 6, 8, 0, nb, nb, false);
    b.generate_box(ctx, 0, 2, 1, 0, 8, 6, nb, nb, false);
    b.generate_box(ctx, 6, 2, 1, 6, 8, 6, nb, nb, false);
    b.generate_box(ctx, 1, 2, 6, 5, 8, 6, nb, nb, false);
    let we_fence = with_bool(with_bool(fence, "west", true), "east", true);
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    b.generate_box(ctx, 0, 3, 2, 0, 5, 4, ns_fence, ns_fence, false);
    b.generate_box(ctx, 6, 3, 2, 6, 5, 2, ns_fence, ns_fence, false);
    b.generate_box(ctx, 6, 3, 4, 6, 5, 4, ns_fence, ns_fence, false);
    b.place_block(ctx, nb, 5, 2, 5);
    b.generate_box(ctx, 4, 2, 5, 4, 3, 5, nb, nb, false);
    b.generate_box(ctx, 3, 2, 5, 3, 4, 5, nb, nb, false);
    b.generate_box(ctx, 2, 2, 5, 2, 5, 5, nb, nb, false);
    b.generate_box(ctx, 1, 2, 5, 1, 6, 5, nb, nb, false);
    b.generate_box(ctx, 1, 7, 1, 5, 7, 4, nb, nb, false);
    b.generate_box(ctx, 6, 8, 2, 6, 8, 4, AIR, AIR, false);
    b.generate_box(ctx, 2, 6, 0, 4, 8, 0, nb, nb, false);
    b.generate_box(ctx, 2, 5, 0, 4, 5, 0, we_fence, we_fence, false);
    for x in 0..=6 {
        for z in 0..=6 {
            b.fill_column_down(ctx, nb, x, -1, z);
        }
    }
}

fn monster_throne(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32, spawner: &AtomicBool) {
    b.generate_box(ctx, 0, 2, 0, 6, 7, 7, AIR, AIR, false);
    b.generate_box(ctx, 1, 0, 0, 5, 1, 7, nb, nb, false);
    b.generate_box(ctx, 1, 2, 1, 5, 2, 7, nb, nb, false);
    b.generate_box(ctx, 1, 3, 2, 5, 3, 7, nb, nb, false);
    b.generate_box(ctx, 1, 4, 3, 5, 4, 7, nb, nb, false);
    b.generate_box(ctx, 1, 2, 0, 1, 4, 2, nb, nb, false);
    b.generate_box(ctx, 5, 2, 0, 5, 4, 2, nb, nb, false);
    b.generate_box(ctx, 1, 5, 2, 1, 5, 3, nb, nb, false);
    b.generate_box(ctx, 5, 5, 2, 5, 5, 3, nb, nb, false);
    b.generate_box(ctx, 0, 5, 3, 0, 5, 8, nb, nb, false);
    b.generate_box(ctx, 6, 5, 3, 6, 5, 8, nb, nb, false);
    b.generate_box(ctx, 1, 5, 8, 5, 5, 8, nb, nb, false);
    let we_fence = with_bool(with_bool(fence, "west", true), "east", true);
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    b.place_block(ctx, with_bool(fence, "west", true), 1, 6, 3);
    b.place_block(ctx, with_bool(fence, "east", true), 5, 6, 3);
    b.place_block(ctx, with_bool(with_bool(fence, "east", true), "north", true), 0, 6, 3);
    b.place_block(ctx, with_bool(with_bool(fence, "west", true), "north", true), 6, 6, 3);
    b.generate_box(ctx, 0, 6, 4, 0, 6, 7, ns_fence, ns_fence, false);
    b.generate_box(ctx, 6, 6, 4, 6, 6, 7, ns_fence, ns_fence, false);
    b.place_block(ctx, with_bool(with_bool(fence, "east", true), "south", true), 0, 6, 8);
    b.place_block(ctx, with_bool(with_bool(fence, "west", true), "south", true), 6, 6, 8);
    b.generate_box(ctx, 1, 6, 8, 5, 6, 8, we_fence, we_fence, false);
    b.place_block(ctx, with_bool(fence, "east", true), 1, 7, 8);
    b.generate_box(ctx, 2, 7, 8, 4, 7, 8, we_fence, we_fence, false);
    b.place_block(ctx, with_bool(fence, "west", true), 5, 7, 8);
    b.place_block(ctx, with_bool(fence, "east", true), 2, 8, 8);
    b.place_block(ctx, we_fence, 3, 8, 8);
    b.place_block(ctx, with_bool(fence, "west", true), 4, 8, 8);
    if !spawner.load(Ordering::Relaxed) {
        let p = b.world_pos(3, 5, 5);
        if ctx.chunk_box.is_inside(p) {
            spawner.store(true, Ordering::Relaxed);
            ctx.set(p, st("minecraft:spawner"));
            ctx.set_spawner(p, "minecraft:blaze");
        }
    }
    for pos_x in 0..=6 {
        for block_entity_z in 0..=6 {
            b.fill_column_down(ctx, nb, pos_x, -1, block_entity_z);
        }
    }
}

fn castle_entrance(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32) {
    b.generate_box(ctx, 0, 3, 0, 12, 4, 12, nb, nb, false);
    b.generate_box(ctx, 0, 5, 0, 12, 13, 12, AIR, AIR, false);
    b.generate_box(ctx, 0, 5, 0, 1, 12, 12, nb, nb, false);
    b.generate_box(ctx, 11, 5, 0, 12, 12, 12, nb, nb, false);
    b.generate_box(ctx, 2, 5, 11, 4, 12, 12, nb, nb, false);
    b.generate_box(ctx, 8, 5, 11, 10, 12, 12, nb, nb, false);
    b.generate_box(ctx, 5, 9, 11, 7, 12, 12, nb, nb, false);
    b.generate_box(ctx, 2, 5, 0, 4, 12, 1, nb, nb, false);
    b.generate_box(ctx, 8, 5, 0, 10, 12, 1, nb, nb, false);
    b.generate_box(ctx, 5, 9, 0, 7, 12, 1, nb, nb, false);
    b.generate_box(ctx, 2, 11, 2, 10, 12, 10, nb, nb, false);
    b.generate_box(ctx, 5, 8, 0, 7, 8, 0, fence, fence, false);
    let we_fence = with_bool(with_bool(fence, "west", true), "east", true);
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    for i_pos_x_z in (1..=11).step_by(2) {
        b.generate_box(ctx, i_pos_x_z, 10, 0, i_pos_x_z, 11, 0, we_fence, we_fence, false);
        b.generate_box(ctx, i_pos_x_z, 10, 12, i_pos_x_z, 11, 12, we_fence, we_fence, false);
        b.generate_box(ctx, 0, 10, i_pos_x_z, 0, 11, i_pos_x_z, ns_fence, ns_fence, false);
        b.generate_box(ctx, 12, 10, i_pos_x_z, 12, 11, i_pos_x_z, ns_fence, ns_fence, false);
        b.place_block(ctx, nb, i_pos_x_z, 13, 0);
        b.place_block(ctx, nb, i_pos_x_z, 13, 12);
        b.place_block(ctx, nb, 0, 13, i_pos_x_z);
        b.place_block(ctx, nb, 12, 13, i_pos_x_z);
        if i_pos_x_z != 11 {
            b.place_block(ctx, we_fence, i_pos_x_z + 1, 13, 0);
            b.place_block(ctx, we_fence, i_pos_x_z + 1, 13, 12);
            b.place_block(ctx, ns_fence, 0, 13, i_pos_x_z + 1);
            b.place_block(ctx, ns_fence, 12, 13, i_pos_x_z + 1);
        }
    }
    b.place_block(ctx, with_bool(with_bool(fence, "north", true), "east", true), 0, 13, 0);
    b.place_block(ctx, with_bool(with_bool(fence, "south", true), "east", true), 0, 13, 12);
    b.place_block(ctx, with_bool(with_bool(fence, "south", true), "west", true), 12, 13, 12);
    b.place_block(ctx, with_bool(with_bool(fence, "north", true), "west", true), 12, 13, 0);
    for i_pos_x_z in (3..=9).step_by(2) {
        b.generate_box(ctx, 1, 7, i_pos_x_z, 1, 8, i_pos_x_z, with_bool(ns_fence, "west", true), with_bool(ns_fence, "west", true), false);
        b.generate_box(ctx, 11, 7, i_pos_x_z, 11, 8, i_pos_x_z, with_bool(ns_fence, "east", true), with_bool(ns_fence, "east", true), false);
    }
    b.generate_box(ctx, 4, 2, 0, 8, 2, 12, nb, nb, false);
    b.generate_box(ctx, 0, 2, 4, 12, 2, 8, nb, nb, false);
    b.generate_box(ctx, 4, 0, 0, 8, 1, 3, nb, nb, false);
    b.generate_box(ctx, 4, 0, 9, 8, 1, 12, nb, nb, false);
    b.generate_box(ctx, 0, 0, 4, 3, 1, 8, nb, nb, false);
    b.generate_box(ctx, 9, 0, 4, 12, 1, 8, nb, nb, false);
    for i_pos_x_z in 4..=8 {
        for z in 0..=2 {
            b.fill_column_down(ctx, nb, i_pos_x_z, -1, z);
            b.fill_column_down(ctx, nb, i_pos_x_z, -1, 12 - z);
        }
    }
    for i_pos_x_z in 0..=2 {
        for z in 4..=8 {
            b.fill_column_down(ctx, nb, i_pos_x_z, -1, z);
            b.fill_column_down(ctx, nb, 12 - i_pos_x_z, -1, z);
        }
    }
    b.generate_box(ctx, 5, 5, 5, 7, 5, 7, nb, nb, false);
    b.generate_box(ctx, 6, 1, 6, 6, 4, 6, AIR, AIR, false);
    b.place_block(ctx, nb, 6, 0, 6);
    b.place_block(ctx, st("minecraft:lava"), 6, 5, 6);
}

fn stalk_room(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32) {
    b.generate_box(ctx, 0, 3, 0, 12, 4, 12, nb, nb, false);
    b.generate_box(ctx, 0, 5, 0, 12, 13, 12, AIR, AIR, false);
    b.generate_box(ctx, 0, 5, 0, 1, 12, 12, nb, nb, false);
    b.generate_box(ctx, 11, 5, 0, 12, 12, 12, nb, nb, false);
    b.generate_box(ctx, 2, 5, 11, 4, 12, 12, nb, nb, false);
    b.generate_box(ctx, 8, 5, 11, 10, 12, 12, nb, nb, false);
    b.generate_box(ctx, 5, 9, 11, 7, 12, 12, nb, nb, false);
    b.generate_box(ctx, 2, 5, 0, 4, 12, 1, nb, nb, false);
    b.generate_box(ctx, 8, 5, 0, 10, 12, 1, nb, nb, false);
    b.generate_box(ctx, 5, 9, 0, 7, 12, 1, nb, nb, false);
    b.generate_box(ctx, 2, 11, 2, 10, 12, 10, nb, nb, false);
    let we_fence = with_bool(with_bool(fence, "west", true), "east", true);
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    let nsw_fence = with_bool(ns_fence, "west", true);
    let nse_fence = with_bool(ns_fence, "east", true);
    for stairs in (1..=11).step_by(2) {
        b.generate_box(ctx, stairs, 10, 0, stairs, 11, 0, we_fence, we_fence, false);
        b.generate_box(ctx, stairs, 10, 12, stairs, 11, 12, we_fence, we_fence, false);
        b.generate_box(ctx, 0, 10, stairs, 0, 11, stairs, ns_fence, ns_fence, false);
        b.generate_box(ctx, 12, 10, stairs, 12, 11, stairs, ns_fence, ns_fence, false);
        b.place_block(ctx, nb, stairs, 13, 0);
        b.place_block(ctx, nb, stairs, 13, 12);
        b.place_block(ctx, nb, 0, 13, stairs);
        b.place_block(ctx, nb, 12, 13, stairs);
        if stairs != 11 {
            b.place_block(ctx, we_fence, stairs + 1, 13, 0);
            b.place_block(ctx, we_fence, stairs + 1, 13, 12);
            b.place_block(ctx, ns_fence, 0, 13, stairs + 1);
            b.place_block(ctx, ns_fence, 12, 13, stairs + 1);
        }
    }
    b.place_block(ctx, with_bool(with_bool(fence, "north", true), "east", true), 0, 13, 0);
    b.place_block(ctx, with_bool(with_bool(fence, "south", true), "east", true), 0, 13, 12);
    b.place_block(ctx, with_bool(with_bool(fence, "south", true), "west", true), 12, 13, 12);
    b.place_block(ctx, with_bool(with_bool(fence, "north", true), "west", true), 12, 13, 0);
    for stairs in (3..=9).step_by(2) {
        b.generate_box(ctx, 1, 7, stairs, 1, 8, stairs, nsw_fence, nsw_fence, false);
        b.generate_box(ctx, 11, 7, stairs, 11, 8, stairs, nse_fence, nse_fence, false);
    }
    let stairs = with_dir(st("minecraft:nether_brick_stairs"), "facing", Dir::North);
    for i in 0..=6 {
        let z = i + 4;
        for x in 5..=7 {
            b.place_block(ctx, stairs, x, 5 + i, z);
        }
        if (5..=8).contains(&z) {
            b.generate_box(ctx, 5, 5, z, 7, i + 4, z, nb, nb, false);
        } else if (9..=10).contains(&z) {
            b.generate_box(ctx, 5, 8, z, 7, i + 4, z, nb, nb, false);
        }
        if i >= 1 {
            b.generate_box(ctx, 5, 6 + i, z, 7, 9 + i, z, AIR, AIR, false);
        }
    }
    for i in 5..=7 {
        b.place_block(ctx, stairs, i, 12, 11);
    }
    b.generate_box(ctx, 5, 6, 7, 5, 7, 7, nse_fence, nse_fence, false);
    b.generate_box(ctx, 7, 6, 7, 7, 7, 7, nsw_fence, nsw_fence, false);
    b.generate_box(ctx, 5, 13, 12, 7, 13, 12, AIR, AIR, false);
    b.generate_box(ctx, 2, 5, 2, 3, 5, 3, nb, nb, false);
    b.generate_box(ctx, 2, 5, 9, 3, 5, 10, nb, nb, false);
    b.generate_box(ctx, 2, 5, 4, 2, 5, 8, nb, nb, false);
    b.generate_box(ctx, 9, 5, 2, 10, 5, 3, nb, nb, false);
    b.generate_box(ctx, 9, 5, 9, 10, 5, 10, nb, nb, false);
    b.generate_box(ctx, 10, 5, 4, 10, 5, 8, nb, nb, false);
    let east = with_dir(stairs, "facing", Dir::East);
    let west = with_dir(stairs, "facing", Dir::West);
    b.place_block(ctx, west, 4, 5, 2);
    b.place_block(ctx, west, 4, 5, 3);
    b.place_block(ctx, west, 4, 5, 9);
    b.place_block(ctx, west, 4, 5, 10);
    b.place_block(ctx, east, 8, 5, 2);
    b.place_block(ctx, east, 8, 5, 3);
    b.place_block(ctx, east, 8, 5, 9);
    b.place_block(ctx, east, 8, 5, 10);
    b.generate_box(ctx, 3, 4, 4, 4, 4, 8, st("minecraft:soul_sand"), st("minecraft:soul_sand"), false);
    b.generate_box(ctx, 8, 4, 4, 9, 4, 8, st("minecraft:soul_sand"), st("minecraft:soul_sand"), false);
    b.generate_box(ctx, 3, 5, 4, 4, 5, 8, st("minecraft:nether_wart"), st("minecraft:nether_wart"), false);
    b.generate_box(ctx, 8, 5, 4, 9, 5, 8, st("minecraft:nether_wart"), st("minecraft:nether_wart"), false);
    b.generate_box(ctx, 4, 2, 0, 8, 2, 12, nb, nb, false);
    b.generate_box(ctx, 0, 2, 4, 12, 2, 8, nb, nb, false);
    b.generate_box(ctx, 4, 0, 0, 8, 1, 3, nb, nb, false);
    b.generate_box(ctx, 4, 0, 9, 8, 1, 12, nb, nb, false);
    b.generate_box(ctx, 0, 0, 4, 3, 1, 8, nb, nb, false);
    b.generate_box(ctx, 9, 0, 4, 12, 1, 8, nb, nb, false);
    for x in 4..=8 {
        for z in 0..=2 {
            b.fill_column_down(ctx, nb, x, -1, z);
            b.fill_column_down(ctx, nb, x, -1, 12 - z);
        }
    }
    for x in 0..=2 {
        for z in 4..=8 {
            b.fill_column_down(ctx, nb, x, -1, z);
            b.fill_column_down(ctx, nb, 12 - x, -1, z);
        }
    }
}

fn small_corridor(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32) {
    b.generate_box(ctx, 0, 0, 0, 4, 1, 4, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 4, 5, 4, AIR, AIR, false);
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    b.generate_box(ctx, 0, 2, 0, 0, 5, 4, nb, nb, false);
    b.generate_box(ctx, 4, 2, 0, 4, 5, 4, nb, nb, false);
    b.generate_box(ctx, 0, 3, 1, 0, 4, 1, ns_fence, ns_fence, false);
    b.generate_box(ctx, 0, 3, 3, 0, 4, 3, ns_fence, ns_fence, false);
    b.generate_box(ctx, 4, 3, 1, 4, 4, 1, ns_fence, ns_fence, false);
    b.generate_box(ctx, 4, 3, 3, 4, 4, 3, ns_fence, ns_fence, false);
    b.generate_box(ctx, 0, 6, 0, 4, 6, 4, nb, nb, false);
    for x in 0..=4 {
        for z in 0..=4 {
            b.fill_column_down(ctx, nb, x, -1, z);
        }
    }
}

fn small_corridor_crossing(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32) {
    b.generate_box(ctx, 0, 0, 0, 4, 1, 4, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 4, 5, 4, AIR, AIR, false);
    b.generate_box(ctx, 0, 2, 0, 0, 5, 0, nb, nb, false);
    b.generate_box(ctx, 4, 2, 0, 4, 5, 0, nb, nb, false);
    b.generate_box(ctx, 0, 2, 4, 0, 5, 4, nb, nb, false);
    b.generate_box(ctx, 4, 2, 4, 4, 5, 4, nb, nb, false);
    b.generate_box(ctx, 0, 6, 0, 4, 6, 4, nb, nb, false);
    for x in 0..=4 {
        for z in 0..=4 {
            b.fill_column_down(ctx, nb, x, -1, z);
        }
    }
}

fn small_corridor_right_turn(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32, chest: &AtomicBool) {
    b.generate_box(ctx, 0, 0, 0, 4, 1, 4, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 4, 5, 4, AIR, AIR, false);
    let we_fence = with_bool(with_bool(fence, "west", true), "east", true);
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    b.generate_box(ctx, 0, 2, 0, 0, 5, 4, nb, nb, false);
    b.generate_box(ctx, 0, 3, 1, 0, 4, 1, ns_fence, ns_fence, false);
    b.generate_box(ctx, 0, 3, 3, 0, 4, 3, ns_fence, ns_fence, false);
    b.generate_box(ctx, 4, 2, 0, 4, 5, 0, nb, nb, false);
    b.generate_box(ctx, 1, 2, 4, 4, 5, 4, nb, nb, false);
    b.generate_box(ctx, 1, 3, 4, 1, 4, 4, we_fence, we_fence, false);
    b.generate_box(ctx, 3, 3, 4, 3, 4, 4, we_fence, we_fence, false);
    if chest.load(Ordering::Relaxed) && ctx.chunk_box.is_inside(b.world_pos(1, 2, 3)) {
        {
            chest.store(false, Ordering::Relaxed);
            b.create_chest(ctx, 1, 2, 3, "minecraft:chests/nether_bridge");
        }
    }
    b.generate_box(ctx, 0, 6, 0, 4, 6, 4, nb, nb, false);
    for x in 0..=4 {
        for z in 0..=4 {
            b.fill_column_down(ctx, nb, x, -1, z);
        }
    }
}

fn small_corridor_left_turn(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32, chest: &AtomicBool) {
    b.generate_box(ctx, 0, 0, 0, 4, 1, 4, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 4, 5, 4, AIR, AIR, false);
    let we_fence = with_bool(with_bool(fence, "west", true), "east", true);
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    b.generate_box(ctx, 4, 2, 0, 4, 5, 4, nb, nb, false);
    b.generate_box(ctx, 4, 3, 1, 4, 4, 1, ns_fence, ns_fence, false);
    b.generate_box(ctx, 4, 3, 3, 4, 4, 3, ns_fence, ns_fence, false);
    b.generate_box(ctx, 0, 2, 0, 0, 5, 0, nb, nb, false);
    b.generate_box(ctx, 0, 2, 4, 3, 5, 4, nb, nb, false);
    b.generate_box(ctx, 1, 3, 4, 1, 4, 4, we_fence, we_fence, false);
    b.generate_box(ctx, 3, 3, 4, 3, 4, 4, we_fence, we_fence, false);
    if chest.load(Ordering::Relaxed) && ctx.chunk_box.is_inside(b.world_pos(3, 2, 3)) {
        {
            chest.store(false, Ordering::Relaxed);
            b.create_chest(ctx, 3, 2, 3, "minecraft:chests/nether_bridge");
        }
    }
    b.generate_box(ctx, 0, 6, 0, 4, 6, 4, nb, nb, false);
    for x in 0..=4 {
        for z in 0..=4 {
            b.fill_column_down(ctx, nb, x, -1, z);
        }
    }
}

fn corridor_stairs(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32) {
    let stairs = with_dir(st("minecraft:nether_brick_stairs"), "facing", Dir::South);
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    for step in 0..=9 {
        let floor = 1.max(7 - step);
        let roof = (floor + 5).max(14 - step).min(13);
        let z = step;
        b.generate_box(ctx, 0, 0, z, 4, floor, z, nb, nb, false);
        b.generate_box(ctx, 1, floor + 1, z, 3, roof - 1, z, AIR, AIR, false);
        if step <= 6 {
            b.place_block(ctx, stairs, 1, floor + 1, z);
            b.place_block(ctx, stairs, 2, floor + 1, z);
            b.place_block(ctx, stairs, 3, floor + 1, z);
        }
        b.generate_box(ctx, 0, roof, z, 4, roof, z, nb, nb, false);
        b.generate_box(ctx, 0, floor + 1, z, 0, roof - 1, z, nb, nb, false);
        b.generate_box(ctx, 4, floor + 1, z, 4, roof - 1, z, nb, nb, false);
        if step & 1 == 0 {
            b.generate_box(ctx, 0, floor + 2, z, 0, floor + 3, z, ns_fence, ns_fence, false);
            b.generate_box(ctx, 4, floor + 2, z, 4, floor + 3, z, ns_fence, ns_fence, false);
        }
        for x in 0..=4 {
            b.fill_column_down(ctx, nb, x, -1, z);
        }
    }
}

fn t_balcony(b: &PieceBase, ctx: &mut PlaceCtx, nb: u32, fence: u32) {
    let ns_fence = with_bool(with_bool(fence, "north", true), "south", true);
    let we_fence = with_bool(with_bool(fence, "west", true), "east", true);
    b.generate_box(ctx, 0, 0, 0, 8, 1, 8, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 8, 5, 8, AIR, AIR, false);
    b.generate_box(ctx, 0, 6, 0, 8, 6, 5, nb, nb, false);
    b.generate_box(ctx, 0, 2, 0, 2, 5, 0, nb, nb, false);
    b.generate_box(ctx, 6, 2, 0, 8, 5, 0, nb, nb, false);
    b.generate_box(ctx, 1, 3, 0, 1, 4, 0, we_fence, we_fence, false);
    b.generate_box(ctx, 7, 3, 0, 7, 4, 0, we_fence, we_fence, false);
    b.generate_box(ctx, 0, 2, 4, 8, 2, 8, nb, nb, false);
    b.generate_box(ctx, 1, 1, 4, 2, 2, 4, AIR, AIR, false);
    b.generate_box(ctx, 6, 1, 4, 7, 2, 4, AIR, AIR, false);
    b.generate_box(ctx, 1, 3, 8, 7, 3, 8, we_fence, we_fence, false);
    b.place_block(ctx, with_bool(with_bool(fence, "east", true), "south", true), 0, 3, 8);
    b.place_block(ctx, with_bool(with_bool(fence, "west", true), "south", true), 8, 3, 8);
    b.generate_box(ctx, 0, 3, 6, 0, 3, 7, ns_fence, ns_fence, false);
    b.generate_box(ctx, 8, 3, 6, 8, 3, 7, ns_fence, ns_fence, false);
    b.generate_box(ctx, 0, 3, 4, 0, 5, 5, nb, nb, false);
    b.generate_box(ctx, 8, 3, 4, 8, 5, 5, nb, nb, false);
    b.generate_box(ctx, 1, 3, 5, 2, 5, 5, nb, nb, false);
    b.generate_box(ctx, 6, 3, 5, 7, 5, 5, nb, nb, false);
    b.generate_box(ctx, 1, 4, 5, 1, 5, 5, we_fence, we_fence, false);
    b.generate_box(ctx, 7, 4, 5, 7, 5, 5, we_fence, we_fence, false);
    for z in 0..=5 {
        for x in 0..=8 {
            b.fill_column_down(ctx, nb, x, -1, z);
        }
    }
}
