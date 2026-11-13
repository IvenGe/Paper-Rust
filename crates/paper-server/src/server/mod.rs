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

mod chat;
pub mod commands;
mod handlers;
mod join;
pub mod player;
mod view;

use crate::access::AccessLists;
use crate::config::{Config, GameMode};
use crate::net::{auth, ConfigPackets, PlayerConn, Shared, StatusSnapshot};
use crate::world::generator::{FlatGenerator, Generator, NoiseGenerator};
use crate::world::{WorkerContext, World, WorldEvent};
use anyhow::{Context, Result};
use bytes::Bytes;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use paper_protocol::codec::Encode;
use paper_protocol::packets::common::ClientSettings;
use paper_protocol::packets::{encode_packet, play, Packet};
use paper_protocol::text::Component;
use paper_protocol::types::GameProfile;
use parking_lot::RwLock;
use player::Player;
use std::collections::HashMap;
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
    workers: WorkerContext,
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

        let seed = props.seed();
        let overworld_min_y = paper_data::synced::dimension_type("minecraft:overworld").map_or(-64, |d| d.min_y);
        let flat = match config.paper.world.generator.as_str() {
            "flat" => true,
            "noise" => false,
            _ => props.level_type.ends_with("flat"),
        };
        let generator: Arc<dyn Generator> = if flat { Arc::new(FlatGenerator::new(overworld_min_y)) } else { Arc::new(NoiseGenerator::new(seed)) };
        if !flat && props.level_type != "minecraft:flat" {
            log::warn!("Vanilla world generation is not ported; '{}' worlds use the built-in '{}' terrain generator.", props.level_type, generator.name());
        }
        let world = World::new(0, &props.level_name, "minecraft:overworld", seed, generator);

        let server = Self {
            default_game_mode: props.gamemode,
            difficulty: props.difficulty,
            config,
            shared: shared.clone(),
            worlds: vec![world],
            players: HashMap::new(),
            player_order: Vec::new(),
            by_uuid: HashMap::new(),
            tick: 0,
            running: true,
            stats: TickStats::new(),
            commands: commands::CommandRegistry::with_builtins(),
            start_time: Instant::now(),
            workers,
            tasks,
            status_dirty: true,
        };
        Ok((server, ServerHandle { shared }))
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
                if let Some(w) = self.worlds.get_mut(world) {
                    w.handle_event(event);
                }
            }
            MainTask::ConsoleCommand(line) => self.dispatch_command(&commands::CommandSource::Console, &line),
            MainTask::Run(f) => f(self),
            MainTask::Shutdown => self.running = false,
        }
    }

    fn tick(&mut self) {
        self.tick += 1;

        let workers = self.workers.clone();
        for world in &mut self.worlds {
            world.tick_time();
            world.pump(&workers);
        }
        // Clients run their own clock; resynchronise once a second like vanilla.
        if self.tick % 20 == 0 {
            for index in 0..self.worlds.len() {
                let payload = self.time_payload(index);
                self.broadcast_payload_in_world(index, payload);
            }
        }

        self.tick_keep_alive();
        self.tick_chunk_views();
        self.tick_tracker();

        if self.tick % 100 == 0 {
            self.broadcast_latency();
        }
        if self.tick % 200 == 0 {
            for world in &mut self.worlds {
                world.unload_unused();
            }
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
        let reason = Component::translatable("multiplayer.disconnect.server_shutdown", vec![]);
        let ids: Vec<i32> = self.player_order.clone();
        for id in ids {
            self.kick(id, &reason);
        }
        log::info!("Saving worlds");
        for world in &self.worlds {
            log::info!("Saving chunks for level '{}'/{}", world.name, world.dimension);
        }
        log::logger().flush();
    }
}
