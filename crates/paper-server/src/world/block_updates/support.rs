//! Blocks that need support or follow their neighbours: what `canSurvive` and `updateShape`
//! do for torches, wall-mounted blocks, carpets, plants, pressure plates, ladders, fences,
//! panes, stairs and snowy grass, and the vanilla placement rules (`getStateForPlacement`)
//! of the blocks this module family handles.

use super::fluid::{fluid_state, Fluid, FluidState};
use super::redstone;
use super::state::{block_id, get_dir, has_prop, in_tag, is_air, prop, set_bool, set_dir, with};
use super::{doors, flags, Dir, Kind, Level, PosExt};
use paper_data::{blocks, physics};
use paper_protocol::codec::BlockPos;

fn facing(state: u32) -> Dir {
    get_dir(state, "facing").unwrap_or(Dir::North)
}

/// `canSupportCenter(below, UP)`: torches and the like stand on it.
fn center_support_up(state: u32) -> bool {
    physics::has_center_support(state, Dir::Up.index())
}

/// `canSurvive`.
pub fn can_survive(level: &Level, state: u32, pos: BlockPos) -> bool {
    match super::kind(state) {
        Kind::Torch | Kind::RedstoneTorch => center_support_up(level.state(pos.below())),
        Kind::WallTorch | Kind::RedstoneWallTorch | Kind::Ladder => {
            let dir = facing(state);
            physics::is_face_sturdy(level.state(pos.rel(dir.opposite())), dir.index())
        }
        Kind::Lever | Kind::Button { .. } => {
            let dir = redstone::attached_direction(state).opposite();
            physics::is_face_sturdy(level.state(pos.rel(dir)), dir.opposite().index())
        }
        Kind::RedstoneWire => redstone::wire_can_survive(level, pos),
        Kind::Repeater | Kind::Comparator => redstone::diode_can_survive_on(level.state(pos.below())),
        Kind::Door { .. } => doors::door_can_survive(level, state, pos),
        Kind::Carpet => !is_air(level.state(pos.below())),
        Kind::PressurePlate { .. } => {
            let below = level.state(pos.below());
            redstone::diode_can_survive_on(below) || center_support_up(below)
        }
        Kind::Vegetation => vegetation_on(level.state(pos.below())),
        Kind::PistonHead => super::piston::head_can_survive(level, state, pos),
        Kind::TallPlant => {
            if prop(state, "half") == Some("upper") {
                let below = level.state(pos.below());
                block_id(below) == block_id(state) && prop(below, "half") == Some("lower")
            } else {
                vegetation_on(level.state(pos.below()))
            }
        }
        _ => true,
    }
}

fn vegetation_on(state: u32) -> bool {
    in_tag!(state, "supports_vegetation")
}

/// `updateShape` of the blocks without a dedicated module.
pub fn update_shape(level: &mut Level, state: u32, pos: BlockPos, dir: Dir, _neighbor_pos: BlockPos, neighbor_state: u32) -> u32 {
    let survive = |level: &Level| can_survive(level, state, pos);
    match super::kind(state) {
        Kind::Torch | Kind::RedstoneTorch | Kind::PressurePlate { .. } if dir == Dir::Down && !survive(level) => blocks::AIR,
        Kind::WallTorch | Kind::RedstoneWallTorch | Kind::Ladder if dir.opposite() == facing(state) && !survive(level) => blocks::AIR,
        Kind::Lever | Kind::Button { .. } if redstone::attached_direction(state).opposite() == dir && !survive(level) => blocks::AIR,
        Kind::Carpet | Kind::Vegetation if !survive(level) => blocks::AIR,
        Kind::TallPlant => {
            let lower = prop(state, "half") == Some("lower");
            let towards_partner = dir.axis() == super::Axis::Y && lower == (dir == Dir::Up);
            let partner = block_id(neighbor_state) == block_id(state) && prop(neighbor_state, "half") != prop(state, "half");
            if (towards_partner && !partner) || (lower && dir == Dir::Down && !survive(level)) {
                blocks::AIR
            } else {
                state
            }
        }
        Kind::SnowyDirt if dir == Dir::Up => set_bool(state, "snowy", in_tag!(neighbor_state, "snow")),
        Kind::Fence | Kind::Pane if dir.axis() != super::Axis::Y => {
            let attach = connects_to(state, neighbor_state, physics::is_face_sturdy(neighbor_state, dir.opposite().index()), dir.opposite());
            set_bool(state, dir.name(), attach)
        }
        Kind::Stairs if dir.axis() != super::Axis::Y => with(state, "shape", stairs_shape(level, state, pos)),
        _ => state,
    }
}

/// `isExceptionForConnection`: blocks fences and panes never attach to.
fn connection_exception(state: u32) -> bool {
    in_tag!(state, "leaves") || in_tag!(state, "shulker_boxes") || matches!(super::state::block_name(state), "minecraft:barrier" | "minecraft:carved_pumpkin" | "minecraft:jack_o_lantern" | "minecraft:melon" | "minecraft:pumpkin")
}

/// Whether a fence or pane `state` connects to `neighbor` (whose face towards it is
/// `face_sturdy`; `towards` points from the neighbour back to the fence).
fn connects_to(state: u32, neighbor: u32, face_sturdy: bool, towards: Dir) -> bool {
    let solid = !connection_exception(neighbor) && face_sturdy;
    match super::kind(state) {
        Kind::Pane => solid || super::kind(neighbor) == Kind::Pane || in_tag!(neighbor, "walls"),
        _ => {
            let same_fence = in_tag!(neighbor, "fences") && in_tag!(neighbor, "wooden_fences") == in_tag!(state, "wooden_fences");
            let gate = super::kind(neighbor) == Kind::FenceGate && facing(neighbor).axis() == towards.clockwise().axis();
            solid || same_fence || gate
        }
    }
}

fn is_stairs(state: u32) -> bool {
    super::kind(state) == Kind::Stairs
}

/// `StairBlock.getStairsShape`.
fn stairs_shape(level: &Level, state: u32, pos: BlockPos) -> &'static str {
    let dir = facing(state);
    let half = prop(state, "half");
    let can_take_shape = |side: Dir| {
        let n = level.state(pos.rel(side));
        !is_stairs(n) || facing(n) != dir || prop(n, "half") != half
    };
    let behind = level.state(pos.rel(dir));
    if is_stairs(behind) && prop(behind, "half") == half {
        let behind_facing = facing(behind);
        if behind_facing.axis() != dir.axis() && can_take_shape(behind_facing.opposite()) {
            return if behind_facing == dir.counter_clockwise() { "outer_left" } else { "outer_right" };
        }
    }
    let front = level.state(pos.rel(dir.opposite()));
    if is_stairs(front) && prop(front, "half") == half {
        let front_facing = facing(front);
        if front_facing.axis() != dir.axis() && can_take_shape(front_facing) {
            return if front_facing == dir.counter_clockwise() { "inner_left" } else { "inner_right" };
        }
    }
    "straight"
}

/// `DoublePlantBlock.preventDropFromBottomPart` (doors behave the same): breaking the top
/// half in creative removes the bottom half without drops.
pub fn prevent_drop_from_bottom_part(level: &mut Level, state: u32, pos: BlockPos) {
    if !matches!(super::kind(state), Kind::Door { .. } | Kind::TallPlant) || prop(state, "half") != Some("upper") {
        return;
    }
    let lower = pos.below();
    let lower_state = level.state(lower);
    if block_id(lower_state) == block_id(state) && prop(lower_state, "half") == Some("lower") {
        let to = if fluid_state(lower_state).fluid == Fluid::Water { super::state::default_state!("water") } else { blocks::AIR };
        level.set_block(lower, to, flags::ALL | flags::SUPPRESS_DROPS);
        level.level_event(2001, lower, lower_state as i32);
    }
}

/// How a block is being placed by a player.
#[derive(Clone, Copy, Debug)]
pub struct PlaceContext {
    /// The horizontal direction the player faces.
    pub facing: Dir,
    /// The direction the player looks most (`getNearestLookingDirection`).
    pub looking: Dir,
    /// The state the block replaces.
    pub replaced: u32,
    /// Where the click hit, relative to the placed block's corner.
    pub click: (f64, f64, f64),
}

/// Vanilla's placement rules (`getStateForPlacement` and `canSurvive`) for a block a player
/// places at `pos`: waterlogging, wire connections, powered doors, lamps and gates, locked
/// repeaters, note block instruments, observer facing, fence and stair shapes. `None` if the
/// block cannot stay there.
pub fn placement_state(level: &Level, state: u32, pos: BlockPos, ctx: PlaceContext) -> Option<u32> {
    let mut s = state;
    if has_prop(s, "waterlogged") {
        s = set_bool(s, "waterlogged", fluid_state(ctx.replaced) == FluidState::source(Fluid::Water));
    }
    match super::kind(s) {
        Kind::RedstoneWire => s = redstone::wire_placement_state(level, pos),
        Kind::RedstoneLamp => s = set_bool(s, "lit", level.has_neighbor_signal(pos)),
        Kind::Repeater => s = set_bool(s, "locked", redstone::repeater_locked(level, s, pos)),
        Kind::NoteBlock => s = with(s, "instrument", redstone::note_instrument(level, pos)),
        Kind::Observer => s = set_bool(set_dir(s, "facing", ctx.looking), "powered", false),
        Kind::Door { .. } => {
            let powered = level.has_neighbor_signal(pos) || level.has_neighbor_signal(pos.above());
            s = set_bool(set_bool(s, "powered", powered), "open", powered);
            s = with(s, "hinge", doors::door_hinge(level, pos, facing(s), ctx.click.0, ctx.click.2));
        }
        Kind::Trapdoor { .. } => {
            if level.has_neighbor_signal(pos) {
                s = set_bool(set_bool(s, "open", true), "powered", true);
            }
        }
        Kind::FenceGate => {
            let powered = level.has_neighbor_signal(pos);
            s = set_bool(set_bool(s, "open", powered), "powered", powered);
            s = set_bool(s, "in_wall", doors::fence_gate_in_wall(level, s, pos));
        }
        Kind::Lever | Kind::Button { .. } if prop(s, "face") != Some("wall") => s = set_dir(s, "facing", ctx.facing),
        Kind::Fence | Kind::Pane => {
            for dir in Dir::HORIZONTAL {
                let n = level.state(pos.rel(dir));
                s = set_bool(s, dir.name(), connects_to(s, n, physics::is_face_sturdy(n, dir.opposite().index()), dir.opposite()));
            }
        }
        Kind::Stairs => s = with(s, "shape", stairs_shape(level, s, pos)),
        Kind::SnowyDirt => s = set_bool(s, "snowy", in_tag!(level.state(pos.above()), "snow")),
        Kind::Piston { .. } => s = set_bool(set_dir(s, "facing", ctx.looking.opposite()), "extended", false),
        _ => {}
    }
    if !can_survive(level, s, pos) {
        return None;
    }
    Some(s)
}

/// What a block does right after a player placed it (`setPlacedBy`).
pub fn placed(level: &mut Level, state: u32, pos: BlockPos) {
    if redstone::is_diode(state) {
        redstone::diode_placed(level, state, pos);
    }
    if matches!(super::kind(state), Kind::Piston { .. }) {
        super::piston::check_if_extend(level, state, pos);
    }
}
