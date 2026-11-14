//! The game server proper: everything that runs on the main thread.
//!
//! ## Threading model
//!
//! * **I/O threads** (tokio): one task per connection; framing, compression, encryption,
//!   packet decoding, the login handshake.
//! * **Main thread** (this module): owns all game state. It runs the 20 Hz tick loop and,
//!   between ticks, handles [`MainTask`]s as they arrive: decoded packets, joins, results
//!   of worker jobs, console commands and calls from the plugin host. Plugins observe a
//!   single-threaded world, exactly as on Paper.
//! * **Worker pool** (rayon): chunk generation, lighting, chunk packet encoding and
//!   compression. Jobs are pure functions over snapshots and report back with a task.

pub mod block_updates;
mod chat;
pub mod commands;
mod handlers;
mod join;
mod combat;
mod beds;
mod machines;
mod mining;
pub mod containers;
pub mod attributes;
pub mod effects;
pub mod enchanting;
pub mod entities;
pub mod loot;
pub mod menu;
pub mod mobs;
pub mod player;
pub(crate) mod playerdata;
pub mod portals;
pub mod recipes;
pub mod scoreboard;
mod animals;
pub mod endermen;
pub mod riding;
pub mod taming;
pub mod throwing;
pub mod crafter;
pub mod golems;
pub mod score_commands;
pub mod stats;
pub mod dragon;
mod weather;
mod farming;
pub mod brewing;
mod spawners;
pub mod workstations;
pub mod survival;
pub mod usercache;
pub mod villagers;
mod view;

pub use join::info_entry as join_info_entry;

use crate::access::AccessLists;
use crate::config::{Config, GameMode};
use crate::net::{auth, ConfigPackets, PlayerConn, Shared, StatusSnapshot};
use crate::world::generator::{FlatGenerator, Generator, NoiseGenerator};
use crate::world::storage::{ChunkIo, WorldStorage};
use crate::world::{WorkerContext, World, WorldEvent};
use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use paper_protocol::codec::Encode;
use paper_protocol::packets::common::ClientSettings;
use paper_protocol::packets::{encode_packet, play, Packet};
use paper_protocol::text::Component;
use paper_protocol::types::GameProfile;
use parking_lot::RwLock;
use player::Player;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Length of one tick.
pub const TICK: Duration = Duration::from_millis(50);

/// A player that finished configuration and is ready to enter the world.
pub struct JoinRequest {
    pub entity_id: i32,
    pub profile: GameProfile,
    pub settings: ClientSettings,
    pub brand: Option<String>,
    pub virtual_host: String,
    pub conn: PlayerConn,
}

/// Work for the main thread.
pub enum MainTask {
    Join(Box<JoinRequest>),
    Packet { entity_id: i32, packet: play::Serverbound },
    Disconnected { entity_id: i32, reason: String },
    World { world: usize, event: WorldEvent },
    ConsoleCommand(String),
    /// Runs an arbitrary closure on the main thread.
    Run(Box<dyn FnOnce(&mut Server) + Send>),
    /// The plugin host made a call that is part of a nested call chain.
    PluginWake,
    /// The plugin host process went away.
    PluginHostDied(String),
    Shutdown,
}

/// Rolling tick statistics for `/tps` and `/mspt`.
pub struct TickStats {
    /// Durations of the most recent ticks in milliseconds.
    recent: std::collections::VecDeque<f32>,
    /// Start times of recent ticks, for measuring the achieved tick rate.
    starts: std::collections::VecDeque<Instant>,
}

impl TickStats {
    const WINDOW: usize = 20 * 60;

    fn new() -> Self {
        Self { recent: Default::default(), starts: Default::default() }
    }

    fn record(&mut self, start: Instant, duration: Duration) {
        self.recent.push_back(duration.as_secs_f32() * 1000.0);
        self.starts.push_back(start);
        while self.recent.len() > Self::WINDOW {
            self.recent.pop_front();
        }
        while self.starts.len() > Self::WINDOW + 1 {
            self.starts.pop_front();
        }
    }

    /// Average tick duration in milliseconds over the last `ticks` ticks.
    pub fn mspt(&self, ticks: usize) -> f32 {
        let n = ticks.min(self.recent.len());
        if n == 0 {
            return 0.0;
        }
        self.recent.iter().rev().take(n).sum::<f32>() / n as f32
    }

    /// The longest of the last `ticks` ticks, in milliseconds.
    pub fn worst(&self, ticks: usize) -> f32 {
        self.recent.iter().rev().take(ticks).copied().fold(0.0, f32::max)
    }

    /// Ticks per second achieved over the last `ticks` ticks, capped at 20.
    pub fn tps(&self, ticks: usize) -> f32 {
        let n = ticks.min(self.starts.len().saturating_sub(1));
        if n == 0 {
            return 20.0;
        }
        let newest = self.starts[self.starts.len() - 1];
        let oldest = self.starts[self.starts.len() - 1 - n];
        let seconds = newest.duration_since(oldest).as_secs_f32();
        if seconds <= 0.0 {
            return 20.0;
        }
        (n as f32 / seconds).min(20.0)
    }
}

pub struct Server {
    pub config: Arc<Config>,
    pub shared: Arc<Shared>,
    pub worlds: Vec<World>,
    pub players: HashMap<i32, Player>,
    /// Entity ids in join order.
    pub player_order: Vec<i32>,
    pub by_uuid: HashMap<Uuid, i32>,
    /// Ticks since startup.
    pub tick: u64,
    pub running: bool,
    pub stats: TickStats,
    pub commands: commands::CommandRegistry,
    pub default_game_mode: GameMode,
    pub difficulty: crate::config::Difficulty,
    pub start_time: Instant,
    /// The plugin host, when Java plugins are enabled and it is running.
    pub plugins: Option<Arc<crate::plugins::HostConnection>>,
    /// Command labels (lower case, with and without namespace) owned by plugins.
    pub plugin_commands: HashSet<String>,
    /// Bukkit event classes (simple names) that have plugin listeners.
    pub plugin_events: HashSet<String>,
    pub plugin_list: Vec<crate::plugins::PluginInfo>,
    /// The last player state sent to the plugin host, per entity id.
    pub plugin_state_cache: HashMap<i32, paper_bridge::Value>,
    // Paper-RS plugin API state (custom inventories, the launcher kept for `/reload`, ...),
    // scoreboards and the name cache; see `plugins::PluginData`, `scoreboard` and `usercache`.
    pub plugin_data: crate::plugins::PluginData,
    pub scoreboards: scoreboard::Scoreboards,
    pub user_cache: usercache::UserCache,
    workers: WorkerContext,
    /// Writes world data in the background; `None` when worlds are kept in memory only.
    io: Option<ChunkIo>,
    /// Vanilla's data files (loot tables, recipes, ...), when the server jar is available.
    pub vanilla: Option<Arc<crate::vanilla::VanillaData>>,
    pub loot: loot::LootTables,
    /// Villager and wandering trader trades from the vanilla data.
    pub trades: villagers::Trades,
    /// Crafting and smelting recipes from the vanilla data.
    pub recipes: Option<Arc<recipes::Recipes>>,
    /// `minecraft:damage_type` entries (death messages, exhaustion, difficulty scaling).
    pub damage_types: HashMap<String, survival::DamageTypeInfo>,
    /// Biome spawner lists by `biome|category`, read from the vanilla data on demand.
    pub spawner_cache: HashMap<String, Vec<mobs::SpawnerEntry>>,
    /// What each brewing stand started brewing with (vanilla keeps it only in memory too).
    pub brewing_ingredients: HashMap<(usize, paper_protocol::codec::BlockPos), i32>,
    /// End gateways cooling down after a teleport: (world, gateway) -> tick it was used.
    pub gateway_cooldowns: HashMap<(usize, paper_protocol::codec::BlockPos), u64>,
    tasks: Receiver<MainTask>,
    status_dirty: bool,
}

/// A handle for controlling a running server from other threads.
#[derive(Clone)]
pub struct ServerHandle {
    pub shared: Arc<Shared>,
}

impl ServerHandle {
    /// The port the server actually listens on.
    pub fn port(&self) -> u16 {
        self.shared.bound_port.load(Ordering::Relaxed) as u16
    }

    /// Queues a command as if typed at the console.
    pub fn console_command(&self, line: &str) {
        let _ = self.shared.to_main.send(MainTask::ConsoleCommand(line.to_owned()));
    }

    /// Asks the server to stop.
    pub fn shutdown(&self) {
        let _ = self.shared.to_main.send(MainTask::Shutdown);
    }

    /// Runs `f` on the main thread and returns its result.
    pub fn run<T: Send + 'static>(&self, f: impl FnOnce(&mut Server) -> T + Send + 'static) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.shared
            .to_main
            .send(MainTask::Run(Box::new(move |server| {
                let _ = tx.send(f(server));
            })))
            .ok()?;
        rx.recv().ok()
    }
}

fn load_favicon(config: &Config) -> Option<String> {
    use base64::Engine;
    let bytes = std::fs::read(config.root.join("server-icon.png")).ok()?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

impl Server {
    /// Creates the server state: worlds, shared state for the I/O threads, the worker pool.
    pub fn new(config: Config) -> Result<(Self, ServerHandle)> {
        let config = Arc::new(config);
        let (to_main, tasks) = crossbeam_channel::unbounded();
        let props = &config.properties;

        let keys = if props.online_mode { Some(auth::ServerKeys::generate()?) } else { None };
        let access = if config.root.as_os_str().is_empty() { AccessLists::in_memory() } else { AccessLists::load(&config.root) };
        let status = StatusSnapshot { motd: Component::from_legacy(&props.motd), online: 0, max: props.max_players, sample: Vec::new(), favicon: load_favicon(&config) };
        let shared = Arc::new(Shared {
            config: config.clone(),
            to_main: to_main.clone(),
            status: RwLock::new(Arc::new(status)),
            next_entity_id: AtomicI32::new(1),
            keys,
            config_packets: ConfigPackets::build(),
            access: RwLock::new(access),
            player_count: AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
            bound_port: AtomicI32::new(0),
            plugin_host: RwLock::new(None),
        });

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(config.worker_threads())
            .thread_name(|i| format!("Worker-{i}"))
            .build()
            .context("creating the worker pool")?;
        let notify_tx = to_main.clone();
        let workers = WorkerContext {
            pool: Arc::new(pool),
            notify: Arc::new(move |world, event| {
                let _ = notify_tx.send(MainTask::World { world, event });
            }),
            compression_threshold: config.compression_threshold(),
            compression_level: config.paper.network.compression_level,
        };

        let vanilla = match crate::vanilla::VanillaData::locate(&config.paper.world.vanilla_jar, &config.root, config.paper.world.download_vanilla_jar) {
            Ok(data) => {
                log::info!("Using game data from {}", data.path().display());
                Some(data)
            }
            Err(e) => {
                log::warn!("Vanilla game data is unavailable ({e:#}): blocks drop themselves, and recipes and mob drops are missing");
                None
            }
        };

        // Worlds are saved under the server directory; tests without one keep them in memory.
        let io = if config.root.as_os_str().is_empty() { None } else { Some(ChunkIo::start()) };
        let storage = match &io {
            Some(io) => Some(WorldStorage::open(&config.root.join(&props.level_name), io.clone())?),
            None => None,
        };
        let level = match &storage {
            Some(storage) => storage.read_level().with_context(|| format!("reading {}", storage.dir.join("level.dat").display()))?,
            None => None,
        };
        // An existing world keeps its seed and generator, whatever the configuration says now.
        let seed = level.as_ref().map_or_else(|| props.seed(), |l| l.seed);
        let overworld_min_y = paper_data::synced::dimension_type("minecraft:overworld").map_or(-64, |d| d.min_y);
        let kind = match &level {
            Some(level) => level.generator.clone(),
            None => match config.paper.world.generator.as_str() {
                "flat" | "noise" | "vanilla" => config.paper.world.generator.clone(),
                _ if props.level_type.ends_with("flat") => "flat".to_owned(),
                _ => "vanilla".to_owned(),
            },
        };
        let generator = Self::make_generator(vanilla.as_deref(), "minecraft:overworld", &kind, seed, overworld_min_y, level.is_some(), config.properties.generate_structures)?;
        let new_world = level.is_none();
        let mut worlds = vec![Self::open_world(0, &props.level_name, "minecraft:overworld", seed, generator.clone(), storage, level.as_ref(), vanilla.clone())?];
        if let (true, Some(vanilla)) = (new_world, generator.staged()) {
            let (x, y, z) = vanilla.find_spawn();
            worlds[0].spawn = paper_protocol::codec::BlockPos::new(x, y, z);
            log::info!("World spawn at {x}, {y}, {z}");
        }
        // The Nether and the End live in their own folders, as in CraftBukkit, and share the
        // overworld's seed.
        let mut dimensions: Vec<(&str, &str, &str)> = Vec::new();
        if props.allow_nether {
            dimensions.push(("_nether", "minecraft:the_nether", "DIM-1"));
        }
        if config.paper.world.allow_end {
            dimensions.push(("_the_end", "minecraft:the_end", "DIM1"));
        }
        for (suffix, dimension, subdir) in dimensions {
            let name = format!("{}{}", props.level_name, suffix);
            let storage = match &io {
                Some(io) => Some(WorldStorage::open_dimension(&config.root.join(&name), subdir, io.clone())?),
                None => None,
            };
            let level = match &storage {
                Some(storage) => storage.read_level().with_context(|| format!("reading {}", storage.dir.join("level.dat").display()))?,
                None => None,
            };
            let kind = level.as_ref().map_or("vanilla", |l| l.generator.as_str()).to_owned();
            let min_y = paper_data::synced::dimension_type(dimension).map_or(0, |d| d.min_y);
            let generator = Self::make_generator(vanilla.as_deref(), dimension, &kind, seed, min_y, level.is_some(), config.properties.generate_structures)?;
            let index = worlds.len();
            let mut world = Self::open_world(index, &name, dimension, seed, generator, storage, level.as_ref(), vanilla.clone())?;
            if level.is_none() {
                // Where portals and /tp put arrivals without a better place.
                world.spawn = match dimension {
                    "minecraft:the_end" => paper_protocol::codec::BlockPos::new(100, 49, 0),
                    _ => paper_protocol::codec::BlockPos::new(0, 64, 0),
                };
            }
            worlds.push(world);
        }

        let user_cache = usercache::UserCache::load(&config.root);
        let mut server = Self {
            default_game_mode: props.gamemode,
            difficulty: props.difficulty,
            config,
            shared: shared.clone(),
            worlds,
            players: HashMap::new(),
            player_order: Vec::new(),
            by_uuid: HashMap::new(),
            tick: 0,
            running: true,
            stats: TickStats::new(),
            commands: commands::CommandRegistry::with_builtins(),
            start_time: Instant::now(),
            plugins: None,
            plugin_commands: HashSet::new(),
            plugin_events: HashSet::new(),
            plugin_list: Vec::new(),
            plugin_state_cache: HashMap::new(),
            // Plugin API state, scoreboards and the name cache (see the struct fields).
            plugin_data: Default::default(),
            scoreboards: scoreboard::Scoreboards::new(),
            user_cache,
            workers,
            io,
            loot: loot::LootTables::new(vanilla.clone()),
            trades: villagers::Trades::new(vanilla.clone()),
            recipes: vanilla.as_ref().map(|v| Arc::new(recipes::Recipes::load(v))),
            damage_types: survival::load_damage_types(vanilla.as_deref()),
            spawner_cache: HashMap::new(),
            brewing_ingredients: HashMap::new(),
            gateway_cooldowns: HashMap::new(),
            vanilla,
            tasks,
            status_dirty: true,
        };
        // The main scoreboard lives with the main world.
        let saved_board = server.worlds.first().and_then(|w| w.storage.as_ref()).map(|s| s.read_scoreboard());
        match saved_board {
            Some(Ok(Some(root))) => server.scoreboards.set_main(scoreboard::Scoreboard::from_nbt(&root)),
            Some(Err(e)) => log::warn!("Could not read the scoreboard: {e:#}"),
            _ => {}
        }
        Ok((server, ServerHandle { shared }))
    }

    /// The generator of a world: `kind` is what its `level.dat` names (`vanilla`, `flat`, or
    /// the built-in `noise`, `nether` and `end` of worlds made before vanilla generation was
    /// ported). Vanilla generation needs the vanilla jar; new worlds fall back to the built-in
    /// generators without it, existing vanilla worlds refuse to start rather than grow
    /// mismatched terrain.
    fn make_generator(vanilla: Option<&crate::vanilla::VanillaData>, dimension: &str, kind: &str, seed: i64, min_y: i32, existing: bool, structures: bool) -> Result<Arc<dyn Generator>> {
        let built_in = |dimension: &str| -> Arc<dyn Generator> {
            match dimension {
                "minecraft:the_nether" => Arc::new(crate::world::generator::NetherGenerator::new(seed)),
                "minecraft:the_end" => Arc::new(crate::world::generator::EndGenerator::new(seed)),
                _ => Arc::new(NoiseGenerator::new(seed)),
            }
        };
        Ok(match kind {
            "flat" => Arc::new(FlatGenerator::new(min_y)),
            "noise" | "nether" | "end" => built_in(dimension),
            _ => {
                let made = vanilla.ok_or_else(|| anyhow!("the vanilla server jar is not available")).and_then(|v| crate::world::staged::VanillaGenerator::new(v, dimension, seed, structures));
                match made {
                    Ok(generator) => Arc::new(generator),
                    Err(e) if existing => bail!("{dimension} uses vanilla world generation, which needs the vanilla server jar's data: {e:#}"),
                    Err(e) => {
                        let fallback = built_in(dimension);
                        log::warn!("Vanilla world generation is unavailable ({e:#}); {dimension} uses the built-in '{}' generator", fallback.name());
                        fallback
                    }
                }
            }
        })
    }

    /// A world with its saved level data and game rules applied.
    #[allow(clippy::too_many_arguments)]
    fn open_world(index: usize, name: &str, dimension: &str, seed: i64, generator: Arc<dyn Generator>, storage: Option<Arc<WorldStorage>>, level: Option<&crate::world::storage::LevelData>, vanilla: Option<Arc<crate::vanilla::VanillaData>>) -> Result<World> {
        let mut world = World::new(index, name, dimension, seed, generator);
        world.vanilla = vanilla;
        if let Some(level) = level {
            world.apply_level(level);
            log::info!("Loaded world '{}' (seed {}, {} terrain)", world.name, seed, world.generator.name());
        }
        if let Some(storage) = &storage {
            if let Some(rules) = storage.read_game_rules().context("reading game rules")? {
                world.rules = rules;
            }
        }
        world.daylight_cycle = world.rules.bool("advance_time");
        world.sync_rule_settings();
        world.storage = storage;
        Ok(world)
    }

    /// A world a plugin asks for (`Bukkit.createWorld`): the loaded world of that name, the
    /// world in that folder under the server directory (with the seed and generator it was
    /// made with), or a new one. `environment` is Bukkit's (`NORMAL`, `NETHER`, `THE_END`),
    /// `kind` its world type (`NORMAL` or `FLAT`). Answers the world's index.
    pub fn create_world(&mut self, name: &str, environment: &str, seed: Option<i64>, kind: &str, structures: bool) -> Result<usize> {
        if let Some(index) = self.worlds.iter().position(|w| w.name == name) {
            return Ok(index);
        }
        if name.is_empty() || name.contains(['/', '\\', ':']) || name.starts_with('.') {
            bail!("invalid world name {name:?}");
        }
        let (dimension, subdir) = match environment {
            "NETHER" => ("minecraft:the_nether", Some("DIM-1")),
            "THE_END" => ("minecraft:the_end", Some("DIM1")),
            "NORMAL" => ("minecraft:overworld", None),
            other => bail!("{other} worlds are not supported by Paper-RS yet"),
        };
        let generator_kind = match kind {
            "NORMAL" => "vanilla",
            "FLAT" => "flat",
            other => bail!("{other} worlds are not supported by Paper-RS yet"),
        };
        let io = self.worlds.first().and_then(|w| w.storage.as_ref()).map(|s| s.io().clone());
        let storage = match io {
            Some(io) => {
                let dir = self.config.root.join(name);
                Some(match subdir {
                    Some(sub) => WorldStorage::open_dimension(&dir, sub, io)?,
                    None => WorldStorage::open(&dir, io)?,
                })
            }
            None => None,
        };
        let level = match &storage {
            Some(storage) => storage.read_level().with_context(|| format!("reading {}", storage.dir.join("level.dat").display()))?,
            None => None,
        };
        let seed = level.as_ref().map(|l| l.seed).or(seed).unwrap_or_else(rand::random);
        let kind = level.as_ref().map_or(generator_kind, |l| l.generator.as_str()).to_owned();
        let vanilla = self.worlds.first().and_then(|w| w.vanilla.clone());
        let min_y = paper_data::synced::dimension_type(dimension).map_or(0, |d| d.min_y);
        let generator = Self::make_generator(vanilla.as_deref(), dimension, &kind, seed, min_y, level.is_some(), structures)?;
        let index = self.worlds.len();
        let mut world = Self::open_world(index, name, dimension, seed, generator.clone(), storage, level.as_ref(), vanilla)?;
        if level.is_none() {
            match (dimension, generator.staged()) {
                ("minecraft:overworld", Some(vanilla)) => {
                    let (x, y, z) = vanilla.find_spawn();
                    world.spawn = paper_protocol::codec::BlockPos::new(x, y, z);
                }
                ("minecraft:the_end", _) => world.spawn = paper_protocol::codec::BlockPos::new(100, 49, 0),
                ("minecraft:the_nether", _) => world.spawn = paper_protocol::codec::BlockPos::new(0, 64, 0),
                _ => {}
            }
        }
        log::info!("Opened world '{name}' ({environment}, seed {seed}, {} terrain) for a plugin", world.generator.name());
        self.worlds.push(world);
        Ok(index)
    }

    /// The world of a dimension (`minecraft:the_nether`, ...), if it exists.
    pub fn world_of_dimension(&self, dimension: &str) -> Option<usize> {
        self.worlds.iter().position(|w| w.dimension == dimension)
    }

    /// Runs the tick loop on the calling thread until the server is stopped.
    pub fn run(&mut self) {
        let mut next_tick = Instant::now();
        while self.running {
            // Until the next tick is due, handle tasks the moment they arrive.
            loop {
                match self.tasks.recv_deadline(next_tick) {
                    Ok(task) => self.handle_task(task),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        self.running = false;
                        break;
                    }
                }
                if !self.running {
                    break;
                }
            }
            if !self.running {
                break;
            }
            let start = Instant::now();
            self.tick();
            self.stats.record(start, start.elapsed());

            next_tick += TICK;
            let now = Instant::now();
            if now > next_tick + Duration::from_secs(2) {
                let behind = now - next_tick;
                log::warn!("Can't keep up! Is the server overloaded? Running {}ms or {} ticks behind", behind.as_millis(), behind.as_millis() / 50);
                next_tick = now;
            }
        }
        self.shutdown();
    }

    fn handle_task(&mut self, task: MainTask) {
        match task {
            MainTask::Join(request) => self.handle_join(*request),
            MainTask::Packet { entity_id, packet } => self.handle_packet(entity_id, packet),
            MainTask::Disconnected { entity_id, reason } => self.handle_disconnect(entity_id, &reason),
            MainTask::World { world, event } => {
                let terrain = match &event {
                    WorldEvent::Terrain(chunk) => Some(chunk.pos),
                    _ => None,
                };
                let ready = self.worlds.get_mut(world).and_then(|w| w.handle_event(event));
                // Entities saved with a chunk come back to life when it is loaded.
                if let Some(pos) = terrain {
                    self.spawn_saved_entities(world, pos);
                }
                // A newly generated chunk gets its animals once it is ready.
                if let Some(pos) = ready {
                    let fresh = self.worlds[world].chunks.get_mut(&pos).is_some_and(|c| std::mem::take(&mut c.fresh));
                    if fresh {
                        self.spawn_for_chunk_generation(world, pos);
                    }
                }
            }
            MainTask::ConsoleCommand(line) => {
                if let Some(line) = crate::plugins::events::on_server_command(self, &line) {
                    self.dispatch_command(&commands::CommandSource::Console, &line);
                }
            }
            MainTask::Run(f) => f(self),
            MainTask::PluginWake => crate::plugins::service_idle(self),
            MainTask::PluginHostDied(reason) => crate::plugins::host_died(self, &reason),
            MainTask::Shutdown => self.running = false,
        }
    }

    fn tick(&mut self) {
        self.tick += 1;
        // Plugin scheduler tasks run at the start of the tick, as in Paper.
        crate::plugins::events::tick(self);

        let workers = self.workers.clone();
        for world in &mut self.worlds {
            world.tick_time();
            world.pump(&workers);
        }
        // Rain, thunder and what they do (see `weather`).
        self.tick_weather();
        // Scheduled block and fluid ticks and block events (see `block_updates`).
        self.tick_block_updates();
        // Clients run their own clock; resynchronise once a second like vanilla.
        if self.tick % 20 == 0 {
            for index in 0..self.worlds.len() {
                let payload = self.time_payload(index);
                self.broadcast_payload_in_world(index, payload);
            }
        }

        self.tick_keep_alive();
        self.tick_chunk_views();
        self.tick_players();
        self.tick_effects();
        self.tick_digging();
        self.tick_sleeping();
        self.tick_entities();
        self.tick_riding();
        self.tick_merchants();
        self.tick_attributes();
        self.tick_spawning();
        self.tick_furnaces();
        self.tick_spawners();
        self.tick_brewing_stands();
        self.tick_hoppers();
        self.tick_pressure_plates();
        self.tick_dragon_fights();
        self.tick_stats();
        self.tick_tracker();

        if self.tick % 100 == 0 {
            self.broadcast_latency();
        }
        if self.tick % 200 == 0 {
            for world in &mut self.worlds {
                world.unload_unused();
            }
        }
        let autosave = self.config.paper.world.autosave_interval_ticks;
        if autosave > 0 && self.tick % autosave == 0 {
            self.save_all(false);
        }
        if self.status_dirty {
            self.refresh_status();
        }
    }

    // --- helpers ---------------------------------------------------------------------------

    pub fn workers(&self) -> &WorkerContext {
        &self.workers
    }

    pub fn player(&self, entity_id: i32) -> Option<&Player> {
        self.players.get(&entity_id)
    }

    pub fn player_by_name(&self, name: &str) -> Option<&Player> {
        self.players.values().find(|p| p.name().eq_ignore_ascii_case(name))
    }

    pub fn player_by_uuid(&self, uuid: Uuid) -> Option<&Player> {
        self.by_uuid.get(&uuid).and_then(|id| self.players.get(id))
    }

    /// Online players in join order.
    pub fn online_players(&self) -> impl Iterator<Item = &Player> {
        self.player_order.iter().filter_map(|id| self.players.get(id))
    }

    /// Sends a packet to one player.
    pub fn send<P: Packet + Encode>(&self, entity_id: i32, packet: &P) {
        if let Some(player) = self.players.get(&entity_id) {
            player.conn.send(packet);
        }
    }

    /// Sends a packet to every player.
    pub fn broadcast<P: Packet + Encode>(&self, packet: &P) {
        let payload = Bytes::from(encode_packet(packet));
        for player in self.players.values() {
            player.conn.send_payload(payload.clone());
        }
    }

    /// Sends a packet to every player except one.
    pub fn broadcast_except<P: Packet + Encode>(&self, except: i32, packet: &P) {
        let payload = Bytes::from(encode_packet(packet));
        for player in self.players.values().filter(|p| p.entity_id != except) {
            player.conn.send_payload(payload.clone());
        }
    }

    pub fn broadcast_payload_in_world(&self, world: usize, payload: Bytes) {
        for player in self.players.values().filter(|p| p.world == world) {
            player.conn.send_payload(payload.clone());
        }
    }

    /// Sends a packet to every player who has the given chunk in view.
    pub fn broadcast_to_chunk_viewers<P: Packet + Encode>(&self, world: usize, chunk: crate::world::chunk::ChunkPos, packet: &P) {
        let payload = Bytes::from(encode_packet(packet));
        for player in self.players.values().filter(|p| p.world == world && p.view.sent.contains(&chunk)) {
            player.conn.send_payload(payload.clone());
        }
    }

    /// The `SetTime` payload for a world.
    fn time_payload(&self, world: usize) -> Bytes {
        let w = &self.worlds[world];
        let clocks = w.clock.map(|clock| play::ClockState { clock, time: w.day_time, fraction: 0.0, rate: if w.daylight_cycle { 1.0 } else { 0.0 } }).into_iter().collect();
        Bytes::from(encode_packet(&play::SetTime { world_age: w.age, clocks }))
    }

    /// Resends the world time to everyone in a world.
    pub fn resend_time(&self, world: usize) {
        let payload = self.time_payload(world);
        self.broadcast_payload_in_world(world, payload);
    }

    /// Moves a world's spawn point and tells its players.
    pub fn set_world_spawn(&mut self, world: usize, pos: paper_protocol::codec::BlockPos, yaw: f32) {
        let w = &mut self.worlds[world];
        w.spawn = pos;
        w.spawn_yaw = yaw;
        let packet = play::SetDefaultSpawnPosition { dimension: w.dimension.clone(), pos, yaw, pitch: 0.0 };
        let payload = Bytes::from(encode_packet(&packet));
        self.broadcast_payload_in_world(world, payload);
    }

    /// Sends every player a fresh command tree (after plugin commands changed).
    pub fn resend_command_trees(&mut self) {
        for id in self.player_order.clone() {
            let Some(level) = self.players.get(&id).map(|p| p.op_level) else { continue };
            let tree = self.command_tree_for(id, level);
            self.send(id, &tree);
        }
    }

    /// Saves players, changed chunks and `level.dat` of every world. With `flush`, waits until
    /// everything is on disk. Returns `false` if this server keeps worlds in memory only.
    pub fn save_all(&mut self, flush: bool) -> bool {
        if self.io.is_none() {
            return false;
        }
        for player in self.player_order.iter().filter_map(|id| self.players.get(id)) {
            self.save_player_data(player);
        }
        let (game_type, hardcore) = (self.default_game_mode.id(), self.config.properties.hardcore);
        if let (Some(storage), Some(board)) = (self.worlds.first().and_then(|w| w.storage.as_ref()), self.scoreboards.get(scoreboard::MAIN)) {
            if let Err(e) = storage.save_scoreboard(&board.to_nbt()) {
                log::error!("Could not save the scoreboard: {e:#}");
            }
        }
        for world in &mut self.worlds {
            let chunks = world.save_chunks();
            if let Some(storage) = &world.storage {
                if let Err(e) = storage.save_level(&world.level_data(game_type, hardcore)) {
                    log::error!("Could not save level.dat of {}: {e:#}", world.name);
                }
                if let Err(e) = storage.save_game_rules(&world.rules) {
                    log::error!("Could not save the game rules of {}: {e:#}", world.name);
                }
            }
            log::debug!("Queued {chunks} chunk(s) of '{}' for saving", world.name);
        }
        if flush {
            if let Some(io) = &self.io {
                io.flush();
            }
            for storage in self.worlds.iter().filter_map(|w| w.storage.as_ref()) {
                storage.sync();
            }
        }
        true
    }

    /// Changes a game rule of a world and applies its side effects.
    pub fn set_game_rule(&mut self, world: usize, name: &str, value: paper_data::game_rules::Value) {
        let Some(w) = self.worlds.get_mut(world) else { return };
        if !w.rules.set(name, value) {
            return;
        }
        w.sync_rule_settings();
        match paper_data::game_rules::get(name).map(|r| r.name.as_str()) {
            Some("minecraft:advance_time") => {
                w.daylight_cycle = value.as_bool();
                let payload = self.time_payload(world);
                self.broadcast_payload_in_world(world, payload);
            }
            Some("minecraft:immediate_respawn") => {
                let packet = play::GameEvent { event: play::game_event::IMMEDIATE_RESPAWN, value: if value.as_bool() { 1.0 } else { 0.0 } };
                for p in self.players.values().filter(|p| p.world == world) {
                    p.conn.send(&packet);
                }
            }
            Some("minecraft:reduced_debug_info") => {
                for p in self.players.values().filter(|p| p.world == world) {
                    p.conn.send(&play::EntityEvent { entity_id: p.entity_id, status: if value.as_bool() { 22 } else { 23 } });
                }
            }
            _ => {}
        }
    }

    pub fn mark_status_dirty(&mut self) {
        self.status_dirty = true;
    }

    fn refresh_status(&mut self) {
        self.status_dirty = false;
        let props = &self.config.properties;
        let old = self.shared.status.read().clone();
        let sample = if props.hide_online_players { Vec::new() } else { self.online_players().take(12).map(|p| (p.name().to_owned(), p.uuid())).collect() };
        let snapshot = StatusSnapshot { motd: old.motd.clone(), online: self.players.len(), max: props.max_players, sample, favicon: old.favicon.clone() };
        *self.shared.status.write() = Arc::new(snapshot);
    }

    fn shutdown(&mut self) {
        log::info!("Stopping server");
        self.shared.stopping.store(true, Ordering::Relaxed);
        // Plugins are disabled before players are disconnected, as in Paper.
        *self.shared.plugin_host.write() = None;
        crate::plugins::shutdown(self);
        let reason = Component::translatable("multiplayer.disconnect.server_shutdown", vec![]);
        let ids: Vec<i32> = self.player_order.clone();
        for id in ids {
            self.kick(id, &reason);
        }
        // Generation jobs hold unfinished chunks: wait for them, or the chunks they hold
        // (and what neighbours wrote into them) would be lost.
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        while self.worlds.iter().any(|w| w.generation_in_flight() > 0) && Instant::now() < deadline {
            if let Ok(task @ MainTask::World { .. }) = self.tasks.recv_timeout(std::time::Duration::from_millis(100)) {
                self.handle_task(task);
            }
        }
        log::info!("Saving worlds");
        for world in &self.worlds {
            log::info!("Saving chunks for level '{}'/{}", world.name, world.dimension);
        }
        if self.save_all(true) {
            log::info!("All worlds are saved");
        }
        // Release the world locks.
        for world in &mut self.worlds {
            world.storage = None;
        }
        self.io = None;
        log::logger().flush();
    }
}
