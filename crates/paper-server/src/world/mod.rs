//! Worlds: chunk storage, the asynchronous chunk pipeline, generation and lighting.
//!
//! A [`World`] is owned and mutated by the server's main thread only. Expensive work is
//! handed to the worker pool as pure jobs over snapshots, and the results come back as
//! [`WorldEvent`]s:
//!
//! ```text
//! ticket added ──► terrain job (generate) ──► light job (needs 3x3 terrain) ──► lit chunk
//!                                                         packet requested ──► encode job
//! ```
//!
//! With vanilla generation, terrain is followed by decoration, which writes into the
//! neighbours too; chunks are only handed to the rest of the server once finished (see
//! [`staged`]).

pub mod attributes;
pub mod block_updates;
pub mod chunk;
pub mod entity;
pub mod entity_nbt;
pub mod generator;
pub mod light;
pub mod portal;
pub mod rules;
pub mod staged;
pub mod storage;
pub mod ticks;

use chunk::{Chunk, ChunkPos, GenStatus, LightSection};
use staged::ProtoEntry;
use generator::Generator;
use light::{ChangedSection, Terrain};
use storage::{LevelData, SaveChunk, WorldStorage};
use paper_data::blocks;
use paper_protocol::codec::BlockPos;
use paper_protocol::frame::frame_packet;
use paper_protocol::packets::encode_packet;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Results of worker jobs, delivered to the main thread.
pub enum WorldEvent {
    /// A finished chunk was generated or loaded.
    Terrain(Box<Chunk>),
    /// An unfinished chunk (vanilla generation) was generated or loaded.
    Proto(Box<ProtoEntry>),
    /// The chunk at `center` was decorated; these are it and its neighbours.
    Decorated { center: ChunkPos, entries: Vec<ProtoEntry> },
    /// Light for a chunk was computed from the terrain at `stamp`.
    Light { pos: ChunkPos, stamp: u64, sky: Vec<Arc<LightSection>>, block: Vec<Arc<LightSection>> },
    /// A chunk packet was encoded and framed for the chunk at `revision`.
    Encoded { pos: ChunkPos, revision: u64, frame: Arc<Vec<u8>> },
}

/// What worker jobs need from the server.
#[derive(Clone)]
pub struct WorkerContext {
    pub pool: Arc<rayon::ThreadPool>,
    /// Delivers a job result to the main thread for the world with the given index.
    pub notify: Arc<dyn Fn(usize, WorldEvent) + Send + Sync>,
    /// Packet compression threshold shared by all connections.
    pub compression_threshold: Option<u32>,
    pub compression_level: u32,
}

/// The outcome of a successful [`World::set_block`].
#[derive(Debug)]
pub struct BlockChange {
    pub pos: BlockPos,
    pub old: u32,
    pub new: u32,
    /// Light sections that changed as a consequence.
    pub light: HashSet<ChangedSection>,
}

pub struct World {
    /// Index of this world in the server's world list.
    pub index: usize,
    /// Folder / Bukkit name, e.g. `world`.
    pub name: String,
    /// Dimension key, e.g. `minecraft:overworld`.
    pub dimension: String,
    /// Network id of the dimension type.
    pub dimension_type: i32,
    pub min_y: i32,
    pub height: i32,
    pub has_skylight: bool,
    pub seed: i64,
    pub generator: Arc<dyn Generator>,
    pub chunks: HashMap<ChunkPos, Chunk>,
    pub spawn: BlockPos,
    pub spawn_yaw: f32,
    /// Ticks this world has existed.
    pub age: i64,
    /// Time of the world's clock in ticks (0 = sunrise).
    pub day_time: i64,
    pub daylight_cycle: bool,
    pub weather: Weather,
    pub rules: rules::GameRules,
    /// Network id of the world's clock in `minecraft:world_clock`, if it has one.
    pub clock: Option<i32>,
    /// Size of the synchronised biome registry.
    pub biome_count: u32,
    /// Where the world is saved; `None` keeps it in memory only.
    pub storage: Option<Arc<WorldStorage>>,
    /// Entities other than players, by entity id.
    pub entities: HashMap<i32, entity::Entity>,
    /// Entity ids per chunk, for area queries and saving.
    pub entity_chunks: HashMap<ChunkPos, HashSet<i32>>,
    /// Block entities removed by block changes since the server last looked (position, old
    /// block state, data).
    pub removed_block_entities: Vec<(BlockPos, u32, paper_nbt::Compound)>,
    /// Block updates: scheduled tick bookkeeping, block events and what clients must be told
    /// (see `world::block_updates`).
    pub updates: block_updates::BlockUpdateState,
    /// Players (or other holders) wanting each chunk loaded and lit.
    tickets: HashMap<ChunkPos, u32>,
    /// Ticketed chunks that are not lit yet, in request order.
    wanted: VecDeque<ChunkPos>,
    generating: HashSet<ChunkPos>,
    /// Light jobs in flight and the terrain stamp they were started from.
    lighting: HashMap<ChunkPos, u64>,
    /// Encoded chunk packets and the revision they were built from.
    packets: HashMap<ChunkPos, (u64, Arc<Vec<u8>>)>,
    encoding: HashMap<ChunkPos, u64>,
    /// Unfinished chunks of vanilla generation (not part of the playable world yet).
    pub proto: HashMap<ChunkPos, ProtoEntry>,
    /// Unfinished chunks a decoration job has taken.
    locked: HashSet<ChunkPos>,
    /// Centers of decoration jobs in flight.
    decorating: HashSet<ChunkPos>,
    /// Chunks being turned from unfinished into finished ones.
    finalizing: HashSet<ChunkPos>,
    /// Chunks to save as soon as their decoration job gives them back.
    save_on_return: HashSet<ChunkPos>,
}

/// A world's weather (vanilla's `ServerLevelData` weather and the levels it fades in and
/// out with, 0.01 a tick).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Weather {
    pub raining: bool,
    pub thundering: bool,
    /// Ticks until rain starts or stops.
    pub rain_time: i32,
    /// Ticks until thunder starts or stops.
    pub thunder_time: i32,
    /// Ticks of clear weather set by `/weather clear`.
    pub clear_time: i32,
    pub rain_level: f32,
    pub thunder_level: f32,
}

impl Weather {
    /// `Level.isRaining`.
    pub fn is_raining(&self) -> bool {
        self.rain_level > 0.2
    }

    /// `Level.isThundering`.
    pub fn is_thundering(&self) -> bool {
        self.thunder_level * self.rain_level > 0.9
    }
}

/// Maximum worker jobs a world keeps in flight per worker thread.
const JOBS_PER_WORKER: usize = 6;

impl World {
    pub fn new(index: usize, name: &str, dimension: &str, seed: i64, generator: Arc<dyn Generator>) -> Self {
        let dim = paper_data::synced::dimension_type(dimension).unwrap_or_else(|| panic!("no dimension type data for {dimension}"));
        let dimension_type = paper_data::synced::id("minecraft:dimension_type", dimension).expect("dimension type is in the synchronised registry");
        let clock_name = match dimension {
            "minecraft:the_end" => Some("minecraft:the_end"),
            "minecraft:the_nether" => None,
            _ => Some("minecraft:overworld"),
        };
        let surface = generator.surface_y(8, 8);
        Self {
            index,
            name: name.to_owned(),
            dimension: dimension.to_owned(),
            dimension_type,
            min_y: dim.min_y,
            height: dim.height,
            has_skylight: dim.has_skylight,
            seed,
            spawn: BlockPos::new(8, surface + 1, 8),
            spawn_yaw: 0.0,
            generator,
            chunks: HashMap::new(),
            age: 0,
            day_time: 0,
            daylight_cycle: true,
            weather: Weather::default(),
            rules: rules::GameRules::default(),
            clock: clock_name.and_then(|c| paper_data::synced::id("minecraft:world_clock", c)),
            biome_count: paper_data::synced::entries("minecraft:worldgen/biome").map_or(1, |e| e.len() as u32),
            storage: None,
            entities: HashMap::new(),
            entity_chunks: HashMap::new(),
            removed_block_entities: Vec::new(),
            updates: block_updates::BlockUpdateState::new(dimension, seed),
            tickets: HashMap::new(),
            wanted: VecDeque::new(),
            generating: HashSet::new(),
            lighting: HashMap::new(),
            packets: HashMap::new(),
            encoding: HashMap::new(),
            proto: HashMap::new(),
            locked: HashSet::new(),
            decorating: HashSet::new(),
            finalizing: HashSet::new(),
            save_on_return: HashSet::new(),
        }
    }

    pub fn max_y(&self) -> i32 {
        self.min_y + self.height
    }

    pub fn in_bounds(&self, y: i32) -> bool {
        y >= self.min_y && y < self.max_y()
    }

    /// First 8 bytes of the SHA-256 of the seed, which clients use for biome blending.
    pub fn hashed_seed(&self) -> i64 {
        // The client only needs a stable value; an avalanche mix of the seed serves the
        // same purpose without pulling a SHA-256 implementation into the server.
        let mut v = self.seed as u64 ^ 0x9E37_79B9_7F4A_7C15;
        v = (v ^ (v >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        v = (v ^ (v >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (v ^ (v >> 31)) as i64
    }

    // --- tickets -----------------------------------------------------------------------

    /// Asks for a chunk to be loaded and lit.
    pub fn add_ticket(&mut self, pos: ChunkPos) {
        let count = self.tickets.entry(pos).or_insert(0);
        *count += 1;
        if *count == 1 && !self.is_ready(pos) {
            self.wanted.push_back(pos);
        }
    }

    pub fn remove_ticket(&mut self, pos: ChunkPos) {
        if let Some(count) = self.tickets.get_mut(&pos) {
            *count -= 1;
            if *count == 0 {
                self.tickets.remove(&pos);
            }
        }
    }

    pub fn has_ticket(&self, pos: ChunkPos) -> bool {
        self.tickets.contains_key(&pos)
    }

    /// Whether the chunk is loaded with valid light, i.e. can be shown to players.
    pub fn is_ready(&self, pos: ChunkPos) -> bool {
        self.chunks.get(&pos).is_some_and(|c| c.lit)
    }

    pub fn loaded_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn jobs_in_flight(&self) -> usize {
        self.generating.len() + self.lighting.len() + self.encoding.len() + self.decorating.len() + self.finalizing.len()
    }

    /// Generation jobs holding unfinished chunks (which would be lost if the server stopped
    /// without waiting for them).
    pub fn generation_in_flight(&self) -> usize {
        self.decorating.len() + self.finalizing.len()
    }

    // --- pipeline ----------------------------------------------------------------------

    /// A value that changes whenever the terrain of the 3x3 neighbourhood of `pos` changes.
    fn terrain_stamp(&self, pos: ChunkPos) -> u64 {
        let mut stamp = 0u64;
        for dz in -1..=1 {
            for dx in -1..=1 {
                if let Some(chunk) = self.chunks.get(&ChunkPos::new(pos.x + dx, pos.z + dz)) {
                    stamp = stamp.wrapping_mul(31).wrapping_add(chunk.revision.wrapping_add(1));
                }
            }
        }
        stamp
    }

    /// Starts worker jobs for wanted chunks. Call once per tick.
    pub fn pump(&mut self, ctx: &WorkerContext) {
        let limit = ctx.pool.current_num_threads().max(1) * JOBS_PER_WORKER;
        let staged = self.generator.staged().is_some();
        // Wanted chunks are in the order they were asked for, nearest to players first; each
        // tick starts from the front so that the nearest chunks get worker time first.
        let mut i = 0;
        while i < self.wanted.len() && self.generating.len() + self.lighting.len() + self.decorating.len() + self.finalizing.len() < limit {
            let pos = self.wanted[i];
            if !self.tickets.contains_key(&pos) || self.is_ready(pos) {
                self.wanted.remove(i);
                continue;
            }
            i += 1;
            // The chunk and its eight neighbours must be there (finished) for light.
            let mut terrain_ready = true;
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = ChunkPos::new(pos.x + dx, pos.z + dz);
                    if self.chunks.contains_key(&p) {
                        continue;
                    }
                    terrain_ready = false;
                    if staged {
                        self.advance_generation(p, ctx);
                    } else {
                        self.request_chunk(p, ctx);
                    }
                }
            }
            if terrain_ready && !self.lighting.contains_key(&pos) {
                self.spawn_light_job(pos, ctx);
            }
        }
    }

    /// Starts loading (or else generating) a chunk that is not here.
    fn request_chunk(&mut self, p: ChunkPos, ctx: &WorkerContext) {
        if self.chunks.contains_key(&p) || self.proto.contains_key(&p) || self.locked.contains(&p) || self.finalizing.contains(&p) || !self.generating.insert(p) {
            return;
        }
        let (generator, min_y, height, index, notify) = (self.generator.clone(), self.min_y, self.height, self.index, ctx.notify.clone());
        let (storage, has_skylight) = (self.storage.clone(), self.has_skylight);
        ctx.pool.spawn(move || {
            // Saved chunks are loaded; missing ones are generated (and saved later).
            let mut loaded = storage.as_ref().and_then(|s| match s.load_chunk(p, min_y, height, has_skylight) {
                Ok(chunk) => chunk,
                Err(e) => {
                    log::error!("Could not load chunk {},{}; generating it again: {e:#}", p.x, p.z);
                    None
                }
            });
            // An unfinished chunk continues where it stopped (with vanilla generation; any
            // other generator makes it again).
            if loaded.as_ref().is_some_and(|c| c.status != GenStatus::Full) {
                match generator.staged() {
                    Some(g) => {
                        let entry = ProtoEntry::from_saved(loaded.as_ref().unwrap(), g);
                        notify(index, WorldEvent::Proto(Box::new(entry)));
                        return;
                    }
                    None => loaded = None,
                }
            }
            if loaded.is_none() {
                if let Some(g) = generator.staged() {
                    let entry = ProtoEntry::new(g.inner.generate_terrain(p.x, p.z));
                    notify(index, WorldEvent::Proto(Box::new(entry)));
                    return;
                }
            }
            let was_saved = loaded.is_some();
            let mut chunk = loaded.unwrap_or_else(|| {
                let mut chunk = generator.generate(p, min_y, height);
                chunk.dirty = storage.is_some();
                chunk.fresh = true;
                chunk
            });
            // Its entities come along (`entities/r.x.z.mca`).
            if was_saved {
                if let Some(storage) = &storage {
                    match storage.load_entities(p) {
                        Ok(entities) => {
                            chunk.entities_saved = !entities.is_empty();
                            chunk.entity_nbt = entities;
                        }
                        Err(e) => log::error!("Could not load the entities of chunk {},{}: {e:#}", p.x, p.z),
                    }
                }
            }
            notify(index, WorldEvent::Terrain(Box::new(chunk)));
        });
    }

    /// Works towards finishing chunk `q` with vanilla generation: it and its eight
    /// neighbours must be decorated, which needs their neighbours' terrain in turn.
    fn advance_generation(&mut self, q: ChunkPos, ctx: &WorkerContext) {
        if self.chunks.contains_key(&q) || self.finalizing.contains(&q) {
            return;
        }
        let mut all_decorated = true;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let r = ChunkPos::new(q.x + dx, q.z + dz);
                // Finished chunks were decorated.
                if self.chunks.contains_key(&r) || self.finalizing.contains(&r) {
                    continue;
                }
                match self.proto.get(&r) {
                    Some(entry) if entry.decorated => {}
                    Some(_) => {
                        all_decorated = false;
                        self.try_decorate(r, ctx);
                    }
                    None => {
                        all_decorated = false;
                        self.request_chunk(r, ctx);
                    }
                }
            }
        }
        if all_decorated && self.proto.contains_key(&q) {
            self.finalize(q, ctx);
        }
    }

    /// Starts decorating chunk `r` once its eight neighbours have terrain and no other
    /// decoration job holds any of them.
    fn try_decorate(&mut self, r: ChunkPos, ctx: &WorkerContext) {
        if self.locked.contains(&r) {
            return;
        }
        let mut ready = true;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let s = ChunkPos::new(r.x + dx, r.z + dz);
                if self.proto.contains_key(&s) || self.chunks.contains_key(&s) {
                    continue;
                }
                ready = false;
                self.request_chunk(s, ctx);
            }
        }
        if !ready {
            return;
        }
        let Some(generator) = self.generator.staged() else { return };
        let mut entries = Vec::with_capacity(9);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let s = ChunkPos::new(r.x + dx, r.z + dz);
                match self.proto.remove(&s) {
                    Some(entry) => {
                        self.locked.insert(s);
                        entries.push(entry);
                    }
                    // A finished neighbour (e.g. next to a chunk deleted from a saved world):
                    // features read it but cannot change it.
                    None => entries.push(ProtoEntry::borrowed(&self.chunks[&s], generator)),
                }
            }
        }
        self.decorating.insert(r);
        let (generator, index, notify) = (self.generator.clone(), self.index, ctx.notify.clone());
        ctx.pool.spawn(move || {
            let generator = generator.staged().expect("a staged generator");
            let entries = staged::decorate(generator, r, entries);
            notify(index, WorldEvent::Decorated { center: r, entries });
        });
    }

    /// Turns a chunk whose neighbours are all decorated into a finished one (on a worker).
    fn finalize(&mut self, q: ChunkPos, ctx: &WorkerContext) {
        let Some(entry) = self.proto.remove(&q) else { return };
        self.finalizing.insert(q);
        let (min_y, height, index, notify, saved) = (self.min_y, self.height, self.index, ctx.notify.clone(), self.storage.is_some());
        ctx.pool.spawn(move || {
            let mut chunk = entry.to_chunk(min_y, height, GenStatus::Full);
            chunk.fresh = true;
            chunk.dirty = saved;
            notify(index, WorldEvent::Terrain(Box::new(chunk)));
        });
    }

    /// Queues an unfinished chunk for saving.
    fn save_proto(&self, entry: &ProtoEntry) {
        let Some(storage) = &self.storage else { return };
        let status = if entry.decorated { GenStatus::Decorated } else { GenStatus::Terrain };
        let chunk = entry.to_chunk(self.min_y, self.height, status);
        storage.save_chunk(SaveChunk { snapshot: chunk.snapshot(), lit: false, has_skylight: self.has_skylight, ticks: Default::default(), status, proto: chunk.proto });
    }

    fn spawn_light_job(&mut self, pos: ChunkPos, ctx: &WorkerContext) {
        let mut grid: [[Option<Terrain>; 3]; 3] = Default::default();
        for dz in -1..=1 {
            for dx in -1..=1 {
                grid[(dz + 1) as usize][(dx + 1) as usize] = self.chunks.get(&ChunkPos::new(pos.x + dx, pos.z + dz)).map(Terrain::of);
            }
        }
        let stamp = self.terrain_stamp(pos);
        self.lighting.insert(pos, stamp);
        let (has_skylight, index, notify) = (self.has_skylight, self.index, ctx.notify.clone());
        ctx.pool.spawn(move || {
            let (sky, block) = light::compute_chunk_light(&grid, has_skylight);
            notify(index, WorldEvent::Light { pos, stamp, sky, block });
        });
    }

    /// Applies a worker result. Returns the position of a chunk that became ready.
    pub fn handle_event(&mut self, event: WorldEvent) -> Option<ChunkPos> {
        match event {
            WorldEvent::Terrain(chunk) => {
                self.generating.remove(&chunk.pos);
                self.finalizing.remove(&chunk.pos);
                // A chunk that appeared meanwhile (e.g. loaded another way) wins.
                let pos = chunk.pos;
                self.chunks.entry(pos).or_insert(*chunk);
                // Block updates: saved ticks and moving pistons continue.
                block_updates::chunk_loaded(self, pos);
                None
            }
            WorldEvent::Proto(entry) => {
                let pos = entry.pos();
                self.generating.remove(&pos);
                if !self.chunks.contains_key(&pos) && !self.locked.contains(&pos) && !self.finalizing.contains(&pos) {
                    self.proto.entry(pos).or_insert(*entry);
                }
                None
            }
            WorldEvent::Decorated { center, entries } => {
                self.decorating.remove(&center);
                for entry in entries {
                    let pos = entry.pos();
                    if entry.borrowed {
                        continue;
                    }
                    self.locked.remove(&pos);
                    // An autosave came while the job had it.
                    let saved = self.save_on_return.remove(&pos);
                    if saved {
                        self.save_proto(&entry);
                    }
                    self.proto.insert(pos, ProtoEntry { dirty: !saved, ..entry });
                }
                None
            }
            WorldEvent::Light { pos, stamp, sky, block } => {
                self.lighting.remove(&pos);
                // Discard results computed from terrain that has changed since.
                if self.terrain_stamp(pos) != stamp {
                    return None;
                }
                let chunk = self.chunks.get_mut(&pos)?;
                if chunk.lit {
                    return None;
                }
                chunk.sky_light = sky;
                chunk.block_light = block;
                chunk.lit = true;
                Some(pos)
            }
            WorldEvent::Encoded { pos, revision, frame } => {
                if self.encoding.get(&pos) == Some(&revision) {
                    self.encoding.remove(&pos);
                }
                if self.chunks.get(&pos).is_some_and(|c| c.revision == revision) {
                    self.packets.insert(pos, (revision, frame));
                }
                None
            }
        }
    }

    /// The framed chunk packet for a ready chunk. Returns `None` while it is being encoded
    /// (an encode job is started if necessary) or if the chunk is not ready.
    pub fn chunk_packet(&mut self, pos: ChunkPos, ctx: &WorkerContext) -> Option<Arc<Vec<u8>>> {
        let chunk = self.chunks.get(&pos).filter(|c| c.lit)?;
        if let Some((revision, frame)) = self.packets.get(&pos) {
            if *revision == chunk.revision {
                return Some(frame.clone());
            }
        }
        if self.encoding.get(&pos) != Some(&chunk.revision) {
            self.encoding.insert(pos, chunk.revision);
            let snapshot = chunk.snapshot();
            let (biome_count, index, notify) = (self.biome_count, self.index, ctx.notify.clone());
            let (threshold, level) = (ctx.compression_threshold, ctx.compression_level);
            ctx.pool.spawn(move || {
                let payload = encode_packet(&snapshot.to_packet(biome_count));
                match frame_packet(&payload, threshold, level) {
                    Ok(frame) => notify(index, WorldEvent::Encoded { pos, revision: snapshot.revision, frame: Arc::new(frame) }),
                    Err(e) => log::error!("Could not encode chunk {},{}: {e}", pos.x, pos.z),
                }
            });
        }
        None
    }

    /// Drops chunks nobody needs: no ticket on the chunk or any neighbour (neighbours keep
    /// terrain around for lighting). Unsaved changes are saved first; without storage, chunks
    /// with changes stay loaded.
    pub fn unload_unused(&mut self) -> usize {
        let before = self.chunks.len();
        let tickets = &self.tickets;
        let (generating, lighting) = (&self.generating, &self.lighting);
        let (storage, has_skylight, age) = (&self.storage, self.has_skylight, self.age);
        let mut unloaded: Vec<(ChunkPos, bool)> = Vec::new();
        let entity_chunks = &self.entity_chunks;
        self.chunks.retain(|pos, chunk| {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = ChunkPos::new(pos.x + dx, pos.z + dz);
                    if tickets.contains_key(&p) || lighting.contains_key(&p) || generating.contains(&p) {
                        return true;
                    }
                }
            }
            if storage.is_none() {
                // Worlds kept in memory keep edited chunks and chunks with entities; others
                // are generated again when needed.
                return chunk.dirty || entity_chunks.get(pos).is_some_and(|ids| !ids.is_empty());
            }
            if chunk.dirty {
                if let Some(storage) = storage {
                    storage.save_chunk(SaveChunk::full(chunk, has_skylight, age));
                }
            }
            unloaded.push((*pos, chunk.entities_saved));
            false
        });
        let chunks = &self.chunks;
        self.packets.retain(|pos, _| chunks.contains_key(pos));
        // Unfinished chunks stay while finishing a ticketed chunk may need them (decorating
        // its neighbours reaches three chunks out); then they are saved and dropped. Without
        // storage they stay: generating them again would lose what neighbours wrote into them.
        if self.storage.is_some() {
            let far: Vec<ChunkPos> = self.proto.keys().copied().filter(|pos| !self.near_ticket(*pos, 3)).collect();
            for pos in far {
                if let Some(entry) = self.proto.remove(&pos) {
                    if entry.dirty {
                        self.save_proto(&entry);
                    }
                }
            }
        }
        // The entities of unloaded chunks are saved and leave memory with them.
        for (pos, was_saved) in unloaded {
            let ids: Vec<i32> = self.entity_chunks.remove(&pos).map(|s| s.into_iter().collect()).unwrap_or_default();
            let saved: Vec<paper_nbt::Compound> = ids.iter().filter_map(|id| self.entities.get(id)).filter_map(entity_nbt::entity_to_nbt).collect();
            if let Some(storage) = &self.storage {
                if !saved.is_empty() || was_saved {
                    storage.save_entities(pos, saved);
                }
            }
            for id in ids {
                self.entities.remove(&id);
            }
        }
        before - self.chunks.len()
    }

    // --- blocks ------------------------------------------------------------------------

    /// Block state at a position, or `None` if its chunk is not loaded.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let chunk = self.chunks.get(&ChunkPos::of_block(x, z))?;
        Some(chunk.get_block((x & 15) as usize, y, (z & 15) as usize))
    }

    /// Sets a block and repairs lighting. Returns `None` if the chunk is not loaded, the
    /// position is outside the world, or the block already has that state.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) -> Option<BlockChange> {
        if !blocks::is_valid_state(state) {
            return None;
        }
        let pos = ChunkPos::of_block(x, z);
        let chunk = self.chunks.get_mut(&pos)?;
        let key = (y, (z & 15) as u8, (x & 15) as u8);
        let block_entity = chunk.block_entities.get(&key).cloned();
        let old = chunk.set_block((x & 15) as usize, y, (z & 15) as usize, state)?;
        if old == state {
            return None;
        }
        // A replaced block entity is handed to the server (containers drop their contents).
        if let Some(nbt) = block_entity.filter(|_| blocks::block_id_of_state(old) != blocks::block_id_of_state(state)) {
            self.removed_block_entities.push((BlockPos::new(x, y, z), old, nbt));
        }
        let light = if chunk.lit { light::update_after_block_change(&mut self.chunks, self.has_skylight, x, y, z, old, state) } else { HashSet::new() };
        Some(BlockChange { pos: BlockPos::new(x, y, z), old, new: state, light })
    }

    /// Copies the game rules block updates depend on into their settings (after loading
    /// the rules or changing one).
    pub fn sync_rule_settings(&mut self) {
        let s = &mut self.updates.settings;
        s.water_source_conversion = self.rules.bool("water_source_conversion");
        s.lava_source_conversion = self.rules.bool("lava_source_conversion");
        s.tnt_explodes = self.rules.bool("tnt_explodes");
        s.fire_spreads = self.rules.int("fire_spread_radius_around_player") != 0;
    }

    /// Whether it rains on a block (`isRainingAt`, without the biome's temperature): rain
    /// falls and nothing above stops it.
    pub fn rain_at(&self, x: i32, y: i32, z: i32) -> bool {
        self.weather.is_raining() && self.has_skylight && self.top_y(x, z).is_some_and(|top| y >= top)
    }

    /// Y of the first free block above the highest motion-blocking block of a column.
    pub fn top_y(&self, x: i32, z: i32) -> Option<i32> {
        let chunk = self.chunks.get(&ChunkPos::of_block(x, z))?;
        Some(chunk.top_y((x & 15) as usize, (z & 15) as usize))
    }

    // --- entities --------------------------------------------------------------------------

    /// Adds an entity to the world and its chunk index.
    pub fn add_entity(&mut self, e: entity::Entity) {
        self.entity_chunks.entry(e.chunk).or_default().insert(e.id);
        self.entities.insert(e.id, e);
    }

    /// Removes an entity; returns it.
    pub fn remove_entity(&mut self, id: i32) -> Option<entity::Entity> {
        let e = self.entities.remove(&id)?;
        if let Some(set) = self.entity_chunks.get_mut(&e.chunk) {
            set.remove(&id);
            if set.is_empty() {
                self.entity_chunks.remove(&e.chunk);
            }
        }
        Some(e)
    }

    /// Re-files an entity under the chunk it is in now.
    pub fn reindex_entity(&mut self, id: i32) {
        let Some(e) = self.entities.get_mut(&id) else { return };
        let now = e.pos.chunk();
        if now == e.chunk {
            return;
        }
        let old = std::mem::replace(&mut e.chunk, now);
        if let Some(set) = self.entity_chunks.get_mut(&old) {
            set.remove(&id);
            if set.is_empty() {
                self.entity_chunks.remove(&old);
            }
        }
        self.entity_chunks.entry(now).or_default().insert(id);
    }

    /// Ids of entities whose box intersects `area`.
    pub fn entities_in(&self, area: &entity::Aabb) -> Vec<i32> {
        let (c0, c1) = (entity::Vec3::new(area.min.x - 2.0, 0.0, area.min.z - 2.0).chunk(), entity::Vec3::new(area.max.x + 2.0, 0.0, area.max.z + 2.0).chunk());
        let mut out = Vec::new();
        for cx in c0.x..=c1.x {
            for cz in c0.z..=c1.z {
                if let Some(ids) = self.entity_chunks.get(&ChunkPos::new(cx, cz)) {
                    for id in ids {
                        if self.entities.get(id).is_some_and(|e| !e.removed && e.bounding_box().intersects(area)) {
                            out.push(*id);
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// Whether a ticket is held within `radius` chunks of `pos`.
    fn near_ticket(&self, pos: ChunkPos, radius: i32) -> bool {
        (-radius..=radius).any(|dz| (-radius..=radius).any(|dx| self.tickets.contains_key(&ChunkPos::new(pos.x + dx, pos.z + dz))))
    }

    /// Queues every changed chunk for saving. Returns how many were queued.
    pub fn save_chunks(&mut self) -> usize {
        let Some(storage) = &self.storage else { return 0 };
        let mut saved = 0;
        for chunk in self.chunks.values_mut().filter(|c| c.dirty) {
            storage.save_chunk(SaveChunk::full(chunk, self.has_skylight, self.age));
            chunk.dirty = false;
            saved += 1;
        }
        // Unfinished chunks too; those a decoration job holds are saved when it is done.
        let dirty: Vec<ChunkPos> = self.proto.iter().filter(|(_, e)| e.dirty).map(|(p, _)| *p).collect();
        for pos in dirty {
            if let Some(entry) = self.proto.get(&pos) {
                self.save_proto(entry);
            }
            if let Some(entry) = self.proto.get_mut(&pos) {
                entry.dirty = false;
            }
            saved += 1;
        }
        self.save_on_return.extend(self.locked.iter().copied());
        let Some(storage) = &self.storage else { return saved };
        // Entities move all the time: every loaded chunk that has (or had) some is written.
        for (pos, chunk) in self.chunks.iter_mut() {
            let list: Vec<paper_nbt::Compound> = self.entity_chunks.get(pos).map(|ids| ids.iter().filter_map(|id| self.entities.get(id)).filter_map(entity_nbt::entity_to_nbt).collect()).unwrap_or_default();
            if list.is_empty() && !chunk.entities_saved {
                continue;
            }
            chunk.entities_saved = !list.is_empty();
            storage.save_entities(*pos, list);
        }
        saved
    }

    /// What `level.dat` records about this world.
    pub fn level_data(&self, game_type: i32, hardcore: bool) -> LevelData {
        LevelData {
            name: self.name.clone(),
            seed: self.seed,
            generator: self.generator.name().to_owned(),
            spawn: (self.spawn.x, self.spawn.y, self.spawn.z),
            spawn_yaw: self.spawn_yaw,
            age: self.age,
            day_time: self.day_time,
            daylight_cycle: self.daylight_cycle,
            game_type,
            hardcore,
            weather: self.weather.clone(),
        }
    }

    /// Restores spawn and time from `level.dat`.
    pub fn apply_level(&mut self, level: &LevelData) {
        self.spawn = BlockPos::new(level.spawn.0, level.spawn.1, level.spawn.2);
        self.spawn_yaw = level.spawn_yaw;
        self.age = level.age;
        self.day_time = level.day_time;
        self.daylight_cycle = level.daylight_cycle;
        self.weather = level.weather.clone();
    }

    /// Advances world time by one tick.
    pub fn tick_time(&mut self) {
        self.age += 1;
        if self.daylight_cycle {
            self.day_time += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use generator::FlatGenerator;
    use std::sync::mpsc;

    fn context() -> (WorkerContext, mpsc::Receiver<(usize, WorldEvent)>) {
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let ctx = WorkerContext {
            pool: Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()),
            notify: Arc::new(move |index, event| {
                let _ = tx.lock().unwrap().send((index, event));
            }),
            compression_threshold: Some(256),
            compression_level: 4,
        };
        (ctx, rx)
    }

    fn drive(world: &mut World, ctx: &WorkerContext, rx: &mpsc::Receiver<(usize, WorldEvent)>, until: impl Fn(&World) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !until(world) {
            assert!(std::time::Instant::now() < deadline, "pipeline did not finish");
            world.pump(ctx);
            if let Ok((_, event)) = rx.recv_timeout(std::time::Duration::from_millis(20)) {
                world.handle_event(event);
            }
        }
    }

    #[test]
    fn pipeline_loads_lights_and_encodes() {
        let (ctx, rx) = context();
        let mut world = World::new(0, "world", "minecraft:overworld", 1, Arc::new(FlatGenerator::new(-64)));
        assert_eq!(world.min_y, -64);
        assert_eq!(world.height, 384);
        assert_eq!(world.spawn.y, -60, "one above the grass layer");

        let pos = ChunkPos::new(2, -3);
        world.add_ticket(pos);
        assert!(!world.is_ready(pos));
        drive(&mut world, &ctx, &rx, |w| w.is_ready(pos));
        // The chunk plus its eight terrain-only neighbours.
        assert_eq!(world.loaded_chunk_count(), 9);
        let grass = blocks::default_state("minecraft:grass_block").unwrap();
        assert_eq!(world.block_at(2 * 16 + 5, -61, -3 * 16 + 7), Some(grass));
        assert_eq!(world.block_at(2 * 16 + 5, -60, -3 * 16 + 7), Some(blocks::AIR));
        assert_eq!(world.top_y(2 * 16, -3 * 16), Some(-60));
        let chunk = &world.chunks[&pos];
        assert_eq!(chunk.sky_light_at(0, -60, 0), 15);
        assert_eq!(chunk.sky_light_at(0, -61, 0), 0);

        // Packet encoding happens on a worker and is cached per revision.
        assert!(world.chunk_packet(pos, &ctx).is_none());
        let mut frame = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while frame.is_none() {
            assert!(std::time::Instant::now() < deadline);
            if let Ok((_, event)) = rx.recv_timeout(std::time::Duration::from_millis(20)) {
                world.handle_event(event);
            }
            frame = world.chunk_packet(pos, &ctx);
        }
        let first = frame.unwrap();
        assert!(Arc::ptr_eq(&first, &world.chunk_packet(pos, &ctx).unwrap()), "served from cache");

        // A block change invalidates the cached packet.
        let stone = blocks::default_state("minecraft:stone").unwrap();
        let change = world.set_block(2 * 16 + 1, -60, -3 * 16 + 1, stone).unwrap();
        assert_eq!((change.old, change.new), (blocks::AIR, stone));
        assert!(!change.light.is_empty(), "placing an opaque block changes sky light");
        assert!(world.chunk_packet(pos, &ctx).is_none());
        assert!(world.set_block(2 * 16 + 1, -60, -3 * 16 + 1, stone).is_none(), "no-op");
        assert!(world.set_block(2 * 16 + 1, 5000, -3 * 16 + 1, stone).is_none(), "outside the world");
        assert!(world.set_block(9999, 0, 9999, stone).is_none(), "unloaded chunk");

        // Without tickets, clean chunks are dropped; the edited one is kept until saved.
        world.remove_ticket(pos);
        drive(&mut world, &ctx, &rx, |w| w.jobs_in_flight() == 0);
        let unloaded = world.unload_unused();
        assert_eq!(unloaded, 8);
        assert_eq!(world.loaded_chunk_count(), 1);
    }

    #[test]
    fn noise_generator_is_deterministic_and_seamless() {
        let generator = generator::NoiseGenerator::new(42);
        let a = generator.generate(ChunkPos::new(3, 4), -64, 384);
        let b = generator.generate(ChunkPos::new(3, 4), -64, 384);
        for (sa, sb) in a.sections.iter().zip(&b.sections) {
            assert_eq!(**sa, **sb);
        }
        // Heights continue smoothly across a chunk border (no cliffs from seams).
        let east = generator.generate(ChunkPos::new(4, 4), -64, 384);
        for z in 0..16 {
            let h1 = a.world_surface[(z << 4) | 15] as i32;
            let h2 = east.world_surface[z << 4] as i32;
            assert!((h1 - h2).abs() <= 12, "seam at z={z}: {h1} vs {h2}");
        }
        // There is ground everywhere and bedrock at the bottom.
        let bedrock = blocks::default_state("minecraft:bedrock").unwrap();
        for i in 0..256 {
            assert!(a.motion_blocking[i] > 16);
        }
        assert_eq!(a.get_block(0, -64, 0), bedrock);
        // Different seeds give different terrain.
        let other = generator::NoiseGenerator::new(43).generate(ChunkPos::new(3, 4), -64, 384);
        assert!(a.sections.iter().zip(&other.sections).any(|(x, y)| **x != **y));
    }
}
