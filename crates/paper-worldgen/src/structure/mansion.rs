//! Woodland mansions (`WoodlandMansionStructure`, `WoodlandMansionPieces`).
//!
//! Written to vanilla 26.3's `WoodlandMansionStructure` and `WoodlandMansionPieces`: the
//! floor grids (`MansionGrid`: corridors, rooms, the third floor), the piece placer
//! (`MansionPiecePlacer`: outer walls, roofs, corridors, carpets, inner walls and the rooms
//! of each floor) and the pieces (`WoodlandMansionPiece`, templates from
//! `woodland_mansion/*` with their data markers). The grid and the placer were checked
//! against `pumpkin-world/src/generation/structure/structures/mansion/{grid,placer}.rs` of
//! Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, from which they are ported;
//! the pieces use this crate's template placement, and the cobblestone foundation is
//! vanilla's `afterPlace` (Pumpkin adds an extra piece instead).
//!
//! Chests get the `chests/woodland_mansion` loot table; vindicators, evokers and allays
//! come out as entity NBT (vanilla's `finalizeSpawn` is left to the caller).

use super::bbox::BoundingBox;
use super::legacy::TemplatePiece;
use super::piece::{create_chest_at, mob, st, with_dir, PlaceCtx};
use super::processor::Processor;
use super::template::Template;
use super::transform::{transform_pos, Mirror, Rotation};
use super::{GenCtx, Loader, Piece, StructureGen, StructureStart};
use crate::feature::world::{Dir, Pos};
use crate::generator::WorldGenerator;
use crate::random::{Random, WorldgenRandom};
use anyhow::Result;
use paper_nbt::Compound;
use std::collections::HashMap;
use std::sync::Arc;

/// The templates under `woodland_mansion/`.
const TEMPLATES: &[&str] = &[
    "1x1_a1", "1x1_a2", "1x1_a3", "1x1_a4", "1x1_a5", "1x1_as1", "1x1_as2", "1x1_as3", "1x1_as4", "1x1_b1", "1x1_b2", "1x1_b3", "1x1_b4", "1x1_b5",
    "1x2_a1", "1x2_a2", "1x2_a3", "1x2_a4", "1x2_a5", "1x2_a6", "1x2_a7", "1x2_a8", "1x2_a9", "1x2_b1", "1x2_b2", "1x2_b3", "1x2_b4", "1x2_b5",
    "1x2_c1", "1x2_c2", "1x2_c3", "1x2_c4", "1x2_c_stairs", "1x2_d1", "1x2_d2", "1x2_d3", "1x2_d4", "1x2_d5", "1x2_d_stairs", "1x2_s1", "1x2_s2",
    "1x2_se1", "2x2_a1", "2x2_a2", "2x2_a3", "2x2_a4", "2x2_b1", "2x2_b2", "2x2_b3", "2x2_b4", "2x2_b5", "2x2_s1", "carpet_east", "carpet_north",
    "carpet_south_1", "carpet_south_2", "carpet_west_1", "carpet_west_2", "corridor_floor", "entrance", "indoors_door_1", "indoors_door_2",
    "indoors_wall_1", "indoors_wall_2", "roof", "roof_corner", "roof_front", "roof_inner_corner", "small_wall", "small_wall_corner", "wall_corner",
    "wall_flat", "wall_window",
];

/// `WoodlandMansionStructure`.
pub(crate) struct WoodlandMansion {
    templates: HashMap<&'static str, Arc<Template>>,
    structure_block: Arc<Processor>,
}

impl WoodlandMansion {
    pub fn load(l: &mut Loader) -> Result<Self> {
        let mut templates = HashMap::new();
        for name in TEMPLATES {
            templates.insert(*name, l.templates.load(l.data, &format!("minecraft:woodland_mansion/{name}"))?);
        }
        Ok(Self { templates, structure_block: l.builtins.structure_block.clone() })
    }
}

impl StructureGen for WoodlandMansion {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        // `getLowestYIn5by5BoxOffset7Blocks`, as for end cities.
        let (x, z) = (ctx.min_block_x() + 7, ctx.min_block_z() + 7);
        let (min_y, max_y) = (ctx.min_y() - 1, ctx.max_y());
        if !ctx.could_exist_in_column(x, z, min_y, max_y) {
            return None;
        }
        let rotation = Rotation::random(&mut ctx.r);
        let (ox, oz) = match rotation {
            Rotation::None => (5, 5),
            Rotation::Clockwise90 => (-5, 5),
            Rotation::Clockwise180 => (-5, -5),
            Rotation::CounterClockwise90 => (5, -5),
        };
        let y = ctx.cols.lowest_y(x, z, ox, oz);
        if y < 60 {
            return None;
        }
        let start = Pos::new(x, y, z);
        if !ctx.valid_biome(start) {
            return None;
        }
        // `generateMansion`.
        let grid = MansionGrid::new(&mut ctx.r);
        let pieces = Placer::create(&mut ctx.r, start, rotation, &grid);
        let pieces = pieces
            .into_iter()
            .map(|p| {
                let template = self.templates.get(p.template.as_str()).cloned().unwrap_or_else(|| panic!("woodland mansion template {}", p.template));
                Box::new(TemplatePiece::mansion(template, p.position, p.rotation, p.mirror, self.structure_block.clone())) as Box<dyn Piece>
            })
            .collect();
        Some(pieces)
    }

    /// `WoodlandMansionPiece`'s NBT constructor.
    fn load_pieces(&self, _gen: &WorldGenerator, _x: i32, _z: i32, children: &[&Compound]) -> Result<Vec<Box<dyn Piece>>> {
        children.iter().map(|t| Ok(Box::new(TemplatePiece::load_mansion(t, &self.templates, self.structure_block.clone())?) as Box<dyn Piece>)).collect()
    }

    /// `WoodlandMansionStructure.afterPlace`: cobblestone under the pieces down to the
    /// ground, through air and fluids.
    fn after_place(&self, start: &StructureStart, ctx: &mut PlaceCtx) {
        let boxes: Vec<BoundingBox> = start.pieces.iter().map(|p| p.bbox()).collect();
        let Some(all) = BoundingBox::encapsulating(boxes.iter().copied()) else { return };
        let bottom = all.min_y;
        let cobblestone = st("minecraft:cobblestone");
        let cb = ctx.chunk_box;
        let level_min = ctx.w.min_y;
        for x in cb.min_x..=cb.max_x {
            for z in cb.min_z..=cb.max_z {
                let p = Pos::new(x, bottom, z);
                if ctx.w.is_air(p) || !all.is_inside(p) || !boxes.iter().any(|b| b.is_inside(p)) {
                    continue;
                }
                for y in (level_min + 1..bottom).rev() {
                    let q = Pos::new(x, y, z);
                    let s = ctx.w.get(q);
                    if !ctx.w.b.is_air(s) && !ctx.w.b.is_liquid(s) {
                        break;
                    }
                    ctx.w.set(q, cobblestone);
                }
            }
        }
    }
}

/// `WoodlandMansionPiece.handleDataMarker`.
pub(crate) fn data_marker(id: &str, pos: Pos, rotation: Rotation, ctx: &mut PlaceCtx) {
    if id.starts_with("Chest") {
        let facing = match id {
            "ChestWest" => Some(Dir::West),
            "ChestEast" => Some(Dir::East),
            "ChestSouth" => Some(Dir::South),
            "ChestNorth" => Some(Dir::North),
            _ => None,
        };
        let mut state = st("minecraft:chest");
        if let Some(d) = facing {
            state = with_dir(state, "facing", rotation.rotate(d));
        }
        create_chest_at(ctx, pos, "minecraft:chests/woodland_mansion", Some(state));
        return;
    }
    let (entity, count) = match id {
        "Mage" => ("minecraft:evoker", 1),
        "Warrior" => ("minecraft:vindicator", 1),
        "Group of Allays" => {
            // `level.getRandom()`: the region's random.
            let mut r = ctx.w.region_random();
            let n = r.next_bounded(3) + 1;
            ctx.w.store_region_random(r);
            ("minecraft:allay", n)
        }
        _ => return,
    };
    for _ in 0..count {
        ctx.add_entity_nbt(mob(entity, pos));
        ctx.w.set(pos, paper_data::blocks::AIR);
    }
}

const CORRIDOR: i32 = 1;
const ROOM: i32 = 2;
const START_ROOM: i32 = 3;
const TEST_ROOM: i32 = 4;
const BLOCKED: i32 = 5;
const ROOM_1X1: i32 = 0x1_0000;
const ROOM_1X2: i32 = 0x2_0000;
const ROOM_2X2: i32 = 0x4_0000;
const ROOM_ORIGIN_FLAG: i32 = 0x10_0000;
const ROOM_DOOR_FLAG: i32 = 0x20_0000;
const ROOM_STAIRS_FLAG: i32 = 0x40_0000;
const ROOM_CORRIDOR_FLAG: i32 = 0x80_0000;
const ROOM_TYPE_MASK: i32 = 0xF_0000;
const ROOM_ID_MASK: i32 = 0xFFFF;

const SIZE: i32 = 11;

/// `WoodlandMansionPieces.SimpleGrid` (11 by 11).
#[derive(Clone)]
struct SimpleGrid {
    cells: [i32; (SIZE * SIZE) as usize],
    outside: i32,
}

impl SimpleGrid {
    fn new(outside: i32) -> Self {
        Self { cells: [0; (SIZE * SIZE) as usize], outside }
    }

    fn set(&mut self, x: i32, y: i32, v: i32) {
        if (0..SIZE).contains(&x) && (0..SIZE).contains(&y) {
            self.cells[(x * SIZE + y) as usize] = v;
        }
    }

    fn set_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, v: i32) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.set(x, y, v);
            }
        }
    }

    fn get(&self, x: i32, y: i32) -> i32 {
        if (0..SIZE).contains(&x) && (0..SIZE).contains(&y) {
            self.cells[(x * SIZE + y) as usize]
        } else {
            self.outside
        }
    }

    fn set_if(&mut self, x: i32, y: i32, old: i32, v: i32) {
        if self.get(x, y) == old {
            self.set(x, y, v);
        }
    }

    fn edges_to(&self, x: i32, y: i32, v: i32) -> bool {
        self.get(x - 1, y) == v || self.get(x + 1, y) == v || self.get(x, y + 1) == v || self.get(x, y - 1) == v
    }
}

/// The grid step of a horizontal direction (x, and z as the grid's y).
fn step(d: Dir) -> (i32, i32) {
    let (x, _, z) = d.offset();
    (x, z)
}

/// `Direction.from2DDataValue`.
fn from_2d(i: i32) -> Dir {
    [Dir::South, Dir::West, Dir::North, Dir::East][(i & 3) as usize]
}

/// `WoodlandMansionPieces.MansionGrid`.
struct MansionGrid {
    base: SimpleGrid,
    third_floor: SimpleGrid,
    floor_rooms: [SimpleGrid; 3],
    entrance_x: i32,
    entrance_y: i32,
}

impl MansionGrid {
    fn new(r: &mut WorldgenRandom) -> Self {
        let (ex, ey) = (7, 4);
        let mut base = SimpleGrid::new(BLOCKED);
        base.set_rect(ex, ey, ex + 1, ey + 1, START_ROOM);
        base.set_rect(ex - 1, ey, ex - 1, ey + 1, ROOM);
        base.set_rect(ex + 2, ey - 2, ex + 3, ey + 3, BLOCKED);
        base.set_rect(ex + 1, ey - 2, ex + 1, ey - 1, CORRIDOR);
        base.set_rect(ex + 1, ey + 2, ex + 1, ey + 3, CORRIDOR);
        base.set(ex - 1, ey - 1, CORRIDOR);
        base.set(ex - 1, ey + 2, CORRIDOR);
        base.set_rect(0, 0, 11, 1, BLOCKED);
        base.set_rect(0, 9, 11, 11, BLOCKED);
        recursive_corridor(r, &mut base, ex, ey - 2, Dir::West, 6);
        recursive_corridor(r, &mut base, ex, ey + 3, Dir::West, 6);
        recursive_corridor(r, &mut base, ex - 2, ey - 1, Dir::West, 3);
        recursive_corridor(r, &mut base, ex - 2, ey + 2, Dir::West, 3);
        while clean_edges(&mut base) {}
        let mut floor_rooms = [SimpleGrid::new(BLOCKED), SimpleGrid::new(BLOCKED), SimpleGrid::new(BLOCKED)];
        identify_rooms(r, &base, &mut floor_rooms[0]);
        identify_rooms(r, &base, &mut floor_rooms[1]);
        floor_rooms[0].set_rect(ex + 1, ey, ex + 1, ey + 1, ROOM_CORRIDOR_FLAG);
        floor_rooms[1].set_rect(ex + 1, ey, ex + 1, ey + 1, ROOM_CORRIDOR_FLAG);
        let mut g = Self { base, third_floor: SimpleGrid::new(BLOCKED), floor_rooms, entrance_x: ex, entrance_y: ey };
        g.setup_third_floor(r);
        let third = g.third_floor.clone();
        identify_rooms(r, &third, &mut g.floor_rooms[2]);
        g
    }

    fn is_room_id(&self, x: i32, y: i32, floor: usize, id: i32) -> bool {
        self.floor_rooms[floor].get(x, y) & ROOM_ID_MASK == id
    }

    /// `get1x2RoomDirection`.
    fn room_direction(&self, x: i32, y: i32, floor: usize, id: i32) -> Option<Dir> {
        Dir::HORIZONTAL.into_iter().find(|d| {
            let (dx, dy) = step(*d);
            self.is_room_id(x + dx, y + dy, floor, id)
        })
    }

    /// `setupThirdFloor`.
    fn setup_third_floor(&mut self, r: &mut WorldgenRandom) {
        let mut candidates = Vec::new();
        for y in 0..SIZE {
            for x in 0..SIZE {
                let data = self.floor_rooms[1].get(x, y);
                if data & ROOM_TYPE_MASK == ROOM_1X2 && data & ROOM_DOOR_FLAG == ROOM_DOOR_FLAG {
                    candidates.push((x, y));
                }
            }
        }
        if candidates.is_empty() {
            self.third_floor.set_rect(0, 0, SIZE, SIZE, BLOCKED);
            return;
        }
        let (rx, ry) = candidates[r.next_bounded(candidates.len() as i32) as usize];
        let data = self.floor_rooms[1].get(rx, ry);
        self.floor_rooms[1].set(rx, ry, data | ROOM_STAIRS_FLAG);
        // Never missing: the other half of a 1x2 room is next to it.
        let d = self.room_direction(rx, ry, 1, data & ROOM_ID_MASK).expect("1x2 room");
        let (dx, dy) = step(d);
        let (ex, ey) = (rx + dx, ry + dy);
        for y in 0..SIZE {
            for x in 0..SIZE {
                if !is_house(&self.base, x, y) {
                    self.third_floor.set(x, y, BLOCKED);
                } else if x == rx && y == ry {
                    self.third_floor.set(x, y, START_ROOM);
                } else if x == ex && y == ey {
                    self.third_floor.set(x, y, START_ROOM);
                    self.floor_rooms[2].set(x, y, ROOM_CORRIDOR_FLAG);
                }
            }
        }
        let open: Vec<Dir> = Dir::HORIZONTAL
            .into_iter()
            .filter(|d| {
                let (dx, dy) = step(*d);
                self.third_floor.get(ex + dx, ey + dy) == 0
            })
            .collect();
        if open.is_empty() {
            self.third_floor.set_rect(0, 0, SIZE, SIZE, BLOCKED);
            self.floor_rooms[1].set(rx, ry, data);
            return;
        }
        let d = open[r.next_bounded(open.len() as i32) as usize];
        let (dx, dy) = step(d);
        recursive_corridor(r, &mut self.third_floor, ex + dx, ey + dy, d, 4);
        while clean_edges(&mut self.third_floor) {}
    }
}

/// `MansionGrid.isHouse`.
fn is_house(g: &SimpleGrid, x: i32, y: i32) -> bool {
    matches!(g.get(x, y), CORRIDOR | ROOM | START_ROOM | TEST_ROOM)
}

/// `MansionGrid.recursiveCorridor`.
fn recursive_corridor(r: &mut WorldgenRandom, g: &mut SimpleGrid, x: i32, y: i32, d: Dir, length: i32) {
    if length <= 0 {
        return;
    }
    let (sx, sy) = step(d);
    g.set(x, y, CORRIDOR);
    g.set_if(x + sx, y + sy, 0, CORRIDOR);
    for _ in 0..8 {
        let next = from_2d(r.next_bounded(4));
        if next == d.opposite() || (next == Dir::East && r.next_bool()) {
            continue;
        }
        let (nx, ny) = (x + sx, y + sy);
        let (tx, ty) = step(next);
        if g.get(nx + tx, ny + ty) == 0 && g.get(nx + tx * 2, ny + ty * 2) == 0 {
            recursive_corridor(r, g, nx + tx, ny + ty, next, length - 1);
            break;
        }
    }
    let (cx, cy) = step(d.clockwise());
    let (ccx, ccy) = step(d.counter_clockwise());
    g.set_if(x + cx, y + cy, 0, ROOM);
    g.set_if(x + ccx, y + ccy, 0, ROOM);
    g.set_if(x + sx + cx, y + sy + cy, 0, ROOM);
    g.set_if(x + sx + ccx, y + sy + ccy, 0, ROOM);
    g.set_if(x + sx * 2, y + sy * 2, 0, ROOM);
    g.set_if(x + cx * 2, y + cy * 2, 0, ROOM);
    g.set_if(x + ccx * 2, y + ccy * 2, 0, ROOM);
}

/// `MansionGrid.cleanEdges`.
fn clean_edges(g: &mut SimpleGrid) -> bool {
    let mut changed = false;
    for y in 0..SIZE {
        for x in 0..SIZE {
            if g.get(x, y) != 0 {
                continue;
            }
            let sides = [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)].into_iter().filter(|&(a, b)| is_house(g, a, b)).count();
            if sides >= 3 {
                g.set(x, y, ROOM);
                changed = true;
            } else if sides == 2 {
                let corners = [(x + 1, y + 1), (x - 1, y + 1), (x + 1, y - 1), (x - 1, y - 1)].into_iter().filter(|&(a, b)| is_house(g, a, b)).count();
                if corners <= 1 {
                    g.set(x, y, ROOM);
                    changed = true;
                }
            }
        }
    }
    changed
}

/// `MansionGrid.identifyRooms`.
fn identify_rooms(r: &mut WorldgenRandom, grid: &SimpleGrid, rooms: &mut SimpleGrid) {
    let mut cells = Vec::new();
    for y in 0..SIZE {
        for x in 0..SIZE {
            if grid.get(x, y) == ROOM {
                cells.push((x, y));
            }
        }
    }
    super::jigsaw::shuffle(&mut cells, r);
    let mut id = 10;
    for (x, y) in cells {
        if rooms.get(x, y) != 0 {
            continue;
        }
        let (mut x0, mut x1, mut y0, mut y1) = (x, x, y, y);
        let mut kind = ROOM_1X1;
        let free = |rooms: &SimpleGrid, a: i32, b: i32| rooms.get(a, b) == 0 && grid.get(a, b) == ROOM;
        if free(rooms, x + 1, y) && free(rooms, x, y + 1) && free(rooms, x + 1, y + 1) {
            x1 += 1;
            y1 += 1;
            kind = ROOM_2X2;
        } else if free(rooms, x - 1, y) && free(rooms, x, y + 1) && free(rooms, x - 1, y + 1) {
            x0 -= 1;
            y1 += 1;
            kind = ROOM_2X2;
        } else if free(rooms, x - 1, y) && free(rooms, x, y - 1) && free(rooms, x - 1, y - 1) {
            x0 -= 1;
            y0 -= 1;
            kind = ROOM_2X2;
        } else if free(rooms, x + 1, y) {
            x1 += 1;
            kind = ROOM_1X2;
        } else if free(rooms, x, y + 1) {
            y1 += 1;
            kind = ROOM_1X2;
        } else if free(rooms, x - 1, y) {
            x0 -= 1;
            kind = ROOM_1X2;
        } else if free(rooms, x, y - 1) {
            y0 -= 1;
            kind = ROOM_1X2;
        }
        let mut door_x = if r.next_bool() { x0 } else { x1 };
        let mut door_y = if r.next_bool() { y0 } else { y1 };
        let mut door = ROOM_DOOR_FLAG;
        if !grid.edges_to(door_x, door_y, CORRIDOR) {
            door_x = if door_x == x0 { x1 } else { x0 };
            door_y = if door_y == y0 { y1 } else { y0 };
            if !grid.edges_to(door_x, door_y, CORRIDOR) {
                door_y = if door_y == y0 { y1 } else { y0 };
                if !grid.edges_to(door_x, door_y, CORRIDOR) {
                    door_x = if door_x == x0 { x1 } else { x0 };
                    door_y = if door_y == y0 { y1 } else { y0 };
                    if !grid.edges_to(door_x, door_y, CORRIDOR) {
                        door = 0;
                        door_x = x0;
                        door_y = y0;
                    }
                }
            }
        }
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                let flags = if cx == door_x && cy == door_y { ROOM_ORIGIN_FLAG | door } else { 0 };
                rooms.set(cx, cy, flags | kind | id);
            }
        }
        id += 1;
    }
}

/// A piece the placer adds (`new WoodlandMansionPiece(...)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PieceDesc {
    pub template: String,
    pub position: Pos,
    pub rotation: Rotation,
    pub mirror: Mirror,
}

/// `PlacementData`.
struct Placement {
    position: Pos,
    rotation: Rotation,
    wall: &'static str,
}

/// The floor room collections (`FirstFloorRoomCollection`, `SecondFloorRoomCollection`;
/// the third floor uses the second floor's).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Floor {
    First,
    Upper,
}

impl Floor {
    fn numbered(prefix: &str, r: &mut WorldgenRandom, n: i32) -> String {
        format!("{prefix}{}", r.next_bounded(n) + 1)
    }

    fn room_1x1(self, r: &mut WorldgenRandom) -> String {
        match self {
            Self::First => Self::numbered("1x1_a", r, 5),
            Self::Upper => Self::numbered("1x1_b", r, 5),
        }
    }

    fn room_1x1_secret(self, r: &mut WorldgenRandom) -> String {
        Self::numbered("1x1_as", r, 4)
    }

    fn room_1x2_side(self, r: &mut WorldgenRandom, stairs: bool) -> String {
        match self {
            Self::First => Self::numbered("1x2_a", r, 9),
            Self::Upper if stairs => "1x2_c_stairs".into(),
            Self::Upper => Self::numbered("1x2_c", r, 4),
        }
    }

    fn room_1x2_front(self, r: &mut WorldgenRandom, stairs: bool) -> String {
        match self {
            Self::First => Self::numbered("1x2_b", r, 5),
            Self::Upper if stairs => "1x2_d_stairs".into(),
            Self::Upper => Self::numbered("1x2_d", r, 5),
        }
    }

    fn room_1x2_secret(self, r: &mut WorldgenRandom) -> String {
        match self {
            Self::First => Self::numbered("1x2_s", r, 2),
            Self::Upper => Self::numbered("1x2_se", r, 1),
        }
    }

    fn room_2x2(self, r: &mut WorldgenRandom) -> String {
        match self {
            Self::First => Self::numbered("2x2_a", r, 4),
            Self::Upper => Self::numbered("2x2_b", r, 5),
        }
    }
}

/// `MansionPiecePlacer`.
struct Placer<'r> {
    r: &'r mut WorldgenRandom,
    pieces: Vec<PieceDesc>,
    start_x: i32,
    start_y: i32,
}

impl<'r> Placer<'r> {
    /// `createMansion`.
    fn create(r: &'r mut WorldgenRandom, origin: Pos, rotation: Rotation, grid: &MansionGrid) -> Vec<PieceDesc> {
        let mut p = Self { r, pieces: Vec::new(), start_x: grid.entrance_x + 1, start_y: grid.entrance_y + 1 };
        p.create_mansion(origin, rotation, grid);
        p.pieces
    }

    fn add(&mut self, template: impl Into<String>, position: Pos, rotation: Rotation) {
        self.add_mirrored(template, position, rotation, Mirror::None);
    }

    fn add_mirrored(&mut self, template: impl Into<String>, position: Pos, rotation: Rotation, mirror: Mirror) {
        self.pieces.push(PieceDesc { template: template.into(), position, rotation, mirror });
    }

    /// The position of a grid cell relative to the entrance.
    fn cell(&self, origin: Pos, rotation: Rotation, x: i32, y: i32, east: i32) -> Pos {
        origin.rel_n(rotation.rotate(Dir::South), 8 + (y - self.start_y) * 8).rel_n(rotation.rotate(Dir::East), east + (x - self.start_x) * 8)
    }

    fn create_mansion(&mut self, origin: Pos, rotation: Rotation, grid: &MansionGrid) {
        let mut ground = Placement { position: origin, rotation, wall: "wall_flat" };
        self.entrance(&mut ground);
        let mut second = Placement { position: ground.position.up_n(8), rotation: ground.rotation, wall: "wall_window" };
        let (end_x, end_y) = (grid.entrance_x + 1, grid.entrance_y);
        let (sx, sy) = (self.start_x, self.start_y);
        self.traverse_outer_walls(&mut ground, &grid.base, Dir::South, sx, sy, end_x, end_y);
        self.traverse_outer_walls(&mut second, &grid.base, Dir::South, sx, sy, end_x, end_y);
        let mut third = Placement { position: ground.position.up_n(19), rotation: ground.rotation, wall: "wall_window" };
        'find: for y in 0..SIZE {
            for x in (0..SIZE).rev() {
                if is_house(&grid.third_floor, x, y) {
                    third.position = third.position.rel_n(rotation.rotate(Dir::South), 8 + (y - self.start_y) * 8);
                    third.position = third.position.rel_n(rotation.rotate(Dir::East), (x - self.start_x) * 8);
                    self.traverse_wall_piece(&mut third);
                    self.traverse_outer_walls(&mut third, &grid.third_floor, Dir::South, x, y, x, y);
                    break 'find;
                }
            }
        }
        self.create_roof(origin.up_n(16), rotation, &grid.base, Some(&grid.third_floor));
        self.create_roof(origin.up_n(27), rotation, &grid.third_floor, None);
        for floor in 0..3usize {
            let floor_origin = origin.up_n(8 * floor as i32 + if floor == 2 { 3 } else { 0 });
            let rooms = &grid.floor_rooms[floor];
            let layout = if floor == 2 { &grid.third_floor } else { &grid.base };
            let (south_carpet, west_carpet) = if floor == 0 { ("carpet_south_1", "carpet_west_1") } else { ("carpet_south_2", "carpet_west_2") };
            for y in 0..SIZE {
                for x in 0..SIZE {
                    if layout.get(x, y) != CORRIDOR {
                        continue;
                    }
                    let p = self.cell(floor_origin, rotation, x, y, 0);
                    self.add("corridor_floor", p, rotation);
                    let corridor = |a: i32, b: i32| layout.get(a, b) == CORRIDOR || rooms.get(a, b) & ROOM_CORRIDOR_FLAG == ROOM_CORRIDOR_FLAG;
                    if corridor(x, y - 1) {
                        self.add("carpet_north", p.rel_n(rotation.rotate(Dir::East), 1).up(), rotation);
                    }
                    if corridor(x + 1, y) {
                        self.add("carpet_east", p.rel_n(rotation.rotate(Dir::South), 1).rel_n(rotation.rotate(Dir::East), 5).up(), rotation);
                    }
                    if corridor(x, y + 1) {
                        self.add(south_carpet, p.rel_n(rotation.rotate(Dir::South), 5).rel_n(rotation.rotate(Dir::West), 1), rotation);
                    }
                    if corridor(x - 1, y) {
                        self.add(west_carpet, p.rel_n(rotation.rotate(Dir::West), 1).rel_n(rotation.rotate(Dir::North), 1), rotation);
                    }
                }
            }
            let (wall, door) = if floor == 0 { ("indoors_wall_1", "indoors_door_1") } else { ("indoors_wall_2", "indoors_door_2") };
            let collection = if floor == 0 { Floor::First } else { Floor::Upper };
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let third_start = floor == 2 && layout.get(x, y) == START_ROOM;
                    if layout.get(x, y) != ROOM && !third_start {
                        continue;
                    }
                    let data = rooms.get(x, y);
                    let kind = data & ROOM_TYPE_MASK;
                    let id = data & ROOM_ID_MASK;
                    let corridor_start = third_start && data & ROOM_CORRIDOR_FLAG == ROOM_CORRIDOR_FLAG;
                    let mut doors = Vec::new();
                    if data & ROOM_DOOR_FLAG == ROOM_DOOR_FLAG {
                        for d in Dir::HORIZONTAL {
                            let (dx, dy) = step(d);
                            if layout.get(x + dx, y + dy) == CORRIDOR {
                                doors.push(d);
                            }
                        }
                    }
                    let door_dir = if !doors.is_empty() {
                        Some(doors[self.r.next_bounded(doors.len() as i32) as usize])
                    } else if data & ROOM_ORIGIN_FLAG == ROOM_ORIGIN_FLAG {
                        Some(Dir::Up)
                    } else {
                        None
                    };
                    let p = self.cell(floor_origin, rotation, x, y, -1);
                    let pick = |d: Dir| if door_dir == Some(d) { door } else { wall };
                    if is_house(layout, x - 1, y) && !grid.is_room_id(x - 1, y, floor, id) {
                        self.add(pick(Dir::West), p, rotation);
                    }
                    if layout.get(x + 1, y) == CORRIDOR && !corridor_start {
                        self.add(pick(Dir::East), p.rel_n(rotation.rotate(Dir::East), 8), rotation);
                    }
                    if is_house(layout, x, y + 1) && !grid.is_room_id(x, y + 1, floor, id) {
                        let q = p.rel_n(rotation.rotate(Dir::South), 7).rel_n(rotation.rotate(Dir::East), 7);
                        self.add(pick(Dir::South), q, rotation.then(Rotation::Clockwise90));
                    }
                    if layout.get(x, y - 1) == CORRIDOR && !corridor_start {
                        let q = p.rel_n(rotation.rotate(Dir::North), 1).rel_n(rotation.rotate(Dir::East), 7);
                        self.add(pick(Dir::North), q, rotation.then(Rotation::Clockwise90));
                    }
                    match (kind, door_dir) {
                        (ROOM_1X1, _) => self.add_room_1x1(p, rotation, door_dir, collection),
                        (ROOM_1X2, Some(side)) => {
                            if let Some(front) = grid.room_direction(x, y, floor, id) {
                                self.add_room_1x2(p, rotation, front, side, collection, data & ROOM_STAIRS_FLAG == ROOM_STAIRS_FLAG);
                            }
                        }
                        (ROOM_2X2, Some(Dir::Up)) => self.add("2x2_s1", p.rel_n(rotation.rotate(Dir::East), 1), rotation),
                        (ROOM_2X2, Some(side)) => {
                            let mut front = side.clockwise();
                            let (dx, dy) = step(front);
                            if !grid.is_room_id(x + dx, y + dy, floor, id) {
                                front = front.opposite();
                            }
                            self.add_room_2x2(p, rotation, front, side, collection);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// `traverseOuterWalls`.
    #[allow(clippy::too_many_arguments)]
    fn traverse_outer_walls(&mut self, data: &mut Placement, layout: &SimpleGrid, mut d: Dir, start_x: i32, start_y: i32, end_x: i32, end_y: i32) {
        let (mut x, mut y) = (start_x, start_y);
        let start_dir = d;
        loop {
            let (sx, sy) = step(d);
            let (cx, cy) = step(d.counter_clockwise());
            if !is_house(layout, x + sx, y + sy) {
                self.traverse_turn(data);
                d = d.clockwise();
                if x != end_x || y != end_y || start_dir != d {
                    self.traverse_wall_piece(data);
                }
            } else if is_house(layout, x + sx + cx, y + sy + cy) {
                traverse_inner_turn(data);
                x += sx;
                y += sy;
                d = d.counter_clockwise();
            } else {
                x += sx;
                y += sy;
                if x != end_x || y != end_y || start_dir != d {
                    self.traverse_wall_piece(data);
                }
            }
            if x == end_x && y == end_y && start_dir == d {
                break;
            }
        }
    }

    fn entrance(&mut self, data: &mut Placement) {
        let p = data.position.rel_n(data.rotation.rotate(Dir::West), 9);
        self.add("entrance", p, data.rotation);
        data.position = data.position.rel_n(data.rotation.rotate(Dir::South), 16);
    }

    fn traverse_wall_piece(&mut self, data: &mut Placement) {
        let p = data.position.rel_n(data.rotation.rotate(Dir::East), 7);
        self.add(data.wall, p, data.rotation);
        data.position = data.position.rel_n(data.rotation.rotate(Dir::South), 8);
    }

    fn traverse_turn(&mut self, data: &mut Placement) {
        data.position = data.position.rel_n(data.rotation.rotate(Dir::South), -1);
        self.add("wall_corner", data.position, data.rotation);
        data.position = data.position.rel_n(data.rotation.rotate(Dir::South), -7);
        data.position = data.position.rel_n(data.rotation.rotate(Dir::West), -6);
        data.rotation = data.rotation.then(Rotation::Clockwise90);
    }

    /// `createRoof`.
    fn create_roof(&mut self, origin: Pos, rotation: Rotation, layout: &SimpleGrid, above: Option<&SimpleGrid>) {
        let east = rotation.rotate(Dir::East);
        let south = rotation.rotate(Dir::South);
        let west = rotation.rotate(Dir::West);
        let north = rotation.rotate(Dir::North);
        let covered = |x: i32, y: i32| above.is_some_and(|a| is_house(a, x, y));
        for y in 0..SIZE {
            for x in 0..SIZE {
                let p = self.cell(origin, rotation, x, y, 0);
                if !is_house(layout, x, y) || covered(x, y) {
                    continue;
                }
                self.add("roof", p.up_n(3), rotation);
                if !is_house(layout, x + 1, y) {
                    self.add("roof_front", p.rel_n(east, 6), rotation);
                }
                if !is_house(layout, x - 1, y) {
                    self.add("roof_front", p.rel_n(east, 0).rel_n(south, 7), rotation.then(Rotation::Clockwise180));
                }
                if !is_house(layout, x, y - 1) {
                    self.add("roof_front", p.rel_n(west, 1), rotation.then(Rotation::CounterClockwise90));
                }
                if !is_house(layout, x, y + 1) {
                    self.add("roof_front", p.rel_n(east, 6).rel_n(south, 6), rotation.then(Rotation::Clockwise90));
                }
            }
        }
        if let Some(above) = above {
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let p = self.cell(origin, rotation, x, y, 0);
                    if !is_house(layout, x, y) || !is_house(above, x, y) {
                        continue;
                    }
                    if !is_house(layout, x + 1, y) {
                        self.add("small_wall", p.rel_n(east, 7), rotation);
                    }
                    if !is_house(layout, x - 1, y) {
                        self.add("small_wall", p.rel_n(west, 1).rel_n(south, 6), rotation.then(Rotation::Clockwise180));
                    }
                    if !is_house(layout, x, y - 1) {
                        self.add("small_wall", p.rel_n(west, 0).rel_n(north, 1), rotation.then(Rotation::CounterClockwise90));
                    }
                    if !is_house(layout, x, y + 1) {
                        self.add("small_wall", p.rel_n(east, 6).rel_n(south, 7), rotation.then(Rotation::Clockwise90));
                    }
                    if !is_house(layout, x + 1, y) {
                        if !is_house(layout, x, y - 1) {
                            self.add("small_wall_corner", p.rel_n(east, 7).rel_n(north, 2), rotation);
                        }
                        if !is_house(layout, x, y + 1) {
                            self.add("small_wall_corner", p.rel_n(east, 8).rel_n(south, 7), rotation.then(Rotation::Clockwise90));
                        }
                    }
                    if !is_house(layout, x - 1, y) {
                        if !is_house(layout, x, y - 1) {
                            self.add("small_wall_corner", p.rel_n(west, 2).rel_n(north, 1), rotation.then(Rotation::CounterClockwise90));
                        }
                        if !is_house(layout, x, y + 1) {
                            self.add("small_wall_corner", p.rel_n(west, 1).rel_n(south, 8), rotation.then(Rotation::Clockwise180));
                        }
                    }
                }
            }
        }
        for y in 0..SIZE {
            for x in 0..SIZE {
                let p = self.cell(origin, rotation, x, y, 0);
                if !is_house(layout, x, y) || covered(x, y) {
                    continue;
                }
                if !is_house(layout, x + 1, y) {
                    let e = p.rel_n(east, 6);
                    if !is_house(layout, x, y + 1) {
                        self.add("roof_corner", e.rel_n(south, 6), rotation);
                    } else if is_house(layout, x + 1, y + 1) {
                        self.add("roof_inner_corner", e.rel_n(south, 5), rotation);
                    }
                    if !is_house(layout, x, y - 1) {
                        self.add("roof_corner", e, rotation.then(Rotation::CounterClockwise90));
                    } else if is_house(layout, x + 1, y - 1) {
                        self.add("roof_inner_corner", p.rel_n(east, 9).rel_n(north, 2), rotation.then(Rotation::Clockwise90));
                    }
                }
                if !is_house(layout, x - 1, y) {
                    let w = p.rel_n(east, 0).rel_n(south, 0);
                    if !is_house(layout, x, y + 1) {
                        self.add("roof_corner", w.rel_n(south, 6), rotation.then(Rotation::Clockwise90));
                    } else if is_house(layout, x - 1, y + 1) {
                        self.add("roof_inner_corner", w.rel_n(south, 8).rel_n(west, 3), rotation.then(Rotation::CounterClockwise90));
                    }
                    if !is_house(layout, x, y - 1) {
                        self.add("roof_corner", w, rotation.then(Rotation::Clockwise180));
                    } else if is_house(layout, x - 1, y - 1) {
                        self.add("roof_inner_corner", w.rel_n(south, 1), rotation.then(Rotation::Clockwise180));
                    }
                }
            }
        }
    }

    /// `addRoom1x1`.
    fn add_room_1x1(&mut self, p: Pos, rotation: Rotation, door: Option<Dir>, floor: Floor) {
        let mut room_rotation = Rotation::None;
        let mut template = floor.room_1x1(self.r);
        match door {
            Some(Dir::East) => {}
            Some(Dir::North) => room_rotation = Rotation::CounterClockwise90,
            Some(Dir::West) => room_rotation = Rotation::Clockwise180,
            Some(Dir::South) => room_rotation = Rotation::Clockwise90,
            _ => template = floor.room_1x1_secret(self.r),
        }
        // `getZeroPositionWithTransform((1, 0, 0), NONE, rotation, 7, 7)`, rotated.
        let zero = match room_rotation {
            Rotation::None => Pos::new(1, 0, 0),
            Rotation::Clockwise90 => Pos::new(7, 0, 0),
            Rotation::Clockwise180 => Pos::new(7, 0, 6),
            Rotation::CounterClockwise90 => Pos::new(1, 0, 6),
        };
        let zero = transform_pos(zero, Mirror::None, rotation, Pos::new(0, 0, 0));
        self.add(template, p.add(zero.x, 0, zero.z), room_rotation.then(rotation));
    }

    /// `addRoom1x2(pos, rotation, frontDirection, sideDirection, ...)`.
    fn add_room_1x2(&mut self, p: Pos, rotation: Rotation, front: Dir, side: Dir, floor: Floor, stairs: bool) {
        let east = rotation.rotate(Dir::East);
        let south = rotation.rotate(Dir::South);
        let (position, rot, mirror, kind) = match (side, front) {
            (Dir::East, Dir::South) => (p.rel_n(east, 1), rotation, Mirror::None, 0),
            (Dir::East, Dir::North) => (p.rel_n(east, 1).rel_n(south, 6), rotation, Mirror::LeftRight, 0),
            (Dir::West, Dir::North) => (p.rel_n(east, 7).rel_n(south, 6), rotation.then(Rotation::Clockwise180), Mirror::None, 0),
            (Dir::West, Dir::South) => (p.rel_n(east, 7), rotation, Mirror::FrontBack, 0),
            (Dir::South, Dir::East) => (p.rel_n(east, 1), rotation.then(Rotation::Clockwise90), Mirror::LeftRight, 0),
            (Dir::South, Dir::West) => (p.rel_n(east, 7), rotation.then(Rotation::Clockwise90), Mirror::None, 0),
            (Dir::North, Dir::West) => (p.rel_n(east, 7).rel_n(south, 6), rotation.then(Rotation::Clockwise90), Mirror::FrontBack, 0),
            (Dir::North, Dir::East) => (p.rel_n(east, 1).rel_n(south, 6), rotation.then(Rotation::CounterClockwise90), Mirror::None, 0),
            (Dir::South, Dir::North) => (p.rel_n(east, 1).rel_n(rotation.rotate(Dir::North), 8), rotation, Mirror::None, 1),
            (Dir::North, Dir::South) => (p.rel_n(east, 7).rel_n(south, 14), rotation.then(Rotation::Clockwise180), Mirror::None, 1),
            (Dir::West, Dir::East) => (p.rel_n(east, 15), rotation.then(Rotation::Clockwise90), Mirror::None, 1),
            (Dir::East, Dir::West) => (p.rel_n(rotation.rotate(Dir::West), 7).rel_n(south, 6), rotation.then(Rotation::CounterClockwise90), Mirror::None, 1),
            (Dir::Up, Dir::East) => (p.rel_n(east, 15), rotation.then(Rotation::Clockwise90), Mirror::None, 2),
            (Dir::Up, Dir::South) => (p.rel_n(east, 1).rel_n(rotation.rotate(Dir::North), 0), rotation, Mirror::None, 2),
            _ => return,
        };
        let template = match kind {
            0 => floor.room_1x2_side(self.r, stairs),
            1 => floor.room_1x2_front(self.r, stairs),
            _ => floor.room_1x2_secret(self.r),
        };
        self.add_mirrored(template, position, rot, mirror);
    }

    /// `addRoom2x2(pos, rotation, frontDirection, sideDirection, ...)`.
    fn add_room_2x2(&mut self, p: Pos, rotation: Rotation, front: Dir, side: Dir, floor: Floor) {
        let (e, s, rot, mirror) = match (side, front) {
            (Dir::East, Dir::South) => (-7, 0, rotation, Mirror::None),
            (Dir::East, Dir::North) => (-7, 6, rotation, Mirror::LeftRight),
            (Dir::North, Dir::East) => (1, 14, rotation.then(Rotation::CounterClockwise90), Mirror::None),
            (Dir::North, Dir::West) => (7, 14, rotation.then(Rotation::CounterClockwise90), Mirror::LeftRight),
            (Dir::South, Dir::West) => (7, -8, rotation.then(Rotation::Clockwise90), Mirror::None),
            (Dir::South, Dir::East) => (1, -8, rotation.then(Rotation::Clockwise90), Mirror::LeftRight),
            (Dir::West, Dir::North) => (15, 6, rotation.then(Rotation::Clockwise180), Mirror::None),
            (Dir::West, Dir::South) => (15, 0, rotation, Mirror::FrontBack),
            _ => (0, 0, rotation, Mirror::None),
        };
        let position = p.rel_n(rotation.rotate(Dir::East), e).rel_n(rotation.rotate(Dir::South), s);
        let template = floor.room_2x2(self.r);
        self.add_mirrored(template, position, rot, mirror);
    }
}

/// `traverseInnerTurn`.
fn traverse_inner_turn(data: &mut Placement) {
    data.position = data.position.rel_n(data.rotation.rotate(Dir::South), 6);
    data.position = data.position.rel_n(data.rotation.rotate(Dir::East), 8);
    data.rotation = data.rotation.then(Rotation::CounterClockwise90);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The piece list of a mansion at (0, 64, 0) from a legacy random seeded with `seed`
    /// (rotation, grid, placer), as Pumpkin's `layouts_match_java_26_2` builds it: the count
    /// and an FNV-1a hash of `template;x,y,z;rotation;mirror` lines.
    fn layout(seed: i64) -> (usize, u64) {
        let mut r = WorldgenRandom::legacy(seed);
        let rotation = Rotation::from_ordinal(r.next_bounded(4) as usize);
        let grid = MansionGrid::new(&mut r);
        let pieces = Placer::create(&mut r, Pos::new(0, 64, 0), rotation, &grid);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for p in &pieces {
            let line = format!("{};{},{},{};{};{}\n", p.template, p.position.x, p.position.y, p.position.z, p.rotation.ordinal(), p.mirror.ordinal());
            for byte in line.bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x100_0000_01b3);
            }
        }
        (pieces.len(), hash)
    }

    /// The layouts Pumpkin's test records for vanilla (`layouts_match_java_26_2` in
    /// `pumpkin-world/src/generation/structure/structures/mansion/placer.rs`).
    #[test]
    fn layouts_match_vanilla() {
        assert_eq!(layout(0), (590, 0xfc1e_ca03_a5d7_b852));
        assert_eq!(layout(1), (579, 0x1b41_bf3a_4fd9_020c));
        assert_eq!(layout(42), (519, 0xe367_67ff_2580_643e));
        assert_eq!(layout(8_675_309), (557, 0x8120_b948_7d0b_eb80));
    }

    #[test]
    fn rooms_cover_the_grid() {
        for seed in 0..32 {
            let mut r = WorldgenRandom::legacy(seed);
            let g = MansionGrid::new(&mut r);
            for floor in 0..2 {
                for y in 0..SIZE {
                    for x in 0..SIZE {
                        if g.base.get(x, y) == ROOM {
                            assert_ne!(g.floor_rooms[floor].get(x, y) & ROOM_ID_MASK, 0, "seed {seed}: room cell ({x}, {y}) of floor {floor}");
                        }
                    }
                }
            }
        }
    }
}
