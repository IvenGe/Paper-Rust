//! Doors, trapdoors, fence gates and beds: opened by hand (except iron) or by redstone, and
//! two-block doors and beds that break together. Follows vanilla's `DoorBlock`,
//! `TrapDoorBlock`, `FenceGateBlock` and `BedBlock`.
//!
//! Derived in part from Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0:
//! `crates/pumpkin-core/src/block/blocks/{doors.rs, trapdoor.rs, fence_gates.rs, bed.rs}`,
//! which served as a reference next to vanilla's behaviour.

use super::fluid::{tick_delay, Fluid, FluidState};
use super::state::{block_id, block_set, cycle, get_bool, get_dir, in_tag, prop, set_bool, set_dir, with};
use super::{flags, sound_source, Dir, Interaction, Kind, Level, PosExt, UseContext};
use paper_data::{blocks, physics};
use paper_protocol::codec::BlockPos;

fn facing(state: u32) -> Dir {
    get_dir(state, "facing").unwrap_or(Dir::North)
}

fn is_lower(state: u32) -> bool {
    prop(state, "half") == Some("lower")
}

fn open_close_pitch(level: &mut Level) -> f32 {
    level.random_float() * 0.1 + 0.9
}

fn play(level: &mut Level, except: Option<i32>, pos: BlockPos, sound: String) {
    let pitch = open_close_pitch(level);
    level.play_sound(except, pos, &sound, sound_source::BLOCKS, 1.0, pitch);
}

fn door_sound(state: u32, open: bool) -> String {
    format!("minecraft:block.{}_door.{}", block_set(state), if open { "open" } else { "close" })
}

fn trapdoor_sound(state: u32, open: bool) -> String {
    format!("minecraft:block.{}_trapdoor.{}", block_set(state), if open { "open" } else { "close" })
}

fn fence_gate_sound(state: u32, open: bool) -> String {
    let action = if open { "open" } else { "close" };
    match block_set(state) {
        "wooden" => format!("minecraft:block.fence_gate.{action}"),
        set => format!("minecraft:block.{set}_fence_gate.{action}"),
    }
}

fn schedule_water_tick(level: &mut Level, pos: BlockPos) {
    let water = FluidState::source(Fluid::Water);
    let delay = tick_delay(Fluid::Water, level.settings());
    level.schedule_fluid_tick(pos, water.type_id(), delay);
}

// --- doors ------------------------------------------------------------------------------------

pub fn door_can_survive(level: &Level, state: u32, pos: BlockPos) -> bool {
    let below = level.state(pos.below());
    if is_lower(state) {
        physics::is_face_sturdy(below, Dir::Up.index())
    } else {
        block_id(below) == block_id(state)
    }
}

/// The halves of a door keep each other's state; a half without its partner breaks.
pub fn door_update_shape(level: &Level, state: u32, pos: BlockPos, dir: Dir, neighbor: u32) -> u32 {
    let lower = is_lower(state);
    if dir.axis() != super::Axis::Y || lower != (dir == Dir::Up) {
        if lower && dir == Dir::Down && !door_can_survive(level, state, pos) {
            return blocks::AIR;
        }
        return state;
    }
    if matches!(super::kind(neighbor), Kind::Door { .. }) && prop(neighbor, "half") != prop(state, "half") {
        with(neighbor, "half", prop(state, "half").unwrap_or("lower"))
    } else {
        blocks::AIR
    }
}

pub fn door_use(level: &mut Level, state: u32, pos: BlockPos, ctx: UseContext, by_hand: bool) -> Interaction {
    if !by_hand {
        return Interaction::Pass;
    }
    let new = cycle(state, "open");
    level.set_block(pos, new, flags::CLIENTS | flags::IMMEDIATE);
    play(level, Some(ctx.player), pos, door_sound(new, get_bool(new, "open")));
    Interaction::Success
}

pub fn door_neighbor_changed(level: &mut Level, state: u32, pos: BlockPos, source_block: u32) {
    let other = pos.rel(if is_lower(state) { Dir::Up } else { Dir::Down });
    let signal = level.has_neighbor_signal(pos) || level.has_neighbor_signal(other);
    if source_block == block_id(state) || signal == get_bool(state, "powered") {
        return;
    }
    if signal != get_bool(state, "open") {
        play(level, None, pos, door_sound(state, signal));
    }
    level.set_block(pos, set_bool(set_bool(state, "powered", signal), "open", signal), flags::CLIENTS);
}

/// `DoorBlock.getHinge`: hinge on the side with more solid blocks or a neighbouring door,
/// else on the side of the click.
pub fn door_hinge(level: &Level, pos: BlockPos, facing: Dir, click_x: f64, click_z: f64) -> &'static str {
    let above = pos.above();
    let full = |p: BlockPos| blocks::flags(level.state(p)) & blocks::FLAG_FULL_CUBE != 0;
    let (left, right) = (facing.counter_clockwise(), facing.clockwise());
    let solid = -(full(pos.rel(left)) as i32) - (full(above.rel(left)) as i32) + full(pos.rel(right)) as i32 + full(above.rel(right)) as i32;
    let lower_door = |p: BlockPos| {
        let s = level.state(p);
        matches!(super::kind(s), Kind::Door { .. }) && is_lower(s)
    };
    let (door_left, door_right) = (lower_door(pos.rel(left)), lower_door(pos.rel(right)));
    if (door_left && !door_right) || solid > 0 {
        return "right";
    }
    if (door_right && !door_left) || solid < 0 {
        return "left";
    }
    let (step_x, _, step_z) = facing.offset();
    let left_hinge = (step_x >= 0 || click_z >= 0.5) && (step_x <= 0 || click_z <= 0.5) && (step_z >= 0 || click_x <= 0.5) && (step_z <= 0 || click_x >= 0.5);
    if left_hinge {
        "left"
    } else {
        "right"
    }
}

// --- trapdoors -------------------------------------------------------------------------------

pub fn trapdoor_use(level: &mut Level, state: u32, pos: BlockPos, ctx: UseContext, by_hand: bool) -> Interaction {
    if !by_hand {
        return Interaction::Pass;
    }
    let new = cycle(state, "open");
    level.set_block(pos, new, flags::CLIENTS);
    if get_bool(new, "waterlogged") {
        schedule_water_tick(level, pos);
    }
    play(level, Some(ctx.player), pos, trapdoor_sound(new, get_bool(new, "open")));
    Interaction::Success
}

pub fn trapdoor_neighbor_changed(level: &mut Level, state: u32, pos: BlockPos) {
    let signal = level.has_neighbor_signal(pos);
    if signal == get_bool(state, "powered") {
        return;
    }
    let mut new = state;
    if get_bool(new, "open") != signal {
        new = set_bool(new, "open", signal);
        play(level, None, pos, trapdoor_sound(new, signal));
    }
    level.set_block(pos, set_bool(new, "powered", signal), flags::CLIENTS);
    if get_bool(new, "waterlogged") {
        schedule_water_tick(level, pos);
    }
}

// --- fence gates -----------------------------------------------------------------------------

pub fn fence_gate_use(level: &mut Level, state: u32, pos: BlockPos, ctx: UseContext) -> Interaction {
    let new = if get_bool(state, "open") {
        set_bool(state, "open", false)
    } else {
        let mut s = state;
        if facing(s) == ctx.facing.opposite() {
            s = set_dir(s, "facing", ctx.facing);
        }
        set_bool(s, "open", true)
    };
    level.set_block(pos, new, flags::CLIENTS | flags::IMMEDIATE);
    play(level, Some(ctx.player), pos, fence_gate_sound(new, get_bool(new, "open")));
    Interaction::Success
}

pub fn fence_gate_neighbor_changed(level: &mut Level, state: u32, pos: BlockPos) {
    let signal = level.has_neighbor_signal(pos);
    if get_bool(state, "powered") == signal {
        return;
    }
    level.set_block(pos, set_bool(set_bool(state, "powered", signal), "open", signal), flags::CLIENTS);
    if get_bool(state, "open") != signal {
        play(level, None, pos, fence_gate_sound(state, signal));
    }
}

/// A fence gate between walls sits lower (`in_wall`).
pub fn fence_gate_update_shape(level: &Level, state: u32, pos: BlockPos, dir: Dir, neighbor: u32) -> u32 {
    if facing(state).clockwise().axis() != dir.axis() {
        return state;
    }
    let in_wall = in_tag!(neighbor, "walls") || in_tag!(level.state(pos.rel(dir.opposite())), "walls");
    set_bool(state, "in_wall", in_wall)
}

pub fn fence_gate_in_wall(level: &Level, state: u32, pos: BlockPos) -> bool {
    let axis_dir = facing(state).clockwise();
    in_tag!(level.state(pos.rel(axis_dir)), "walls") || in_tag!(level.state(pos.rel(axis_dir.opposite())), "walls")
}

// --- beds -------------------------------------------------------------------------------------

/// The two parts of a bed break together.
pub fn bed_update_shape(state: u32, dir: Dir, neighbor: u32) -> u32 {
    let foot = prop(state, "part") == Some("foot");
    let towards_other = if foot { facing(state) } else { facing(state).opposite() };
    if dir != towards_other {
        return state;
    }
    if block_id(neighbor) == block_id(state) && prop(neighbor, "part") != prop(state, "part") {
        set_bool(state, "occupied", get_bool(neighbor, "occupied"))
    } else {
        blocks::AIR
    }
}
