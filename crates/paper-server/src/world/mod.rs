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

pub mod block_updates;
pub mod chunk;
pub mod generator;
pub mod light;
pub mod storage;
pub mod ticks;

use chunk::{Chunk, ChunkPos, LightSection};
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
    /// Terrain for a chunk was generated.
    Terrain(Box<Chunk>),
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
    /// Network id of the world's clock in `minecraft:world_clock`, if it has one.
    pub clock: Option<i32>,
    /// Size of the synchronised biome registry.
    pub biome_count: u32,
    /// Where the world is saved; `None` keeps it in memory only.
    pub storage: Option<Arc<WorldStorage>>,
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
            clock: clock_name.and_then(|c| paper_data::synced::id("minecraft:world_clock", c)),
            biome_count: paper_data::synced::entries("minecraft:worldgen/biome").map_or(1, |e| e.len() as u32),
            storage: None,
            updates: block_updates::BlockUpdateState::new(dimension, seed),
            tickets: HashMap::new(),
            wanted: VecDeque::new(),
            generating: HashSet::new(),
            lighting: HashMap::new(),
            packets: HashMap::new(),
            encoding: HashMap::new(),
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
        self.generating.len() + self.lighting.len() + self.encoding.len()
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
        let mut remaining = self.wanted.len();
        while remaining > 0 && self.generating.len() + self.lighting.len() < limit {
            remaining -= 1;
            let Some(pos) = self.wanted.pop_front() else { break };
            if !self.tickets.contains_key(&pos) || self.is_ready(pos) {
                continue;
            }
            // Terrain for the chunk and its eight neighbours first.
            let mut terrain_ready = true;
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = ChunkPos::new(pos.x + dx, pos.z + dz);
                    if self.chunks.contains_key(&p) {
                        continue;
                    }
                    terrain_ready = false;
                    if self.generating.insert(p) {
                        let (generator, min_y, height, index, notify) = (self.generator.clone(), self.min_y, self.height, self.index, ctx.notify.clone());
                        let (storage, has_skylight) = (self.storage.clone(), self.has_skylight);
                        ctx.pool.spawn(move || {
                            // Saved chunks are loaded; missing ones are generated (and saved later).
                            let loaded = storage.as_ref().and_then(|s| match s.load_chunk(p, min_y, height, has_skylight) {
                                Ok(chunk) => chunk,
                                Err(e) => {
                                    log::error!("Could not load chunk {},{}; generating it again: {e:#}", p.x, p.z);
                                    None
                                }
                            });
                            let chunk = loaded.unwrap_or_else(|| {
                                let mut chunk = generator.generate(p, min_y, height);
                                chunk.dirty = storage.is_some();
                                chunk
                            });
                            notify(index, WorldEvent::Terrain(Box::new(chunk)));
                        });
                    }
                }
            }
            if terrain_ready && !self.lighting.contains_key(&pos) {
                self.spawn_light_job(pos, ctx);
            }
            self.wanted.push_back(pos);
        }
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
                // A chunk that appeared meanwhile (e.g. loaded another way) wins.
                let chunk = self.chunks.entry(chunk.pos).or_insert(*chunk);
                // Block updates: ticks saved with the chunk count from now.
                chunk.ticks.unpack(self.age);
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
        self.chunks.retain(|pos, chunk| {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = ChunkPos::new(pos.x + dx, pos.z + dz);
                    if tickets.contains_key(&p) || lighting.contains_key(&p) || generating.contains(&p) {
                        return true;
                    }
                }
            }
            if chunk.dirty {
                match storage {
                    Some(storage) => storage.save_chunk(SaveChunk { snapshot: chunk.snapshot(), lit: chunk.lit, has_skylight, ticks: chunk.ticks.save(age) }),
                    None => return true,
                }
            }
            false
        });
        let chunks = &self.chunks;
        self.packets.retain(|pos, _| chunks.contains_key(pos));
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
        let old = chunk.set_block((x & 15) as usize, y, (z & 15) as usize, state)?;
        if old == state {
            return None;
        }
        let light = if chunk.lit { light::update_after_block_change(&mut self.chunks, self.has_skylight, x, y, z, old, state) } else { HashSet::new() };
        Some(BlockChange { pos: BlockPos::new(x, y, z), old, new: state, light })
    }

    /// Y of the first free block above the highest motion-blocking block of a column.
    pub fn top_y(&self, x: i32, z: i32) -> Option<i32> {
        let chunk = self.chunks.get(&ChunkPos::of_block(x, z))?;
        Some(chunk.top_y((x & 15) as usize, (z & 15) as usize))
    }

    /// Queues every changed chunk for saving. Returns how many were queued.
    pub fn save_chunks(&mut self) -> usize {
        let Some(storage) = &self.storage else { return 0 };
        let mut saved = 0;
        for chunk in self.chunks.values_mut().filter(|c| c.dirty) {
            storage.save_chunk(SaveChunk { snapshot: chunk.snapshot(), lit: chunk.lit, has_skylight: self.has_skylight, ticks: chunk.ticks.save(self.age) });
            chunk.dirty = false;
            saved += 1;
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
        }
    }

    /// Restores spawn and time from `level.dat`.
    pub fn apply_level(&mut self, level: &LevelData) {
        self.spawn = BlockPos::new(level.spawn.0, level.spawn.1, level.spawn.2);
        self.spawn_yaw = level.spawn_yaw;
        self.age = level.age;
        self.day_time = level.day_time;
        self.daylight_cycle = level.daylight_cycle;
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
