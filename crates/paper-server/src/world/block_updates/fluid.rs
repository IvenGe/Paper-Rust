//! Water and lava: vanilla's `FlowingFluid`, `WaterFluid`, `LavaFluid` and `LiquidBlock`.
//!
//! Derived in part from Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0:
//! `crates/pumpkin-core/src/block/fluid/` (flowing_trait.rs, pathfinder.rs, lava.rs), which
//! served as a reference next to vanilla's behaviour.
//!
//! A fluid block's `level` property 0 is a source; 1-7 is flowing fluid with 8 - level
//! "amount"; 8 and above is falling fluid. Waterlogged blocks, kelp, seagrass and bubble
//! columns hold a water source. Flowing fluid recomputes itself from its neighbours on every
//! fluid tick (`getNewLiquid`), then spreads: down if it can, otherwise sideways towards the
//! nearest way down within the slope search distance.

use super::state::{block, block_id, default_state, get_bool, get_int, has_prop, in_tag, set_bool, set_int};
use super::{flags, Dir, Kind, Level, PosExt, UpdateSettings};
use paper_data::{blocks, physics};
use paper_protocol::codec::BlockPos;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fluid {
    Empty,
    Water,
    Lava,
}

/// A fluid in a block: vanilla's `FluidState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluidState {
    pub fluid: Fluid,
    /// 1-8; sources and falling fluid have 8, empty has 0.
    pub amount: u8,
    pub source: bool,
    pub falling: bool,
}

fn fluid_registry_id(name: &str) -> u32 {
    paper_data::registries::id("minecraft:fluid", name).unwrap_or(0)
}

impl FluidState {
    pub const EMPTY: FluidState = FluidState { fluid: Fluid::Empty, amount: 0, source: false, falling: false };

    pub fn source(fluid: Fluid) -> FluidState {
        FluidState { fluid, amount: 8, source: true, falling: false }
    }

    pub fn flowing(fluid: Fluid, amount: u8, falling: bool) -> FluidState {
        FluidState { fluid, amount, source: false, falling }
    }

    pub fn is_empty(self) -> bool {
        self.fluid == Fluid::Empty
    }

    /// Id of the fluid type in `minecraft:fluid` (`water` for sources, `flowing_water`
    /// otherwise), which is what fluid ticks are scheduled for.
    pub fn type_id(self) -> u32 {
        use std::sync::OnceLock;
        static IDS: OnceLock<[u32; 5]> = OnceLock::new();
        let ids = IDS.get_or_init(|| {
            ["minecraft:empty", "minecraft:flowing_water", "minecraft:water", "minecraft:flowing_lava", "minecraft:lava"].map(fluid_registry_id)
        });
        match (self.fluid, self.source) {
            (Fluid::Empty, _) => ids[0],
            (Fluid::Water, false) => ids[1],
            (Fluid::Water, true) => ids[2],
            (Fluid::Lava, false) => ids[3],
            (Fluid::Lava, true) => ids[4],
        }
    }

    /// `createLegacyBlock`: the block that holds just this fluid.
    pub fn legacy_block(self) -> u32 {
        let block = match self.fluid {
            Fluid::Empty => return blocks::AIR,
            Fluid::Water => default_state!("water"),
            Fluid::Lava => default_state!("lava"),
        };
        let level = if self.source { 0 } else { 8 - self.amount.min(8) as i32 + if self.falling { 8 } else { 0 } };
        set_int(block, "level", level)
    }

    /// `getOwnHeight`.
    pub fn own_height(self) -> f32 {
        self.amount as f32 / 9.0
    }

    /// `getHeight`: full height under the same fluid.
    pub fn height(self, level: &Level, pos: BlockPos) -> f32 {
        if fluid_state(level.state(pos.above())).fluid == self.fluid && !self.is_empty() {
            1.0
        } else {
            self.own_height()
        }
    }
}

/// The fluid a block state holds.
pub fn fluid_state(state: u32) -> FluidState {
    if blocks::flags(state) & blocks::FLAG_FLUID == 0 {
        return FluidState::EMPTY;
    }
    let fluid = match super::kind(state) {
        Kind::Water => Fluid::Water,
        Kind::Lava => Fluid::Lava,
        // Waterlogged blocks, kelp, seagrass, bubble columns.
        _ => return FluidState::source(Fluid::Water),
    };
    match get_int(state, "level") {
        0 => FluidState::source(fluid),
        level @ 1..=7 => FluidState::flowing(fluid, (8 - level) as u8, false),
        _ => FluidState::flowing(fluid, 8, true),
    }
}

/// Fluid ticks between updates.
pub fn tick_delay(fluid: Fluid, settings: UpdateSettings) -> i32 {
    match fluid {
        Fluid::Lava if !settings.fast_lava => 30,
        Fluid::Lava => 10,
        _ => 5,
    }
}

fn slope_find_distance(fluid: Fluid, settings: UpdateSettings) -> i32 {
    match fluid {
        Fluid::Lava if !settings.fast_lava => 2,
        _ => 4,
    }
}

fn drop_off(fluid: Fluid, settings: UpdateSettings) -> i32 {
    match fluid {
        Fluid::Lava if !settings.fast_lava => 2,
        _ => 1,
    }
}

fn can_convert_to_source(fluid: Fluid, settings: UpdateSettings) -> bool {
    match fluid {
        Fluid::Water => settings.water_source_conversion,
        Fluid::Lava => settings.lava_source_conversion,
        Fluid::Empty => false,
    }
}

fn is_source_of(fluid: Fluid, fs: FluidState) -> bool {
    fs.fluid == fluid && fs.source
}

// --- what fluids can enter -------------------------------------------------------------------

/// A `LiquidBlockContainer`: waterloggable blocks, kelp and seagrass.
fn is_liquid_container(state: u32) -> bool {
    has_prop(state, "waterlogged") || matches!(block_id(state), b if b == block!("kelp") || b == block!("kelp_plant") || b == block!("seagrass") || b == block!("tall_seagrass"))
}

/// `canPlaceLiquid` of a container: waterloggable blocks take still water.
fn can_place_liquid(state: u32, fluid: FluidState) -> bool {
    has_prop(state, "waterlogged") && fluid.fluid == Fluid::Water && fluid.source
}

/// `canHoldAnyFluid`: containers, and blocks fluids wash away (in 26.3 the
/// `washed_away_by_fluids` and `blocks_fluid_flow` tags).
fn can_hold_any_fluid(state: u32) -> bool {
    if is_liquid_container(state) {
        return true;
    }
    !in_tag!(state, "blocks_fluid_flow") && in_tag!(state, "washed_away_by_fluids")
}

fn can_hold_specific_fluid(state: u32, fluid: FluidState) -> bool {
    if is_liquid_container(state) {
        can_place_liquid(state, fluid)
    } else {
        true
    }
}

fn can_hold_fluid(state: u32, fluid: FluidState) -> bool {
    can_hold_any_fluid(state) && can_hold_specific_fluid(state, fluid)
}

/// `canPassThroughWall`: whether the collision shapes of the two blocks leave an opening
/// between them.
fn can_pass_through_wall(dir: Dir, source: u32, target: u32) -> bool {
    let full = |s: u32| blocks::flags(s) & blocks::FLAG_FULL_CUBE != 0;
    let empty = |s: u32| blocks::flags(s) & blocks::FLAG_COLLISION == 0;
    if full(target) || full(source) {
        return false;
    }
    if empty(target) && empty(source) {
        return true;
    }
    !(physics::collision_face_full(source, dir.index()) || physics::collision_face_full(target, dir.opposite().index()))
}

/// `FluidState.canBeReplacedWith`: whether fluid `incoming` may replace `existing`.
fn can_be_replaced_with(existing: FluidState, level: &Level, pos: BlockPos, incoming: FluidState, dir: Dir) -> bool {
    match existing.fluid {
        Fluid::Empty => true,
        Fluid::Water => dir == Dir::Down && incoming.fluid != Fluid::Water,
        Fluid::Lava => existing.height(level, pos) >= 0.444_444_45 && incoming.fluid == Fluid::Water,
    }
}

fn can_maybe_pass_through(fluid: Fluid, source_state: u32, dir: Dir, test_state: u32, test_fluid: FluidState) -> bool {
    !is_source_of(fluid, test_fluid) && can_hold_any_fluid(test_state) && can_pass_through_wall(dir, source_state, test_state)
}

fn can_pass_through(fluid: Fluid, source_state: u32, dir: Dir, test_state: u32, test_fluid: FluidState) -> bool {
    can_maybe_pass_through(fluid, source_state, dir, test_state, test_fluid) && can_hold_specific_fluid(test_state, FluidState::flowing(fluid, 1, false))
}

/// `isWaterHole`: fluid at `top` could fall into `bottom`.
fn is_hole(fluid: Fluid, top_state: u32, bottom_state: u32) -> bool {
    if !can_pass_through_wall(Dir::Down, top_state, bottom_state) {
        return false;
    }
    fluid_state(bottom_state).fluid == fluid || can_hold_fluid(bottom_state, FluidState::flowing(fluid, 1, false))
}

// --- the fluid tick ----------------------------------------------------------------------------

/// `FlowingFluid.tick`.
pub fn tick(level: &mut Level, pos: BlockPos, mut state: u32, mut fs: FluidState) {
    let fluid = fs.fluid;
    if !fs.source {
        let new = get_new_liquid(level, fluid, pos, level.state(pos));
        let delay = spread_delay(level, fluid, pos, fs, new);
        if new.is_empty() {
            fs = new;
            state = blocks::AIR;
            level.set_block(pos, state, flags::ALL);
        } else if new != fs {
            fs = new;
            state = new.legacy_block();
            level.set_block(pos, state, flags::ALL);
            level.schedule_fluid_tick(pos, new.type_id(), delay);
        }
    }
    spread(level, fluid, pos, state, fs);
}

fn spread_delay(level: &mut Level, fluid: Fluid, pos: BlockPos, old: FluidState, new: FluidState) -> i32 {
    let delay = tick_delay(fluid, level.settings());
    if fluid == Fluid::Lava && !old.is_empty() && !new.is_empty() && !old.falling && !new.falling && new.height(level, pos) > old.height(level, pos) && level.random() % 4 != 0 {
        return delay * 4;
    }
    delay
}

/// `getNewLiquid`: what fluid belongs at `pos` given its neighbours.
fn get_new_liquid(level: &Level, fluid: Fluid, pos: BlockPos, state: u32) -> FluidState {
    let settings = level.settings();
    let mut highest = 0;
    let mut sources = 0;
    for dir in Dir::HORIZONTAL {
        let npos = pos.rel(dir);
        let nstate = level.state(npos);
        let nfluid = fluid_state(nstate);
        if nfluid.fluid == fluid && can_pass_through_wall(dir, state, nstate) {
            if nfluid.source {
                sources += 1;
            }
            highest = highest.max(nfluid.amount as i32);
        }
    }
    if sources >= 2 && can_convert_to_source(fluid, settings) {
        let below = level.state(pos.below());
        if physics::is_solid(below) || is_source_of(fluid, fluid_state(below)) {
            return FluidState::source(fluid);
        }
    }
    let above = level.state(pos.above());
    let above_fluid = fluid_state(above);
    if above_fluid.fluid == fluid && can_pass_through_wall(Dir::Up, state, above) {
        return FluidState::flowing(fluid, 8, true);
    }
    let amount = highest - drop_off(fluid, settings);
    if amount <= 0 {
        FluidState::EMPTY
    } else {
        FluidState::flowing(fluid, amount as u8, false)
    }
}

/// `FlowingFluid.spread`.
fn spread(level: &mut Level, fluid: Fluid, pos: BlockPos, state: u32, fs: FluidState) {
    if fs.is_empty() {
        return;
    }
    let below = pos.below();
    let below_state = level.state(below);
    let below_fluid = fluid_state(below_state);
    if can_maybe_pass_through(fluid, state, Dir::Down, below_state, below_fluid) {
        let new = get_new_liquid(level, fluid, below, below_state);
        if can_be_replaced_with(below_fluid, level, below, new, Dir::Down) && can_hold_specific_fluid(below_state, new) {
            spread_to(level, fluid, below, below_state, Dir::Down, new);
            if source_neighbor_count(level, fluid, pos) >= 3 {
                spread_to_sides(level, fluid, pos, fs, state);
            }
            return;
        }
    }
    if fs.source || !is_hole(fluid, state, below_state) {
        spread_to_sides(level, fluid, pos, fs, state);
    }
}

fn source_neighbor_count(level: &Level, fluid: Fluid, pos: BlockPos) -> usize {
    Dir::HORIZONTAL.iter().filter(|dir| is_source_of(fluid, fluid_state(level.state(pos.rel(**dir))))).count()
}

fn spread_to_sides(level: &mut Level, fluid: Fluid, pos: BlockPos, fs: FluidState, state: u32) {
    let neighbor = if fs.falling { 7 } else { fs.amount as i32 - drop_off(fluid, level.settings()) };
    if neighbor <= 0 {
        return;
    }
    for (dir, new) in get_spread(level, fluid, pos, state) {
        let npos = pos.rel(dir);
        let nstate = level.state(npos);
        spread_to(level, fluid, npos, nstate, dir, new);
    }
}

/// Caches block states and hole checks around the block that spreads (`SpreadContext`).
struct SpreadContext {
    states: HashMap<(i32, i32), u32>,
    holes: HashMap<(i32, i32), bool>,
}

impl SpreadContext {
    fn state(&mut self, level: &Level, pos: BlockPos) -> u32 {
        *self.states.entry((pos.x, pos.z)).or_insert_with(|| level.state(pos))
    }

    fn is_hole(&mut self, level: &Level, fluid: Fluid, pos: BlockPos) -> bool {
        if let Some(hole) = self.holes.get(&(pos.x, pos.z)) {
            return *hole;
        }
        let state = self.state(level, pos);
        let hole = is_hole(fluid, state, level.state(pos.below()));
        self.holes.insert((pos.x, pos.z), hole);
        hole
    }
}

/// `getSpread`: the sides to flow to, those closest to a way down, in direction order
/// (north, south, west, east).
fn get_spread(level: &Level, fluid: Fluid, pos: BlockPos, state: u32) -> Vec<(Dir, FluidState)> {
    let mut lowest = 1000;
    let mut result: Vec<(Dir, FluidState)> = Vec::new();
    let mut ctx: Option<SpreadContext> = None;
    for dir in Dir::HORIZONTAL {
        let tpos = pos.rel(dir);
        let tstate = level.state(tpos);
        let tfluid = fluid_state(tstate);
        if !can_maybe_pass_through(fluid, state, dir, tstate, tfluid) {
            continue;
        }
        let new = get_new_liquid(level, fluid, tpos, tstate);
        if !can_hold_specific_fluid(tstate, new) {
            continue;
        }
        let ctx = ctx.get_or_insert_with(|| SpreadContext { states: HashMap::new(), holes: HashMap::new() });
        let distance = if ctx.is_hole(level, fluid, tpos) { 0 } else { slope_distance(level, fluid, tpos, 1, dir.opposite(), tstate, ctx) };
        if distance < lowest {
            result.clear();
        }
        if distance <= lowest {
            if can_be_replaced_with(tfluid, level, tpos, new, dir) {
                result.push((dir, new));
            }
            lowest = distance;
        }
    }
    result.sort_by_key(|(dir, _)| dir.index());
    result
}

/// `getSlopeDistance`: how far a way down is, searching up to the slope find distance.
fn slope_distance(level: &Level, fluid: Fluid, pos: BlockPos, pass: i32, from: Dir, state: u32, ctx: &mut SpreadContext) -> i32 {
    let mut lowest = 1000;
    for dir in Dir::HORIZONTAL {
        if dir == from {
            continue;
        }
        let tpos = pos.rel(dir);
        let tstate = ctx.state(level, tpos);
        let tfluid = fluid_state(tstate);
        if can_pass_through(fluid, state, dir, tstate, tfluid) {
            if ctx.is_hole(level, fluid, tpos) {
                return pass;
            }
            if pass < slope_find_distance(fluid, level.settings()) {
                let distance = slope_distance(level, fluid, tpos, pass + 1, dir.opposite(), tstate, ctx);
                lowest = lowest.min(distance);
            }
        }
    }
    lowest
}

/// `spreadTo`: puts fluid `target` at `pos`, replacing what was there.
fn spread_to(level: &mut Level, fluid: Fluid, pos: BlockPos, state: u32, dir: Dir, target: FluidState) {
    if fluid == Fluid::Lava && dir == Dir::Down && fluid_state(level.state(pos)).fluid == Fluid::Water {
        // Lava falling onto water.
        if matches!(super::kind(state), Kind::Water | Kind::Lava) {
            level.set_block(pos, default_state!("stone"), flags::ALL);
        }
        fizz(level, pos);
        return;
    }
    if is_liquid_container(state) {
        place_liquid(level, pos, state, target);
        return;
    }
    if !super::state::is_air(state) {
        before_destroying_block(level, fluid, pos, state);
    }
    level.set_block(pos, target.legacy_block(), flags::ALL);
}

/// `LiquidBlockContainer.placeLiquid`: a waterloggable block takes in a water source.
fn place_liquid(level: &mut Level, pos: BlockPos, state: u32, fluid: FluidState) -> bool {
    if !has_prop(state, "waterlogged") || get_bool(state, "waterlogged") || !(fluid.fluid == Fluid::Water && fluid.source) {
        return false;
    }
    level.set_block(pos, set_bool(state, "waterlogged", true), flags::ALL);
    level.schedule_fluid_tick(pos, fluid.type_id(), tick_delay(Fluid::Water, level.settings()));
    true
}

/// `beforeDestroyingBlock`: water washes the block's items out; lava burns them.
fn before_destroying_block(level: &mut Level, fluid: Fluid, pos: BlockPos, state: u32) {
    match fluid {
        Fluid::Lava => fizz(level, pos),
        _ => level.drop_resources(state, pos),
    }
}

/// The lava extinguish sound and smoke.
fn fizz(level: &mut Level, pos: BlockPos) {
    level.level_event(1501, pos, 0);
}

// --- the liquid blocks --------------------------------------------------------------------------

/// `LiquidBlock.onPlace`.
pub fn on_place(level: &mut Level, state: u32, pos: BlockPos) {
    if should_spread_liquid(level, pos, state) {
        let fs = fluid_state(state);
        level.schedule_fluid_tick(pos, fs.type_id(), tick_delay(fs.fluid, level.settings()));
    }
}

/// `LiquidBlock.neighborChanged`.
pub fn neighbor_changed(level: &mut Level, state: u32, pos: BlockPos) {
    on_place(level, state, pos);
}

/// `LiquidBlock.updateShape`: next to a source (or as a source), check again.
pub fn update_shape(level: &mut Level, state: u32, pos: BlockPos, neighbor_state: u32) -> u32 {
    let fs = fluid_state(state);
    if fs.source || fluid_state(neighbor_state).source {
        level.schedule_fluid_tick(pos, fs.type_id(), tick_delay(fs.fluid, level.settings()));
    }
    state
}

/// `LiquidBlock.shouldSpreadLiquid`: lava touching water hardens (obsidian from a source,
/// cobblestone from flowing lava), and lava over soul soil next to blue ice becomes basalt.
fn should_spread_liquid(level: &mut Level, pos: BlockPos, state: u32) -> bool {
    if fluid_state(state).fluid != Fluid::Lava {
        return true;
    }
    let over_soul_soil = block_id(level.state(pos.below())) == block!("soul_soil");
    // LiquidBlock.POSSIBLE_FLOW_DIRECTIONS, looking the other way.
    for dir in [Dir::Down, Dir::South, Dir::North, Dir::East, Dir::West] {
        let npos = pos.rel(dir.opposite());
        let nstate = level.state(npos);
        if fluid_state(nstate).fluid == Fluid::Water {
            let block = if fluid_state(level.state(pos)).source { default_state!("obsidian") } else { default_state!("cobblestone") };
            level.set_block_and_update(pos, block);
            fizz(level, pos);
            return false;
        }
        if over_soul_soil && block_id(nstate) == block!("blue_ice") {
            level.set_block_and_update(pos, default_state!("basalt"));
            fizz(level, pos);
            return false;
        }
    }
    true
}
