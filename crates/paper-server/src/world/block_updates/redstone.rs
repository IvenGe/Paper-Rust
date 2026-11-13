//! Redstone: power, wire, torches, levers, buttons, repeaters, comparators, observers, lamps
//! and note blocks, following vanilla's `RedStoneWireBlock` (with the default wire
//! evaluator), `RedstoneTorchBlock`, `LeverBlock`, `ButtonBlock`, `DiodeBlock`,
//! `RepeaterBlock`, `ComparatorBlock`, `ObserverBlock`, `RedstoneLampBlock` and
//! `NoteBlock`.
//!
//! Derived in part from Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0:
//! `crates/pumpkin-core/src/block/blocks/redstone/` (redstone_wire.rs, redstone_torch.rs,
//! abstract_redstone_gate.rs, repeater.rs, comparator.rs, observer.rs, lever.rs, buttons.rs,
//! redstone_lamp.rs) and `block/blocks/note.rs`, which served as a reference next to
//! vanilla's behaviour.
//!
//! Power model: `signal` is the weak power a block gives towards a neighbour, `direct_signal`
//! the strong power. A redstone conductor (most full blocks) that receives strong power gives
//! it on as weak power to everything around it. A wire measuring its input turns all wires'
//! signals off (`Level::without_wire_signals`), so wire power never passes through blocks.

use super::state::{block, block_id, block_name, block_set, cycle, default_of, default_state, get_bool, get_dir, get_int, kind, prop, set_bool, set_int, with};
use super::{flags, java_hash_order, sound_source, Dir, Interaction, Kind, Level, PosExt, UseContext};
use crate::world::chunk::ChunkPos;
use crate::world::ticks::TickPriority;
use crate::world::World;
use paper_data::physics;
use paper_nbt::{Compound, Tag};
use paper_protocol::codec::BlockPos;

/// Redstone torch burnout: more than [`TORCH_MAX_TOGGLES`] turn-offs within this many game
/// ticks burns a torch out for [`TORCH_RESTART_DELAY`] ticks.
pub const TORCH_TOGGLE_WINDOW: i64 = 60;
const TORCH_MAX_TOGGLES: usize = 8;
const TORCH_RESTART_DELAY: i32 = 160;

fn powered(state: u32) -> bool {
    get_bool(state, "powered")
}

fn facing(state: u32) -> Dir {
    get_dir(state, "facing").unwrap_or(Dir::North)
}

pub fn is_wire(state: u32) -> bool {
    block_id(state) == block!("redstone_wire")
}

pub fn is_diode(state: u32) -> bool {
    matches!(kind(state), Kind::Repeater | Kind::Comparator)
}

// --- power -------------------------------------------------------------------------------------

/// `getSignal`: weak power towards the neighbour in direction `-dir`.
pub fn signal(level: &Level, state: u32, pos: BlockPos, dir: Dir) -> u8 {
    match kind(state) {
        Kind::RedstoneWire => wire_signal(level, state, pos, dir),
        Kind::RedstoneTorch => lit_signal(state, dir != Dir::Up),
        Kind::RedstoneWallTorch => lit_signal(state, facing(state) != dir),
        Kind::Lever | Kind::Button { .. } | Kind::LightningRod | Kind::TripwireHook | Kind::Lectern | Kind::DetectorRail => {
            if powered(state) {
                15
            } else {
                0
            }
        }
        Kind::PressurePlate { weighted: false } => {
            if powered(state) {
                15
            } else {
                0
            }
        }
        Kind::PressurePlate { weighted: true } | Kind::DaylightDetector | Kind::TargetBlock | Kind::SculkSensor => get_int(state, "power") as u8,
        Kind::RedstoneBlock => 15,
        Kind::Repeater | Kind::Comparator => diode_signal(level, state, pos, dir),
        Kind::Observer => {
            if powered(state) && facing(state) == dir {
                15
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn lit_signal(state: u32, towards: bool) -> u8 {
    if get_bool(state, "lit") && towards {
        15
    } else {
        0
    }
}

/// `getDirectSignal`: strong power towards the neighbour in direction `-dir`.
pub fn direct_signal(level: &Level, state: u32, pos: BlockPos, dir: Dir) -> u8 {
    match kind(state) {
        Kind::RedstoneWire => wire_signal(level, state, pos, dir),
        Kind::RedstoneTorch | Kind::RedstoneWallTorch => {
            if dir == Dir::Down {
                signal(level, state, pos, dir)
            } else {
                0
            }
        }
        Kind::Lever | Kind::Button { .. } => {
            if powered(state) && attached_direction(state) == dir {
                15
            } else {
                0
            }
        }
        Kind::Repeater | Kind::Comparator | Kind::Observer => signal(level, state, pos, dir),
        Kind::PressurePlate { .. } | Kind::SculkSensor | Kind::Lectern | Kind::DetectorRail => {
            if dir == Dir::Up {
                signal(level, state, pos, dir)
            } else {
                0
            }
        }
        Kind::LightningRod | Kind::TripwireHook => {
            if powered(state) && facing(state) == dir {
                15
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// `isSignalSource`: the block emits power on its own (wires connect to it).
pub fn is_signal_source(level: &Level, state: u32) -> bool {
    match kind(state) {
        Kind::RedstoneWire => level.wire_signals(),
        Kind::RedstoneTorch
        | Kind::RedstoneWallTorch
        | Kind::Lever
        | Kind::Button { .. }
        | Kind::RedstoneBlock
        | Kind::Repeater
        | Kind::Comparator
        | Kind::Observer
        | Kind::DaylightDetector
        | Kind::TargetBlock
        | Kind::PressurePlate { .. }
        | Kind::LightningRod
        | Kind::TripwireHook
        | Kind::SculkSensor
        | Kind::Lectern
        | Kind::DetectorRail
        | Kind::PassiveSource => true,
        _ => false,
    }
}

/// `getAnalogOutputSignal`: what a comparator reads from a block that has an output.
pub fn analog_output(level: &Level, state: u32, pos: BlockPos) -> u8 {
    let name = block_name(state).strip_prefix("minecraft:").unwrap_or("");
    match name {
        "water_cauldron" | "powder_snow_cauldron" | "composter" => get_int(state, "level") as u8,
        "lava_cauldron" => 3,
        "cauldron" => 0,
        "cake" => ((7 - get_int(state, "bites")) * 2) as u8,
        "end_portal_frame" => {
            if get_bool(state, "eye") {
                15
            } else {
                0
            }
        }
        "respawn_anchor" => (get_int(state, "charges") as f32 / 4.0 * 15.0).floor() as u8,
        "beehive" | "bee_nest" => get_int(state, "honey_level") as u8,
        n if n.ends_with("copper_bulb") => {
            if get_bool(state, "lit") {
                15
            } else {
                0
            }
        }
        n if n.ends_with("candle_cake") => 14,
        _ => container_signal(level.world, pos, state),
    }
}

/// HOOK for the inventory system: the comparator signal of a container block (chests,
/// barrels, hoppers, furnaces, ...), vanilla's `getRedstoneSignalFromContainer`.
///
/// Until containers are simulated this reads the `Items` of the block entity as it is stored
/// in the chunk (so containers from loaded worlds work); replace the body once container
/// contents live elsewhere.
pub fn container_signal(world: &World, pos: BlockPos, state: u32) -> u8 {
    let name = block_name(state).strip_prefix("minecraft:").unwrap_or("");
    let size = match name {
        "hopper" | "brewing_stand" => 5,
        "dispenser" | "dropper" | "crafter" => 9,
        "furnace" | "blast_furnace" | "smoker" => 3,
        "chiseled_bookshelf" => 6,
        "decorated_pot" | "jukebox" | "lectern" => 1,
        n if n.ends_with("chest") || n.ends_with("barrel") || n.ends_with("shulker_box") => 27,
        _ => return 0,
    };
    let mut positions = vec![pos];
    if name.ends_with("chest") {
        // A double chest reads both halves.
        let other = match prop(state, "type") {
            Some("left") => Some(facing(state).clockwise()),
            Some("right") => Some(facing(state).counter_clockwise()),
            _ => None,
        };
        if let Some(dir) = other {
            positions.push(pos.rel(dir));
        }
    }
    let mut total = 0.0f32;
    for p in &positions {
        let Some(chunk) = world.chunks.get(&ChunkPos::of_block(p.x, p.z)) else { continue };
        let Some(entity) = chunk.block_entities.get(&(p.y, (p.z & 15) as u8, (p.x & 15) as u8)) else { continue };
        for item in entity.list("Items").unwrap_or(&[]) {
            let Some(item) = item.as_compound() else { continue };
            let count = item.i32("count").or_else(|| item.i32("Count")).unwrap_or(1).max(0) as f32;
            let max = item.string("id").and_then(paper_data::items::by_name).map_or(64, |i| i.max_stack_size.max(1)) as f32;
            total += count / max;
        }
    }
    let fraction = total / (size * positions.len()) as f32;
    (fraction * 14.0).floor() as u8 + u8::from(fraction > 0.0)
}

// --- redstone wire ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Up,
    Side,
    None,
}

fn side(state: u32, dir: Dir) -> Side {
    match prop(state, dir.name()) {
        Some("up") => Side::Up,
        Some("side") => Side::Side,
        _ => Side::None,
    }
}

fn set_side(state: u32, dir: Dir, side: Side) -> u32 {
    with(state, dir.name(), match side {
        Side::Up => "up",
        Side::Side => "side",
        Side::None => "none",
    })
}

fn connected(state: u32, dir: Dir) -> bool {
    side(state, dir) != Side::None
}

fn is_dot(state: u32) -> bool {
    Dir::HORIZONTAL.iter().all(|d| !connected(state, *d))
}

fn is_cross(state: u32) -> bool {
    Dir::HORIZONTAL.iter().all(|d| connected(state, *d))
}

fn wire_power(state: u32) -> u8 {
    if is_wire(state) {
        get_int(state, "power") as u8
    } else {
        0
    }
}

fn cross_state() -> u32 {
    Dir::HORIZONTAL.iter().fold(default_state!("redstone_wire"), |s, d| set_side(s, *d, Side::Side))
}

/// The state a wire placed at `pos` gets: a cross, shaped to its neighbours.
pub fn wire_placement_state(level: &Level, pos: BlockPos) -> u32 {
    connection_state(level, cross_state(), pos)
}

/// `getConnectionState`.
fn connection_state(level: &Level, state: u32, pos: BlockPos) -> u32 {
    let was_dot = is_dot(state);
    let mut s = missing_connections(level, set_int(default_state!("redstone_wire"), "power", get_int(state, "power")), pos);
    if was_dot && is_dot(s) {
        return s;
    }
    let (n, south, e, w) = (connected(s, Dir::North), connected(s, Dir::South), connected(s, Dir::East), connected(s, Dir::West));
    let north_south_empty = !n && !south;
    let east_west_empty = !e && !w;
    if !w && north_south_empty {
        s = set_side(s, Dir::West, Side::Side);
    }
    if !e && north_south_empty {
        s = set_side(s, Dir::East, Side::Side);
    }
    if !n && east_west_empty {
        s = set_side(s, Dir::North, Side::Side);
    }
    if !south && east_west_empty {
        s = set_side(s, Dir::South, Side::Side);
    }
    s
}

fn missing_connections(level: &Level, mut state: u32, pos: BlockPos) -> u32 {
    let can_connect_up = !physics::is_redstone_conductor(level.state(pos.above()));
    for dir in Dir::HORIZONTAL {
        if !connected(state, dir) {
            state = set_side(state, dir, connecting_side(level, pos, dir, can_connect_up));
        }
    }
    state
}

/// `getConnectingSide`.
fn connecting_side(level: &Level, pos: BlockPos, dir: Dir, can_connect_up: bool) -> Side {
    let rpos = pos.rel(dir);
    let rstate = level.state(rpos);
    if can_connect_up {
        let placeable_above = matches!(kind(rstate), Kind::Trapdoor { .. }) || can_survive_on(rstate);
        if placeable_above && should_connect_to(level, level.state(rpos.above()), None) {
            return if physics::is_face_sturdy(rstate, dir.opposite().index()) { Side::Up } else { Side::Side };
        }
    }
    if !should_connect_to(level, rstate, Some(dir)) && (physics::is_redstone_conductor(rstate) || !should_connect_to(level, level.state(rpos.below()), None)) {
        Side::None
    } else {
        Side::Side
    }
}

/// `shouldConnectTo`.
fn should_connect_to(level: &Level, state: u32, dir: Option<Dir>) -> bool {
    match kind(state) {
        Kind::RedstoneWire => true,
        Kind::Repeater => {
            let f = facing(state);
            dir == Some(f) || dir == Some(f.opposite())
        }
        Kind::Observer => dir == Some(facing(state)),
        _ => is_signal_source(level, state) && dir.is_some(),
    }
}

/// Wire can sit on a sturdy top face or a hopper.
fn can_survive_on(state: u32) -> bool {
    physics::is_face_sturdy(state, Dir::Up.index()) || block_id(state) == block!("hopper")
}

pub fn wire_can_survive(level: &Level, pos: BlockPos) -> bool {
    can_survive_on(level.state(pos.below()))
}

fn wire_signal(level: &Level, state: u32, pos: BlockPos, dir: Dir) -> u8 {
    if !level.wire_signals() || dir == Dir::Down {
        return 0;
    }
    let power = get_int(state, "power") as u8;
    if power == 0 {
        return 0;
    }
    if dir != Dir::Up && !connected(connection_state(level, state, pos), dir.opposite()) {
        0
    } else {
        power
    }
}

pub fn wire_update_shape(level: &Level, state: u32, pos: BlockPos, dir: Dir, neighbor_state: u32) -> u32 {
    match dir {
        Dir::Down => {
            if can_survive_on(neighbor_state) {
                state
            } else {
                super::state::default_of(block!("air"))
            }
        }
        Dir::Up => connection_state(level, state, pos),
        _ => {
            let can_connect_up = !physics::is_redstone_conductor(level.state(pos.above()));
            let new_side = connecting_side(level, pos, dir, can_connect_up);
            if (new_side != Side::None) == connected(state, dir) && !is_cross(state) {
                set_side(state, dir, new_side)
            } else {
                connection_state(level, set_side(set_int(cross_state(), "power", get_int(state, "power")), dir, new_side), pos)
            }
        }
    }
}

/// `updateIndirectNeighbourShapes`: wires one block up or down diagonally re-check their
/// connection towards this one.
pub fn wire_update_indirect_shapes(level: &mut Level, state: u32, pos: BlockPos, flags: u32, limit: i32) {
    for dir in Dir::HORIZONTAL {
        if side(state, dir) == Side::None || is_wire(level.state(pos.rel(dir))) {
            continue;
        }
        for vertical in [Dir::Down, Dir::Up] {
            let diagonal = pos.rel(dir).rel(vertical);
            if is_wire(level.state(diagonal)) {
                let npos = diagonal.rel(dir.opposite());
                let nstate = level.state(npos);
                level.neighbor_shape_changed(dir.opposite(), diagonal, npos, nstate, flags, limit);
            }
        }
    }
}

/// `DefaultRedstoneWireEvaluator.updatePowerStrength`.
fn update_power_strength(level: &mut Level, pos: BlockPos, state: u32) {
    let target = target_strength(level, pos);
    if get_int(state, "power") as u8 == target {
        return;
    }
    if level.state(pos) == state {
        level.set_block(pos, set_int(state, "power", target as i32), flags::CLIENTS);
    }
    let mut around = vec![pos];
    around.extend(Dir::ALL.iter().map(|d| pos.rel(*d)));
    for p in java_hash_order(&around) {
        level.update_neighbors_at(p, block!("redstone_wire"));
    }
}

fn target_strength(level: &Level, pos: BlockPos) -> u8 {
    let block_signal = level.without_wire_signals(|| level.get_best_neighbor_signal(pos));
    if block_signal == 15 {
        15
    } else {
        block_signal.max(incoming_wire_signal(level, pos))
    }
}

/// `getIncomingWireSignal`: the strongest neighbouring wire (also one up or down a step),
/// minus one.
fn incoming_wire_signal(level: &Level, pos: BlockPos) -> u8 {
    let mut signal = 0;
    let above_conducts = physics::is_redstone_conductor(level.state(pos.above()));
    for dir in Dir::HORIZONTAL {
        let npos = pos.rel(dir);
        let nstate = level.state(npos);
        signal = signal.max(wire_power(nstate));
        if physics::is_redstone_conductor(nstate) {
            if !above_conducts {
                signal = signal.max(wire_power(level.state(npos.above())));
            }
        } else {
            signal = signal.max(wire_power(level.state(npos.below())));
        }
    }
    signal.saturating_sub(1)
}

fn check_corner_change_at(level: &mut Level, pos: BlockPos) {
    if is_wire(level.state(pos)) {
        level.update_neighbors_at(pos, block!("redstone_wire"));
        for dir in Dir::ALL {
            level.update_neighbors_at(pos.rel(dir), block!("redstone_wire"));
        }
    }
}

fn update_neighbors_of_neighboring_wires(level: &mut Level, pos: BlockPos) {
    for dir in Dir::HORIZONTAL {
        check_corner_change_at(level, pos.rel(dir));
    }
    for dir in Dir::HORIZONTAL {
        let target = pos.rel(dir);
        if physics::is_redstone_conductor(level.state(target)) {
            check_corner_change_at(level, target.above());
        } else {
            check_corner_change_at(level, target.below());
        }
    }
}

pub fn wire_on_place(level: &mut Level, state: u32, pos: BlockPos, old: u32) {
    if is_wire(old) {
        return;
    }
    update_power_strength(level, pos, state);
    for dir in Dir::VERTICAL {
        level.update_neighbors_at(pos.rel(dir), block!("redstone_wire"));
    }
    update_neighbors_of_neighboring_wires(level, pos);
}

pub fn wire_after_removal(level: &mut Level, old: u32, pos: BlockPos) {
    for dir in Dir::ALL {
        level.update_neighbors_at(pos.rel(dir), block!("redstone_wire"));
    }
    update_power_strength(level, pos, old);
    update_neighbors_of_neighboring_wires(level, pos);
}

pub fn wire_neighbor_changed(level: &mut Level, state: u32, pos: BlockPos) {
    if wire_can_survive(level, pos) {
        update_power_strength(level, pos, state);
    } else {
        level.drop_resources(state, pos);
        level.remove_block(pos, false);
    }
}

/// Right-clicking a cross or dot toggles between the two.
pub fn wire_use(level: &mut Level, state: u32, pos: BlockPos, ctx: UseContext) -> Interaction {
    if !ctx.may_build || !(is_cross(state) || is_dot(state)) {
        return Interaction::Pass;
    }
    let toggled = if is_cross(state) { default_state!("redstone_wire") } else { cross_state() };
    let new = connection_state(level, set_int(toggled, "power", get_int(state, "power")), pos);
    if new == state {
        return Interaction::Pass;
    }
    level.set_block(pos, new, flags::ALL);
    for dir in Dir::HORIZONTAL {
        let rpos = pos.rel(dir);
        if connected(state, dir) != connected(new, dir) && physics::is_redstone_conductor(level.state(rpos)) {
            level.update_neighbors_at_except(rpos, block!("redstone_wire"), dir.opposite());
        }
    }
    Interaction::Success
}

// --- redstone torches ---------------------------------------------------------------------------

pub fn torch_notify(level: &mut Level, state: u32, pos: BlockPos) {
    for dir in Dir::ALL {
        level.update_neighbors_at(pos.rel(dir), block_id(state));
    }
}

fn torch_powered(level: &Level, state: u32, pos: BlockPos) -> bool {
    match kind(state) {
        Kind::RedstoneWallTorch => {
            let dir = facing(state).opposite();
            level.has_signal(pos.rel(dir), dir)
        }
        _ => level.has_signal(pos.below(), Dir::Down),
    }
}

pub fn torch_neighbor_changed(level: &mut Level, state: u32, pos: BlockPos) {
    if get_bool(state, "lit") == torch_powered(level, state, pos) && !level.will_tick_this_tick(pos, block_id(state)) {
        level.schedule_tick(pos, block_id(state), 2);
    }
}

pub fn torch_tick(level: &mut Level, state: u32, pos: BlockPos) {
    let now = level.game_time();
    let toggles = &mut level.world.updates.torch_toggles;
    while toggles.front().is_some_and(|(_, when)| now - *when > TORCH_TOGGLE_WINDOW) {
        toggles.pop_front();
    }
    let powered = torch_powered(level, state, pos);
    if get_bool(state, "lit") {
        if powered {
            level.set_block(pos, set_bool(state, "lit", false), flags::ALL);
            if toggled_too_often(level, pos, true) {
                level.level_event(1502, pos, 0);
                let block = block_id(level.state(pos));
                level.schedule_tick(pos, block, TORCH_RESTART_DELAY);
            }
        }
    } else if !powered && !toggled_too_often(level, pos, false) {
        level.set_block(pos, set_bool(state, "lit", true), flags::ALL);
    }
}

fn toggled_too_often(level: &mut Level, pos: BlockPos, add: bool) -> bool {
    let now = level.game_time();
    let toggles = &mut level.world.updates.torch_toggles;
    if add {
        toggles.push_back((pos, now));
    }
    toggles.iter().filter(|(p, _)| *p == pos).count() >= TORCH_MAX_TOGGLES
}

// --- levers and buttons -------------------------------------------------------------------------

/// The direction from a lever or button to the block it is attached to... inverted: vanilla's
/// `getConnectedDirection` (`up` for a floor lever, which hangs on the block below).
pub fn attached_direction(state: u32) -> Dir {
    match prop(state, "face") {
        Some("ceiling") => Dir::Down,
        Some("floor") => Dir::Up,
        _ => facing(state),
    }
}

/// Levers and buttons update their own neighbours and those of the block they are on.
pub fn attached_update_neighbours(level: &mut Level, state: u32, pos: BlockPos) {
    let front = attached_direction(state).opposite();
    level.update_neighbors_at(pos, block_id(state));
    level.update_neighbors_at(pos.rel(front), block_id(state));
}

pub fn lever_use(level: &mut Level, state: u32, pos: BlockPos) -> Interaction {
    let new = cycle(state, "powered");
    level.set_block(pos, new, flags::ALL);
    attached_update_neighbours(level, new, pos);
    let pitch = if powered(new) { 0.6 } else { 0.5 };
    level.play_sound(None, pos, "minecraft:block.lever.click", sound_source::BLOCKS, 0.3, pitch);
    Interaction::Success
}

fn button_press_ticks(state: u32) -> i32 {
    match kind(state) {
        Kind::Button { wooden: true } => 30,
        _ => 20,
    }
}

fn button_sound(level: &mut Level, except: Option<i32>, pos: BlockPos, state: u32, pressed: bool) {
    let sound = format!("minecraft:block.{}_button.click_{}", block_set(state), if pressed { "on" } else { "off" });
    level.play_sound(if pressed { except } else { None }, pos, &sound, sound_source::BLOCKS, 1.0, 1.0);
}

pub fn button_use(level: &mut Level, state: u32, pos: BlockPos, ctx: UseContext) -> Interaction {
    if powered(state) {
        return Interaction::Consume;
    }
    level.set_block(pos, set_bool(state, "powered", true), flags::ALL);
    attached_update_neighbours(level, state, pos);
    level.schedule_tick(pos, block_id(state), button_press_ticks(state));
    button_sound(level, Some(ctx.player), pos, state, true);
    Interaction::Success
}

/// A pressed button pops out again (nothing keeps it pressed without arrows).
pub fn button_tick(level: &mut Level, state: u32, pos: BlockPos) {
    if !powered(state) {
        return;
    }
    level.set_block(pos, set_bool(state, "powered", false), flags::ALL);
    attached_update_neighbours(level, state, pos);
    button_sound(level, None, pos, state, false);
}

// --- repeaters and comparators ------------------------------------------------------------------

fn diode_delay(state: u32) -> i32 {
    match kind(state) {
        Kind::Repeater => get_int(state, "delay") * 2,
        _ => 2,
    }
}

/// `DiodeBlock.getInputSignal`: power from behind.
fn diode_input(level: &Level, state: u32, pos: BlockPos) -> u8 {
    let dir = facing(state);
    let target = pos.rel(dir);
    let input = level.get_signal(target, dir);
    if input >= 15 {
        return input;
    }
    input.max(wire_power(level.state(target)))
}

/// `ComparatorBlock.getInputSignal`: also reads blocks with a comparator output, directly
/// behind or behind a conductor.
fn comparator_input(level: &Level, state: u32, pos: BlockPos) -> u8 {
    let mut result = diode_input(level, state, pos);
    let dir = facing(state);
    let source = pos.rel(dir);
    let source_state = level.state(source);
    if physics::has_analog_output(source_state) {
        result = analog_output(level, source_state, source);
    } else if result < 15 && physics::is_redstone_conductor(source_state) {
        let further = source.rel(dir);
        let further_state = level.state(further);
        if physics::has_analog_output(further_state) {
            result = analog_output(level, further_state, further);
        }
    }
    result
}

fn input_signal(level: &Level, state: u32, pos: BlockPos) -> u8 {
    match kind(state) {
        Kind::Comparator => comparator_input(level, state, pos),
        _ => diode_input(level, state, pos),
    }
}

/// `getAlternateSignal`: the stronger side input (repeaters only see diodes there).
fn side_input(level: &Level, state: u32, pos: BlockPos) -> u8 {
    let dir = facing(state);
    let only_diodes = kind(state) == Kind::Repeater;
    let (cw, ccw) = (dir.clockwise(), dir.counter_clockwise());
    level.get_control_input_signal(pos.rel(cw), cw, only_diodes).max(level.get_control_input_signal(pos.rel(ccw), ccw, only_diodes))
}

fn subtract_mode(state: u32) -> bool {
    prop(state, "mode") == Some("subtract")
}

fn should_turn_on(level: &Level, state: u32, pos: BlockPos) -> bool {
    match kind(state) {
        Kind::Comparator => {
            let input = comparator_input(level, state, pos);
            if input == 0 {
                return false;
            }
            let side = side_input(level, state, pos);
            input > side || (input == side && !subtract_mode(state))
        }
        _ => input_signal(level, state, pos) > 0,
    }
}

/// A repeater powered from the side by another diode is locked.
pub fn repeater_locked(level: &Level, state: u32, pos: BlockPos) -> bool {
    kind(state) == Kind::Repeater && side_input(level, state, pos) > 0
}

/// `shouldPrioritize`: the diode feeds into another diode that does not face back.
fn should_prioritize(level: &Level, state: u32, pos: BlockPos) -> bool {
    let dir = facing(state).opposite();
    let other = level.state(pos.rel(dir));
    is_diode(other) && facing(other) != dir
}

fn comparator_output_signal(world: &World, pos: BlockPos) -> u8 {
    world
        .chunks
        .get(&ChunkPos::of_block(pos.x, pos.z))
        .and_then(|c| c.block_entities.get(&(pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8)))
        .and_then(|e| e.i32("OutputSignal"))
        .unwrap_or(0)
        .clamp(0, 15) as u8
}

fn set_comparator_output(world: &mut World, pos: BlockPos, value: u8) {
    let Some(chunk) = world.chunks.get_mut(&ChunkPos::of_block(pos.x, pos.z)) else { return };
    let entity = chunk.block_entities.entry((pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8)).or_insert_with(|| Compound::new().with("id", Tag::String("minecraft:comparator".into())));
    if entity.i32("OutputSignal") != Some(value as i32) {
        entity.insert("OutputSignal", Tag::Int(value as i32));
        chunk.dirty = true;
    }
}

fn diode_signal(level: &Level, state: u32, pos: BlockPos, dir: Dir) -> u8 {
    if !powered(state) || facing(state) != dir {
        return 0;
    }
    match kind(state) {
        Kind::Comparator => comparator_output_signal(level.world, pos),
        _ => 15,
    }
}

/// `updateNeighborsInFront`: the block the diode (or observer) points into and its
/// neighbours.
pub fn diode_update_in_front(level: &mut Level, state: u32, pos: BlockPos) {
    let dir = facing(state);
    let front = pos.rel(dir.opposite());
    level.neighbor_changed_at(front, block_id(state));
    level.update_neighbors_at_except(front, block_id(state), dir);
}

pub fn diode_on_place(level: &mut Level, state: u32, pos: BlockPos) {
    diode_update_in_front(level, state, pos);
}

/// A comparator keeps its output in a block entity.
pub fn comparator_on_place(level: &mut Level, _state: u32, pos: BlockPos) {
    let output = comparator_output_signal(level.world, pos);
    set_comparator_output(level.world, pos, output);
}

/// Diodes sit on rigid support (hoppers, cauldrons and composters count).
pub fn diode_can_survive_on(state: u32) -> bool {
    physics::is_face_sturdy(state, Dir::Up.index()) || matches!(block_name(state), "minecraft:hopper" | "minecraft:cauldron" | "minecraft:water_cauldron" | "minecraft:lava_cauldron" | "minecraft:powder_snow_cauldron" | "minecraft:composter")
}

pub fn diode_neighbor_changed(level: &mut Level, state: u32, pos: BlockPos) {
    if level.can_survive(state, pos) {
        check_tick_on_neighbor(level, state, pos);
        return;
    }
    level.drop_resources(state, pos);
    level.remove_block(pos, false);
    for dir in Dir::ALL {
        level.update_neighbors_at(pos.rel(dir), block_id(state));
    }
}

fn check_tick_on_neighbor(level: &mut Level, state: u32, pos: BlockPos) {
    let block = block_id(state);
    if kind(state) == Kind::Comparator {
        if level.will_tick_this_tick(pos, block) {
            return;
        }
        let output = comparator_signal_now(level, state, pos);
        let old = comparator_output_signal(level.world, pos);
        if output != old || powered(state) != should_turn_on(level, state, pos) {
            let priority = if should_prioritize(level, state, pos) { TickPriority::High } else { TickPriority::Normal };
            level.schedule_tick_with(pos, block, 2, priority);
        }
        return;
    }
    if repeater_locked(level, state, pos) {
        return;
    }
    let on = powered(state);
    if on != should_turn_on(level, state, pos) && !level.will_tick_this_tick(pos, block) {
        let priority = if should_prioritize(level, state, pos) {
            TickPriority::ExtremelyHigh
        } else if on {
            TickPriority::VeryHigh
        } else {
            TickPriority::High
        };
        level.schedule_tick_with(pos, block, diode_delay(state), priority);
    }
}

/// `DiodeBlock.tick` for repeaters.
pub fn repeater_tick(level: &mut Level, state: u32, pos: BlockPos) {
    if repeater_locked(level, state, pos) {
        return;
    }
    let on = powered(state);
    let should = should_turn_on(level, state, pos);
    if on && !should {
        level.set_block(pos, set_bool(state, "powered", false), flags::CLIENTS);
    } else if !on {
        level.set_block(pos, set_bool(state, "powered", true), flags::CLIENTS);
        if !should {
            level.schedule_tick_with(pos, block_id(state), diode_delay(state), TickPriority::VeryHigh);
        }
    }
}

/// `calculateOutputSignal`: compare passes the rear signal unless a side is stronger;
/// subtract takes the side away.
fn comparator_signal_now(level: &Level, state: u32, pos: BlockPos) -> u8 {
    let input = comparator_input(level, state, pos);
    if input == 0 {
        return 0;
    }
    let side = side_input(level, state, pos);
    if side > input {
        0
    } else if subtract_mode(state) {
        input - side
    } else {
        input
    }
}

/// `refreshOutputState`.
fn comparator_refresh(level: &mut Level, state: u32, pos: BlockPos) {
    let output = comparator_signal_now(level, state, pos);
    let old = comparator_output_signal(level.world, pos);
    set_comparator_output(level.world, pos, output);
    if old != output || !subtract_mode(state) {
        let should = should_turn_on(level, state, pos);
        let on = powered(state);
        if on && !should {
            level.set_block(pos, set_bool(state, "powered", false), flags::CLIENTS);
        } else if !on && should {
            level.set_block(pos, set_bool(state, "powered", true), flags::CLIENTS);
        }
        diode_update_in_front(level, state, pos);
    }
}

pub fn comparator_tick(level: &mut Level, state: u32, pos: BlockPos) {
    comparator_refresh(level, state, pos);
}

pub fn repeater_update_shape(level: &Level, state: u32, pos: BlockPos, dir: Dir, neighbor_state: u32) -> u32 {
    if dir == Dir::Down && !diode_can_survive_on(neighbor_state) {
        return default_of(block!("air"));
    }
    if dir.axis() != facing(state).axis() {
        return set_bool(state, "locked", repeater_locked(level, state, pos));
    }
    state
}

pub fn comparator_update_shape(_level: &Level, state: u32, dir: Dir, neighbor_state: u32) -> u32 {
    if dir == Dir::Down && !diode_can_survive_on(neighbor_state) {
        default_of(block!("air"))
    } else {
        state
    }
}

/// Right-clicking a repeater cycles its delay.
pub fn repeater_use(level: &mut Level, state: u32, pos: BlockPos, ctx: UseContext) -> Interaction {
    if !ctx.may_build {
        return Interaction::Pass;
    }
    level.set_block(pos, cycle(state, "delay"), flags::ALL);
    Interaction::Success
}

/// Right-clicking a comparator toggles compare and subtract mode.
pub fn comparator_use(level: &mut Level, state: u32, pos: BlockPos, ctx: UseContext) -> Interaction {
    if !ctx.may_build {
        return Interaction::Pass;
    }
    let new = cycle(state, "mode");
    let pitch = if subtract_mode(new) { 0.55 } else { 0.5 };
    level.play_sound(Some(ctx.player), pos, "minecraft:block.comparator.click", sound_source::BLOCKS, 0.3, pitch);
    level.set_block(pos, new, flags::CLIENTS);
    comparator_refresh(level, new, pos);
    Interaction::Success
}

/// After placement (`setPlacedBy`): a powered diode turns on next tick.
pub fn diode_placed(level: &mut Level, state: u32, pos: BlockPos) {
    if should_turn_on(level, state, pos) {
        level.schedule_tick(pos, block_id(state), 1);
    }
}

// --- redstone lamps -----------------------------------------------------------------------------

pub fn lamp_neighbor_changed(level: &mut Level, state: u32, pos: BlockPos) {
    let lit = get_bool(state, "lit");
    if lit == level.has_neighbor_signal(pos) {
        return;
    }
    if lit {
        level.schedule_tick(pos, block_id(state), 4);
    } else {
        level.set_block(pos, cycle(state, "lit"), flags::CLIENTS);
    }
}

pub fn lamp_tick(level: &mut Level, state: u32, pos: BlockPos) {
    if get_bool(state, "lit") && !level.has_neighbor_signal(pos) {
        level.set_block(pos, cycle(state, "lit"), flags::CLIENTS);
    }
}

// --- observers ----------------------------------------------------------------------------------

pub fn observer_tick(level: &mut Level, state: u32, pos: BlockPos) {
    if powered(state) {
        level.set_block(pos, set_bool(state, "powered", false), flags::CLIENTS);
    } else {
        level.set_block(pos, set_bool(state, "powered", true), flags::CLIENTS);
        level.schedule_tick(pos, block_id(state), 2);
    }
    diode_update_in_front(level, state, pos);
}

/// An observer sees the block in front of it change.
pub fn observer_update_shape(level: &mut Level, state: u32, pos: BlockPos, dir: Dir) -> u32 {
    if facing(state) == dir && !powered(state) && !level.has_scheduled_tick(pos, block_id(state)) {
        level.schedule_tick(pos, block_id(state), 2);
    }
    state
}

pub fn observer_on_place(level: &mut Level, state: u32, pos: BlockPos, old: u32) {
    if block_id(state) == block_id(old) || !powered(state) || level.has_scheduled_tick(pos, block_id(state)) {
        return;
    }
    let off = set_bool(state, "powered", false);
    level.set_block(pos, off, flags::CLIENTS | flags::KNOWN_SHAPE);
    diode_update_in_front(level, off, pos);
}

pub fn observer_after_removal(level: &mut Level, old: u32, pos: BlockPos) {
    if powered(old) && level.has_scheduled_tick(pos, block_id(old)) {
        diode_update_in_front(level, set_bool(old, "powered", false), pos);
    }
}

// --- note blocks --------------------------------------------------------------------------------

fn mob_head_instrument(instrument: &str) -> bool {
    matches!(instrument, "zombie" | "skeleton" | "creeper" | "dragon" | "wither_skeleton" | "piglin" | "custom_head")
}

/// `setInstrument`: a mob head on top, else the block below decides.
pub fn note_instrument(level: &Level, pos: BlockPos) -> &'static str {
    let above = physics::instrument(level.state(pos.above()));
    if mob_head_instrument(above) {
        return above;
    }
    let below = physics::instrument(level.state(pos.below()));
    if mob_head_instrument(below) {
        "harp"
    } else {
        below
    }
}

pub fn note_update_shape(level: &Level, state: u32, pos: BlockPos, dir: Dir) -> u32 {
    if dir.axis() == super::Axis::Y {
        with(state, "instrument", note_instrument(level, pos))
    } else {
        state
    }
}

fn play_note(level: &mut Level, state: u32, pos: BlockPos) {
    let instrument = prop(state, "instrument").unwrap_or("harp");
    if mob_head_instrument(instrument) || super::state::is_air(level.state(pos.above())) {
        level.block_event(pos, block_id(state), 0, 0);
    }
}

pub fn note_neighbor_changed(level: &mut Level, state: u32, pos: BlockPos) {
    let signal = level.has_neighbor_signal(pos);
    if signal != powered(state) {
        if signal {
            play_note(level, state, pos);
        }
        level.set_block(pos, set_bool(state, "powered", signal), flags::ALL);
    }
}

/// Right-clicking a note block raises its pitch and plays it.
pub fn note_use(level: &mut Level, state: u32, pos: BlockPos, _ctx: UseContext) -> Interaction {
    let new = cycle(state, "note");
    level.set_block(pos, new, flags::ALL);
    play_note(level, new, pos);
    Interaction::Success
}

/// The note block event: plays the sound (clients add the note particle themselves).
pub fn note_trigger(level: &mut Level, state: u32, pos: BlockPos) -> bool {
    let instrument = prop(state, "instrument").unwrap_or("harp");
    let pitch = if mob_head_instrument(instrument) { 1.0 } else { 2f32.powf((get_int(state, "note") - 12) as f32 / 12.0) };
    let sound = match instrument {
        // Player heads play the sound stored in their block entity; not modelled.
        "custom_head" => return false,
        "dragon" => "minecraft:block.note_block.imitate.ender_dragon".to_owned(),
        i if mob_head_instrument(i) => format!("minecraft:block.note_block.imitate.{i}"),
        i => format!("minecraft:block.note_block.{i}"),
    };
    level.play_sound(None, pos, &sound, sound_source::RECORDS, 3.0, pitch);
    true
}
