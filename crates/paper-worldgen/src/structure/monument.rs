//! Ocean monuments (`OceanMonumentStructure`, `OceanMonumentPieces`).
//!
//! Written to vanilla 26.3's `OceanMonumentStructure` and `OceanMonumentPieces`: the
//! surrounding biome check, the room graph (`generateRoomGraph`), the room fitters, the
//! building and its rooms, and the three elder guardians. The block layouts were ported from
//! `pumpkin-world/src/generation/structure/structures/ocean_monument/{graph,building,rooms}.rs`
//! of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, and put back into vanilla's
//! statement order; `generateBoxOnFillOnly` compares with the default water state, as
//! vanilla does.
//!
//! As in vanilla, the start holds a single piece, the building (`minecraft:omb`); the rooms
//! are the building's children and are placed by its `postProcess`. Monuments do not adapt
//! the terrain with the beardifier: the building carves its own water around itself
//! (`generateWaterBox`) and fills pillars down to the floor. The elder guardians come out as
//! entity NBT; the guardians that spawn later come from the structure's spawn overrides
//! (see [`crate::WorldGenerator::structure_spawns_at`]), not from generation.

use super::bbox::BoundingBox;
use super::persist::read_base;
use super::piece::{mob, st, PieceBase, PlaceCtx};
use super::{GenCtx, Loader, Piece, StructureGen};
use crate::chunk::Heightmap;
use crate::feature::world::Dir;
use crate::generator::WorldGenerator;
use paper_nbt::Compound;
use crate::random::{Random, WorldgenRandom};
use anyhow::Result;
use paper_data::blocks;
use serde_json::Value;

fn base_gray() -> u32 {
    st("minecraft:prismarine")
}

fn base_light() -> u32 {
    st("minecraft:prismarine_bricks")
}

fn base_black() -> u32 {
    st("minecraft:dark_prismarine")
}

fn lamp() -> u32 {
    st("minecraft:sea_lantern")
}

/// `OceanMonumentStructure`.
pub(crate) struct OceanMonument {
    /// `#required_ocean_monument_surrounding`.
    surrounding: u128,
}

impl OceanMonument {
    pub fn load(l: &Loader) -> Result<Self> {
        Ok(Self { surrounding: l.biome_set(&Value::String("#minecraft:required_ocean_monument_surrounding".into()))? })
    }
}

impl StructureGen for OceanMonument {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        // `getBiomesWithin(blockX(9), seaLevel, blockZ(9), 29)`: every biome must surround
        // monuments.
        let (x, y, z) = (ctx.min_block_x() + 9, ctx.gen.sea_level, ctx.min_block_z() + 9);
        let r = 29;
        for qz in (z - r) >> 2..=(z + r) >> 2 {
            for qx in (x - r) >> 2..=(x + r) >> 2 {
                for qy in (y - r) >> 2..=(y + r) >> 2 {
                    if self.surrounding & (1u128 << ctx.cols.biome_at_quart(qx, qy, qz)) == 0 {
                        return None;
                    }
                }
            }
        }
        let stub = ctx.on_top_of_chunk_center(Heightmap::OceanFloorWg)?;
        if !ctx.valid_biome(stub) {
            return None;
        }
        // `createTopPiece`.
        let (x, z) = (ctx.min_block_x() - 29, ctx.min_block_z() - 29);
        let dir = Dir::random_horizontal(&mut ctx.r);
        let building = MonumentBuilding::new(&mut ctx.r, x, z, dir);
        Some(vec![Box::new(building)])
    }

    /// `regeneratePiecesAfterLoad`: vanilla saves the building alone and rebuilds it, with
    /// its rooms, from the seed and the saved orientation.
    fn load_pieces(&self, gen: &WorldGenerator, x: i32, z: i32, children: &[&Compound]) -> Result<Vec<Box<dyn Piece>>> {
        let Some(first) = children.first() else { return Ok(Vec::new()) };
        let base = read_base(first, &["minecraft:omb"])?;
        let mut r = WorldgenRandom::legacy(0);
        r.set_large_feature_seed(gen.seed, x, z);
        let random_dir = Dir::random_horizontal(&mut r);
        let dir = base.orientation.unwrap_or(random_dir);
        Ok(vec![Box::new(MonumentBuilding::new(&mut r, base.bbox.min_x, base.bbox.min_z, dir))])
    }
}

/// `OceanMonumentPieces.getRoomIndex`.
const fn room_index(x: i32, y: i32, z: i32) -> i32 {
    y * 25 + z * 5 + x
}

const SOURCE_INDEX: i32 = room_index(2, 0, 0);
const TOP_CONNECT_INDEX: i32 = room_index(2, 2, 0);
const LEFT_WING_CONNECT_INDEX: i32 = room_index(0, 1, 0);
const RIGHT_WING_CONNECT_INDEX: i32 = room_index(4, 1, 0);

/// `OceanMonumentPieces.RoomDefinition`. Directions are indexed by their 3D data value.
#[derive(Clone, Debug)]
struct Room {
    index: i32,
    connections: [Option<usize>; 6],
    openings: [bool; 6],
    claimed: bool,
    source: bool,
}

impl Room {
    fn new(index: i32) -> Self {
        Self { index, connections: [None; 6], openings: [false; 6], claimed: false, source: false }
    }

    fn open(&self, d: Dir) -> bool {
        self.openings[d.index()]
    }

    fn count_openings(&self) -> usize {
        self.openings.iter().filter(|o| **o).count()
    }

    fn is_special(&self) -> bool {
        self.index >= 75
    }
}

/// The room graph of a monument.
struct Graph {
    rooms: Vec<Room>,
}

impl Graph {
    fn push(&mut self, index: i32) -> usize {
        self.rooms.push(Room::new(index));
        self.rooms.len() - 1
    }

    /// `setConnection`.
    fn connect(&mut self, room: usize, d: Dir, other: usize) {
        self.rooms[room].connections[d.index()] = Some(other);
        self.rooms[other].connections[d.opposite().index()] = Some(room);
    }

    fn conn(&self, room: usize, d: Dir) -> usize {
        self.rooms[room].connections[d.index()].expect("monument room connection")
    }

    fn update_openings(&mut self, room: usize) {
        let r = &mut self.rooms[room];
        for i in 0..6 {
            r.openings[i] = r.connections[i].is_some();
        }
    }

    /// `findSource`: the source room is reachable through open connections.
    fn find_source(&self, start: usize) -> bool {
        let mut seen = vec![false; self.rooms.len()];
        let mut stack = vec![start];
        while let Some(room) = stack.pop() {
            let r = &self.rooms[room];
            if r.source {
                return true;
            }
            if std::mem::replace(&mut seen[room], true) {
                continue;
            }
            for i in 0..6 {
                if let (true, Some(next)) = (r.openings[i], r.connections[i]) {
                    if !seen[next] {
                        stack.push(next);
                    }
                }
            }
        }
        false
    }

    /// `MonumentBuilding.generateRoomGraph`: the graph, the source and core rooms, and the
    /// rooms in the order they are fitted.
    fn generate(r: &mut WorldgenRandom) -> (Self, usize, usize, Vec<usize>) {
        let mut g = Graph { rooms: Vec::with_capacity(49) };
        let mut grid: [Option<usize>; 75] = [None; 75];
        for (y, xs, zs) in [(0, 0..5, 0..4), (1, 0..5, 0..4), (2, 1..4, 0..2)] {
            for x in xs {
                for z in zs.clone() {
                    let i = room_index(x, y, z);
                    grid[i as usize] = Some(g.push(i));
                }
            }
        }
        let source = grid[SOURCE_INDEX as usize].expect("source room");
        for x in 0..5 {
            for z in 0..5 {
                for y in 0..3 {
                    let Some(room) = grid[room_index(x, y, z) as usize] else { continue };
                    for d in Dir::ALL {
                        let (dx, dy, dz) = d.offset();
                        let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                        if !(0..5).contains(&nx) || !(0..5).contains(&nz) || !(0..3).contains(&ny) {
                            continue;
                        }
                        if let Some(other) = grid[room_index(nx, ny, nz) as usize] {
                            // Vanilla connects rooms along z with the opposite direction.
                            g.connect(room, if nz == z { d } else { d.opposite() }, other);
                        }
                    }
                }
            }
        }
        let roof = g.push(1003);
        let left_wing = g.push(1001);
        let right_wing = g.push(1002);
        g.connect(grid[TOP_CONNECT_INDEX as usize].expect("room"), Dir::Up, roof);
        g.connect(grid[LEFT_WING_CONNECT_INDEX as usize].expect("room"), Dir::South, left_wing);
        g.connect(grid[RIGHT_WING_CONNECT_INDEX as usize].expect("room"), Dir::South, right_wing);
        for s in [roof, left_wing, right_wing] {
            g.rooms[s].claimed = true;
        }
        g.rooms[source].source = true;
        let core = grid[room_index(r.next_bounded(4), 0, 2) as usize].expect("core room");
        let east = g.conn(core, Dir::East);
        let north = g.conn(core, Dir::North);
        let north_east = g.conn(east, Dir::North);
        for room in [core, east, north, north_east, g.conn(core, Dir::Up), g.conn(east, Dir::Up), g.conn(north, Dir::Up), g.conn(north_east, Dir::Up)] {
            g.rooms[room].claimed = true;
        }
        let mut list = Vec::with_capacity(49);
        for room in grid.into_iter().flatten() {
            g.update_openings(room);
            list.push(room);
        }
        // Only the roof is updated: the wing rooms keep their side closed.
        g.update_openings(roof);
        super::jigsaw::shuffle(&mut list, r);
        for &room in &list {
            let (mut closed, mut attempts) = (0, 0);
            while closed < 2 && attempts < 5 {
                attempts += 1;
                let d = r.next_bounded(6) as usize;
                if !g.rooms[room].openings[d] {
                    continue;
                }
                let opposite = Dir::ALL[d].opposite().index();
                let other = g.rooms[room].connections[d].expect("open connection");
                g.rooms[room].openings[d] = false;
                g.rooms[other].openings[opposite] = false;
                if g.find_source(room) && g.find_source(other) {
                    closed += 1;
                } else {
                    g.rooms[room].openings[d] = true;
                    g.rooms[other].openings[opposite] = true;
                }
            }
        }
        list.extend([roof, left_wing, right_wing]);
        (g, source, core, list)
    }

    fn claim(&mut self, rooms: &[usize]) {
        for &room in rooms {
            self.rooms[room].claimed = true;
        }
    }

    fn claimed(&self, room: usize) -> bool {
        self.rooms[room].claimed
    }

    /// The room fitters in vanilla's order (`FitDoubleXYRoom`, `FitDoubleYZRoom`,
    /// `FitDoubleZRoom`, `FitDoubleXRoom`, `FitDoubleYRoom`, `FitSimpleTopRoom`,
    /// `FitSimpleRoom`): the first that fits claims its rooms.
    fn fit(&mut self, room: usize, r: &mut WorldgenRandom) -> RoomKind {
        let rm = &self.rooms[room];
        let opens_to = |d: Dir| rm.open(d) && !self.claimed(self.conn(room, d));
        if opens_to(Dir::East) && opens_to(Dir::Up) {
            let east = self.conn(room, Dir::East);
            if self.rooms[east].open(Dir::Up) && !self.claimed(self.conn(east, Dir::Up)) {
                let rooms = [room, east, self.conn(room, Dir::Up), self.conn(east, Dir::Up)];
                self.claim(&rooms);
                return RoomKind::DoubleXY;
            }
        }
        let rm = &self.rooms[room];
        let opens_to = |d: Dir| rm.open(d) && !self.claimed(self.conn(room, d));
        if opens_to(Dir::North) && opens_to(Dir::Up) {
            let north = self.conn(room, Dir::North);
            if self.rooms[north].open(Dir::Up) && !self.claimed(self.conn(north, Dir::Up)) {
                let rooms = [room, north, self.conn(room, Dir::Up), self.conn(north, Dir::Up)];
                self.claim(&rooms);
                return RoomKind::DoubleYZ;
            }
        }
        let rm = &self.rooms[room];
        let opens_to = |d: Dir| rm.open(d) && !self.claimed(self.conn(room, d));
        for (d, kind) in [(Dir::North, RoomKind::DoubleZ), (Dir::East, RoomKind::DoubleX), (Dir::Up, RoomKind::DoubleY)] {
            if opens_to(d) {
                let other = self.conn(room, d);
                self.claim(&[room, other]);
                return kind;
            }
        }
        let top = ![Dir::West, Dir::East, Dir::North, Dir::South, Dir::Up].into_iter().any(|d| rm.open(d));
        self.claim(&[room]);
        if top {
            RoomKind::SimpleTop
        } else {
            // `OceanMonumentSimpleRoom`'s `mainDesign`.
            RoomKind::Simple(r.next_bounded(3))
        }
    }
}

/// The room pieces (`OceanMonumentPiece` subclasses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoomKind {
    Entry,
    Core,
    DoubleX,
    DoubleXY,
    DoubleY,
    DoubleYZ,
    DoubleZ,
    /// With its `mainDesign`.
    Simple(i32),
    SimpleTop,
    /// With its `mainDesign` (`flag & 1`).
    Wing(i32),
    Penthouse,
}

impl RoomKind {
    fn id(self) -> &'static str {
        match self {
            Self::Entry => "minecraft:omentry",
            Self::Core => "minecraft:omcr",
            Self::DoubleX => "minecraft:omdxr",
            Self::DoubleXY => "minecraft:omdxyr",
            Self::DoubleY => "minecraft:omdyr",
            Self::DoubleYZ => "minecraft:omdyzr",
            Self::DoubleZ => "minecraft:omdzr",
            Self::Simple(_) => "minecraft:omsimple",
            Self::SimpleTop => "minecraft:omsimplet",
            Self::Wing(_) => "minecraft:omwr",
            Self::Penthouse => "minecraft:ompenthouse",
        }
    }

    /// Width, height and depth in rooms.
    fn size(self) -> (i32, i32, i32) {
        match self {
            Self::Core => (2, 2, 2),
            Self::DoubleX => (2, 1, 1),
            Self::DoubleXY => (2, 2, 1),
            Self::DoubleY => (1, 2, 1),
            Self::DoubleYZ => (1, 2, 2),
            Self::DoubleZ => (1, 1, 2),
            _ => (1, 1, 1),
        }
    }
}

/// A room of the building.
#[derive(Clone, Debug)]
struct RoomPiece {
    base: PieceBase,
    kind: RoomKind,
    /// The room of the graph (grid rooms only).
    room: usize,
}

impl RoomPiece {
    /// `OceanMonumentPiece(type, direction, genDepth, room, w, h, d)`: the box of the room's
    /// grid cell, relative to the grid origin.
    fn for_room(kind: RoomKind, dir: Dir, room: usize, index: i32) -> Self {
        let (w, h, d) = kind.size();
        let (rx, rz, ry) = (index % 5, index / 5 % 5, index / 25);
        let mut bbox = BoundingBox::make(0, 0, 0, dir, w * 8, h * 4, d * 8);
        match dir {
            Dir::North => bbox.move_by(rx * 8, ry * 4, -(rz + d) * 8 + 1),
            Dir::South => bbox.move_by(rx * 8, ry * 4, rz * 8),
            Dir::West => bbox.move_by(-(rz + d) * 8 + 1, ry * 4, rx * 8),
            _ => bbox.move_by(rz * 8, ry * 4, rx * 8),
        }
        Self { base: PieceBase::new(kind.id(), 1, bbox).with_orientation(Some(dir)), kind, room }
    }

    fn special(kind: RoomKind, dir: Dir, bbox: BoundingBox) -> Self {
        Self { base: PieceBase::new(kind.id(), 1, bbox).with_orientation(Some(dir)), kind, room: usize::MAX }
    }
}

/// `OceanMonumentPieces.MonumentBuilding`, with its rooms.
pub(crate) struct MonumentBuilding {
    base: PieceBase,
    graph: Graph,
    children: Vec<RoomPiece>,
}

impl MonumentBuilding {
    pub fn new(r: &mut WorldgenRandom, x: i32, z: i32, dir: Dir) -> Self {
        let base = PieceBase::new("minecraft:omb", 0, BoundingBox::make(x, 39, z, dir, 58, 23, 58)).with_orientation(Some(dir));
        let (mut graph, source, core, list) = Graph::generate(r);
        graph.rooms[source].claimed = true;
        let mut children = vec![
            RoomPiece::for_room(RoomKind::Entry, dir, source, graph.rooms[source].index),
            RoomPiece::for_room(RoomKind::Core, dir, core, graph.rooms[core].index),
        ];
        for room in list {
            if graph.rooms[room].claimed || graph.rooms[room].is_special() {
                continue;
            }
            let kind = graph.fit(room, r);
            children.push(RoomPiece::for_room(kind, dir, room, graph.rooms[room].index));
        }
        let origin = base.world_pos(9, 0, 22);
        for c in &mut children {
            c.base.bbox.move_by(origin.x, origin.y, origin.z);
        }
        let left = BoundingBox::from_corners(base.world_pos(1, 1, 1), base.world_pos(23, 8, 21));
        let right = BoundingBox::from_corners(base.world_pos(34, 1, 1), base.world_pos(56, 8, 21));
        let penthouse = BoundingBox::from_corners(base.world_pos(22, 13, 22), base.world_pos(35, 17, 35));
        let design = r.next_i32();
        children.push(RoomPiece::special(RoomKind::Wing(design & 1), dir, left));
        children.push(RoomPiece::special(RoomKind::Wing(design.wrapping_add(1) & 1), dir, right));
        children.push(RoomPiece::special(RoomKind::Penthouse, dir, penthouse));
        Self { base, graph, children }
    }

    /// The building's rooms: piece type, bounding box and orientation (for tests).
    #[allow(dead_code)]
    pub(crate) fn rooms(&self) -> Vec<(&'static str, BoundingBox)> {
        self.children.iter().map(|c| (c.base.kind, c.base.bbox)).collect()
    }
}

impl Piece for MonumentBuilding {
    fn base(&self) -> &PieceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PieceBase {
        &mut self.base
    }

    fn translate(&mut self, dx: i32, dy: i32, dz: i32) {
        self.base.bbox.move_by(dx, dy, dz);
        for c in &mut self.children {
            c.base.bbox.move_by(dx, dy, dz);
        }
    }

    fn post_process(&self, ctx: &mut PlaceCtx) {
        let b = &self.base;
        let height = ctx.w.sea_level.max(64) - b.bbox.min_y;
        water_box(b, ctx, 0, 0, 0, 58, height, 58);
        wing(b, ctx, false, 0);
        wing(b, ctx, true, 33);
        entrance_arches(b, ctx);
        entrance_wall(b, ctx);
        roof(b, ctx);
        lower_wall(b, ctx);
        middle_wall(b, ctx);
        upper_wall(b, ctx);
        let light = base_light();
        for i in 0..7 {
            let mut j = 0;
            while j < 7 {
                if j == 0 && i == 3 {
                    j = 6;
                }
                let (px, pz) = (i * 9, j * 9);
                for dx in 0..4 {
                    for dz in 0..4 {
                        b.place_block(ctx, light, px + dx, 0, pz + dz);
                        b.fill_column_down(ctx, light, px + dx, -1, pz + dz);
                    }
                }
                j += if i != 0 && i != 6 { 6 } else { 1 };
            }
        }
        for i in 0..5 {
            water_box(b, ctx, -1 - i, i * 2, -1 - i, -1 - i, 23, 58 + i);
            water_box(b, ctx, 58 + i, i * 2, -1 - i, 58 + i, 23, 58 + i);
            water_box(b, ctx, -i, i * 2, -1 - i, 57 + i, 23, -1 - i);
            water_box(b, ctx, -i, i * 2, 58 + i, 57 + i, 23, 58 + i);
        }
        for c in &self.children {
            if c.base.bbox.intersects(&ctx.chunk_box) {
                place_room(c, &self.graph, ctx);
            }
        }
    }
}

/// Whether the part of a local box can reach into the chunk (all the helpers below only
/// write inside the chunk box and draw no random numbers).
#[allow(clippy::too_many_arguments)]
fn reaches(b: &PieceBase, ctx: &PlaceCtx, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32) -> bool {
    BoundingBox::from_corners(b.world_pos(x0, y0, z0), b.world_pos(x1, y1, z1)).intersects(&ctx.chunk_box)
}

/// `generateBox` with one state.
#[allow(clippy::too_many_arguments)]
fn fill(b: &PieceBase, ctx: &mut PlaceCtx, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, state: u32) {
    if reaches(b, ctx, x0, y0, z0, x1, y1, z1) {
        b.generate_box(ctx, x0, y0, z0, x1, y1, z1, state, state, false);
    }
}

/// `generateWaterBox`: water below sea level and air above it, keeping ice and water.
#[allow(clippy::too_many_arguments)]
fn water_box(b: &PieceBase, ctx: &mut PlaceCtx, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32) {
    if !reaches(b, ctx, x0, y0, z0, x1, y1, z1) {
        return;
    }
    let k = &ctx.w.b.known;
    let keep = [k.ice, k.packed_ice, k.blue_ice, k.water].map(blocks::block_id_of_state);
    let water = st("minecraft:water");
    for y in y0..=y1 {
        let above_sea = b.world_y(y) >= ctx.w.sea_level;
        for x in x0..=x1 {
            for z in z0..=z1 {
                let state = b.get_block(ctx, x, y, z);
                if keep.contains(&blocks::block_id_of_state(state)) {
                    continue;
                }
                b.place_block(ctx, if above_sea && state != water { blocks::AIR } else { water }, x, y, z);
            }
        }
    }
}

/// `generateBoxOnFillOnly`: replaces the default water state only.
#[allow(clippy::too_many_arguments)]
fn fill_on_water(b: &PieceBase, ctx: &mut PlaceCtx, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, state: u32) {
    if !reaches(b, ctx, x0, y0, z0, x1, y1, z1) {
        return;
    }
    let water = st("minecraft:water");
    for y in y0..=y1 {
        for x in x0..=x1 {
            for z in z0..=z1 {
                if b.get_block(ctx, x, y, z) == water {
                    b.place_block(ctx, state, x, y, z);
                }
            }
        }
    }
}

/// `generateDefaultFloor`.
fn default_floor(b: &PieceBase, ctx: &mut PlaceCtx, x: i32, z: i32, opening: bool) {
    let (gray, light) = (base_gray(), base_light());
    if opening {
        fill(b, ctx, x, 0, z, x + 2, 0, z + 7, gray);
        fill(b, ctx, x + 5, 0, z, x + 7, 0, z + 7, gray);
        fill(b, ctx, x + 3, 0, z, x + 4, 0, z + 2, gray);
        fill(b, ctx, x + 3, 0, z + 5, x + 4, 0, z + 7, gray);
        fill(b, ctx, x + 3, 0, z + 2, x + 4, 0, z + 2, light);
        fill(b, ctx, x + 3, 0, z + 5, x + 4, 0, z + 5, light);
        fill(b, ctx, x + 2, 0, z + 3, x + 2, 0, z + 4, light);
        fill(b, ctx, x + 5, 0, z + 3, x + 5, 0, z + 4, light);
    } else {
        fill(b, ctx, x, 0, z, x + 7, 0, z + 7, gray);
    }
}

/// `chunkIntersects`.
fn chunk_intersects(b: &PieceBase, ctx: &PlaceCtx, x0: i32, z0: i32, x1: i32, z1: i32) -> bool {
    let (a, c) = (b.world_pos(x0, 0, z0), b.world_pos(x1, 0, z1));
    ctx.chunk_box.intersects_xz(a.x.min(c.x), a.z.min(c.z), a.x.max(c.x), a.z.max(c.z))
}

/// `spawnElder`: an elder guardian (persistent, at full health) as entity NBT.
fn spawn_elder(b: &PieceBase, ctx: &mut PlaceCtx, x: i32, y: i32, z: i32) {
    let p = b.world_pos(x, y, z);
    if ctx.chunk_box.is_inside(p) {
        ctx.add_entity_nbt(mob("minecraft:elder_guardian", p));
    }
}

/// `MonumentBuilding.generateWing`.
fn wing(b: &PieceBase, ctx: &mut PlaceCtx, flipped: bool, x: i32) {
    if !chunk_intersects(b, ctx, x, 0, x + 23, 20) {
        return;
    }
    let (gray, light) = (base_gray(), base_light());
    fill(b, ctx, x, 0, 0, x + 24, 0, 20, gray);
    water_box(b, ctx, x, 1, 0, x + 24, 10, 20);
    for i in 0..4 {
        fill(b, ctx, x + i, i + 1, i, x + i, i + 1, 20, light);
        fill(b, ctx, x + i + 7, i + 5, i + 7, x + i + 7, i + 5, 20, light);
        fill(b, ctx, x + 17 - i, i + 5, i + 7, x + 17 - i, i + 5, 20, light);
        fill(b, ctx, x + 24 - i, i + 1, i, x + 24 - i, i + 1, 20, light);
        fill(b, ctx, x + i + 1, i + 1, i, x + 23 - i, i + 1, i, light);
        fill(b, ctx, x + i + 8, i + 5, i + 7, x + 16 - i, i + 5, i + 7, light);
    }
    fill(b, ctx, x + 4, 4, 4, x + 6, 4, 20, gray);
    fill(b, ctx, x + 7, 4, 4, x + 17, 4, 6, gray);
    fill(b, ctx, x + 18, 4, 4, x + 20, 4, 20, gray);
    fill(b, ctx, x + 11, 8, 11, x + 13, 8, 20, gray);
    b.place_block(ctx, light, x + 12, 9, 12);
    b.place_block(ctx, light, x + 12, 9, 15);
    b.place_block(ctx, light, x + 12, 9, 18);
    let near = x + if flipped { 19 } else { 5 };
    let far = x + if flipped { 5 } else { 19 };
    for z in (5..=20).rev().step_by(3) {
        b.place_block(ctx, light, near, 5, z);
    }
    for z in (7..=19).rev().step_by(3) {
        b.place_block(ctx, light, far, 5, z);
    }
    for i in 0..4 {
        let px = if flipped { x + 24 - (17 - i * 3) } else { x + 17 - i * 3 };
        b.place_block(ctx, light, px, 5, 5);
    }
    b.place_block(ctx, light, far, 5, 5);
    fill(b, ctx, x + 11, 1, 12, x + 13, 7, 12, gray);
    fill(b, ctx, x + 12, 1, 11, x + 12, 7, 13, gray);
}

/// `MonumentBuilding.generateEntranceArchs`.
fn entrance_arches(b: &PieceBase, ctx: &mut PlaceCtx) {
    if !chunk_intersects(b, ctx, 22, 5, 35, 17) {
        return;
    }
    let (gray, light, lamp) = (base_gray(), base_light(), lamp());
    water_box(b, ctx, 25, 0, 0, 32, 8, 20);
    for i in 0..4 {
        let z = 5 + i * 4;
        fill(b, ctx, 24, 2, z, 24, 4, z, light);
        fill(b, ctx, 22, 4, z, 23, 4, z, light);
        b.place_block(ctx, light, 25, 5, z);
        b.place_block(ctx, light, 26, 6, z);
        b.place_block(ctx, lamp, 26, 5, z);
        fill(b, ctx, 33, 2, z, 33, 4, z, light);
        fill(b, ctx, 34, 4, z, 35, 4, z, light);
        b.place_block(ctx, light, 32, 5, z);
        b.place_block(ctx, light, 31, 6, z);
        b.place_block(ctx, lamp, 31, 5, z);
        fill(b, ctx, 27, 6, z, 30, 6, z, gray);
    }
}

/// `MonumentBuilding.generateEntranceWall`.
fn entrance_wall(b: &PieceBase, ctx: &mut PlaceCtx) {
    if !chunk_intersects(b, ctx, 15, 20, 42, 21) {
        return;
    }
    let (gray, light, black) = (base_gray(), base_light(), base_black());
    fill(b, ctx, 15, 0, 21, 42, 0, 21, gray);
    water_box(b, ctx, 26, 1, 21, 31, 3, 21);
    fill(b, ctx, 21, 12, 21, 36, 12, 21, gray);
    fill(b, ctx, 17, 11, 21, 40, 11, 21, gray);
    fill(b, ctx, 16, 10, 21, 41, 10, 21, gray);
    fill(b, ctx, 15, 7, 21, 42, 9, 21, gray);
    fill(b, ctx, 16, 6, 21, 41, 6, 21, gray);
    fill(b, ctx, 17, 5, 21, 40, 5, 21, gray);
    fill(b, ctx, 21, 4, 21, 36, 4, 21, gray);
    fill(b, ctx, 22, 3, 21, 26, 3, 21, gray);
    fill(b, ctx, 31, 3, 21, 35, 3, 21, gray);
    fill(b, ctx, 23, 2, 21, 25, 2, 21, gray);
    fill(b, ctx, 32, 2, 21, 34, 2, 21, gray);
    fill(b, ctx, 28, 4, 20, 29, 4, 21, light);
    for (x, y) in [(27, 3), (30, 3), (26, 2), (31, 2), (25, 1), (32, 1)] {
        b.place_block(ctx, light, x, y, 21);
    }
    for i in 0..7 {
        b.place_block(ctx, black, 28 - i, 6 + i, 21);
        b.place_block(ctx, black, 29 + i, 6 + i, 21);
    }
    for i in 0..4 {
        b.place_block(ctx, black, 28 - i, 9 + i, 21);
        b.place_block(ctx, black, 29 + i, 9 + i, 21);
    }
    b.place_block(ctx, black, 28, 12, 21);
    b.place_block(ctx, black, 29, 12, 21);
    for i in 0..3 {
        b.place_block(ctx, black, 22 - i * 2, 8, 21);
        b.place_block(ctx, black, 22 - i * 2, 9, 21);
        b.place_block(ctx, black, 35 + i * 2, 8, 21);
        b.place_block(ctx, black, 35 + i * 2, 9, 21);
    }
    for (x0, y0, x1, y1) in [(15, 13, 42, 15), (15, 1, 15, 6), (16, 1, 16, 5), (17, 1, 20, 4), (21, 1, 21, 3), (22, 1, 22, 2), (23, 1, 24, 1), (42, 1, 42, 6), (41, 1, 41, 5), (37, 1, 40, 4), (36, 1, 36, 3), (33, 1, 34, 1), (35, 1, 35, 2)] {
        water_box(b, ctx, x0, y0, 21, x1, y1, 21);
    }
}

/// `MonumentBuilding.generateRoofPiece`.
fn roof(b: &PieceBase, ctx: &mut PlaceCtx) {
    if !chunk_intersects(b, ctx, 21, 21, 36, 36) {
        return;
    }
    let (gray, light, lamp) = (base_gray(), base_light(), lamp());
    fill(b, ctx, 21, 0, 22, 36, 0, 36, gray);
    water_box(b, ctx, 21, 1, 22, 36, 23, 36);
    for i in 0..4 {
        fill(b, ctx, 21 + i, 13 + i, 21 + i, 36 - i, 13 + i, 21 + i, light);
        fill(b, ctx, 21 + i, 13 + i, 36 - i, 36 - i, 13 + i, 36 - i, light);
        fill(b, ctx, 21 + i, 13 + i, 22 + i, 21 + i, 13 + i, 35 - i, light);
        fill(b, ctx, 36 - i, 13 + i, 22 + i, 36 - i, 13 + i, 35 - i, light);
    }
    fill(b, ctx, 25, 16, 25, 32, 16, 32, gray);
    fill(b, ctx, 25, 17, 25, 25, 19, 25, light);
    fill(b, ctx, 32, 17, 25, 32, 19, 25, light);
    fill(b, ctx, 25, 17, 32, 25, 19, 32, light);
    fill(b, ctx, 32, 17, 32, 32, 19, 32, light);
    for (x, y, z, s) in [
        (26, 20, 26, light),
        (27, 21, 27, light),
        (27, 20, 27, lamp),
        (26, 20, 31, light),
        (27, 21, 30, light),
        (27, 20, 30, lamp),
        (31, 20, 31, light),
        (30, 21, 30, light),
        (30, 20, 30, lamp),
        (31, 20, 26, light),
        (30, 21, 27, light),
        (30, 20, 27, lamp),
    ] {
        b.place_block(ctx, s, x, y, z);
    }
    fill(b, ctx, 28, 21, 27, 29, 21, 27, gray);
    fill(b, ctx, 27, 21, 28, 27, 21, 29, gray);
    fill(b, ctx, 28, 21, 30, 29, 21, 30, gray);
    fill(b, ctx, 30, 21, 28, 30, 21, 29, gray);
}

/// `MonumentBuilding.generateLowerWall`.
fn lower_wall(b: &PieceBase, ctx: &mut PlaceCtx) {
    let (gray, light) = (base_gray(), base_light());
    if chunk_intersects(b, ctx, 0, 21, 6, 58) {
        fill(b, ctx, 0, 0, 21, 6, 0, 57, gray);
        water_box(b, ctx, 0, 1, 21, 6, 7, 57);
        fill(b, ctx, 4, 4, 21, 6, 4, 53, gray);
        for i in 0..4 {
            fill(b, ctx, i, i + 1, 21, i, i + 1, 57 - i, light);
        }
        for z in (23..53).step_by(3) {
            b.place_block(ctx, light, 5, 5, z);
        }
        b.place_block(ctx, light, 5, 5, 52);
        for i in 0..4 {
            fill(b, ctx, i, i + 1, 21, i, i + 1, 57 - i, light);
        }
        fill(b, ctx, 4, 1, 52, 6, 3, 52, gray);
        fill(b, ctx, 5, 1, 51, 5, 3, 53, gray);
    }
    if chunk_intersects(b, ctx, 51, 21, 58, 58) {
        fill(b, ctx, 51, 0, 21, 57, 0, 57, gray);
        water_box(b, ctx, 51, 1, 21, 57, 7, 57);
        fill(b, ctx, 51, 4, 21, 53, 4, 53, gray);
        for i in 0..4 {
            fill(b, ctx, 57 - i, i + 1, 21, 57 - i, i + 1, 57 - i, light);
        }
        for z in (23..53).step_by(3) {
            b.place_block(ctx, light, 52, 5, z);
        }
        b.place_block(ctx, light, 52, 5, 52);
        fill(b, ctx, 51, 1, 52, 53, 3, 52, gray);
        fill(b, ctx, 52, 1, 51, 52, 3, 53, gray);
    }
    if chunk_intersects(b, ctx, 0, 51, 57, 57) {
        fill(b, ctx, 7, 0, 51, 50, 0, 57, gray);
        water_box(b, ctx, 7, 1, 51, 50, 10, 57);
        for i in 0..4 {
            fill(b, ctx, i + 1, i + 1, 57 - i, 56 - i, i + 1, 57 - i, light);
        }
    }
}

/// `MonumentBuilding.generateMiddleWall`.
fn middle_wall(b: &PieceBase, ctx: &mut PlaceCtx) {
    let (gray, light) = (base_gray(), base_light());
    if chunk_intersects(b, ctx, 7, 21, 13, 50) {
        fill(b, ctx, 7, 0, 21, 13, 0, 50, gray);
        water_box(b, ctx, 7, 1, 21, 13, 10, 50);
        fill(b, ctx, 11, 8, 21, 13, 8, 53, gray);
        for i in 0..4 {
            fill(b, ctx, i + 7, i + 5, 21, i + 7, i + 5, 54, light);
        }
        for z in (21..=45).step_by(3) {
            b.place_block(ctx, light, 12, 9, z);
        }
    }
    if chunk_intersects(b, ctx, 44, 21, 50, 54) {
        fill(b, ctx, 44, 0, 21, 50, 0, 50, gray);
        water_box(b, ctx, 44, 1, 21, 50, 10, 50);
        fill(b, ctx, 44, 8, 21, 46, 8, 53, gray);
        for i in 0..4 {
            fill(b, ctx, 50 - i, i + 5, 21, 50 - i, i + 5, 54, light);
        }
        for z in (21..=45).step_by(3) {
            b.place_block(ctx, light, 45, 9, z);
        }
    }
    if chunk_intersects(b, ctx, 8, 44, 49, 54) {
        fill(b, ctx, 14, 0, 44, 43, 0, 50, gray);
        water_box(b, ctx, 14, 1, 44, 43, 10, 50);
        for x in (12..=45).step_by(3) {
            b.place_block(ctx, light, x, 9, 45);
            b.place_block(ctx, light, x, 9, 52);
            if matches!(x, 12 | 18 | 24 | 33 | 39 | 45) {
                for (y, z) in [(9, 47), (9, 50), (10, 45), (10, 46), (10, 51), (10, 52), (11, 47), (11, 50), (12, 48), (12, 49)] {
                    b.place_block(ctx, light, x, y, z);
                }
            }
        }
        for i in 0..3 {
            fill(b, ctx, 8 + i, 5 + i, 54, 49 - i, 5 + i, 54, gray);
        }
        fill(b, ctx, 11, 8, 54, 46, 8, 54, light);
        fill(b, ctx, 14, 8, 44, 43, 8, 53, gray);
    }
}

/// `MonumentBuilding.generateUpperWall`.
fn upper_wall(b: &PieceBase, ctx: &mut PlaceCtx) {
    let (gray, light) = (base_gray(), base_light());
    if chunk_intersects(b, ctx, 14, 21, 20, 43) {
        fill(b, ctx, 14, 0, 21, 20, 0, 43, gray);
        water_box(b, ctx, 14, 1, 22, 20, 14, 43);
        fill(b, ctx, 18, 12, 22, 20, 12, 39, gray);
        fill(b, ctx, 18, 12, 21, 20, 12, 21, light);
        for i in 0..4 {
            fill(b, ctx, i + 14, i + 9, 21, i + 14, i + 9, 43 - i, light);
        }
        for z in (23..=39).step_by(3) {
            b.place_block(ctx, light, 19, 13, z);
        }
    }
    if chunk_intersects(b, ctx, 37, 21, 43, 43) {
        fill(b, ctx, 37, 0, 21, 43, 0, 43, gray);
        water_box(b, ctx, 37, 1, 22, 43, 14, 43);
        fill(b, ctx, 37, 12, 22, 39, 12, 39, gray);
        fill(b, ctx, 37, 12, 21, 39, 12, 21, light);
        for i in 0..4 {
            fill(b, ctx, 43 - i, i + 9, 21, 43 - i, i + 9, 43 - i, light);
        }
        for z in (23..=39).step_by(3) {
            b.place_block(ctx, light, 38, 13, z);
        }
    }
    if chunk_intersects(b, ctx, 15, 37, 42, 43) {
        fill(b, ctx, 21, 0, 37, 36, 0, 43, gray);
        water_box(b, ctx, 21, 1, 37, 36, 14, 43);
        fill(b, ctx, 21, 12, 37, 36, 12, 39, gray);
        for i in 0..4 {
            fill(b, ctx, 15 + i, i + 9, 43 - i, 42 - i, i + 9, 43 - i, light);
        }
        for x in (21..=36).step_by(3) {
            b.place_block(ctx, light, x, 13, 38);
        }
    }
}

/// A room's `postProcess`.
fn place_room(piece: &RoomPiece, g: &Graph, ctx: &mut PlaceCtx) {
    let b = &piece.base;
    match piece.kind {
        RoomKind::Entry => entry_room(b, &g.rooms[piece.room], ctx),
        RoomKind::Core => core_room(b, ctx),
        RoomKind::DoubleX => double_x_room(b, g, piece.room, ctx),
        RoomKind::DoubleXY => double_xy_room(b, g, piece.room, ctx),
        RoomKind::DoubleY => double_y_room(b, g, piece.room, ctx),
        RoomKind::DoubleYZ => double_yz_room(b, g, piece.room, ctx),
        RoomKind::DoubleZ => double_z_room(b, g, piece.room, ctx),
        RoomKind::Simple(design) => simple_room(b, &g.rooms[piece.room], design, ctx),
        RoomKind::SimpleTop => simple_top_room(b, &g.rooms[piece.room], ctx),
        RoomKind::Wing(design) => wing_room(b, design, ctx),
        RoomKind::Penthouse => penthouse(b, ctx),
    }
}

/// `OceanMonumentEntryRoom`.
fn entry_room(b: &PieceBase, room: &Room, ctx: &mut PlaceCtx) {
    let light = base_light();
    fill(b, ctx, 0, 3, 0, 2, 3, 7, light);
    fill(b, ctx, 5, 3, 0, 7, 3, 7, light);
    fill(b, ctx, 0, 2, 0, 1, 2, 7, light);
    fill(b, ctx, 6, 2, 0, 7, 2, 7, light);
    fill(b, ctx, 0, 1, 0, 0, 1, 7, light);
    fill(b, ctx, 7, 1, 0, 7, 1, 7, light);
    fill(b, ctx, 0, 1, 7, 7, 3, 7, light);
    fill(b, ctx, 1, 1, 0, 2, 3, 0, light);
    fill(b, ctx, 5, 1, 0, 6, 3, 0, light);
    if room.open(Dir::North) {
        water_box(b, ctx, 3, 1, 7, 4, 2, 7);
    }
    if room.open(Dir::West) {
        water_box(b, ctx, 0, 1, 3, 1, 2, 4);
    }
    if room.open(Dir::East) {
        water_box(b, ctx, 6, 1, 3, 7, 2, 4);
    }
}

/// `OceanMonumentCoreRoom`.
fn core_room(b: &PieceBase, ctx: &mut PlaceCtx) {
    let (gray, light, black, lamp) = (base_gray(), base_light(), base_black(), lamp());
    fill_on_water(b, ctx, 1, 8, 0, 14, 8, 14, gray);
    fill(b, ctx, 0, 7, 0, 0, 7, 15, light);
    fill(b, ctx, 15, 7, 0, 15, 7, 15, light);
    fill(b, ctx, 1, 7, 0, 15, 7, 0, light);
    fill(b, ctx, 1, 7, 15, 14, 7, 15, light);
    for y in 1..=6 {
        let s = if y == 2 || y == 6 { gray } else { light };
        for x in [0, 15] {
            fill(b, ctx, x, y, 0, x, y, 1, s);
            fill(b, ctx, x, y, 6, x, y, 9, s);
            fill(b, ctx, x, y, 14, x, y, 15, s);
        }
        fill(b, ctx, 1, y, 0, 1, y, 0, s);
        fill(b, ctx, 6, y, 0, 9, y, 0, s);
        fill(b, ctx, 14, y, 0, 14, y, 0, s);
        fill(b, ctx, 1, y, 15, 14, y, 15, s);
    }
    fill(b, ctx, 6, 3, 6, 9, 6, 9, black);
    fill(b, ctx, 7, 4, 7, 8, 5, 8, st("minecraft:gold_block"));
    for y in [3, 6] {
        for x in [6, 9] {
            b.place_block(ctx, lamp, x, y, 6);
            b.place_block(ctx, lamp, x, y, 9);
        }
    }
    for (x0, y0, z0, x1, y1, z1) in [
        (5, 1, 6, 5, 2, 6),
        (5, 1, 9, 5, 2, 9),
        (10, 1, 6, 10, 2, 6),
        (10, 1, 9, 10, 2, 9),
        (6, 1, 5, 6, 2, 5),
        (9, 1, 5, 9, 2, 5),
        (6, 1, 10, 6, 2, 10),
        (9, 1, 10, 9, 2, 10),
        (5, 2, 5, 5, 6, 5),
        (5, 2, 10, 5, 6, 10),
        (10, 2, 5, 10, 6, 5),
        (10, 2, 10, 10, 6, 10),
        (5, 7, 1, 5, 7, 6),
        (10, 7, 1, 10, 7, 6),
        (5, 7, 9, 5, 7, 14),
        (10, 7, 9, 10, 7, 14),
        (1, 7, 5, 6, 7, 5),
        (1, 7, 10, 6, 7, 10),
        (9, 7, 5, 14, 7, 5),
        (9, 7, 10, 14, 7, 10),
        (2, 1, 2, 2, 1, 3),
        (3, 1, 2, 3, 1, 2),
        (13, 1, 2, 13, 1, 3),
        (12, 1, 2, 12, 1, 2),
        (2, 1, 12, 2, 1, 13),
        (3, 1, 13, 3, 1, 13),
        (13, 1, 12, 13, 1, 13),
        (12, 1, 13, 12, 1, 13),
    ] {
        fill(b, ctx, x0, y0, z0, x1, y1, z1, light);
    }
}

/// The three rows of the outer walls of a room `width` blocks wide and `depth` deep at
/// `y`, as most rooms build them (`x = 0`, `x = width - 1`, `z = 0` from 1 to `south_end`,
/// `z = depth - 1` from 1 to `width - 2`).
fn walls(b: &PieceBase, ctx: &mut PlaceCtx, y: i32, width: i32, depth: i32, south_end: i32, s: u32) {
    fill(b, ctx, 0, y, 0, 0, y, depth - 1, s);
    fill(b, ctx, width - 1, y, 0, width - 1, y, depth - 1, s);
    fill(b, ctx, 1, y, 0, south_end, y, 0, s);
    fill(b, ctx, 1, y, depth - 1, width - 2, y, depth - 1, s);
}

/// `OceanMonumentDoubleXRoom`.
fn double_x_room(b: &PieceBase, g: &Graph, room: usize, ctx: &mut PlaceCtx) {
    let (gray, light, lamp) = (base_gray(), base_light(), lamp());
    let west = &g.rooms[room];
    let east = &g.rooms[g.conn(room, Dir::East)];
    if west.index / 25 > 0 {
        default_floor(b, ctx, 8, 0, east.open(Dir::Down));
        default_floor(b, ctx, 0, 0, west.open(Dir::Down));
    }
    if west.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 1, 4, 1, 7, 4, 6, gray);
    }
    if east.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 8, 4, 1, 14, 4, 6, gray);
    }
    walls(b, ctx, 3, 16, 8, 15, light);
    walls(b, ctx, 2, 16, 8, 15, gray);
    walls(b, ctx, 1, 16, 8, 15, light);
    fill(b, ctx, 5, 1, 0, 10, 1, 4, light);
    fill(b, ctx, 6, 2, 0, 9, 2, 3, gray);
    fill(b, ctx, 5, 3, 0, 10, 3, 4, light);
    b.place_block(ctx, lamp, 6, 2, 3);
    b.place_block(ctx, lamp, 9, 2, 3);
    if west.open(Dir::South) {
        water_box(b, ctx, 3, 1, 0, 4, 2, 0);
    }
    if west.open(Dir::North) {
        water_box(b, ctx, 3, 1, 7, 4, 2, 7);
    }
    if west.open(Dir::West) {
        water_box(b, ctx, 0, 1, 3, 0, 2, 4);
    }
    if east.open(Dir::South) {
        water_box(b, ctx, 11, 1, 0, 12, 2, 0);
    }
    if east.open(Dir::North) {
        water_box(b, ctx, 11, 1, 7, 12, 2, 7);
    }
    if east.open(Dir::East) {
        water_box(b, ctx, 15, 1, 3, 15, 2, 4);
    }
}

/// `OceanMonumentDoubleXYRoom`.
fn double_xy_room(b: &PieceBase, g: &Graph, room: usize, ctx: &mut PlaceCtx) {
    let (gray, light, lamp) = (base_gray(), base_light(), lamp());
    let east_i = g.conn(room, Dir::East);
    let west = &g.rooms[room];
    let east = &g.rooms[east_i];
    let west_up = &g.rooms[g.conn(room, Dir::Up)];
    let east_up = &g.rooms[g.conn(east_i, Dir::Up)];
    if west.index / 25 > 0 {
        default_floor(b, ctx, 8, 0, east.open(Dir::Down));
        default_floor(b, ctx, 0, 0, west.open(Dir::Down));
    }
    if west_up.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 1, 8, 1, 7, 8, 6, gray);
    }
    if east_up.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 8, 8, 1, 14, 8, 6, gray);
    }
    for y in 1..=7 {
        walls(b, ctx, y, 16, 8, 15, if y == 2 || y == 6 { gray } else { light });
    }
    for (x0, y0, z0, x1, y1, z1) in [
        (2, 1, 3, 2, 7, 4),
        (3, 1, 2, 4, 7, 2),
        (3, 1, 5, 4, 7, 5),
        (13, 1, 3, 13, 7, 4),
        (11, 1, 2, 12, 7, 2),
        (11, 1, 5, 12, 7, 5),
        (5, 1, 3, 5, 3, 4),
        (10, 1, 3, 10, 3, 4),
        (5, 7, 2, 10, 7, 5),
        (5, 5, 2, 5, 7, 2),
        (10, 5, 2, 10, 7, 2),
        (5, 5, 5, 5, 7, 5),
        (10, 5, 5, 10, 7, 5),
    ] {
        fill(b, ctx, x0, y0, z0, x1, y1, z1, light);
    }
    b.place_block(ctx, light, 6, 6, 2);
    b.place_block(ctx, light, 9, 6, 2);
    b.place_block(ctx, light, 6, 6, 5);
    b.place_block(ctx, light, 9, 6, 5);
    fill(b, ctx, 5, 4, 3, 6, 4, 4, light);
    fill(b, ctx, 9, 4, 3, 10, 4, 4, light);
    b.place_block(ctx, lamp, 5, 4, 2);
    b.place_block(ctx, lamp, 5, 4, 5);
    b.place_block(ctx, lamp, 10, 4, 2);
    b.place_block(ctx, lamp, 10, 4, 5);
    for (r, y, x, side, side_x) in [(west, 1, 3, Dir::West, 0), (east, 1, 11, Dir::East, 15), (west_up, 5, 3, Dir::West, 0), (east_up, 5, 11, Dir::East, 15)] {
        if r.open(Dir::South) {
            water_box(b, ctx, x, y, 0, x + 1, y + 1, 0);
        }
        if r.open(Dir::North) {
            water_box(b, ctx, x, y, 7, x + 1, y + 1, 7);
        }
        if r.open(side) {
            water_box(b, ctx, side_x, y, 3, side_x, y + 1, 4);
        }
    }
}

/// `OceanMonumentDoubleYRoom`.
fn double_y_room(b: &PieceBase, g: &Graph, room: usize, ctx: &mut PlaceCtx) {
    let (gray, light) = (base_gray(), base_light());
    let lower = &g.rooms[room];
    if lower.index / 25 > 0 {
        default_floor(b, ctx, 0, 0, lower.open(Dir::Down));
    }
    let upper = &g.rooms[g.conn(room, Dir::Up)];
    if upper.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 1, 8, 1, 6, 8, 6, gray);
    }
    fill(b, ctx, 0, 4, 0, 0, 4, 7, light);
    fill(b, ctx, 7, 4, 0, 7, 4, 7, light);
    fill(b, ctx, 1, 4, 0, 6, 4, 0, light);
    fill(b, ctx, 1, 4, 7, 6, 4, 7, light);
    fill(b, ctx, 2, 4, 1, 2, 4, 2, light);
    fill(b, ctx, 1, 4, 2, 1, 4, 2, light);
    fill(b, ctx, 5, 4, 1, 5, 4, 2, light);
    fill(b, ctx, 6, 4, 2, 6, 4, 2, light);
    fill(b, ctx, 2, 4, 5, 2, 4, 6, light);
    fill(b, ctx, 1, 4, 5, 1, 4, 5, light);
    fill(b, ctx, 5, 4, 5, 5, 4, 6, light);
    fill(b, ctx, 6, 4, 5, 6, 4, 5, light);
    for (y, r) in [(1, lower), (5, upper)] {
        for (d, z) in [(Dir::South, 0), (Dir::North, 7)] {
            if r.open(d) {
                fill(b, ctx, 2, y, z, 2, y + 2, z, light);
                fill(b, ctx, 5, y, z, 5, y + 2, z, light);
                fill(b, ctx, 3, y + 2, z, 4, y + 2, z, light);
            } else {
                fill(b, ctx, 0, y, z, 7, y + 2, z, light);
                fill(b, ctx, 0, y + 1, z, 7, y + 1, z, gray);
            }
        }
        for (d, x) in [(Dir::West, 0), (Dir::East, 7)] {
            if r.open(d) {
                fill(b, ctx, x, y, 2, x, y + 2, 2, light);
                fill(b, ctx, x, y, 5, x, y + 2, 5, light);
                fill(b, ctx, x, y + 2, 3, x, y + 2, 4, light);
            } else {
                fill(b, ctx, x, y, 0, x, y + 2, 7, light);
                fill(b, ctx, x, y + 1, 0, x, y + 1, 7, gray);
            }
        }
    }
}

/// `OceanMonumentDoubleYZRoom`.
fn double_yz_room(b: &PieceBase, g: &Graph, room: usize, ctx: &mut PlaceCtx) {
    let (gray, light, black, lamp) = (base_gray(), base_light(), base_black(), lamp());
    let north_i = g.conn(room, Dir::North);
    let north = &g.rooms[north_i];
    let south = &g.rooms[room];
    let north_up = &g.rooms[g.conn(north_i, Dir::Up)];
    let south_up = &g.rooms[g.conn(room, Dir::Up)];
    if south.index / 25 > 0 {
        default_floor(b, ctx, 0, 8, north.open(Dir::Down));
        default_floor(b, ctx, 0, 0, south.open(Dir::Down));
    }
    if south_up.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 1, 8, 1, 6, 8, 7, gray);
    }
    if north_up.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 1, 8, 8, 6, 8, 14, gray);
    }
    for y in 1..=7 {
        walls(b, ctx, y, 8, 16, 6, if y == 2 || y == 6 { gray } else { light });
    }
    for y in 1..=7 {
        fill(b, ctx, 3, y, 7, 4, y, 8, if y == 2 || y == 6 { lamp } else { black });
    }
    if south.open(Dir::South) {
        water_box(b, ctx, 3, 1, 0, 4, 2, 0);
    }
    if south.open(Dir::East) {
        water_box(b, ctx, 7, 1, 3, 7, 2, 4);
    }
    if south.open(Dir::West) {
        water_box(b, ctx, 0, 1, 3, 0, 2, 4);
    }
    if north.open(Dir::North) {
        water_box(b, ctx, 3, 1, 15, 4, 2, 15);
    }
    if north.open(Dir::West) {
        water_box(b, ctx, 0, 1, 11, 0, 2, 12);
    }
    if north.open(Dir::East) {
        water_box(b, ctx, 7, 1, 11, 7, 2, 12);
    }
    if south_up.open(Dir::South) {
        water_box(b, ctx, 3, 5, 0, 4, 6, 0);
    }
    if south_up.open(Dir::East) {
        water_box(b, ctx, 7, 5, 3, 7, 6, 4);
        fill(b, ctx, 5, 4, 2, 6, 4, 5, light);
        fill(b, ctx, 6, 1, 2, 6, 3, 2, light);
        fill(b, ctx, 6, 1, 5, 6, 3, 5, light);
    }
    if south_up.open(Dir::West) {
        water_box(b, ctx, 0, 5, 3, 0, 6, 4);
        fill(b, ctx, 1, 4, 2, 2, 4, 5, light);
        fill(b, ctx, 1, 1, 2, 1, 3, 2, light);
        fill(b, ctx, 1, 1, 5, 1, 3, 5, light);
    }
    if north_up.open(Dir::North) {
        water_box(b, ctx, 3, 5, 15, 4, 6, 15);
    }
    if north_up.open(Dir::West) {
        water_box(b, ctx, 0, 5, 11, 0, 6, 12);
        fill(b, ctx, 1, 4, 10, 2, 4, 13, light);
        fill(b, ctx, 1, 1, 10, 1, 3, 10, light);
        fill(b, ctx, 1, 1, 13, 1, 3, 13, light);
    }
    if north_up.open(Dir::East) {
        water_box(b, ctx, 7, 5, 11, 7, 6, 12);
        fill(b, ctx, 5, 4, 10, 6, 4, 13, light);
        fill(b, ctx, 6, 1, 10, 6, 3, 10, light);
        fill(b, ctx, 6, 1, 13, 6, 3, 13, light);
    }
}

/// `OceanMonumentDoubleZRoom`.
fn double_z_room(b: &PieceBase, g: &Graph, room: usize, ctx: &mut PlaceCtx) {
    let (gray, light, lamp) = (base_gray(), base_light(), lamp());
    let north = &g.rooms[g.conn(room, Dir::North)];
    let south = &g.rooms[room];
    if south.index / 25 > 0 {
        default_floor(b, ctx, 0, 8, north.open(Dir::Down));
        default_floor(b, ctx, 0, 0, south.open(Dir::Down));
    }
    if south.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 1, 4, 1, 6, 4, 7, gray);
    }
    if north.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 1, 4, 8, 6, 4, 14, gray);
    }
    walls(b, ctx, 3, 8, 16, 7, light);
    walls(b, ctx, 2, 8, 16, 7, gray);
    walls(b, ctx, 1, 8, 16, 7, light);
    for (x0, y0, z0, x1, y1, z1) in [
        (1, 1, 1, 1, 1, 2),
        (6, 1, 1, 6, 1, 2),
        (1, 3, 1, 1, 3, 2),
        (6, 3, 1, 6, 3, 2),
        (1, 1, 13, 1, 1, 14),
        (6, 1, 13, 6, 1, 14),
        (1, 3, 13, 1, 3, 14),
        (6, 3, 13, 6, 3, 14),
        (2, 1, 6, 2, 3, 6),
        (5, 1, 6, 5, 3, 6),
        (2, 1, 9, 2, 3, 9),
        (5, 1, 9, 5, 3, 9),
        (3, 2, 6, 4, 2, 6),
        (3, 2, 9, 4, 2, 9),
        (2, 2, 7, 2, 2, 8),
        (5, 2, 7, 5, 2, 8),
    ] {
        fill(b, ctx, x0, y0, z0, x1, y1, z1, light);
    }
    b.place_block(ctx, lamp, 2, 2, 5);
    b.place_block(ctx, lamp, 5, 2, 5);
    b.place_block(ctx, lamp, 2, 2, 10);
    b.place_block(ctx, lamp, 5, 2, 10);
    b.place_block(ctx, light, 2, 3, 5);
    b.place_block(ctx, light, 5, 3, 5);
    b.place_block(ctx, light, 2, 3, 10);
    b.place_block(ctx, light, 5, 3, 10);
    if south.open(Dir::South) {
        water_box(b, ctx, 3, 1, 0, 4, 2, 0);
    }
    if south.open(Dir::East) {
        water_box(b, ctx, 7, 1, 3, 7, 2, 4);
    }
    if south.open(Dir::West) {
        water_box(b, ctx, 0, 1, 3, 0, 2, 4);
    }
    if north.open(Dir::North) {
        water_box(b, ctx, 3, 1, 15, 4, 2, 15);
    }
    if north.open(Dir::West) {
        water_box(b, ctx, 0, 1, 11, 0, 2, 12);
    }
    if north.open(Dir::East) {
        water_box(b, ctx, 7, 1, 11, 7, 2, 12);
    }
}

/// `OceanMonumentSimpleRoom`.
fn simple_room(b: &PieceBase, room: &Room, design: i32, ctx: &mut PlaceCtx) {
    let (gray, light, black, lamp) = (base_gray(), base_light(), base_black(), lamp());
    if room.index / 25 > 0 {
        default_floor(b, ctx, 0, 0, room.open(Dir::Down));
    }
    if room.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 1, 4, 1, 6, 4, 6, gray);
    }
    // Drawn whenever a chunk places the room (so it can differ between chunks, as in
    // vanilla).
    let center = design != 0 && ctx.r.next_bool() && !room.open(Dir::Down) && !room.open(Dir::Up) && room.count_openings() > 1;
    match design {
        0 => {
            for (x0, z0, x1, z1, edge_x, edge_z0, edge_z1, row_z, row_x0, row_x1, lx, lz) in [
                (0, 0, 2, 2, 0, 0, 2, 0, 1, 2, 1, 1),
                (5, 0, 7, 2, 7, 0, 2, 0, 5, 6, 6, 1),
                (0, 5, 2, 7, 0, 5, 7, 7, 1, 2, 1, 6),
                (5, 5, 7, 7, 7, 5, 7, 7, 5, 6, 6, 6),
            ] {
                fill(b, ctx, x0, 1, z0, x1, 1, z1, light);
                fill(b, ctx, x0, 3, z0, x1, 3, z1, light);
                fill(b, ctx, edge_x, 2, edge_z0, edge_x, 2, edge_z1, gray);
                fill(b, ctx, row_x0, 2, row_z, row_x1, 2, row_z, gray);
                b.place_block(ctx, lamp, lx, 2, lz);
            }
            if room.open(Dir::South) {
                fill(b, ctx, 3, 3, 0, 4, 3, 0, light);
            } else {
                fill(b, ctx, 3, 3, 0, 4, 3, 1, light);
                fill(b, ctx, 3, 2, 0, 4, 2, 0, gray);
                fill(b, ctx, 3, 1, 0, 4, 1, 1, light);
            }
            if room.open(Dir::North) {
                fill(b, ctx, 3, 3, 7, 4, 3, 7, light);
            } else {
                fill(b, ctx, 3, 3, 6, 4, 3, 7, light);
                fill(b, ctx, 3, 2, 7, 4, 2, 7, gray);
                fill(b, ctx, 3, 1, 6, 4, 1, 7, light);
            }
            if room.open(Dir::West) {
                fill(b, ctx, 0, 3, 3, 0, 3, 4, light);
            } else {
                fill(b, ctx, 0, 3, 3, 1, 3, 4, light);
                fill(b, ctx, 0, 2, 3, 0, 2, 4, gray);
                fill(b, ctx, 0, 1, 3, 1, 1, 4, light);
            }
            if room.open(Dir::East) {
                fill(b, ctx, 7, 3, 3, 7, 3, 4, light);
            } else {
                fill(b, ctx, 6, 3, 3, 7, 3, 4, light);
                fill(b, ctx, 7, 2, 3, 7, 2, 4, gray);
                fill(b, ctx, 6, 1, 3, 7, 1, 4, light);
            }
        }
        1 => {
            for (x, z) in [(2, 2), (2, 5), (5, 5), (5, 2)] {
                fill(b, ctx, x, 1, z, x, 3, z, light);
            }
            for (x, z) in [(2, 2), (2, 5), (5, 5), (5, 2)] {
                b.place_block(ctx, lamp, x, 2, z);
            }
            for (x0, z0, x1, z1) in [(0, 0, 1, 0), (0, 1, 0, 1), (0, 7, 1, 7), (0, 6, 0, 6), (6, 7, 7, 7), (7, 6, 7, 6), (6, 0, 7, 0), (7, 1, 7, 1)] {
                fill(b, ctx, x0, 1, z0, x1, 3, z1, light);
            }
            for (x, z) in [(1, 0), (0, 1), (1, 7), (0, 6), (6, 7), (7, 6), (6, 0), (7, 1)] {
                b.place_block(ctx, gray, x, 2, z);
            }
            for (d, x0, z0, x1, z1) in [(Dir::South, 1, 0, 6, 0), (Dir::North, 1, 7, 6, 7), (Dir::West, 0, 1, 0, 6), (Dir::East, 7, 1, 7, 6)] {
                if !room.open(d) {
                    fill(b, ctx, x0, 3, z0, x1, 3, z1, light);
                    fill(b, ctx, x0, 2, z0, x1, 2, z1, gray);
                    fill(b, ctx, x0, 1, z0, x1, 1, z1, light);
                }
            }
        }
        _ => {
            for (y, s) in [(1, light), (2, black), (3, light)] {
                walls(b, ctx, y, 8, 8, 6, s);
            }
            fill(b, ctx, 0, 1, 3, 0, 2, 4, black);
            fill(b, ctx, 7, 1, 3, 7, 2, 4, black);
            fill(b, ctx, 3, 1, 0, 4, 2, 0, black);
            fill(b, ctx, 3, 1, 7, 4, 2, 7, black);
            if room.open(Dir::South) {
                water_box(b, ctx, 3, 1, 0, 4, 2, 0);
            }
            if room.open(Dir::North) {
                water_box(b, ctx, 3, 1, 7, 4, 2, 7);
            }
            if room.open(Dir::West) {
                water_box(b, ctx, 0, 1, 3, 0, 2, 4);
            }
            if room.open(Dir::East) {
                water_box(b, ctx, 7, 1, 3, 7, 2, 4);
            }
        }
    }
    if center {
        fill(b, ctx, 3, 1, 3, 4, 1, 4, light);
        fill(b, ctx, 3, 2, 3, 4, 2, 4, gray);
        fill(b, ctx, 3, 3, 3, 4, 3, 4, light);
    }
}

/// `OceanMonumentSimpleTopRoom`.
fn simple_top_room(b: &PieceBase, room: &Room, ctx: &mut PlaceCtx) {
    let (gray, light, black) = (base_gray(), base_light(), base_black());
    if room.index / 25 > 0 {
        default_floor(b, ctx, 0, 0, room.open(Dir::Down));
    }
    if room.connections[Dir::Up.index()].is_none() {
        fill_on_water(b, ctx, 1, 4, 1, 6, 4, 6, gray);
    }
    let sponge = st("minecraft:wet_sponge");
    for x in 1..=6 {
        for z in 1..=6 {
            if ctx.r.next_bounded(3) != 0 {
                let y = 2 + if ctx.r.next_bounded(4) == 0 { 0 } else { 1 };
                fill(b, ctx, x, y, z, x, 3, z, sponge);
            }
        }
    }
    for (y, s) in [(1, light), (2, black), (3, light)] {
        walls(b, ctx, y, 8, 8, 6, s);
    }
    fill(b, ctx, 0, 1, 3, 0, 2, 4, black);
    fill(b, ctx, 7, 1, 3, 7, 2, 4, black);
    fill(b, ctx, 3, 1, 0, 4, 2, 0, black);
    fill(b, ctx, 3, 1, 7, 4, 2, 7, black);
    if room.open(Dir::South) {
        water_box(b, ctx, 3, 1, 0, 4, 2, 0);
    }
}

/// `OceanMonumentWingRoom`.
fn wing_room(b: &PieceBase, design: i32, ctx: &mut PlaceCtx) {
    let (light, black, lamp) = (base_light(), base_black(), lamp());
    if design == 0 {
        for i in 0..4 {
            fill(b, ctx, 10 - i, 3 - i, 20 - i, 12 + i, 3 - i, 20, light);
        }
        fill(b, ctx, 7, 0, 6, 15, 0, 16, light);
        fill(b, ctx, 6, 0, 6, 6, 3, 20, light);
        fill(b, ctx, 16, 0, 6, 16, 3, 20, light);
        fill(b, ctx, 7, 1, 7, 7, 1, 20, light);
        fill(b, ctx, 15, 1, 7, 15, 1, 20, light);
        fill(b, ctx, 7, 1, 6, 9, 3, 6, light);
        fill(b, ctx, 13, 1, 6, 15, 3, 6, light);
        fill(b, ctx, 8, 1, 7, 9, 1, 7, light);
        fill(b, ctx, 13, 1, 7, 14, 1, 7, light);
        fill(b, ctx, 9, 0, 5, 13, 0, 5, light);
        fill(b, ctx, 10, 0, 7, 12, 0, 7, black);
        fill(b, ctx, 8, 0, 10, 8, 0, 12, black);
        fill(b, ctx, 14, 0, 10, 14, 0, 12, black);
        for z in (7..=18).rev().step_by(3) {
            b.place_block(ctx, lamp, 6, 3, z);
            b.place_block(ctx, lamp, 16, 3, z);
        }
        b.place_block(ctx, lamp, 10, 0, 10);
        b.place_block(ctx, lamp, 12, 0, 10);
        b.place_block(ctx, lamp, 10, 0, 12);
        b.place_block(ctx, lamp, 12, 0, 12);
        b.place_block(ctx, lamp, 8, 3, 6);
        b.place_block(ctx, lamp, 14, 3, 6);
        for (x, z) in [(4, 4), (18, 4), (4, 18), (18, 18)] {
            b.place_block(ctx, light, x, 2, z);
            b.place_block(ctx, lamp, x, 1, z);
            b.place_block(ctx, light, x, 0, z);
        }
        b.place_block(ctx, light, 9, 7, 20);
        b.place_block(ctx, light, 13, 7, 20);
        fill(b, ctx, 6, 0, 21, 7, 4, 21, light);
        fill(b, ctx, 15, 0, 21, 16, 4, 21, light);
        spawn_elder(b, ctx, 11, 2, 16);
    } else {
        fill(b, ctx, 9, 3, 18, 13, 3, 20, light);
        fill(b, ctx, 9, 0, 18, 9, 2, 18, light);
        fill(b, ctx, 13, 0, 18, 13, 2, 18, light);
        for x in [9, 13] {
            b.place_block(ctx, light, x, 6, 20);
            b.place_block(ctx, lamp, x, 5, 20);
            b.place_block(ctx, light, x, 4, 20);
        }
        fill(b, ctx, 7, 3, 7, 15, 3, 14, light);
        for x in [10, 12] {
            fill(b, ctx, x, 0, 10, x, 6, 10, light);
            fill(b, ctx, x, 0, 12, x, 6, 12, light);
            b.place_block(ctx, lamp, x, 0, 10);
            b.place_block(ctx, lamp, x, 0, 12);
            b.place_block(ctx, lamp, x, 4, 10);
            b.place_block(ctx, lamp, x, 4, 12);
        }
        for x in [8, 14] {
            fill(b, ctx, x, 0, 7, x, 2, 7, light);
            fill(b, ctx, x, 0, 14, x, 2, 14, light);
        }
        fill(b, ctx, 8, 3, 8, 8, 3, 13, black);
        fill(b, ctx, 14, 3, 8, 14, 3, 13, black);
        spawn_elder(b, ctx, 11, 5, 13);
    }
}

/// `OceanMonumentPenthouse`.
fn penthouse(b: &PieceBase, ctx: &mut PlaceCtx) {
    let (gray, light, black, lamp) = (base_gray(), base_light(), base_black(), lamp());
    fill(b, ctx, 2, -1, 2, 11, -1, 11, light);
    fill(b, ctx, 0, -1, 0, 1, -1, 11, gray);
    fill(b, ctx, 12, -1, 0, 13, -1, 11, gray);
    fill(b, ctx, 2, -1, 0, 11, -1, 1, gray);
    fill(b, ctx, 2, -1, 12, 11, -1, 13, gray);
    fill(b, ctx, 0, 0, 0, 0, 0, 13, light);
    fill(b, ctx, 13, 0, 0, 13, 0, 13, light);
    fill(b, ctx, 1, 0, 0, 12, 0, 0, light);
    fill(b, ctx, 1, 0, 13, 12, 0, 13, light);
    for i in (2..=11).step_by(3) {
        b.place_block(ctx, lamp, 0, 0, i);
        b.place_block(ctx, lamp, 13, 0, i);
        b.place_block(ctx, lamp, i, 0, 0);
    }
    fill(b, ctx, 2, 0, 3, 4, 0, 9, light);
    fill(b, ctx, 9, 0, 3, 11, 0, 9, light);
    fill(b, ctx, 4, 0, 9, 9, 0, 11, light);
    b.place_block(ctx, light, 5, 0, 8);
    b.place_block(ctx, light, 8, 0, 8);
    b.place_block(ctx, light, 10, 0, 10);
    b.place_block(ctx, light, 3, 0, 10);
    fill(b, ctx, 3, 0, 3, 3, 0, 7, black);
    fill(b, ctx, 10, 0, 3, 10, 0, 7, black);
    fill(b, ctx, 6, 0, 10, 7, 0, 10, black);
    for x in [3, 10] {
        for z in (2..=8).step_by(3) {
            fill(b, ctx, x, 0, z, x, 2, z, light);
        }
    }
    fill(b, ctx, 5, 0, 10, 5, 2, 10, light);
    fill(b, ctx, 8, 0, 10, 8, 2, 10, light);
    fill(b, ctx, 6, -1, 7, 7, -1, 8, black);
    water_box(b, ctx, 6, -1, 3, 7, -1, 4);
    spawn_elder(b, ctx, 6, 1, 6);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_graph_keeps_every_room_connected() {
        for seed in 0..64 {
            let mut r = WorldgenRandom::legacy(seed);
            let (g, source, core, list) = Graph::generate(&mut r);
            assert_eq!(g.rooms.len(), 49);
            assert_eq!(list.len(), 49);
            assert!(g.rooms[source].source);
            assert_eq!(g.rooms[core].index / 5 % 5, 2);
            for (i, room) in g.rooms.iter().enumerate() {
                if !room.is_special() {
                    assert!(g.find_source(i), "seed {seed}: room {} cut off", room.index);
                }
            }
        }
    }

    #[test]
    fn rooms_stay_inside_the_building() {
        for (seed, dir) in [(1, Dir::North), (2, Dir::East), (3, Dir::South), (4, Dir::West)] {
            let mut r = WorldgenRandom::legacy(seed);
            let m = MonumentBuilding::new(&mut r, 100, -200, dir);
            let bb = m.base.bbox;
            assert_eq!((bb.min_x, bb.min_y, bb.min_z, bb.max_x, bb.max_y, bb.max_z), (100, 39, -200, 157, 61, -143));
            let rooms = m.rooms();
            assert_eq!(rooms.iter().filter(|r| r.0 == "minecraft:omwr").count(), 2);
            assert_eq!(rooms.iter().filter(|r| r.0 == "minecraft:omcr").count(), 1);
            for (kind, b) in rooms {
                assert!(b.min_x >= bb.min_x && b.max_x <= bb.max_x && b.min_z >= bb.min_z && b.max_z <= bb.max_z, "{kind} {b:?} outside {bb:?}");
                assert!(b.min_y >= bb.min_y && b.max_y <= bb.max_y, "{kind} {b:?} outside {bb:?}");
            }
        }
    }
}
