//! Block updates: what happens around a block when it changes.
//!
//! This is vanilla's `Level.setBlock` machinery for a [`World`]:
//!
//! * [`Level::set_block`] changes a block with vanilla's update flags. The old block may
//!   react to its removal (`affectNeighborsAfterRemoval`), the new one to its placement
//!   (`onPlace`), the six neighbours are told (`neighborChanged`, in the order west, east,
//!   down, up, north, south) and re-check their shape against it (`updateShape`, which can
//!   also make a block that can no longer survive break).
//! * Neighbour and shape updates go through a queue like vanilla's
//!   `CollectingNeighborUpdater`: a block finishes reacting before the updates it caused run,
//!   and those run before the remaining updates of the change that caused them.
//! * Blocks schedule ticks (`world::ticks`), and note blocks and pistons queue block events,
//!   which [`tick`] runs once per game tick.
//! * Everything clients must learn about (block changes with their light, sounds, world
//!   events, block events) and block drops are recorded as [`Effect`]s in the world's
//!   [`BlockUpdateState`]; the server publishes them (`server::block_updates`).
//!
//! The behaviour of individual blocks lives in the submodules: [`fluid`] (water and lava),
//! [`redstone`], [`doors`] (doors, trapdoors, fence gates, beds) and [`support`] (blocks that
//! need support or connect to their neighbours).

pub mod doors;
pub mod fire;
pub mod fluid;
pub mod piston;
pub mod redstone;
pub mod state;
pub mod support;
#[cfg(test)]
mod tests;

use super::chunk::ChunkPos;
use super::ticks::{self, TickKind, TickPriority, MAX_TICKS_PER_TICK};
use super::{BlockChange, World};
use paper_data::physics;
use paper_nbt::Compound;
use paper_protocol::codec::BlockPos;
pub use state::{Axis, Dir, Kind, PosExt};
use state::{block, block_id, default_state, kind};
use std::cell::Cell;
use std::collections::{HashSet, VecDeque};

/// Vanilla's `Block.UPDATE_*` flags for [`Level::set_block`].
pub mod flags {
    /// Tell the six neighbours (`neighborChanged`).
    pub const NEIGHBORS: u32 = 1;
    /// Send the change to clients.
    pub const CLIENTS: u32 = 2;
    pub const INVISIBLE: u32 = 4;
    pub const IMMEDIATE: u32 = 8;
    /// Skip shape updates of the neighbours.
    pub const KNOWN_SHAPE: u32 = 16;
    /// A block broken as a consequence drops nothing.
    pub const SUPPRESS_DROPS: u32 = 32;
    pub const MOVE_BY_PISTON: u32 = 64;
    pub const SKIP_SHAPE_UPDATE_ON_WIRE: u32 = 128;
    pub const SKIP_BLOCK_ENTITY_SIDEEFFECTS: u32 = 256;
    pub const SKIP_ON_PLACE: u32 = 512;
    /// `UPDATE_ALL`: what a normal block change does.
    pub const ALL: u32 = NEIGHBORS | CLIENTS;
    pub const ALL_IMMEDIATE: u32 = ALL | IMMEDIATE;
}

/// How deep shape updates may recurse (`Block.UPDATE_LIMIT`).
pub const UPDATE_LIMIT: i32 = 512;
/// Neighbour updates one change may cause before the rest are skipped
/// (`maxChainedNeighborUpdates`).
pub const MAX_CHAINED_NEIGHBOR_UPDATES: usize = 1_000_000;

/// Sound categories (`SoundSource`) as sent in the sound packet.
pub mod sound_source {
    pub const RECORDS: i32 = 2;
    pub const BLOCKS: i32 = 4;
}

/// Something the server must do after block updates ran: tell clients, or drop items.
#[derive(Debug)]
pub enum Effect {
    /// A block changed (publish it with its light changes).
    Block(BlockChange),
    /// A `LevelEvent` (`1501` lava fizz, `1502` redstone torch burnout, `2001` block broken, ...).
    LevelEvent { event: i32, pos: BlockPos, data: i32 },
    /// A sound at the centre of a block; `except` is the player whose client plays it itself.
    Sound { pos: BlockPos, sound: String, source: i32, volume: f32, pitch: f32, seed: i64, except: Option<i32> },
    /// A block event that ran (`ClientboundBlockEventPacket`): note blocks, pistons.
    BlockEvent { pos: BlockPos, block: u32, action: u8, param: u8 },
    /// A block was destroyed by block physics (washed away, lost its support, ...) and
    /// drops its items. `block_entity` is its block entity data, if it had one.
    Drops { pos: BlockPos, state: u32, block_entity: Option<Compound> },
    /// Powered TNT was primed (the block is already gone): the server spawns the entity.
    PrimeTnt { pos: BlockPos, igniter: Option<i32> },
}

/// Game rules and dimension properties block updates depend on.
#[derive(Clone, Copy, Debug)]
pub struct UpdateSettings {
    /// The dimension's `minecraft:gameplay/fast_lava` attribute (the nether): lava ticks
    /// every 10 instead of 30 game ticks, spreads 7 instead of 3 blocks and searches 4
    /// instead of 2 blocks for a way down.
    pub fast_lava: bool,
    /// `water_source_conversion` game rule.
    pub water_source_conversion: bool,
    /// `lava_source_conversion` game rule.
    pub lava_source_conversion: bool,
    /// `tnt_explodes` game rule: powered TNT is primed.
    pub tnt_explodes: bool,
    /// `fire_spread_radius_around_player` is not 0: fire ages, burns and spreads.
    pub fire_spreads: bool,
}

impl UpdateSettings {
    /// Vanilla's defaults for a dimension type (`minecraft:overworld`, ...).
    pub fn for_dimension(dimension: &str) -> UpdateSettings {
        let fast_lava = paper_data::synced::dimension_type(dimension).is_some_and(|d| d.fast_lava);
        UpdateSettings { fast_lava, water_source_conversion: true, lava_source_conversion: false, tnt_explodes: true, fire_spreads: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct BlockEventData {
    pos: BlockPos,
    block: u32,
    action: u8,
    param: u8,
}

/// The block update state a world keeps between ticks.
pub struct BlockUpdateState {
    pub settings: UpdateSettings,
    /// Vanilla's sub-tick counter: the scheduling order of ticks.
    next_sub_order: i64,
    /// Ticks collected for the running game tick that have not run yet (`willTickThisTick`).
    running: [HashSet<(BlockPos, u32)>; 2],
    /// Recent redstone torch turn-offs, for burnout.
    torch_toggles: VecDeque<(BlockPos, i64)>,
    block_events: VecDeque<BlockEventData>,
    queued_events: HashSet<BlockEventData>,
    /// Waiting to be published by the server.
    pub effects: Vec<Effect>,
    random: u64,
    /// Positions of moving piston blocks, in the order they started moving.
    pub(crate) moving_pistons: Vec<BlockPos>,
    /// Scheduled ticks are running (vanilla's `isHandlingTick`).
    pub(crate) handling_tick: bool,
}

impl BlockUpdateState {
    pub fn new(dimension: &str, seed: i64) -> BlockUpdateState {
        BlockUpdateState {
            settings: UpdateSettings::for_dimension(dimension),
            next_sub_order: 0,
            running: [HashSet::new(), HashSet::new()],
            torch_toggles: VecDeque::new(),
            block_events: VecDeque::new(),
            queued_events: HashSet::new(),
            effects: Vec::new(),
            random: (seed as u64) ^ 0x5DEE_CE66_D1CE_4E5B,
            moving_pistons: Vec::new(),
            handling_tick: false,
        }
    }

    /// The next pseudo-random number (splitmix64).
    fn next_random(&mut self) -> u64 {
        self.random = self.random.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.random;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// One queued neighbour or shape update (vanilla's `NeighborUpdates` records).
#[derive(Clone, Copy, Debug)]
enum Update {
    /// `updateNeighborsAtExceptFromFacing`: the six neighbours of `source`, one at a time.
    Multi { source: BlockPos, block: u32, skip: Option<Dir>, index: u8 },
    /// `neighborChanged(pos, block)`: reads the state when it runs.
    Simple { pos: BlockPos, block: u32 },
    /// `neighborChanged(state, pos, block, movedByPiston)`: with a known state.
    Full { pos: BlockPos, state: u32, block: u32, moved: bool },
    /// `shapeUpdate`.
    Shape { dir: Dir, pos: BlockPos, neighbor_pos: BlockPos, neighbor_state: u32, flags: u32, limit: i32 },
}

/// What one step of an [`Update`] does.
enum Step {
    Neighbor { pos: BlockPos, state: Option<u32>, block: u32, moved: bool },
    Shape { dir: Dir, pos: BlockPos, neighbor_pos: BlockPos, neighbor_state: u32, flags: u32, limit: i32 },
}

impl Update {
    fn multi(source: BlockPos, block: u32, skip: Option<Dir>) -> Update {
        let index = if Some(Dir::UPDATE_ORDER[0]) == skip { 1 } else { 0 };
        Update::Multi { source, block, skip, index }
    }

    /// The next step and whether more steps remain.
    fn next_step(&mut self) -> (Step, bool) {
        match self {
            Update::Multi { source, block, skip, index } => {
                let pos = source.rel(Dir::UPDATE_ORDER[*index as usize]);
                *index += 1;
                if (*index as usize) < 6 && Some(Dir::UPDATE_ORDER[*index as usize]) == *skip {
                    *index += 1;
                }
                (Step::Neighbor { pos, state: None, block: *block, moved: false }, (*index as usize) < 6)
            }
            Update::Simple { pos, block } => (Step::Neighbor { pos: *pos, state: None, block: *block, moved: false }, false),
            Update::Full { pos, state, block, moved } => (Step::Neighbor { pos: *pos, state: Some(*state), block: *block, moved: *moved }, false),
            Update::Shape { dir, pos, neighbor_pos, neighbor_state, flags, limit } => {
                (Step::Shape { dir: *dir, pos: *pos, neighbor_pos: *neighbor_pos, neighbor_state: *neighbor_state, flags: *flags, limit: *limit }, false)
            }
        }
    }
}

/// The result of using a block (`InteractionResult`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interaction {
    /// Nothing happened: the item in hand may be used (placed) instead.
    Pass,
    /// The block reacted.
    Success,
    /// The click was used up without visible effect.
    Consume,
}

impl Interaction {
    pub fn consumes_action(self) -> bool {
        self != Interaction::Pass
    }
}

/// A player using (right-clicking) a block.
#[derive(Clone, Copy, Debug)]
pub struct UseContext {
    pub player: i32,
    /// Not in adventure or spectator mode (`Abilities.mayBuild`).
    pub may_build: bool,
    /// The horizontal direction the player faces.
    pub facing: Dir,
}

/// Block updates on one world. Create one per operation; it borrows the world.
pub struct Level<'w> {
    pub world: &'w mut World,
    /// The player whose action this is (sounds their client plays itself are not sent back).
    pub actor: Option<i32>,
    stack: Vec<Update>,
    added: Vec<Update>,
    count: usize,
    /// Vanilla's `RedStoneWireBlock.shouldSignal`: off while a wire measures the power it
    /// receives from blocks, so wires do not see each other through blocks.
    wire_signals: Cell<bool>,
}

impl<'w> Level<'w> {
    pub fn new(world: &'w mut World) -> Level<'w> {
        Level { world, actor: None, stack: Vec::new(), added: Vec::new(), count: 0, wire_signals: Cell::new(true) }
    }

    pub fn settings(&self) -> UpdateSettings {
        self.world.updates.settings
    }

    /// The dimension's tag of blocks fire burns on forever.
    pub fn world_infiniburn(&self) -> &'static str {
        paper_data::synced::dimension_type(&self.world.dimension).map_or("minecraft:infiniburn_overworld", |d| d.infiniburn)
    }

    pub fn random(&mut self) -> u64 {
        self.world.updates.next_random()
    }

    /// A pseudo-random float in `[0, 1)`.
    pub fn random_float(&mut self) -> f32 {
        (self.random() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn game_time(&self) -> i64 {
        self.world.age
    }

    // --- reading -------------------------------------------------------------------------

    /// The block state at a position; `void_air` outside the world or in unloaded chunks.
    pub fn state(&self, pos: BlockPos) -> u32 {
        if !self.world.in_bounds(pos.y) {
            return default_state!("void_air");
        }
        self.world.block_at(pos.x, pos.y, pos.z).unwrap_or(default_state!("void_air"))
    }

    pub fn is_loaded(&self, pos: BlockPos) -> bool {
        self.world.in_bounds(pos.y) && self.world.chunks.contains_key(&ChunkPos::of_block(pos.x, pos.z))
    }

    // --- writing -------------------------------------------------------------------------

    /// Vanilla's `Level.setBlock(pos, state, flags)`: changes a block and runs the updates
    /// the flags ask for. Returns whether the block changed.
    pub fn set_block(&mut self, pos: BlockPos, state: u32, flags: u32) -> bool {
        self.set_block_limited(pos, state, flags, UPDATE_LIMIT)
    }

    pub fn set_block_limited(&mut self, pos: BlockPos, state: u32, flags: u32, limit: i32) -> bool {
        let Some(change) = self.world.set_block(pos.x, pos.y, pos.z, state) else { return false };
        let old = change.old;
        self.world.updates.effects.push(Effect::Block(change));
        let moved = flags & flags::MOVE_BY_PISTON != 0;
        // LevelChunk.setBlockState
        if block_id(old) != block_id(state) && (flags & flags::NEIGHBORS != 0 || moved) {
            self.affect_neighbors_after_removal(old, pos, moved);
        }
        if block_id(self.state(pos)) != block_id(state) {
            // The old block's removal replaced the new one already.
            return false;
        }
        if flags & flags::SKIP_ON_PLACE == 0 {
            self.on_place(state, pos, old, moved);
        }
        // Level.setBlock
        if self.state(pos) == state {
            if flags & flags::NEIGHBORS != 0 {
                self.update_neighbors_at(pos, block_id(old));
                if physics::has_analog_output(state) {
                    self.update_neighbour_for_output_signal(pos, block_id(state));
                }
            }
            if flags & flags::KNOWN_SHAPE == 0 && limit > 0 {
                let shape_flags = flags & !(flags::NEIGHBORS | flags::SUPPRESS_DROPS);
                self.update_indirect_neighbour_shapes(old, pos, shape_flags, limit - 1);
                self.update_neighbour_shapes(state, pos, shape_flags, limit - 1);
                self.update_indirect_neighbour_shapes(state, pos, shape_flags, limit - 1);
            }
        }
        true
    }

    /// `setBlockAndUpdate`.
    pub fn set_block_and_update(&mut self, pos: BlockPos, state: u32) -> bool {
        self.set_block(pos, state, flags::ALL)
    }

    /// `removeBlock`: replaces the block with the fluid it contained (water for a
    /// waterlogged block), or air.
    pub fn remove_block(&mut self, pos: BlockPos, moved: bool) -> bool {
        let fluid = fluid::fluid_state(self.state(pos)).legacy_block();
        self.set_block(pos, fluid, flags::ALL | if moved { flags::MOVE_BY_PISTON } else { 0 })
    }

    /// `destroyBlock`: breaks a block with the break effect, dropping its items if `drop`.
    pub fn destroy_block(&mut self, pos: BlockPos, drop: bool) -> bool {
        self.destroy_block_limited(pos, drop, UPDATE_LIMIT)
    }

    pub fn destroy_block_limited(&mut self, pos: BlockPos, drop: bool, limit: i32) -> bool {
        let state = self.state(pos);
        if state::is_air(state) || !self.is_loaded(pos) {
            return false;
        }
        if !in_tag_fire(state) {
            self.level_event(2001, pos, state as i32);
        }
        if drop {
            self.drop_resources(state, pos);
        }
        let fluid = fluid::fluid_state(state).legacy_block();
        self.set_block_limited(pos, fluid, flags::ALL, limit)
    }

    /// `Block.dropResources`: the block's items fall out (the server turns this into item
    /// drops once items exist in the world).
    pub fn drop_resources(&mut self, state: u32, pos: BlockPos) {
        let block_entity = self.world.chunks.get(&ChunkPos::of_block(pos.x, pos.z)).and_then(|c| c.block_entities.get(&(pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8)).cloned());
        self.world.updates.effects.push(Effect::Drops { pos, state, block_entity });
    }

    /// `Block.updateOrDestroy`: applies the result of a shape update.
    fn update_or_destroy(&mut self, old: u32, new: u32, pos: BlockPos, flags: u32, limit: i32) {
        if new != old {
            if state::is_air(new) {
                self.destroy_block_limited(pos, flags & flags::SUPPRESS_DROPS == 0, limit);
            } else {
                self.set_block_limited(pos, new, flags & !flags::SUPPRESS_DROPS, limit);
            }
        }
    }

    // --- neighbour and shape updates -----------------------------------------------------

    /// `updateNeighborsAt`: the six neighbours of `pos` get `neighborChanged`.
    pub fn update_neighbors_at(&mut self, pos: BlockPos, source_block: u32) {
        self.add_and_run(Update::multi(pos, source_block, None));
    }

    /// `updateNeighborsAtExceptFromFacing`.
    pub fn update_neighbors_at_except(&mut self, pos: BlockPos, source_block: u32, skip: Dir) {
        self.add_and_run(Update::multi(pos, source_block, Some(skip)));
    }

    /// `neighborChanged(pos, block)`: one block re-checks its surroundings.
    pub fn neighbor_changed_at(&mut self, pos: BlockPos, source_block: u32) {
        self.add_and_run(Update::Simple { pos, block: source_block });
    }

    /// `neighborChanged(state, pos, block, movedByPiston)`.
    pub fn neighbor_changed_with(&mut self, state: u32, pos: BlockPos, source_block: u32, moved: bool) {
        self.add_and_run(Update::Full { pos, state, block: source_block, moved });
    }

    /// `neighborShapeChanged`: the block at `pos` re-checks its shape against its neighbour
    /// at `neighbor_pos`, in direction `dir`.
    pub fn neighbor_shape_changed(&mut self, dir: Dir, pos: BlockPos, neighbor_pos: BlockPos, neighbor_state: u32, flags: u32, limit: i32) {
        self.add_and_run(Update::Shape { dir, pos, neighbor_pos, neighbor_state, flags, limit });
    }

    /// `updateNeighbourShapes`: every neighbour re-checks its shape against `state` at `pos`.
    pub fn update_neighbour_shapes(&mut self, state: u32, pos: BlockPos, flags: u32, limit: i32) {
        for dir in Dir::SHAPE_UPDATE_ORDER {
            self.neighbor_shape_changed(dir.opposite(), pos.rel(dir), pos, state, flags, limit);
        }
    }

    /// `updateNeighbourForOutputSignal`: comparators next to (or behind a block next to) a
    /// block with a comparator output re-read it.
    pub fn update_neighbour_for_output_signal(&mut self, pos: BlockPos, source_block: u32) {
        for dir in Dir::HORIZONTAL {
            let mut npos = pos.rel(dir);
            if !self.is_loaded(npos) {
                continue;
            }
            let mut nstate = self.state(npos);
            if block_id(nstate) == block!("comparator") {
                self.neighbor_changed_with(nstate, npos, source_block, false);
            } else if physics::is_redstone_conductor(nstate) {
                npos = npos.rel(dir);
                nstate = self.state(npos);
                if block_id(nstate) == block!("comparator") {
                    self.neighbor_changed_with(nstate, npos, source_block, false);
                }
            }
        }
    }

    fn add_and_run(&mut self, update: Update) {
        let running = self.count > 0;
        let over_limit = self.count >= MAX_CHAINED_NEIGHBOR_UPDATES;
        self.count += 1;
        if !over_limit {
            if running {
                self.added.push(update);
            } else {
                self.stack.push(update);
            }
        } else if self.count - 1 == MAX_CHAINED_NEIGHBOR_UPDATES {
            log::error!("Too many chained neighbor updates. Skipping the rest.");
        }
        if !running {
            self.run_updates();
        }
    }

    /// Vanilla's `CollectingNeighborUpdater.runUpdates`: updates added while one runs go on
    /// top of the stack in the order they were added, and run before it continues.
    fn run_updates(&mut self) {
        while !self.stack.is_empty() || !self.added.is_empty() {
            // Last added first, so the first added ends up on top.
            while let Some(update) = self.added.pop() {
                self.stack.push(update);
            }
            while let Some(top) = self.stack.last_mut() {
                let (step, more) = top.next_step();
                if !more {
                    self.stack.pop();
                }
                self.run_step(step);
                if !more || !self.added.is_empty() {
                    break;
                }
            }
        }
        self.count = 0;
    }

    fn run_step(&mut self, step: Step) {
        match step {
            Step::Neighbor { pos, state, block, moved } => {
                let state = state.unwrap_or_else(|| self.state(pos));
                self.neighbor_changed(state, pos, block, moved);
            }
            Step::Shape { dir, pos, neighbor_pos, neighbor_state, flags, limit } => {
                let state = self.state(pos);
                if flags & flags::SKIP_SHAPE_UPDATE_ON_WIRE == 0 || block_id(state) != block!("redstone_wire") {
                    let new = self.update_shape(state, pos, dir, neighbor_pos, neighbor_state);
                    self.update_or_destroy(state, new, pos, flags, limit);
                }
            }
        }
    }

    // --- ticks ---------------------------------------------------------------------------

    fn schedule(&mut self, kind: TickKind, pos: BlockPos, ty: u32, delay: i32, priority: TickPriority) -> bool {
        let trigger = self.world.age + delay as i64;
        let Some(chunk) = self.world.chunks.get_mut(&ChunkPos::of_block(pos.x, pos.z)) else { return false };
        let sub_order = self.world.updates.next_sub_order;
        let added = chunk.ticks.queue_mut(kind).schedule(ticks::ScheduledTick { ty, pos, trigger, priority, sub_order });
        if added {
            self.world.updates.next_sub_order += 1;
            // The tick must be saved even if no block of the chunk changes.
            chunk.dirty = true;
        }
        added
    }

    /// `scheduleTick(pos, block, delay)`: the block at `pos` gets a tick after `delay` game
    /// ticks if it is still of that block then.
    pub fn schedule_tick(&mut self, pos: BlockPos, block: u32, delay: i32) -> bool {
        self.schedule(TickKind::Block, pos, block, delay, TickPriority::Normal)
    }

    pub fn schedule_tick_with(&mut self, pos: BlockPos, block: u32, delay: i32, priority: TickPriority) -> bool {
        self.schedule(TickKind::Block, pos, block, delay, priority)
    }

    /// Schedules a fluid tick; `fluid` is a `minecraft:fluid` id.
    pub fn schedule_fluid_tick(&mut self, pos: BlockPos, fluid: u32, delay: i32) -> bool {
        self.schedule(TickKind::Fluid, pos, fluid, delay, TickPriority::Normal)
    }

    pub fn has_scheduled_tick(&self, pos: BlockPos, block: u32) -> bool {
        self.world.chunks.get(&ChunkPos::of_block(pos.x, pos.z)).is_some_and(|c| c.ticks.blocks.has(pos, block))
    }

    /// `willTickThisTick`: a tick for the block at `pos` was collected for the running game
    /// tick and has not run yet.
    pub fn will_tick_this_tick(&self, pos: BlockPos, block: u32) -> bool {
        self.world.updates.running[0].contains(&(pos, block))
    }

    fn tick_block(&mut self, pos: BlockPos, block: u32) {
        let state = self.state(pos);
        if block_id(state) == block {
            self.tick(state, pos);
        }
    }

    fn tick_fluid(&mut self, pos: BlockPos, fluid_type: u32) {
        let state = self.state(pos);
        let fs = fluid::fluid_state(state);
        if !fs.is_empty() && fs.type_id() == fluid_type {
            fluid::tick(self, pos, state, fs);
        }
    }

    // --- block events and effects --------------------------------------------------------

    /// `blockEvent`: queues an event that runs at the end of the tick (note block sounds,
    /// piston moves) and is sent to clients if the block still accepts it.
    pub fn block_event(&mut self, pos: BlockPos, block: u32, action: u8, param: u8) {
        let event = BlockEventData { pos, block, action, param };
        let updates = &mut self.world.updates;
        if updates.queued_events.insert(event) {
            updates.block_events.push_back(event);
        }
    }

    fn run_block_events(&mut self, ticking: &dyn Fn(&World, ChunkPos) -> bool) {
        let mut later = Vec::new();
        while let Some(event) = self.world.updates.block_events.pop_front() {
            self.world.updates.queued_events.remove(&event);
            if !ticking(self.world, ChunkPos::of_block(event.pos.x, event.pos.z)) {
                later.push(event);
                continue;
            }
            let state = self.state(event.pos);
            if block_id(state) != event.block {
                continue;
            }
            // Clients get the event before the block changes it causes (vanilla sends block
            // changes at the end of the tick), so they can animate piston moves.
            let index = self.world.updates.effects.len();
            self.world.updates.effects.push(Effect::BlockEvent { pos: event.pos, block: event.block, action: event.action, param: event.param });
            if !self.trigger_event(state, event.pos, event.action, event.param) {
                self.world.updates.effects.remove(index);
            }
        }
        for event in later {
            self.block_event(event.pos, event.block, event.action, event.param);
        }
    }

    /// `levelEvent`: an effect (sound and/or particles) every nearby player sees.
    pub fn level_event(&mut self, event: i32, pos: BlockPos, data: i32) {
        self.world.updates.effects.push(Effect::LevelEvent { event, pos, data });
    }

    /// `playSound(player, pos, sound, source, volume, pitch)`: `except` hears it from its own
    /// client.
    pub fn play_sound(&mut self, except: Option<i32>, pos: BlockPos, sound: &str, source: i32, volume: f32, pitch: f32) {
        let seed = self.random() as i64;
        self.world.updates.effects.push(Effect::Sound { pos, sound: sound.to_owned(), source, volume, pitch, seed, except });
    }

    // --- signals (vanilla's SignalGetter) -------------------------------------------------

    /// `getSignal(pos, dir)`: the power the block at `pos` gives in direction `dir` (from the
    /// receiver towards it); a powered conductor gives the strong power it receives.
    pub fn get_signal(&self, pos: BlockPos, dir: Dir) -> u8 {
        let state = self.state(pos);
        let signal = self.signal(state, pos, dir);
        if physics::is_redstone_conductor(state) {
            signal.max(self.get_direct_signal_to(pos))
        } else {
            signal
        }
    }

    pub fn has_signal(&self, pos: BlockPos, dir: Dir) -> bool {
        self.get_signal(pos, dir) > 0
    }

    /// `getDirectSignal`: strong power.
    pub fn get_direct_signal(&self, pos: BlockPos, dir: Dir) -> u8 {
        let state = self.state(pos);
        self.direct_signal(state, pos, dir)
    }

    /// `getDirectSignalTo`: the strongest strong power a block receives.
    pub fn get_direct_signal_to(&self, pos: BlockPos) -> u8 {
        let mut result = 0;
        for dir in Dir::ALL {
            result = result.max(self.get_direct_signal(pos.rel(dir), dir));
            if result >= 15 {
                break;
            }
        }
        result
    }

    /// `hasNeighborSignal`: whether any neighbour powers the block.
    pub fn has_neighbor_signal(&self, pos: BlockPos) -> bool {
        Dir::ALL.iter().any(|dir| self.get_signal(pos.rel(*dir), *dir) > 0)
    }

    /// `getBestNeighborSignal`.
    pub fn get_best_neighbor_signal(&self, pos: BlockPos) -> u8 {
        let mut best = 0;
        for dir in Dir::ALL {
            let signal = self.get_signal(pos.rel(dir), dir);
            if signal >= 15 {
                return 15;
            }
            best = best.max(signal);
        }
        best
    }

    /// `getControlInputSignal`: what a diode's side input reads.
    pub fn get_control_input_signal(&self, pos: BlockPos, dir: Dir, only_diodes: bool) -> u8 {
        let state = self.state(pos);
        if only_diodes {
            return if redstone::is_diode(state) { self.get_direct_signal(pos, dir) } else { 0 };
        }
        match kind(state) {
            Kind::RedstoneBlock => 15,
            Kind::RedstoneWire => state::get_int(state, "power") as u8,
            _ if self.is_signal_source(state) => self.get_direct_signal(pos, dir),
            _ => 0,
        }
    }

    /// Measures with wires invisible (vanilla's `shouldSignal = false`).
    pub(crate) fn without_wire_signals<T>(&self, f: impl FnOnce() -> T) -> T {
        let before = self.wire_signals.replace(false);
        let result = f();
        self.wire_signals.set(before);
        result
    }

    pub(crate) fn wire_signals(&self) -> bool {
        self.wire_signals.get()
    }

    // --- block behaviour dispatch --------------------------------------------------------

    /// `onPlace`.
    fn on_place(&mut self, state: u32, pos: BlockPos, old: u32, _moved: bool) {
        match kind(state) {
            Kind::Water | Kind::Lava => fluid::on_place(self, state, pos),
            Kind::RedstoneWire => redstone::wire_on_place(self, state, pos, old),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => redstone::torch_notify(self, state, pos),
            Kind::Repeater => redstone::diode_on_place(self, state, pos),
            Kind::Comparator => {
                redstone::comparator_on_place(self, state, pos);
                redstone::diode_on_place(self, state, pos);
            }
            Kind::Observer => redstone::observer_on_place(self, state, pos, old),
            Kind::Piston { .. } => piston::on_place(self, state, pos, old),
            Kind::Tnt if kind(old) != Kind::Tnt => self.prime_if_powered(pos),
            Kind::Fire => fire::on_place(self, state, pos, old),
            _ => {}
        }
    }

    /// `TntBlock.onPlace` / `neighborChanged`: powered TNT is primed and removed.
    fn prime_if_powered(&mut self, pos: BlockPos) {
        if self.settings().tnt_explodes && self.has_neighbor_signal(pos) {
            let igniter = self.actor;
            self.world.updates.effects.push(Effect::PrimeTnt { pos, igniter });
            self.remove_block(pos, false);
        }
    }

    /// `affectNeighborsAfterRemoval`: the old block reacts to having been replaced.
    fn affect_neighbors_after_removal(&mut self, old: u32, pos: BlockPos, moved: bool) {
        match kind(old) {
            Kind::RedstoneWire if !moved => redstone::wire_after_removal(self, old, pos),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch if !moved => redstone::torch_notify(self, old, pos),
            Kind::Lever | Kind::Button { .. } if !moved && state::get_bool(old, "powered") => redstone::attached_update_neighbours(self, old, pos),
            Kind::Repeater | Kind::Comparator if !moved => redstone::diode_update_in_front(self, old, pos),
            Kind::Observer => redstone::observer_after_removal(self, old, pos),
            Kind::PistonHead => piston::head_after_removal(self, old, pos),
            _ => {}
        }
    }

    /// `neighborChanged`: a neighbour changed; `source_block` is its block id.
    fn neighbor_changed(&mut self, state: u32, pos: BlockPos, source_block: u32, _moved: bool) {
        match kind(state) {
            Kind::Water | Kind::Lava => fluid::neighbor_changed(self, state, pos),
            Kind::RedstoneWire => redstone::wire_neighbor_changed(self, state, pos),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => redstone::torch_neighbor_changed(self, state, pos),
            Kind::Repeater | Kind::Comparator => redstone::diode_neighbor_changed(self, state, pos),
            Kind::RedstoneLamp => redstone::lamp_neighbor_changed(self, state, pos),
            Kind::NoteBlock => redstone::note_neighbor_changed(self, state, pos),
            Kind::Door { .. } => doors::door_neighbor_changed(self, state, pos, source_block),
            Kind::Trapdoor { .. } => doors::trapdoor_neighbor_changed(self, state, pos),
            Kind::FenceGate => doors::fence_gate_neighbor_changed(self, state, pos),
            Kind::Piston { .. } => piston::check_if_extend(self, state, pos),
            Kind::PistonHead => piston::head_neighbor_changed(self, state, pos, source_block),
            Kind::Tnt => self.prime_if_powered(pos),
            _ => {}
        }
    }

    /// `updateShape`: the block's state given that its neighbour in `dir` is now
    /// `neighbor_state`. Air means the block breaks.
    fn update_shape(&mut self, state: u32, pos: BlockPos, dir: Dir, neighbor_pos: BlockPos, neighbor_state: u32) -> u32 {
        // Blocks holding water (waterlogged blocks, kelp, seagrass, bubble columns) let it
        // flow again when a neighbour changes.
        let k = kind(state);
        if !matches!(k, Kind::Water | Kind::Lava) && fluid::fluid_state(state).fluid == fluid::Fluid::Water {
            let water = fluid::Fluid::Water;
            self.schedule_fluid_tick(pos, fluid::FluidState::source(water).type_id(), fluid::tick_delay(water, self.settings()));
        }
        match k {
            Kind::Water | Kind::Lava => fluid::update_shape(self, state, pos, neighbor_state),
            Kind::RedstoneWire => redstone::wire_update_shape(self, state, pos, dir, neighbor_state),
            Kind::Repeater => redstone::repeater_update_shape(self, state, pos, dir, neighbor_state),
            Kind::Comparator => redstone::comparator_update_shape(self, state, dir, neighbor_state),
            Kind::Observer => redstone::observer_update_shape(self, state, pos, dir),
            Kind::NoteBlock => redstone::note_update_shape(self, state, pos, dir),
            Kind::Door { .. } => doors::door_update_shape(self, state, pos, dir, neighbor_state),
            Kind::FenceGate => doors::fence_gate_update_shape(self, state, pos, dir, neighbor_state),
            Kind::Bed => doors::bed_update_shape(state, dir, neighbor_state),
            Kind::PistonHead => piston::head_update_shape(self, state, pos, dir),
            Kind::Fire => fire::update_shape(self, state, pos),
            Kind::NetherPortal => fire::portal_update_shape(self, state, pos, dir, neighbor_state),
            _ => support::update_shape(self, state, pos, dir, neighbor_pos, neighbor_state),
        }
    }

    /// `updateIndirectNeighbourShapes`: only redstone wire looks diagonally.
    fn update_indirect_neighbour_shapes(&mut self, state: u32, pos: BlockPos, flags: u32, limit: i32) {
        if kind(state) == Kind::RedstoneWire {
            redstone::wire_update_indirect_shapes(self, state, pos, flags, limit);
        }
    }

    /// A scheduled block tick (`tick`).
    fn tick(&mut self, state: u32, pos: BlockPos) {
        match kind(state) {
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => redstone::torch_tick(self, state, pos),
            Kind::Repeater => redstone::repeater_tick(self, state, pos),
            Kind::Comparator => redstone::comparator_tick(self, state, pos),
            Kind::RedstoneLamp => redstone::lamp_tick(self, state, pos),
            Kind::Observer => redstone::observer_tick(self, state, pos),
            Kind::Button { .. } => redstone::button_tick(self, state, pos),
            Kind::Fire => fire::tick(self, state, pos),
            _ => {}
        }
    }

    /// `triggerEvent`: runs a block event; `true` sends it to clients.
    fn trigger_event(&mut self, state: u32, pos: BlockPos, action: u8, param: u8) -> bool {
        match kind(state) {
            Kind::NoteBlock => redstone::note_trigger(self, state, pos),
            Kind::Piston { .. } => piston::trigger_event(self, state, pos, action, param),
            _ => false,
        }
    }

    /// `getSignal` of a state.
    pub fn signal(&self, state: u32, pos: BlockPos, dir: Dir) -> u8 {
        redstone::signal(self, state, pos, dir)
    }

    /// `getDirectSignal` of a state.
    pub fn direct_signal(&self, state: u32, pos: BlockPos, dir: Dir) -> u8 {
        redstone::direct_signal(self, state, pos, dir)
    }

    /// `isSignalSource`.
    pub fn is_signal_source(&self, state: u32) -> bool {
        redstone::is_signal_source(self, state)
    }

    /// `canSurvive`: whether the block can stay where it is.
    pub fn can_survive(&self, state: u32, pos: BlockPos) -> bool {
        support::can_survive(self, state, pos)
    }

    /// `useWithoutItem`: a player right-clicks the block.
    pub fn use_without_item(&mut self, state: u32, pos: BlockPos, ctx: UseContext) -> Interaction {
        self.actor = Some(ctx.player);
        let result = match kind(state) {
            Kind::Lever => redstone::lever_use(self, state, pos),
            Kind::Button { .. } => redstone::button_use(self, state, pos, ctx),
            Kind::Repeater => redstone::repeater_use(self, state, pos, ctx),
            Kind::Comparator => redstone::comparator_use(self, state, pos, ctx),
            Kind::RedstoneWire => redstone::wire_use(self, state, pos, ctx),
            Kind::NoteBlock => redstone::note_use(self, state, pos, ctx),
            Kind::Door { by_hand } => doors::door_use(self, state, pos, ctx, by_hand),
            Kind::Trapdoor { by_hand } => doors::trapdoor_use(self, state, pos, ctx, by_hand),
            Kind::FenceGate => doors::fence_gate_use(self, state, pos, ctx),
            _ => Interaction::Pass,
        };
        self.actor = None;
        result
    }

    /// `playerWillDestroy`: a player is about to break the block at `pos`.
    pub fn player_will_destroy(&mut self, state: u32, pos: BlockPos, creative: bool) {
        if creative {
            support::prevent_drop_from_bottom_part(self, state, pos);
            if kind(state) == Kind::PistonHead {
                piston::head_player_will_destroy(self, state, pos);
            }
        }
    }
}

fn in_tag_fire(state: u32) -> bool {
    state::in_tag!(state, "fire")
}

/// Runs a world's block ticks, fluid ticks and block events for the current game tick
/// (call after the world's time advanced). `ticking` says which chunks may run theirs.
pub fn tick(world: &mut World, ticking: &dyn Fn(&World, ChunkPos) -> bool) {
    let now = world.age;
    for (index, kind) in [(0, TickKind::Block), (1, TickKind::Fluid)] {
        // Only chunks with due ticks are asked whether they tick.
        let candidates: HashSet<ChunkPos> =
            world.chunks.iter().filter(|(_, c)| c.ticks.queue(kind).first().is_some_and(|t| t.trigger <= now)).map(|(p, _)| *p).filter(|p| ticking(world, *p)).collect();
        if candidates.is_empty() {
            continue;
        }
        let due = ticks::collect_due(&mut world.chunks, kind, now, MAX_TICKS_PER_TICK, |p| candidates.contains(&p));
        if due.is_empty() {
            continue;
        }
        world.updates.running[index] = due.iter().map(|t| (t.pos, t.ty)).collect();
        world.updates.handling_tick = true;
        let mut level = Level::new(world);
        for t in &due {
            level.world.updates.running[index].remove(&(t.pos, t.ty));
            match kind {
                TickKind::Block => level.tick_block(t.pos, t.ty),
                TickKind::Fluid => level.tick_fluid(t.pos, t.ty),
            }
        }
        world.updates.running[index].clear();
        world.updates.handling_tick = false;
    }
    if !world.updates.block_events.is_empty() {
        Level::new(world).run_block_events(ticking);
    }
    // Block entities tick last: moving pistons.
    if !world.updates.moving_pistons.is_empty() {
        piston::tick_moving(&mut Level::new(world));
    }
    // Forget torch turn-offs older than the burnout window.
    while world.updates.torch_toggles.front().is_some_and(|(_, when)| now - *when > redstone::TORCH_TOGGLE_WINDOW) {
        world.updates.torch_toggles.pop_front();
    }
}

/// Java's `HashSet<BlockPos>` iteration order for a handful of positions (fewer than 13, so
/// the table keeps its 16 buckets): by bucket of `BlockPos.hashCode()`, then insertion
/// order. Redstone wire notifies its surroundings in this order, which is visible in
/// contraptions.
pub fn java_hash_order(positions: &[BlockPos]) -> Vec<BlockPos> {
    let bucket = |p: &BlockPos| {
        let h = p.y.wrapping_add(p.z.wrapping_mul(31)).wrapping_mul(31).wrapping_add(p.x);
        ((h ^ ((h as u32) >> 16) as i32) & 15) as usize
    };
    let mut out: Vec<BlockPos> = Vec::with_capacity(positions.len());
    for b in 0..16 {
        for p in positions {
            if bucket(p) == b && !out.contains(p) {
                out.push(*p);
            }
        }
    }
    out
}

/// A chunk entered the world (loaded or generated): its saved ticks count from now and its
/// moving pistons continue.
pub fn chunk_loaded(world: &mut World, pos: ChunkPos) {
    let age = world.age;
    if let Some(chunk) = world.chunks.get_mut(&pos) {
        chunk.ticks.unpack(age);
        if !chunk.block_entities.is_empty() {
            piston::chunk_loaded(world, pos);
        }
    }
}

impl World {
    /// Block updates for this world.
    pub fn level(&mut self) -> Level<'_> {
        Level::new(self)
    }
}
