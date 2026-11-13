//! Portal frames: vanilla's `PortalShape` for Nether portals (an obsidian rectangle,
//! inside 2x3 to 21x21) and the ring of twelve End portal frames with eyes.

use super::World;
use paper_data::blocks;
use paper_protocol::codec::BlockPos;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Z,
}

impl Axis {
    pub fn name(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Z => "z",
        }
    }

    pub fn of_state(state: u32) -> Axis {
        if blocks::property(state, "axis") == Some("z") {
            Axis::Z
        } else {
            Axis::X
        }
    }

    pub fn other(self) -> Axis {
        match self {
            Axis::X => Axis::Z,
            Axis::Z => Axis::X,
        }
    }

    /// The "right" direction of a portal along this axis: west for X, south for Z.
    pub fn right(self) -> (i32, i32) {
        match self {
            Axis::X => (-1, 0),
            Axis::Z => (0, 1),
        }
    }
}

fn name(world: &World, pos: BlockPos) -> &'static str {
    world.block_at(pos.x, pos.y, pos.z).map_or("minecraft:air", |s| blocks::block_of_state(s).name)
}

/// Inside a portal frame: air, fire or portal blocks.
fn is_empty(world: &World, pos: BlockPos) -> bool {
    match world.block_at(pos.x, pos.y, pos.z) {
        Some(s) => blocks::is_air(s) || matches!(blocks::block_of_state(s).name, "minecraft:fire" | "minecraft:soul_fire" | "minecraft:nether_portal"),
        None => false,
    }
}

fn is_frame(world: &World, pos: BlockPos) -> bool {
    name(world, pos) == "minecraft:obsidian"
}

fn step(pos: BlockPos, dir: (i32, i32), n: i32) -> BlockPos {
    pos.offset(dir.0 * n, 0, dir.1 * n)
}

/// A Nether portal frame (`PortalShape`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortalShape {
    pub axis: Axis,
    pub bottom_left: BlockPos,
    pub width: i32,
    pub height: i32,
    /// Portal blocks already inside.
    pub portal_blocks: i32,
}

impl PortalShape {
    /// The frame around `pos` along `axis` (`findAnyShape`); check [`Self::is_valid`].
    pub fn find_any(world: &World, pos: BlockPos, axis: Axis) -> PortalShape {
        let right = axis.right();
        let invalid = |bottom_left| PortalShape { axis, bottom_left, width: 0, height: 0, portal_blocks: 0 };
        let Some(bottom_left) = Self::bottom_left(world, pos, right) else { return invalid(pos) };
        let width = Self::distance_until_edge_above_frame(world, bottom_left, right);
        if !(2..=21).contains(&width) {
            return invalid(bottom_left);
        }
        let mut portal_blocks = 0;
        let height = Self::distance_until_top(world, bottom_left, right, width, &mut portal_blocks);
        let height = if (3..=21).contains(&height) && Self::has_top_frame(world, bottom_left, right, width, height) { height } else { 0 };
        PortalShape { axis, bottom_left, width, height, portal_blocks }
    }

    /// An empty valid frame along `preferred` or the other axis (`findEmptyPortalShape`):
    /// where lighting a fire creates a portal.
    pub fn find_empty(world: &World, pos: BlockPos, preferred: Axis) -> Option<PortalShape> {
        [preferred, preferred.other()].into_iter().map(|axis| Self::find_any(world, pos, axis)).find(|s| s.is_valid() && s.portal_blocks == 0)
    }

    fn bottom_left(world: &World, mut pos: BlockPos, right: (i32, i32)) -> Option<BlockPos> {
        let lowest = world.min_y.max(pos.y - 21);
        while pos.y > lowest && is_empty(world, pos.offset(0, -1, 0)) {
            pos = pos.offset(0, -1, 0);
        }
        let left = (-right.0, -right.1);
        let distance = Self::distance_until_edge_above_frame(world, pos, left) - 1;
        (distance >= 0).then(|| step(pos, left, distance))
    }

    fn distance_until_edge_above_frame(world: &World, pos: BlockPos, dir: (i32, i32)) -> i32 {
        for i in 0..=21 {
            let p = step(pos, dir, i);
            if !is_empty(world, p) {
                return if is_frame(world, p) { i } else { 0 };
            }
            if !is_frame(world, p.offset(0, -1, 0)) {
                break;
            }
        }
        0
    }

    fn distance_until_top(world: &World, bottom_left: BlockPos, right: (i32, i32), width: i32, portal_blocks: &mut i32) -> i32 {
        for i in 0..21 {
            let row = bottom_left.offset(0, i, 0);
            if !is_frame(world, step(row, right, -1)) || !is_frame(world, step(row, right, width)) {
                return i;
            }
            for j in 0..width {
                let p = step(row, right, j);
                if !is_empty(world, p) {
                    return i;
                }
                if name(world, p) == "minecraft:nether_portal" {
                    *portal_blocks += 1;
                }
            }
        }
        21
    }

    fn has_top_frame(world: &World, bottom_left: BlockPos, right: (i32, i32), width: i32, height: i32) -> bool {
        (0..width).all(|i| is_frame(world, step(bottom_left.offset(0, height, 0), right, i)))
    }

    pub fn is_valid(&self) -> bool {
        (2..=21).contains(&self.width) && (3..=21).contains(&self.height)
    }

    /// Valid and full of portal blocks: a lit portal that may stay.
    pub fn is_complete(&self) -> bool {
        self.is_valid() && self.portal_blocks == self.width * self.height
    }

    /// The positions inside the frame.
    pub fn inside(&self) -> Vec<BlockPos> {
        let right = self.axis.right();
        let mut out = Vec::with_capacity((self.width * self.height) as usize);
        for y in 0..self.height {
            for i in 0..self.width {
                out.push(step(self.bottom_left.offset(0, y, 0), right, i));
            }
        }
        out
    }

    /// Where an arriving entity stands: the middle of the bottom row.
    pub fn arrival(&self) -> (f64, f64, f64) {
        let right = self.axis.right();
        let half = (self.width - 1) as f64 / 2.0;
        (self.bottom_left.x as f64 + 0.5 + right.0 as f64 * half, self.bottom_left.y as f64, self.bottom_left.z as f64 + 0.5 + right.1 as f64 * half)
    }
}

const HORIZONTAL: [(&str, (i32, i32)); 4] = [("north", (0, -1)), ("south", (0, 1)), ("west", (-1, 0)), ("east", (1, 0))];

fn opposite(facing: &str) -> &'static str {
    match facing {
        "north" => "south",
        "south" => "north",
        "west" => "east",
        _ => "west",
    }
}

/// Whether twelve frames with eyes face the 3x3 square around `center`.
pub fn is_end_portal_ring(world: &World, center: BlockPos) -> bool {
    HORIZONTAL.iter().all(|(facing, (dx, dz))| {
        (-1..=1).all(|side| {
            // Frames on the north side (dz = -2) run along x, and so on.
            let pos = if *dx == 0 { center.offset(side, 0, dz * 2) } else { center.offset(dx * 2, 0, side) };
            let Some(state) = world.block_at(pos.x, pos.y, pos.z) else { return false };
            blocks::block_of_state(state).name == "minecraft:end_portal_frame" && blocks::property(state, "eye") == Some("true") && blocks::property(state, "facing") == Some(opposite(facing))
        })
    })
}

/// The centre of a complete ring that the frame at `pos` belongs to, if any.
pub fn end_portal_center(world: &World, pos: BlockPos) -> Option<BlockPos> {
    let state = world.block_at(pos.x, pos.y, pos.z)?;
    let facing = blocks::property(state, "facing")?;
    let (_, (fx, fz)) = HORIZONTAL.iter().find(|(f, _)| *f == facing)?;
    // The centre is two blocks in front, shifted by up to one block sideways.
    (-1..=1).map(|side| pos.offset(fx * 2 + fz.abs() * side, 0, fz * 2 + fx.abs() * side)).find(|c| is_end_portal_ring(world, *c))
}
