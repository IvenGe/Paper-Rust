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
//! * **Tick pool** (rayon): parts of the tick that split into independent pieces (moving
//!   mobs, searching for spawn spots, tracking entities) while the main thread waits, so the
//!   game state is still changed from one place at a time.

pub mod block_updates;
pub mod borders;
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
pub mod variants;
pub mod player;
pub(crate) mod playerdata;
pub mod portals;
pub mod recipes;
pub mod scoreboard;
pub(crate) mod animals;
pub mod endermen;
pub mod riding;
pub mod taming;
pub mod throwing;
pub mod crafter;
pub mod crossbows;
pub mod boats;
pub mod shields;
pub mod signs;

/// Suspicious sand and gravel that cannot land break without dropping (`disableDrop`).
fn falling_drops_nothing(state: u32) -> bool {
    matches!(paper_data::blocks::block_of_state(state).name, "minecraft:suspicious_sand" | "minecraft:suspicious_gravel")
}
pub mod elytra;
pub mod fireworks;
pub mod fishing;
pub mod golems;
pub mod hanging;
pub mod score_commands;
pub mod stats;
pub mod dragon;
mod weather;
mod farming;
pub mod brewing;
mod spawners;
pub mod workstations;
pub mod stations;
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
use crate::world::{FastMap, WorkerContext, World, WorldEvent};
use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError};
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

impl MainTask {
    /// What kind of work it is, for the tick profile.
    fn kind(&self) -> &'static str {
        match self {
            MainTask::Join(_) => "tasks: joins",
            MainTask::Packet { .. } => "tasks: packets",
            MainTask::Disconnected { .. } => "tasks: disconnects",
            MainTask::World { .. } => "tasks: chunk results",
            MainTask::ConsoleCommand(_) => "tasks: console",
            MainTask::Run(_) => "tasks: closures",
            MainTask::PluginWake | MainTask::PluginHostDied(_) => "tasks: plugin host",
            MainTask::Shutdown => "tasks: shutdown",
        }
    }
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

/// Time spent in each part of the tick, and on tasks between ticks (packets, finished worker
/// jobs), summed until taken (`Server::take_profile`). Two clock reads a part, so it is
/// always on.
#[derive(Clone, Debug, Default)]
pub struct TickProfile {
    pub ticks: u64,
    pub phases: Vec<(&'static str, Duration)>,
    /// The slowest tick: how long it took, and its parts that took a millisecond or more.
    pub worst: Option<(Duration, Vec<(&'static str, Duration)>)>,
    /// The parts of the tick under way.
    current: Vec<(&'static str, Duration)>,
}

impl TickProfile {
    pub fn add(&mut self, phase: &'static str, took: Duration) {
        match self.phases.iter_mut().find(|(name, _)| *name == phase) {
            Some((_, total)) => *total += took,
            None => self.phases.push((phase, took)),
        }
    }

    /// A part of the tick under way. Parts of parts are named `part/subpart`.
    pub(crate) fn add_to_tick(&mut self, phase: &'static str, took: Duration) {
        self.add(phase, took);
        if took >= Duration::from_millis(1) {
            self.current.push((phase, took));
        }
    }

    /// The tick is over: it took `took`.
    fn end_tick(&mut self, took: Duration) {
        let parts = std::mem::take(&mut self.current);
        if self.worst.as_ref().is_none_or(|(worst, _)| took > *worst) {
            self.worst = Some((took, parts));
        }
    }

    /// Milliseconds a tick, per part, most first.
    pub fn per_tick(&self) -> Vec<(&'static str, f64)> {
        let ticks = self.ticks.max(1) as f64;
        let mut out: Vec<(&'static str, f64)> = self.phases.iter().map(|(name, total)| (*name, total.as_secs_f64() * 1000.0 / ticks)).collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }
}

pub struct Server {
    pub config: Arc<Config>,
    pub shared: Arc<Shared>,
    pub worlds: Vec<World>,
    pub players: FastMap<i32, Player>,
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
    /// The events Java plugins listen to (`plugin_events` adds the Rust plugins' ones).
    pub java_events: HashSet<String>,
    /// Plugins written in Rust (see `plugins::native`).
    pub native: crate::plugins::native::NativePlugins,
    /// Per player, the last right click on a block plugins saw (`PlayerInteractEvent`): the
    /// hand, the tick and whether they denied the item's use, for the use of the item that
    /// follows the click (CraftBukkit's `firedInteract`).
    pub(crate) last_interact: HashMap<i32, (i32, u64, bool)>,
    pub plugin_list: Vec<crate::plugins::PluginInfo>,
    /// The last player state sent to the plugin host, per entity id.
    pub plugin_state_cache: HashMap<i32, paper_bridge::Value>,
    /// The fingerprint of each player's state when it was last sent (see
    /// `plugins::events::flush_states`); dropped when the player sends a packet.
    pub(crate) plugin_state_marks: FastMap<i32, u64>,
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
    /// What each biome spawns, read from the vanilla data on demand.
    pub spawner_cache: RwLock<HashMap<String, Arc<mobs::BiomeSpawns>>>,
    /// What each brewing stand started brewing with (vanilla keeps it only in memory too).
    pub brewing_ingredients: HashMap<(usize, paper_protocol::codec::BlockPos), i32>,
    /// End gateways cooling down after a teleport: (world, gateway) -> tick it was used.
    pub gateway_cooldowns: HashMap<(usize, paper_protocol::codec::BlockPos), u64>,
    /// Where the time of recent ticks went (see `TickProfile`).
    pub profile: TickProfile,
    /// Threads that share the work of a tick that can be split up; the main thread waits
    /// for them, so the game state is still only changed from one place at a time.
    pub(crate) tick_pool: Arc<rayon::ThreadPool>,
    /// Mobs that had their turn this tick and are still to move (see `mobs::move_mobs`).
    moving: Vec<i32>,
    tasks: Receiver<MainTask>,
    /// Tasks put aside while the main thread waited for chunks (`load_chunk_now`).
    /// (Behind a lock that is never taken, only reached through `get_mut`, so that the server
    /// can be shared with the tick pool: tasks can be sent, not shared.)
    deferred: parking_lot::Mutex<std::collections::VecDeque<MainTask>>,
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
            outgoing: Default::default(),
        });

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(config.worker_threads())
            .thread_name(|i| format!("Worker-{i}"))
            .build()
            .context("creating the worker pool")?;
        // The block state tables mobs and fluids look things up in, made on another thread
        // while the server starts.
        std::thread::spawn(mobs::warm_tables);
        let tick_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(config.tick_threads())
                .thread_name(|i| format!("Tick-{i}"))
                .build()
                .context("creating the tick pool")?,
        );
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
            players: FastMap::default(),
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
            java_events: HashSet::new(),
            native: Default::default(),
            last_interact: HashMap::new(),
            plugin_list: Vec::new(),
            plugin_state_cache: HashMap::new(),
            plugin_state_marks: FastMap::default(),
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
            spawner_cache: Default::default(),
            brewing_ingredients: HashMap::new(),
            gateway_cooldowns: HashMap::new(),
            profile: TickProfile::default(),
            tick_pool,
            moving: Vec::new(),
            vanilla,
            tasks,
            deferred: Default::default(),
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
            world.plugin_data = storage.read_plugin_data().context("reading the world's plugin data")?;
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
    pub fn create_world(&mut self, name: &str, environment: &str, seed: Option<i64>, kind: &str, structures: bool, key: Option<&str>) -> Result<usize> {
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
        world.key = key.map_or_else(|| format!("minecraft:{}", name.to_lowercase()), str::to_owned);
        log::info!("Opened world '{name}' ({environment}, seed {seed}, {} terrain) for a plugin", world.generator.name());
        self.worlds.push(world);
        Ok(index)
    }

    /// The world of a dimension (`minecraft:the_nether`, ...), if it exists.
    pub fn world_of_dimension(&self, dimension: &str) -> Option<usize> {
        self.worlds.iter().position(|w| w.dimension == dimension && !w.unloaded)
    }

    /// `Bukkit.unloadWorld`: a world other than the first, with no players in it, leaves the
    /// server, saved first if `save` (plugins heard `WorldUnloadEvent` already). Loading it
    /// again opens it anew from its folder. Returns whether it went.
    pub fn unload_world(&mut self, world: usize, save: bool) -> bool {
        if world == 0 || self.worlds.get(world).is_none_or(|w| w.unloaded) || self.players.values().any(|p| p.world == world) {
            return false;
        }
        if save {
            self.save_world(world, true);
        }
        let name = self.worlds[world].name.clone();
        self.worlds[world].unload();
        log::info!("Unloaded world '{name}'");
        true
    }

    /// Runs the tick loop on the calling thread until the server is stopped.
    pub fn run(&mut self) {
        let mut next_tick = Instant::now();
        while self.running {
            // Tasks put aside while waiting for chunks come first.
            while let Some(task) = self.deferred.get_mut().pop_front() {
                self.handle_task(task);
            }
            // Until the next tick is due, handle tasks the moment they arrive. What they send
            // players goes out whenever there is no task left to handle.
            loop {
                let task = match self.tasks.try_recv() {
                    Ok(task) => Ok(task),
                    Err(TryRecvError::Empty) => {
                        self.shared.outgoing.flush();
                        self.tasks.recv_deadline(next_tick)
                    }
                    Err(TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
                };
                match task {
                    Ok(task) => {
                        let started = Instant::now();
                        let kind = task.kind();
                        self.handle_task(task);
                        self.profile.add(kind, started.elapsed());
                    }
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
            self.shared.outgoing.flush();
            self.profile.end_tick(start.elapsed());
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

    /// What has been timed so far, starting over.
    pub fn take_profile(&mut self) -> TickProfile {
        std::mem::take(&mut self.profile)
    }

    /// Unloads the chunks of every world that nobody needs any more, telling plugins that
    /// listen first (`ChunkUnloadEvent`).
    pub fn unload_unused_chunks(&mut self) {
        for index in 0..self.worlds.len() {
            if crate::plugins::events::listens(self, "ChunkUnloadEvent") {
                for pos in self.worlds[index].unloadable_chunks() {
                    crate::plugins::events::on_chunk_unload(self, index, pos);
                }
            }
            self.worlds[index].unload_unused();
        }
    }

    fn handle_task(&mut self, task: MainTask) {
        match task {
            MainTask::Join(request) => self.handle_join(*request),
            MainTask::Packet { entity_id, packet } => {
                // What the packet changes is sent to the plugin host whole before it next
                // runs plugin code.
                self.plugin_state_marks.remove(&entity_id);
                self.handle_packet(entity_id, packet)
            }
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
                    crate::plugins::events::on_chunk_load(self, world, pos, fresh);
                    if fresh {
                        crate::plugins::events::on_chunk_populate(self, world, pos);
                    }
                }
            }
            MainTask::ConsoleCommand(line) => {
                if let Some(line) = crate::plugins::events::on_server_command(self, &line) {
                    // The console takes commands with or without a slash, as vanilla's does.
                    let line = line.trim();
                    self.dispatch_command(&commands::CommandSource::Console, line.strip_prefix('/').unwrap_or(line));
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
        self.profile.ticks += 1;
        // Each part of the tick, timed (see `TickProfile`).
        macro_rules! phase {
            ($name:literal, $body:expr) => {{
                let started = Instant::now();
                $body;
                self.profile.add_to_tick($name, started.elapsed());
            }};
        }
        // Plugin scheduler tasks run at the start of the tick, as in Paper.
        phase!("plugin tasks", crate::plugins::events::tick(self));

        let workers = self.workers.clone();
        phase!("chunk jobs", {
            for world in &mut self.worlds {
                world.tick_time();
                world.pump(&workers);
            }
        });
        // Rain, thunder and what they do (see `weather`).
        phase!("weather", self.tick_weather());
        // World borders moving to a new size (see `borders`).
        phase!("borders", self.tick_borders());
        // Scheduled block and fluid ticks and block events (see `block_updates`).
        phase!("block updates", self.tick_block_updates());
        // Clients run their own clock; resynchronise once a second like vanilla.
        if self.tick % 20 == 0 {
            for index in 0..self.worlds.len() {
                self.broadcast_time(index);
            }
        }

        phase!("keep alive", self.tick_keep_alive());
        phase!("chunk views", self.tick_chunk_views());
        phase!("players", self.tick_players());
        phase!("effects", self.tick_effects());
        phase!("digging", self.tick_digging());
        phase!("sleeping", self.tick_sleeping());
        phase!("entities", self.tick_entities());
        phase!("riding", self.tick_riding());
        phase!("merchants", self.tick_merchants());
        phase!("attributes", self.tick_attributes());
        phase!("spawning", self.tick_spawning());
        phase!("furnaces", self.tick_furnaces());
        phase!("spawners", self.tick_spawners());
        phase!("brewing", self.tick_brewing_stands());
        phase!("hoppers", self.tick_hoppers());
        phase!("pressure plates", self.tick_pressure_plates());
        phase!("dragon fights", self.tick_dragon_fights());
        phase!("stats", self.tick_stats());
        phase!("player tracker", self.tick_tracker());

        if self.tick % 100 == 0 {
            self.broadcast_latency();
        }
        if self.tick % 200 == 0 {
            phase!("chunk unloading", self.unload_unused_chunks());
        }
        let autosave = self.config.paper.world.autosave_interval_ticks;
        if autosave > 0 && self.tick % autosave == 0 {
            phase!("autosave", self.save_all(false));
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

    /// The time a player sees (`ServerPlayer.getPlayerTime`): the world's, or the one a
    /// plugin set (`Player.setPlayerTime`), relative to the world's time or fixed.
    pub fn player_time(&self, world: usize, player: &Player) -> i64 {
        let day_time = self.worlds[world].day_time;
        match player.api.time {
            Some((offset, true)) => day_time + offset,
            Some((offset, false)) => day_time - day_time.rem_euclid(24000) + offset,
            None => day_time,
        }
    }

    /// The `SetTime` payload for one player in a world: as the world's unless a plugin gave
    /// the player its own time, which stands still unless it is relative.
    fn time_payload_for(&self, world: usize, player: &Player) -> Bytes {
        let Some((_, relative)) = player.api.time else { return self.time_payload(world) };
        let w = &self.worlds[world];
        let time = self.player_time(world, player);
        let rate = if relative && w.daylight_cycle { 1.0 } else { 0.0 };
        let clocks = w.clock.map(|clock| play::ClockState { clock, time, fraction: 0.0, rate }).into_iter().collect();
        Bytes::from(encode_packet(&play::SetTime { world_age: w.age, clocks }))
    }

    /// Sends the time to everyone in a world, each player's own where a plugin set one.
    pub fn broadcast_time(&self, world: usize) {
        let shared = self.time_payload(world);
        for player in self.players.values().filter(|p| p.world == world) {
            if player.api.time.is_some() {
                player.conn.send_payload(self.time_payload_for(world, player));
            } else {
                player.conn.send_payload(shared.clone());
            }
        }
    }

    /// Loads or generates a chunk now (`World.getChunkAt` for plugins): drives the chunk
    /// pipeline until the chunk is ready, handling only chunk results meanwhile and putting
    /// other tasks aside for the main loop. The chunk stays until the next unload pass finds
    /// nobody needs it. Returns whether it is ready.
    pub fn load_chunk_now(&mut self, world: usize, pos: crate::world::chunk::ChunkPos) -> bool {
        if self.worlds[world].is_ready(pos) {
            return true;
        }
        self.worlds[world].add_ticket(pos);
        let deadline = Instant::now() + Duration::from_secs(30);
        let workers = self.workers.clone();
        while !self.worlds[world].is_ready(pos) && Instant::now() < deadline {
            self.worlds[world].pump(&workers);
            self.shared.outgoing.flush();
            match self.tasks.recv_timeout(Duration::from_millis(2)) {
                Ok(task @ MainTask::World { .. }) => self.handle_task(task),
                Ok(other) => self.deferred.get_mut().push_back(other),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.worlds[world].remove_ticket(pos);
        let ready = self.worlds[world].is_ready(pos);
        if !ready {
            log::warn!("Chunk {},{} of {} took too long to load for a plugin", pos.x, pos.z, self.worlds[world].name);
        }
        ready
    }

    /// Sends one player the time they see.
    pub fn send_time(&self, id: i32) {
        if let Some(p) = self.players.get(&id) {
            p.conn.send_payload(self.time_payload_for(p.world, p));
        }
    }

    /// Resends the world time to everyone in a world.
    pub fn resend_time(&self, world: usize) {
        self.broadcast_time(world);
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
            save_world_files(world, game_type, hardcore, &self.tick_pool);
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

    /// Saves one world (`World#save`): its chunks, `level.dat`, game rules and plugin data,
    /// and waits until they are on disk if `flush`. False if worlds are kept in memory only.
    pub fn save_world(&mut self, world: usize, flush: bool) -> bool {
        if self.io.is_none() || world >= self.worlds.len() {
            return false;
        }
        let (game_type, hardcore) = (self.default_game_mode.id(), self.config.properties.hardcore);
        save_world_files(&mut self.worlds[world], game_type, hardcore, &self.tick_pool);
        if flush {
            if let Some(io) = &self.io {
                io.flush();
            }
            if let Some(storage) = &self.worlds[world].storage {
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
                self.broadcast_time(world);
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
        crate::plugins::native::shutdown(self);
        let reason = Component::translatable("multiplayer.disconnect.server_shutdown", vec![]);
        let ids: Vec<i32> = self.player_order.clone();
        for id in ids {
            self.kick(id, &reason, "UNKNOWN");
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

/// Queues a world's chunks, `level.dat`, game rules and plugin data for writing.
fn save_world_files(world: &mut World, game_type: i32, hardcore: bool, pool: &rayon::ThreadPool) {
    let chunks = world.save_chunks_in(Some(pool));
    if let Some(storage) = &world.storage {
        if let Err(e) = storage.save_level(&world.level_data(game_type, hardcore)) {
            log::error!("Could not save level.dat of {}: {e:#}", world.name);
        }
        if let Err(e) = storage.save_game_rules(&world.rules) {
            log::error!("Could not save the game rules of {}: {e:#}", world.name);
        }
        if let Some(data) = &world.plugin_data {
            if let Err(e) = storage.save_plugin_data(data) {
                log::error!("Could not save the plugin data of {}: {e:#}", world.name);
            }
        }
    }
    log::debug!("Queued {chunks} chunk(s) of '{}' for saving", world.name);
}

/// The tick pool reads the server while the main thread waits, so it must be shareable.
const _: fn() = || {
    fn shareable<T: Sync>() {}
    shareable::<Server>();
};
