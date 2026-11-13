//! Scheduled block and fluid ticks, modelled on vanilla's `LevelTicks` / `LevelChunkTicks`.
//!
//! Every chunk keeps its own queues ([`ChunkTicks`]), so ticks are saved and loaded with the
//! chunk (`block_ticks` / `fluid_ticks` in the chunk NBT) and stop existing for the world
//! when it unloads. A tick is identified by its position and type: a block id for block
//! ticks, a `minecraft:fluid` id for fluid ticks. Scheduling a tick that is already pending
//! does nothing. Due ticks run ordered by due time, then priority, then scheduling order, at
//! most [`MAX_TICKS_PER_TICK`] of each kind per game tick; the rest wait for the next tick.

use super::chunk::{Chunk, ChunkPos};
use paper_data::blocks;
use paper_nbt::{Compound, Tag};
use paper_protocol::codec::BlockPos;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Ticks of each kind a world runs per game tick at most (vanilla's limit).
pub const MAX_TICKS_PER_TICK: usize = 65536;

/// Vanilla's `TickPriority`: among ticks due at the same time, lower values run first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TickPriority {
    ExtremelyHigh = -3,
    VeryHigh = -2,
    High = -1,
    Normal = 0,
    Low = 1,
    VeryLow = 2,
    ExtremelyLow = 3,
}

impl TickPriority {
    pub fn value(self) -> i32 {
        self as i32
    }

    /// The priority with this value; out-of-range values are clamped like vanilla does.
    pub fn from_value(value: i32) -> TickPriority {
        match value {
            i32::MIN..=-3 => TickPriority::ExtremelyHigh,
            -2 => TickPriority::VeryHigh,
            -1 => TickPriority::High,
            0 => TickPriority::Normal,
            1 => TickPriority::Low,
            2 => TickPriority::VeryLow,
            _ => TickPriority::ExtremelyLow,
        }
    }
}

/// Whether a tick is a block tick or a fluid tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TickKind {
    Block,
    Fluid,
}

impl TickKind {
    /// The registry a tick's type lives in.
    fn registry(self) -> &'static str {
        match self {
            TickKind::Block => "minecraft:block",
            TickKind::Fluid => "minecraft:fluid",
        }
    }

    /// Registry name of a tick type, as saved in chunk NBT.
    pub fn type_name(self, id: u32) -> Option<&'static str> {
        match self {
            TickKind::Block => blocks::BLOCKS.get(id as usize).map(|b| b.name),
            TickKind::Fluid => paper_data::registries::name(self.registry(), id),
        }
    }

    pub fn type_id(self, name: &str) -> Option<u32> {
        match self {
            TickKind::Block => blocks::id(name),
            TickKind::Fluid => paper_data::registries::id(self.registry(), name),
        }
    }
}

/// A pending tick. Orders by due time, priority and scheduling order (then position and
/// type, which only makes the order total).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScheduledTick {
    /// Block id or fluid id.
    pub ty: u32,
    pub pos: BlockPos,
    /// Game time (world age) at which the tick is due.
    pub trigger: i64,
    pub priority: TickPriority,
    /// Order of scheduling within the world (vanilla's sub-tick count); ticks loaded from
    /// disk get negative values so they run before newer ones.
    pub sub_order: i64,
}

impl Ord for ScheduledTick {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.trigger, self.priority, self.sub_order, self.pos.x, self.pos.y, self.pos.z, self.ty).cmp(&(
            other.trigger,
            other.priority,
            other.sub_order,
            other.pos.x,
            other.pos.y,
            other.pos.z,
            other.ty,
        ))
    }
}

impl PartialOrd for ScheduledTick {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The pending ticks of one kind in one chunk.
#[derive(Clone, Debug, Default)]
pub struct TickQueue {
    queue: BTreeSet<ScheduledTick>,
    pending: HashSet<(BlockPos, u32)>,
}

impl TickQueue {
    /// Adds a tick unless one of the same type is already pending at the position.
    pub fn schedule(&mut self, tick: ScheduledTick) -> bool {
        if !self.pending.insert((tick.pos, tick.ty)) {
            return false;
        }
        self.queue.insert(tick);
        true
    }

    pub fn has(&self, pos: BlockPos, ty: u32) -> bool {
        self.pending.contains(&(pos, ty))
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// The tick due first.
    pub fn first(&self) -> Option<&ScheduledTick> {
        self.queue.first()
    }

    /// Pending ticks in run order.
    pub fn iter(&self) -> impl Iterator<Item = &ScheduledTick> {
        self.queue.iter()
    }

    fn remove(&mut self, tick: &ScheduledTick) {
        if self.queue.remove(tick) {
            self.pending.remove(&(tick.pos, tick.ty));
        }
    }

    fn save(&self, now: i64, out: &mut Vec<SavedTick>) {
        out.extend(self.queue.iter().map(|t| SavedTick { ty: t.ty, pos: t.pos, delay: (t.trigger - now).clamp(i32::MIN as i64, i32::MAX as i64) as i32, priority: t.priority }));
    }
}

/// A tick as saved in chunk NBT: its delay is relative to the time the chunk was saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedTick {
    pub ty: u32,
    pub pos: BlockPos,
    pub delay: i32,
    pub priority: TickPriority,
}

/// The ticks of a chunk in their saved form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedTicks {
    pub blocks: Vec<SavedTick>,
    pub fluids: Vec<SavedTick>,
}

impl SavedTicks {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.fluids.is_empty()
    }
}

/// Block and fluid ticks of one chunk.
#[derive(Clone, Debug, Default)]
pub struct ChunkTicks {
    pub blocks: TickQueue,
    pub fluids: TickQueue,
    /// Ticks loaded with the chunk and not scheduled yet: they are scheduled relative to the
    /// world time when the chunk enters a world ([`ChunkTicks::unpack`]).
    pub saved: Option<Box<SavedTicks>>,
}

impl ChunkTicks {
    pub fn queue(&self, kind: TickKind) -> &TickQueue {
        match kind {
            TickKind::Block => &self.blocks,
            TickKind::Fluid => &self.fluids,
        }
    }

    pub fn queue_mut(&mut self, kind: TickKind) -> &mut TickQueue {
        match kind {
            TickKind::Block => &mut self.blocks,
            TickKind::Fluid => &mut self.fluids,
        }
    }

    /// Ticks loaded from disk, waiting for [`ChunkTicks::unpack`].
    pub fn from_saved(saved: SavedTicks) -> ChunkTicks {
        ChunkTicks { saved: (!saved.is_empty()).then(|| Box::new(saved)), ..Default::default() }
    }

    /// Schedules the loaded ticks relative to `now`. Like vanilla, they get negative
    /// scheduling orders in saved order, so they run before ticks scheduled later.
    pub fn unpack(&mut self, now: i64) {
        let Some(saved) = self.saved.take() else { return };
        for (list, queue) in [(&saved.blocks, &mut self.blocks), (&saved.fluids, &mut self.fluids)] {
            let first = -(list.len() as i64);
            for (i, t) in list.iter().enumerate() {
                queue.schedule(ScheduledTick { ty: t.ty, pos: t.pos, trigger: now + t.delay as i64, priority: t.priority, sub_order: first + i as i64 });
            }
        }
    }

    /// The pending ticks with delays relative to `now`, in run order.
    pub fn save(&self, now: i64) -> SavedTicks {
        let mut out = match &self.saved {
            Some(saved) => (**saved).clone(),
            None => SavedTicks::default(),
        };
        self.blocks.save(now, &mut out.blocks);
        self.fluids.save(now, &mut out.fluids);
        out
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.fluids.is_empty() && self.saved.is_none()
    }
}

/// Collects the ticks of one kind to run at game time `now`: due ticks of the chunks
/// `ticking` accepts, in run order, at most `budget`. They are removed from their chunks;
/// ticks beyond the budget stay scheduled.
pub fn collect_due(chunks: &mut HashMap<ChunkPos, Chunk>, kind: TickKind, now: i64, budget: usize, ticking: impl Fn(ChunkPos) -> bool) -> Vec<ScheduledTick> {
    let mut due: Vec<ScheduledTick> = Vec::new();
    for (pos, chunk) in chunks.iter() {
        let queue = chunk.ticks.queue(kind);
        if queue.first().is_none_or(|t| t.trigger > now) || !ticking(*pos) {
            continue;
        }
        due.extend(queue.iter().take_while(|t| t.trigger <= now).take(budget).copied());
    }
    due.sort_unstable();
    due.truncate(budget);
    for tick in &due {
        if let Some(chunk) = chunks.get_mut(&ChunkPos::of_block(tick.pos.x, tick.pos.z)) {
            chunk.ticks.queue_mut(kind).remove(tick);
        }
    }
    due
}

// --- NBT ---------------------------------------------------------------------------------

/// The `block_ticks` / `fluid_ticks` list of a chunk: `{i: type, x, y, z, t: delay, p: priority}`.
pub fn ticks_to_nbt(ticks: &[SavedTick], kind: TickKind) -> Tag {
    Tag::List(
        ticks
            .iter()
            .filter_map(|t| {
                let name = kind.type_name(t.ty)?;
                Some(Tag::Compound(
                    Compound::new()
                        .with("i", Tag::String(name.to_owned()))
                        .with("x", Tag::Int(t.pos.x))
                        .with("y", Tag::Int(t.pos.y))
                        .with("z", Tag::Int(t.pos.z))
                        .with("t", Tag::Int(t.delay))
                        .with("p", Tag::Int(t.priority.value())),
                ))
            })
            .collect(),
    )
}

/// Reads a `block_ticks` / `fluid_ticks` list. Entries with unknown types or positions
/// outside the chunk are dropped, as vanilla does.
pub fn ticks_from_nbt(list: &[Tag], kind: TickKind, chunk: ChunkPos) -> Vec<SavedTick> {
    list.iter()
        .filter_map(|tag| {
            let c = tag.as_compound()?;
            let ty = kind.type_id(c.string("i")?)?;
            let pos = BlockPos::new(c.i32("x")?, c.i32("y")?, c.i32("z")?);
            if ChunkPos::of_block(pos.x, pos.z) != chunk {
                return None;
            }
            Some(SavedTick { ty, pos, delay: c.i32("t").unwrap_or(0), priority: TickPriority::from_value(c.i32("p").unwrap_or(0)) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(ty: u32, x: i32, trigger: i64, priority: TickPriority, sub_order: i64) -> ScheduledTick {
        ScheduledTick { ty, pos: BlockPos::new(x, 0, 0), trigger, priority, sub_order }
    }

    #[test]
    fn queue_orders_and_deduplicates() {
        let mut q = TickQueue::default();
        assert!(q.schedule(tick(1, 0, 10, TickPriority::Normal, 0)));
        assert!(!q.schedule(tick(1, 0, 5, TickPriority::High, 1)), "same position and type");
        assert!(q.schedule(tick(2, 0, 10, TickPriority::Normal, 2)), "another type at the same position");
        assert!(q.schedule(tick(1, 1, 10, TickPriority::High, 3)));
        assert!(q.schedule(tick(1, 2, 9, TickPriority::Low, 4)));
        let order: Vec<(u32, i32)> = q.iter().map(|t| (t.ty, t.pos.x)).collect();
        assert_eq!(order, [(1, 2), (1, 1), (1, 0), (2, 0)], "due time, then priority, then scheduling order");
        assert!(q.has(BlockPos::new(0, 0, 0), 1));
        assert!(!q.has(BlockPos::new(0, 0, 0), 3));
    }

    #[test]
    fn save_and_unpack_keep_order_and_delays() {
        let mut ticks = ChunkTicks::default();
        ticks.blocks.schedule(tick(7, 1, 105, TickPriority::Normal, 40));
        ticks.blocks.schedule(tick(7, 2, 103, TickPriority::VeryHigh, 41));
        ticks.fluids.schedule(tick(2, 3, 100, TickPriority::Normal, 42));
        let saved = ticks.save(100);
        assert_eq!(saved.blocks.iter().map(|t| (t.pos.x, t.delay)).collect::<Vec<_>>(), [(2, 3), (1, 5)]);
        assert_eq!(saved.fluids[0].delay, 0);

        let mut loaded = ChunkTicks::from_saved(saved.clone());
        assert!(loaded.blocks.is_empty() && !loaded.is_empty());
        assert_eq!(loaded.save(0), saved, "still-saved ticks are written back unchanged");
        loaded.unpack(1000);
        let blocks: Vec<(i32, i64, i64)> = loaded.blocks.iter().map(|t| (t.pos.x, t.trigger, t.sub_order)).collect();
        assert_eq!(blocks, [(2, 1003, -2), (1, 1005, -1)]);
        assert_eq!(loaded.save(1000), saved);
    }

    #[test]
    fn nbt_round_trip() {
        let water = paper_data::registries::id("minecraft:fluid", "minecraft:flowing_water").unwrap();
        let torch = blocks::id("minecraft:redstone_torch").unwrap();
        let chunk = ChunkPos::new(1, -1);
        let fluids = vec![SavedTick { ty: water, pos: BlockPos::new(17, -60, -3), delay: 4, priority: TickPriority::Normal }];
        let blocks_list = vec![SavedTick { ty: torch, pos: BlockPos::new(31, 70, -16), delay: -2, priority: TickPriority::ExtremelyHigh }];
        let tag = ticks_to_nbt(&fluids, TickKind::Fluid);
        let c = tag.as_list().unwrap()[0].as_compound().unwrap();
        assert_eq!(c.string("i"), Some("minecraft:flowing_water"));
        assert_eq!((c.i32("x"), c.i32("t"), c.i32("p")), (Some(17), Some(4), Some(0)));
        assert_eq!(ticks_from_nbt(tag.as_list().unwrap(), TickKind::Fluid, chunk), fluids);
        let tag = ticks_to_nbt(&blocks_list, TickKind::Block);
        assert_eq!(ticks_from_nbt(tag.as_list().unwrap(), TickKind::Block, chunk), blocks_list);
        // Ticks outside the chunk are dropped.
        assert!(ticks_from_nbt(tag.as_list().unwrap(), TickKind::Block, ChunkPos::new(0, 0)).is_empty());
    }
}
