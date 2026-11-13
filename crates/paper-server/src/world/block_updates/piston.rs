//! Pistons and sticky pistons: vanilla's `PistonBaseBlock`, `PistonStructureResolver`,
//! `PistonHeadBlock`, `MovingPistonBlock` and `PistonMovingBlockEntity`.
//!
//! Derived in part from Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0:
//! `crates/pumpkin-core/src/block/blocks/piston/` (piston.rs, piston_head.rs,
//! piston_extension.rs, mod.rs), which served as a reference next to vanilla's behaviour.
//!
//! A piston reacts to power with a block event (0 extend, 1 retract, 2 retract without
//! pulling). When the event runs it resolves which blocks move (at most 12; slime and honey
//! blocks drag their neighbours along) and turns them into `moving_piston` blocks whose block
//! entity (kept in the chunk in vanilla's format, so it is saved too) finishes the move two
//! game ticks later. Clients get the block event first and animate the move themselves.
//! Entities are not pushed.

use super::state::{block, block_id, default_state, get_bool, get_dir, kind, prop, set_bool, set_dir, with};
use super::{flags, sound_source, Dir, Kind, Level, PosExt, UPDATE_LIMIT};
use crate::world::chunk::ChunkPos;
use crate::world::World;
use paper_data::physics::{self, PushReaction};
use paper_data::blocks;
use paper_nbt::{Compound, Tag};
use paper_protocol::codec::BlockPos;

/// Blocks a piston moves at most.
pub const MAX_PUSH_DEPTH: usize = 12;

fn facing(state: u32) -> Dir {
    get_dir(state, "facing").unwrap_or(Dir::North)
}

fn dir_from_index(index: u8) -> Dir {
    Dir::ALL[(index & 7).min(5) as usize]
}

fn is_piston_base(state: u32) -> bool {
    matches!(kind(state), Kind::Piston { .. })
}

fn sticky(state: u32) -> bool {
    matches!(kind(state), Kind::Piston { sticky: true })
}

// --- moving blocks (the moving piston block entity) -----------------------------------------

/// The state of a `moving_piston` block: what moves, which way, how far it got.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovingBlock {
    pub moved_state: u32,
    pub direction: Dir,
    pub extending: bool,
    /// The piston's own head (or, retracting, its base) rather than a pushed block.
    pub source: bool,
    pub progress: f32,
    pub progress_o: f32,
    pub last_ticked: i64,
}

fn key(pos: BlockPos) -> (i32, u8, u8) {
    (pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8)
}

fn state_nbt(state: u32) -> Compound {
    let mut c = Compound::new().with("Name", Tag::String(blocks::block_of_state(state).name.to_owned()));
    let props = blocks::properties_of(state);
    if !props.is_empty() {
        let mut p = Compound::new();
        for (k, v) in props {
            p.insert(k, Tag::String(v.to_owned()));
        }
        c.insert("Properties", Tag::Compound(p));
    }
    c
}

fn state_from_nbt(c: &Compound) -> Option<u32> {
    let mut state = blocks::default_state(c.string("Name")?)?;
    for (k, v) in c.compound("Properties").map(|p| p.iter().collect::<Vec<_>>()).unwrap_or_default() {
        if let Some(v) = v.as_str() {
            state = with(state, k, v);
        }
    }
    Some(state)
}

/// The moving block at `pos`, read from its block entity.
pub fn moving_block(world: &World, pos: BlockPos) -> Option<MovingBlock> {
    let entity = world.chunks.get(&ChunkPos::of_block(pos.x, pos.z))?.block_entities.get(&key(pos))?;
    if entity.string("id") != Some("minecraft:piston") {
        return None;
    }
    let progress_o = entity.f64("progress").unwrap_or(0.0) as f32;
    Some(MovingBlock {
        moved_state: entity.compound("blockState").and_then(state_from_nbt).unwrap_or(blocks::AIR),
        direction: dir_from_index(entity.i32("facing").unwrap_or(0) as u8),
        extending: entity.bool("extending").unwrap_or(false),
        source: entity.bool("source").unwrap_or(false),
        progress: entity.f64("paper-rs:progress").map_or(progress_o, |p| p as f32),
        progress_o,
        last_ticked: entity.i64("paper-rs:last_ticked").unwrap_or(i64::MIN),
    })
}

fn store_moving_block(world: &mut World, pos: BlockPos, moving: &MovingBlock) {
    let Some(chunk) = world.chunks.get_mut(&ChunkPos::of_block(pos.x, pos.z)) else { return };
    let entity = Compound::new()
        .with("id", Tag::String("minecraft:piston".into()))
        .with("blockState", Tag::Compound(state_nbt(moving.moved_state)))
        .with("facing", Tag::Int(moving.direction.index() as i32))
        .with("progress", Tag::Float(moving.progress_o))
        .with("extending", Tag::Byte(moving.extending as i8))
        .with("source", Tag::Byte(moving.source as i8))
        .with("paper-rs:progress", Tag::Float(moving.progress))
        .with("paper-rs:last_ticked", Tag::Long(moving.last_ticked));
    chunk.block_entities.insert(key(pos), entity);
    chunk.dirty = true;
}

fn remove_moving_block(world: &mut World, pos: BlockPos) {
    if let Some(chunk) = world.chunks.get_mut(&ChunkPos::of_block(pos.x, pos.z)) {
        if chunk.block_entities.get(&key(pos)).is_some_and(|e| e.string("id") == Some("minecraft:piston")) {
            chunk.block_entities.remove(&key(pos));
        }
    }
}

/// `MovingPistonBlock.newMovingBlockEntity` + `setBlockEntity`.
fn start_moving(level: &mut Level, pos: BlockPos, moved_state: u32, direction: Dir, extending: bool, source: bool) {
    let moving = MovingBlock { moved_state, direction, extending, source, progress: 0.0, progress_o: 0.0, last_ticked: i64::MIN };
    store_moving_block(level.world, pos, &moving);
    if !level.world.updates.moving_pistons.contains(&pos) {
        level.world.updates.moving_pistons.push(pos);
    }
}

/// Registers the moving pistons of a chunk that was just loaded.
pub fn chunk_loaded(world: &mut World, pos: ChunkPos) {
    let Some(chunk) = world.chunks.get(&pos) else { return };
    let moving: Vec<BlockPos> = chunk
        .block_entities
        .iter()
        .filter(|(_, e)| e.string("id") == Some("minecraft:piston"))
        .map(|((y, z, x), _)| BlockPos::new(pos.x * 16 + *x as i32, *y, pos.z * 16 + *z as i32))
        .collect();
    for p in moving {
        if !world.updates.moving_pistons.contains(&p) {
            world.updates.moving_pistons.push(p);
        }
    }
}

/// `Block.updateFromNeighbourShapes`: a state adjusted to all its neighbours.
fn update_from_neighbour_shapes(level: &mut Level, mut state: u32, pos: BlockPos) -> u32 {
    for dir in Dir::SHAPE_UPDATE_ORDER {
        let npos = pos.rel(dir);
        let nstate = level.state(npos);
        state = level.update_shape(state, pos, dir, npos, nstate);
    }
    state
}

/// `PistonMovingBlockEntity.tick` for every moving block of the world.
pub fn tick_moving(level: &mut Level) {
    let positions = std::mem::take(&mut level.world.updates.moving_pistons);
    let mut keep = Vec::with_capacity(positions.len());
    for pos in positions {
        let Some(mut moving) = moving_block(level.world, pos) else { continue };
        if block_id(level.state(pos)) != block!("moving_piston") {
            remove_moving_block(level.world, pos);
            continue;
        }
        moving.last_ticked = level.game_time();
        moving.progress_o = moving.progress;
        if moving.progress_o >= 1.0 {
            remove_moving_block(level.world, pos);
            finish(level, pos, &moving, false);
            continue;
        }
        moving.progress = (moving.progress + 0.5).min(1.0);
        store_moving_block(level.world, pos, &moving);
        keep.push(pos);
    }
    // Moves started while ticking (by blocks finishing their move) tick from the next tick.
    keep.append(&mut level.world.updates.moving_pistons);
    level.world.updates.moving_pistons = keep;
}

/// Places the moved block (vanilla's last tick, or `finalTick` when `early`).
fn finish(level: &mut Level, pos: BlockPos, moving: &MovingBlock, early: bool) {
    if block_id(level.state(pos)) != block!("moving_piston") {
        return;
    }
    if early {
        let new = if moving.source { blocks::AIR } else { update_from_neighbour_shapes(level, moving.moved_state, pos) };
        level.set_block(pos, new, flags::ALL);
        level.neighbor_changed_at(pos, block_id(new));
        return;
    }
    let new = update_from_neighbour_shapes(level, moving.moved_state, pos);
    if blocks::is_air(new) {
        level.set_block(pos, moving.moved_state, flags::MOVE_BY_PISTON | flags::KNOWN_SHAPE | flags::INVISIBLE);
        level.update_or_destroy(moving.moved_state, new, pos, flags::ALL, UPDATE_LIMIT);
    } else {
        let new = if get_bool(new, "waterlogged") { set_bool(new, "waterlogged", false) } else { new };
        level.set_block(pos, new, flags::ALL | flags::MOVE_BY_PISTON);
        level.neighbor_changed_at(pos, block_id(new));
    }
}

/// `finalTick`: a moving block ends its move at once.
fn final_tick(level: &mut Level, pos: BlockPos) {
    let Some(moving) = moving_block(level.world, pos) else { return };
    if moving.progress_o < 1.0 {
        remove_moving_block(level.world, pos);
        finish(level, pos, &moving, true);
    }
}

// --- the piston base -------------------------------------------------------------------------------

/// `isPushable`.
pub fn is_pushable(level: &Level, state: u32, pos: BlockPos, dir: Dir, allow_destroy: bool, connection: Dir) -> bool {
    if pos.y < level.world.min_y || pos.y >= level.world.max_y() || !level.is_loaded(pos) {
        return false;
    }
    if blocks::is_air(state) {
        return true;
    }
    if matches!(blocks::block_of_state(state).name, "minecraft:obsidian" | "minecraft:crying_obsidian" | "minecraft:respawn_anchor" | "minecraft:reinforced_deepslate") {
        return false;
    }
    if (dir == Dir::Down && pos.y == level.world.min_y) || (dir == Dir::Up && pos.y == level.world.max_y() - 1) {
        return false;
    }
    if is_piston_base(state) {
        if get_bool(state, "extended") {
            return false;
        }
    } else {
        if blocks::block_of_state(state).hardness < 0.0 {
            return false;
        }
        match physics::push_reaction(state) {
            PushReaction::Block => return false,
            PushReaction::Destroy => return allow_destroy,
            PushReaction::PushOnly => return dir == connection,
            PushReaction::Normal => {}
        }
    }
    blocks::flags(state) & blocks::FLAG_BLOCK_ENTITY == 0
}

/// Quasi-connectivity: power around the piston, or around the block above it.
fn neighbor_signal(level: &Level, pos: BlockPos, push: Dir) -> bool {
    if Dir::ALL.iter().any(|d| *d != push && level.has_signal(pos.rel(*d), *d)) {
        return true;
    }
    if level.has_signal(pos, Dir::Down) {
        return true;
    }
    let above = pos.above();
    Dir::ALL.iter().any(|d| *d != Dir::Down && level.has_signal(above.rel(*d), *d))
}

/// `checkIfExtend`: queue the block event that extends or retracts the piston.
pub fn check_if_extend(level: &mut Level, state: u32, pos: BlockPos) {
    let dir = facing(state);
    let extend = neighbor_signal(level, pos, dir);
    let extended = get_bool(state, "extended");
    if extend && !extended {
        if Resolver::new(pos, dir, true).resolve(level).is_some() {
            level.block_event(pos, block_id(state), 0, dir.index() as u8);
        }
    } else if !extend && extended {
        let pushed = pos.rel(dir).rel(dir);
        let pushed_state = level.state(pushed);
        let mut event = 1;
        if block_id(pushed_state) == block!("moving_piston") && facing(pushed_state) == dir {
            if let Some(moving) = moving_block(level.world, pushed) {
                if moving.extending && (moving.progress < 0.5 || level.game_time() == moving.last_ticked || level.world.updates.handling_tick) {
                    event = 2;
                }
            }
        }
        level.block_event(pos, block_id(state), event, dir.index() as u8);
    }
}

pub fn on_place(level: &mut Level, state: u32, pos: BlockPos, old: u32) {
    if block_id(old) != block_id(state) && moving_block(level.world, pos).is_none() {
        check_if_extend(level, state, pos);
    }
}

/// The piston block event.
pub fn trigger_event(level: &mut Level, state: u32, pos: BlockPos, action: u8, param: u8) -> bool {
    let dir = facing(state);
    let extended_state = set_bool(state, "extended", true);
    let extend = neighbor_signal(level, pos, dir);
    if extend && (action == 1 || action == 2) {
        level.set_block(pos, extended_state, flags::CLIENTS);
        return false;
    }
    if !extend && action == 0 {
        return false;
    }
    if action == 0 {
        if !move_blocks(level, state, pos, dir, true) {
            return false;
        }
        level.set_block(pos, extended_state, flags::ALL | flags::MOVE_BY_PISTON);
        let pitch = level.random_float() * 0.25 + 0.6;
        level.play_sound(None, pos, "minecraft:block.piston.extend", sound_source::BLOCKS, 0.5, pitch);
        return true;
    }
    // Retract (1) or retract without pulling (2).
    let arm = pos.rel(dir);
    if moving_block(level.world, arm).is_some() {
        final_tick(level, arm);
    }
    let piston_type = if sticky(state) { "sticky" } else { "normal" };
    let moving_piston = with(set_dir(default_state!("moving_piston"), "facing", dir), "type", piston_type);
    level.set_block(pos, moving_piston, flags::SKIP_BLOCK_ENTITY_SIDEEFFECTS | flags::KNOWN_SHAPE | flags::INVISIBLE);
    let base = set_dir(super::state::default_of(block_id(state)), "facing", dir_from_index(param));
    start_moving(level, pos, base, dir, false, true);
    level.update_neighbors_at(pos, block!("moving_piston"));
    level.update_neighbour_shapes(moving_piston, pos, flags::CLIENTS, UPDATE_LIMIT);
    if sticky(state) {
        let two = arm.rel(dir);
        let two_state = level.state(two);
        let mut piston_piece = false;
        if block_id(two_state) == block!("moving_piston") {
            if let Some(moving) = moving_block(level.world, two) {
                if moving.direction == dir && moving.extending {
                    final_tick(level, two);
                    piston_piece = true;
                }
            }
        }
        if !piston_piece {
            let two_state = level.state(two);
            let pullable = action == 1
                && !blocks::is_air(two_state)
                && is_pushable(level, two_state, two, dir.opposite(), false, dir)
                && (physics::push_reaction(two_state) == PushReaction::Normal || is_piston_base(two_state));
            if pullable {
                move_blocks(level, state, pos, dir, false);
            } else {
                level.remove_block(arm, false);
            }
        }
    } else {
        level.remove_block(arm, false);
    }
    let pitch = level.random_float() * 0.15 + 0.6;
    level.play_sound(None, pos, "minecraft:block.piston.contract", sound_source::BLOCKS, 0.5, pitch);
    true
}

/// `moveBlocks`: turns the moving structure into moving blocks one step further.
fn move_blocks(level: &mut Level, piston: u32, pos: BlockPos, dir: Dir, extending: bool) -> bool {
    let arm = pos.rel(dir);
    if !extending && kind(level.state(arm)) == Kind::PistonHead {
        level.set_block(arm, blocks::AIR, flags::SKIP_BLOCK_ENTITY_SIDEEFFECTS | flags::KNOWN_SHAPE | flags::INVISIBLE);
    }
    let Some((to_push, to_destroy)) = Resolver::new(pos, dir, extending).resolve(level) else { return false };
    let mut deleted: Vec<(BlockPos, u32)> = to_push.iter().map(|p| (*p, level.state(*p))).collect();
    let shapes: Vec<u32> = deleted.iter().map(|(_, s)| *s).collect();
    let push_dir = if extending { dir } else { dir.opposite() };
    let mut updated: Vec<u32> = Vec::with_capacity(to_push.len() + to_destroy.len());
    for p in to_destroy.iter().rev() {
        let state = level.state(*p);
        level.drop_resources(state, *p);
        level.set_block(*p, blocks::AIR, flags::KNOWN_SHAPE | flags::CLIENTS);
        updated.push(state);
    }
    for (i, p) in to_push.iter().enumerate().rev() {
        let state = level.state(*p);
        let target = p.rel(push_dir);
        deleted.retain(|(d, _)| *d != target);
        let moving = set_dir(default_state!("moving_piston"), "facing", dir);
        level.set_block(target, moving, flags::SKIP_BLOCK_ENTITY_SIDEEFFECTS | flags::MOVE_BY_PISTON | flags::INVISIBLE);
        start_moving(level, target, shapes[i], dir, extending, false);
        updated.push(state);
    }
    if extending {
        let piston_type = if sticky(piston) { "sticky" } else { "normal" };
        let head = with(set_dir(default_state!("piston_head"), "facing", dir), "type", piston_type);
        let moving = with(set_dir(default_state!("moving_piston"), "facing", dir), "type", piston_type);
        deleted.retain(|(d, _)| *d != arm);
        level.set_block(arm, moving, flags::SKIP_BLOCK_ENTITY_SIDEEFFECTS | flags::MOVE_BY_PISTON | flags::INVISIBLE);
        start_moving(level, arm, head, dir, true, true);
    }
    for (p, _) in &deleted {
        level.set_block(*p, blocks::AIR, flags::MOVE_BY_PISTON | flags::KNOWN_SHAPE | flags::CLIENTS);
    }
    for (p, old) in &deleted {
        level.update_indirect_neighbour_shapes(*old, *p, flags::CLIENTS, UPDATE_LIMIT);
        level.update_neighbour_shapes(blocks::AIR, *p, flags::CLIENTS, UPDATE_LIMIT);
    }
    let mut index = 0;
    for p in to_destroy.iter().rev() {
        let state = updated[index];
        index += 1;
        level.affect_neighbors_after_removal(state, *p, false);
        level.update_indirect_neighbour_shapes(state, *p, flags::CLIENTS, UPDATE_LIMIT);
        level.update_neighbors_at(*p, block_id(state));
    }
    for p in to_push.iter().rev() {
        let state = updated[index];
        index += 1;
        level.update_neighbors_at(*p, block_id(state));
    }
    if extending {
        level.update_neighbors_at(arm, block!("piston_head"));
    }
    true
}

/// `PistonStructureResolver`.
struct Resolver {
    piston: BlockPos,
    piston_dir: Dir,
    push_dir: Dir,
    start: BlockPos,
    extending: bool,
    to_push: Vec<BlockPos>,
    to_destroy: Vec<BlockPos>,
}

fn is_sticky_block(state: u32) -> bool {
    let id = block_id(state);
    id == block!("slime_block") || id == block!("honey_block")
}

fn can_stick(a: u32, b: u32) -> bool {
    let (slime, honey) = (block!("slime_block"), block!("honey_block"));
    let (a_id, b_id) = (block_id(a), block_id(b));
    if (a_id == honey && b_id == slime) || (a_id == slime && b_id == honey) {
        return false;
    }
    is_sticky_block(a) || is_sticky_block(b)
}

impl Resolver {
    fn new(piston: BlockPos, piston_dir: Dir, extending: bool) -> Resolver {
        let (push_dir, start) = if extending { (piston_dir, piston.rel(piston_dir)) } else { (piston_dir.opposite(), piston.rel(piston_dir).rel(piston_dir)) };
        Resolver { piston, piston_dir, push_dir, start, extending, to_push: Vec::new(), to_destroy: Vec::new() }
    }

    /// The blocks to push (in push order) and to destroy, or `None` if the piston is stuck.
    fn resolve(mut self, level: &Level) -> Option<(Vec<BlockPos>, Vec<BlockPos>)> {
        let state = level.state(self.start);
        if !is_pushable(level, state, self.start, self.push_dir, false, self.piston_dir) {
            if self.extending && physics::push_reaction(state) == PushReaction::Destroy {
                self.to_destroy.push(self.start);
                return Some((self.to_push, self.to_destroy));
            }
            return None;
        }
        if !self.add_line(level, self.start, self.push_dir) {
            return None;
        }
        let mut i = 0;
        while i < self.to_push.len() {
            let p = self.to_push[i];
            if is_sticky_block(level.state(p)) && !self.add_branches(level, p) {
                return None;
            }
            i += 1;
        }
        Some((self.to_push, self.to_destroy))
    }

    fn add_line(&mut self, level: &Level, start: BlockPos, dir: Dir) -> bool {
        let mut next = level.state(start);
        if blocks::is_air(next) || !is_pushable(level, next, start, self.push_dir, false, dir) || start == self.piston || self.to_push.contains(&start) {
            return true;
        }
        let mut count = 1;
        if count + self.to_push.len() > MAX_PUSH_DEPTH {
            return false;
        }
        let back = self.push_dir.opposite();
        let offset = |p: BlockPos, d: Dir, n: usize| (0..n).fold(p, |p, _| p.rel(d));
        while is_sticky_block(next) {
            let p = offset(start, back, count);
            let previous = next;
            next = level.state(p);
            if blocks::is_air(next) || !can_stick(previous, next) || !is_pushable(level, next, p, self.push_dir, false, back) || p == self.piston {
                break;
            }
            count += 1;
            if count + self.to_push.len() > MAX_PUSH_DEPTH {
                return false;
            }
        }
        let mut added = 0;
        for i in (0..count).rev() {
            self.to_push.push(offset(start, back, i));
            added += 1;
        }
        let mut i = 1;
        loop {
            let p = offset(start, self.push_dir, i);
            if let Some(collision) = self.to_push.iter().position(|q| *q == p) {
                self.reorder_at_collision(added, collision);
                for j in 0..=collision + added {
                    let q = self.to_push[j];
                    if is_sticky_block(level.state(q)) && !self.add_branches(level, q) {
                        return false;
                    }
                }
                return true;
            }
            next = level.state(p);
            if blocks::is_air(next) {
                return true;
            }
            if !is_pushable(level, next, p, self.push_dir, true, self.push_dir) || p == self.piston {
                return false;
            }
            if physics::push_reaction(next) == PushReaction::Destroy {
                self.to_destroy.push(p);
                return true;
            }
            if self.to_push.len() >= MAX_PUSH_DEPTH {
                return false;
            }
            self.to_push.push(p);
            added += 1;
            i += 1;
        }
    }

    fn reorder_at_collision(&mut self, added: usize, collision: usize) {
        let len = self.to_push.len();
        let mut reordered = self.to_push[..collision].to_vec();
        reordered.extend_from_slice(&self.to_push[len - added..]);
        reordered.extend_from_slice(&self.to_push[collision..len - added]);
        self.to_push = reordered;
    }

    fn add_branches(&mut self, level: &Level, from: BlockPos) -> bool {
        let from_state = level.state(from);
        for dir in Dir::ALL {
            if dir.axis() == self.push_dir.axis() {
                continue;
            }
            let p = from.rel(dir);
            if can_stick(level.state(p), from_state) && !self.add_line(level, p, dir) {
                return false;
            }
        }
        true
    }
}

// --- piston heads ------------------------------------------------------------------------------

fn fitting_base(head: u32, base: u32) -> bool {
    let wanted = if prop(head, "type") == Some("sticky") { block!("sticky_piston") } else { block!("piston") };
    block_id(base) == wanted && get_bool(base, "extended") && facing(base) == facing(head)
}

pub fn head_can_survive(level: &Level, state: u32, pos: BlockPos) -> bool {
    let behind = level.state(pos.rel(facing(state).opposite()));
    fitting_base(state, behind) || (block_id(behind) == block!("moving_piston") && facing(behind) == facing(state))
}

pub fn head_update_shape(level: &Level, state: u32, pos: BlockPos, dir: Dir) -> u32 {
    if dir.opposite() == facing(state) && !head_can_survive(level, state, pos) {
        blocks::AIR
    } else {
        state
    }
}

/// Breaking a piston head breaks its piston.
pub fn head_after_removal(level: &mut Level, state: u32, pos: BlockPos) {
    let base = pos.rel(facing(state).opposite());
    if fitting_base(state, level.state(base)) {
        level.destroy_block(base, true);
    }
}

pub fn head_neighbor_changed(level: &mut Level, state: u32, pos: BlockPos, source_block: u32) {
    if head_can_survive(level, state, pos) {
        level.neighbor_changed_at(pos.rel(facing(state).opposite()), source_block);
    }
}

/// In creative, breaking the head removes the piston without drops.
pub fn head_player_will_destroy(level: &mut Level, state: u32, pos: BlockPos) {
    let base = pos.rel(facing(state).opposite());
    if fitting_base(state, level.state(base)) {
        level.destroy_block(base, false);
    }
}
