//! Tripwires and tripwire hooks (vanilla's `TripWireBlock` and `TripWireHookBlock`).
//!
//! String between two hooks that face each other (up to 40 blocks of it) attaches both
//! hooks. Something in the string powers it, and with it both hooks, until it has left (the
//! string checks again every 10 ticks). Breaking the string sets the hooks off, unless it was
//! cut with shears (which disarms it first); breaking a hook detaches the other.

use super::state::{block_id, default_of, get_bool, get_dir, kind, set_bool, set_dir};
use super::{flags, sound_source, Dir, Kind, Level, PosExt};
use paper_protocol::codec::BlockPos;

/// How far a hook looks for the hook facing it (`WIRE_DIST_MAX`).
const WIRE_DIST_MAX: i32 = 42;
/// Ticks between checks of a powered string and of an attached hook (`RECHECK_PERIOD`).
pub const RECHECK_PERIOD: i32 = 10;

fn is_hook(state: u32) -> bool {
    kind(state) == Kind::TripwireHook
}

fn is_wire(state: u32) -> bool {
    kind(state) == Kind::Tripwire
}

fn facing(state: u32) -> Dir {
    get_dir(state, "facing").unwrap_or(Dir::North)
}

fn step(pos: BlockPos, dir: Dir, n: i32) -> BlockPos {
    let (dx, dy, dz) = dir.offset();
    pos.offset(dx * n, dy * n, dz * n)
}

// --- the hook ---------------------------------------------------------------------------------

/// `TripWireHookBlock.calculateState`: looks along the string for the hook facing this one,
/// attaches (or detaches) both and the string between them, and powers them if armed string
/// between them is pressed. `being_removed`: the hook at `pos` is going away. `search_range`
/// and `wire`: the string at that distance is (about to be) `wire`.
pub fn calculate_state(level: &mut Level, pos: BlockPos, hook: u32, being_removed: bool, notify: bool, search_range: i32, wire: Option<u32>) {
    let Some(dir) = get_dir(hook, "facing") else { return };
    let was_attached = get_bool(hook, "attached");
    let was_powered = get_bool(hook, "powered");
    let hook_block = block_id(hook);
    let mut attached = !being_removed;
    let mut powered = false;
    let mut other = 0;
    let mut wires = [None; WIRE_DIST_MAX as usize];
    for j in 1..WIRE_DIST_MAX {
        let at = step(pos, dir, j);
        let mut state = level.state(at);
        if is_hook(state) {
            if facing(state) == dir.opposite() {
                other = j;
            }
            break;
        }
        if !is_wire(state) && j != search_range {
            wires[j as usize] = None;
            attached = false;
        } else {
            if j == search_range {
                state = wire.unwrap_or(state);
            }
            let armed = !get_bool(state, "disarmed");
            powered |= armed && get_bool(state, "powered");
            wires[j as usize] = Some(state);
            if j == search_range {
                level.schedule_tick(pos, hook_block, RECHECK_PERIOD);
                attached &= armed;
            }
        }
    }
    attached &= other > 1;
    powered &= attached;
    let new = set_bool(set_bool(default_of(hook_block), "attached", attached), "powered", powered);
    if other > 0 {
        let at = step(pos, dir, other);
        let back = dir.opposite();
        level.set_block(at, set_dir(new, "facing", back), flags::ALL);
        notify_neighbors(level, hook_block, at, back);
        emit_state(level, at, attached, powered, was_attached, was_powered);
    }
    emit_state(level, pos, attached, powered, was_attached, was_powered);
    if !being_removed {
        level.set_block(pos, set_dir(new, "facing", dir), flags::ALL);
        if notify {
            notify_neighbors(level, hook_block, pos, dir);
        }
    }
    if was_attached != attached {
        for k in 1..other {
            let at = step(pos, dir, k);
            let Some(w) = wires[k as usize] else { continue };
            let now = level.state(at);
            if is_wire(now) || is_hook(now) {
                level.set_block(at, set_bool(w, "attached", attached), flags::ALL);
            }
        }
    }
}

/// The hook's sounds (`emitState`).
fn emit_state(level: &mut Level, pos: BlockPos, attached: bool, powered: bool, was_attached: bool, was_powered: bool) {
    let (sound, pitch) = if powered && !was_powered {
        ("minecraft:block.tripwire.click_on", 0.6)
    } else if !powered && was_powered {
        ("minecraft:block.tripwire.click_off", 0.5)
    } else if attached && !was_attached {
        ("minecraft:block.tripwire.attach", 0.7)
    } else if !attached && was_attached {
        let r = level.random_float();
        ("minecraft:block.tripwire.detach", 1.2 / (r * 0.2 + 0.9))
    } else {
        return;
    };
    level.play_sound(None, pos, sound, sound_source::BLOCKS, 0.4, pitch);
}

/// The hook and the block it hangs on tell their neighbours (`notifyNeighbors`).
fn notify_neighbors(level: &mut Level, block: u32, pos: BlockPos, facing: Dir) {
    level.update_neighbors_at(pos, block);
    level.update_neighbors_at(pos.rel(facing.opposite()), block);
}

/// A hook's scheduled check (`tick`).
pub fn hook_tick(level: &mut Level, state: u32, pos: BlockPos) {
    calculate_state(level, pos, state, false, true, -1, None);
}

/// A hook placed by a player looks for its partner (`setPlacedBy`).
pub fn hook_placed(level: &mut Level, state: u32, pos: BlockPos) {
    calculate_state(level, pos, state, false, false, -1, None);
}

/// A removed hook detaches its partner (`affectNeighborsAfterRemoval`).
pub fn hook_after_removal(level: &mut Level, old: u32, pos: BlockPos) {
    let (attached, powered) = (get_bool(old, "attached"), get_bool(old, "powered"));
    if attached || powered {
        calculate_state(level, pos, old, true, false, -1, None);
    }
    if powered {
        notify_neighbors(level, block_id(old), pos, facing(old));
    }
}

// --- the string -------------------------------------------------------------------------------

/// Whether string connects to `neighbor` in direction `dir` (`shouldConnectTo`).
fn connects_to(neighbor: u32, dir: Dir) -> bool {
    if is_hook(neighbor) {
        facing(neighbor) == dir.opposite()
    } else {
        is_wire(neighbor)
    }
}

/// The connections of string placed at `pos` (`getStateForPlacement`).
pub fn wire_placement(level: &Level, mut state: u32, pos: BlockPos) -> u32 {
    for dir in Dir::HORIZONTAL {
        state = set_bool(state, dir.name(), connects_to(level.state(pos.rel(dir)), dir));
    }
    state
}

/// `updateShape`: string follows its horizontal neighbours.
pub fn wire_update_shape(state: u32, dir: Dir, neighbor: u32) -> u32 {
    if dir.axis() == super::Axis::Y {
        return state;
    }
    set_bool(state, dir.name(), connects_to(neighbor, dir))
}

/// New string tells the hooks (`onPlace`).
pub fn wire_on_place(level: &mut Level, state: u32, pos: BlockPos, old: u32) {
    if block_id(old) != block_id(state) {
        update_source(level, pos, state);
    }
}

/// Broken string sets the hooks off, unless disarmed (`affectNeighborsAfterRemoval`).
pub fn wire_after_removal(level: &mut Level, old: u32, pos: BlockPos) {
    update_source(level, pos, set_bool(old, "powered", true));
}

/// Recalculates the hooks of the string through `pos` (`updateSource`): the hook to the south
/// or west finds its partner on its own.
fn update_source(level: &mut Level, pos: BlockPos, state: u32) {
    for dir in [Dir::South, Dir::West] {
        for i in 1..WIRE_DIST_MAX {
            let at = step(pos, dir, i);
            let s = level.state(at);
            if is_hook(s) {
                if facing(s) == dir.opposite() {
                    calculate_state(level, at, s, false, true, i, Some(state));
                }
                break;
            }
            if !is_wire(s) {
                break;
            }
        }
    }
}

/// Whether something is in the string at `pos` (`checkPressed`); while it is, it checks again
/// every 10 ticks.
pub fn check_pressed(level: &mut Level, pos: BlockPos, pressed: bool) {
    let state = level.state(pos);
    if !is_wire(state) {
        return;
    }
    if get_bool(state, "powered") != pressed {
        let new = set_bool(state, "powered", pressed);
        level.set_block(pos, new, flags::ALL);
        update_source(level, pos, new);
    }
    if pressed {
        level.schedule_tick(pos, block_id(state), RECHECK_PERIOD);
    }
}

/// Shears cut string without setting the hooks off (`playerWillDestroy`).
pub fn disarm(level: &mut Level, state: u32, pos: BlockPos) {
    if is_wire(state) {
        level.set_block(pos, set_bool(state, "disarmed", true), flags::INVISIBLE | flags::SKIP_BLOCK_ENTITY_SIDEEFFECTS);
    }
}

/// The box things press string in (its outline): lower once attached.
pub fn wire_box(state: u32) -> (f64, f64) {
    if get_bool(state, "attached") {
        (1.0 / 16.0, 2.5 / 16.0)
    } else {
        (0.0, 0.5)
    }
}
