//! Separate-mode Java plugins.
//!
//! Java plugins run in a separate JVM (the *plugin host*, `plugin-host/` in this repository)
//! that talks to this server over the plugin bridge (`paper-bridge`). This module starts that
//! process, owns the connection and implements both directions of the protocol:
//!
//! * the server calls the host to fire events, run plugin commands and drive the plugin
//!   scheduler once per tick ([`HostConnection::call`]);
//! * the host calls the server whenever plugin code uses the API ([`api`]).
//!
//! ## The main-thread baton
//!
//! Plugins expect Paper's single main thread. A call from this server's main thread is sent
//! with [`FLAG_SYNC`]; the host runs it on *its* main thread. If plugin code calls back into
//! the API, that call arrives here while the main thread is still waiting and is executed
//! inside the wait, with full access to the server. Calls therefore nest across the two
//! processes like ordinary function calls, and only one side runs game logic at a time.
//! Calls from the host's other threads (async tasks, async events) arrive without the flag;
//! they are queued as [`MainTask`]s and run between ticks.

pub mod api;
pub mod entity;
pub mod events;
pub mod inventory;
pub mod offline;
pub mod scoreboard;

use crate::config::{Config, OnHostCrash};
use crate::server::{MainTask, Server};
use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use paper_bridge::{bmap, read_frame, write_frame, Frame, Value, FLAG_SYNC, PROTOCOL_VERSION};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the main thread receives while it waits inside a sync call.
enum Inbound {
    Return { id: u64, ok: bool, value: Value },
    Call { id: u64, method: String, args: Value },
    Closed(String),
}

/// State the server keeps for the plugin API.
#[derive(Default)]
pub struct PluginData {
    /// Custom inventories made by plugins, by id.
    pub inventories: HashMap<u64, inventory::CustomInventory>,
    pub next_inventory: u64,
    /// How the plugin host was started, to start it again for `/reload`.
    pub launcher: Option<Arc<Launcher>>,
    /// The current host was started by `/reload`: it loads with the players already online.
    pub reloading: bool,
    /// Players whose `InventoryCloseEvent` is being fired.
    pub closing: HashSet<i32>,
}

/// A loaded plugin, as reported by the host.
#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub enabled: bool,
    /// `paper` for paper-plugin.yml plugins, `bukkit` for plugin.yml plugins.
    pub kind: String,
}

/// An error from a call into the plugin host.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("the plugin host is not connected")]
    Disconnected,
    #[error("the plugin host reported an error: {0}")]
    Remote(String),
}

type Continuation = Box<dyn FnOnce(&mut Server, Result<Value, CallError>) + Send>;

enum Pending {
    /// An I/O thread waits for the answer.
    Blocking(std::sync::mpsc::Sender<Result<Value, CallError>>),
    /// The main thread runs this when the answer arrives.
    Continue(Continuation),
}

/// The connection to the plugin host.
pub struct HostConnection {
    writer: Mutex<BufWriter<TcpStream>>,
    next_id: AtomicU64,
    /// Answers and sync calls for the main thread while it waits.
    main_rx: Receiver<Inbound>,
    /// Ids of sync calls the main thread is waiting for.
    main_calls: Mutex<HashSet<u64>>,
    pending: Mutex<HashMap<u64, Pending>>,
    closed: AtomicBool,
    /// The server is stopping this host on purpose (shutdown or `/reload`): its connection
    /// going away is not a crash.
    retiring: AtomicBool,
    slow_call: Duration,
    child: Mutex<Option<Child>>,
    to_main: Sender<MainTask>,
}

impl HostConnection {
    fn send(&self, frame: &Frame) -> Result<(), CallError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(CallError::Disconnected);
        }
        let mut writer = self.writer.lock();
        write_frame(&mut *writer, frame).map_err(|_| CallError::Disconnected)?;
        writer.flush().map_err(|_| CallError::Disconnected)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Sends a notification to the host.
    pub fn notify(&self, method: &str, args: Value) {
        let _ = self.send(&Frame::Notify { method: method.into(), args });
    }

    /// Calls the host from the main thread and waits, executing the host's nested calls
    /// meanwhile. Must only be called on the thread that owns `server`.
    pub fn call(self: &Arc<Self>, server: &mut Server, method: &str, args: Value) -> Result<Value, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.main_calls.lock().insert(id);
        let result = self.call_inner(server, id, method, args);
        self.main_calls.lock().remove(&id);
        result
    }

    fn call_inner(self: &Arc<Self>, server: &mut Server, id: u64, method: &str, args: Value) -> Result<Value, CallError> {
        self.send(&Frame::Call { id, flags: FLAG_SYNC, method: method.into(), args })?;
        let started = Instant::now();
        let mut warned = false;
        // Loading and enabling plugins legitimately takes a while.
        let slow_call = if method == "host.load" || method == "host.post_world" { self.slow_call.max(Duration::from_secs(30)) } else { self.slow_call };
        loop {
            match self.main_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(Inbound::Return { id: rid, ok, value }) if rid == id => {
                    return if ok { Ok(value) } else { Err(CallError::Remote(value.as_str().unwrap_or("error").to_owned())) };
                }
                Ok(Inbound::Return { id: rid, .. }) => log::warn!("Plugin host answered call {rid} out of order; ignoring it"),
                Ok(Inbound::Call { id: cid, method: m, args }) => {
                    let (ok, value) = match api::handle(server, &m, &args) {
                        Ok(v) => (true, v),
                        Err(e) => (false, Value::Str(format!("{e:#}"))),
                    };
                    let _ = self.send(&Frame::Return { id: cid, ok, value });
                }
                Ok(Inbound::Closed(reason)) => {
                    log::debug!("Plugin host closed during {method}: {reason}");
                    return Err(CallError::Disconnected);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.is_closed() {
                        return Err(CallError::Disconnected);
                    }
                    if !warned && started.elapsed() > slow_call {
                        warned = true;
                        log::warn!("The plugin host has been handling {method} for {:.1}s; the server is waiting", started.elapsed().as_secs_f64());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Err(CallError::Disconnected),
            }
        }
    }

    /// Calls the host without blocking; `then` runs on the main thread with the answer.
    pub fn call_then(&self, method: &str, args: Value, then: impl FnOnce(&mut Server, Result<Value, CallError>) + Send + 'static) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().insert(id, Pending::Continue(Box::new(then)));
        if let Err(e) = self.send(&Frame::Call { id, flags: 0, method: method.into(), args }) {
            if let Some(Pending::Continue(f)) = self.pending.lock().remove(&id) {
                // Deliver the failure through the task queue so callers see one code path.
                let _ = self.to_main.send(MainTask::Run(Box::new(move |server| f(server, Err(e)))));
            }
        }
    }

    /// Calls the host from a thread other than the main thread (e.g. a login task) and
    /// blocks that thread until the answer arrives or `timeout` passes.
    pub fn call_blocking(&self, method: &str, args: Value, timeout: Duration) -> Result<Value, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending.lock().insert(id, Pending::Blocking(tx));
        self.send(&Frame::Call { id, flags: 0, method: method.into(), args })?;
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(CallError::Remote(format!("{method} timed out")))
            }
        }
    }

    /// Stops the host process if it is still running.
    pub fn kill(&self) {
        self.closed.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.lock().take() {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            log::warn!("The plugin host did not exit; killing it");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Reads frames from the host and routes them.
fn reader_loop(conn: Arc<HostConnection>, stream: TcpStream, main_tx: Sender<Inbound>, to_main: Sender<MainTask>) {
    let mut reader = BufReader::with_capacity(64 * 1024, stream);
    let reason = loop {
        match read_frame(&mut reader) {
            Ok(Some(Frame::Return { id, ok, value })) => {
                if conn.main_calls.lock().contains(&id) {
                    let _ = main_tx.send(Inbound::Return { id, ok, value });
                    continue;
                }
                let result = if ok { Ok(value) } else { Err(CallError::Remote(value.as_str().unwrap_or("error").to_owned())) };
                match conn.pending.lock().remove(&id) {
                    Some(Pending::Blocking(tx)) => {
                        let _ = tx.send(result);
                    }
                    Some(Pending::Continue(f)) => {
                        let _ = to_main.send(MainTask::Run(Box::new(move |server| f(server, result))));
                    }
                    None => log::warn!("Plugin host answered unknown call {id}"),
                }
            }
            Ok(Some(Frame::Call { id, flags, method, args })) => {
                if flags & FLAG_SYNC != 0 {
                    // Part of a nested call chain: the main thread is (or is about to be)
                    // waiting inside `call`. Wake it in case it is idle.
                    let _ = main_tx.send(Inbound::Call { id, method, args });
                    let _ = to_main.send(MainTask::PluginWake);
                } else {
                    let conn = conn.clone();
                    let _ = to_main.send(MainTask::Run(Box::new(move |server| {
                        let (ok, value) = match api::handle(server, &method, &args) {
                            Ok(v) => (true, v),
                            Err(e) => (false, Value::Str(format!("{e:#}"))),
                        };
                        let _ = conn.send(&Frame::Return { id, ok, value });
                    })));
                }
            }
            Ok(Some(Frame::Notify { method, args })) => {
                let conn = conn.clone();
                let _ = to_main.send(MainTask::Run(Box::new(move |server| {
                    // A host stopped by `/reload` may still report its last changes; only the
                    // current host's notifications count.
                    if server.plugins.as_ref().is_some_and(|current| Arc::ptr_eq(current, &conn)) {
                        events::handle_notify(server, &method, &args);
                    }
                })));
            }
            Ok(Some(other)) => log::warn!("Unexpected frame from the plugin host: {other:?}"),
            Ok(None) => break "the plugin host closed the connection".to_owned(),
            Err(e) => break format!("{e}"),
        }
    };
    let was_closed = conn.closed.swap(true, Ordering::Relaxed);
    let pending: Vec<(u64, Pending)> = conn.pending.lock().drain().collect();
    for (_, p) in pending {
        match p {
            Pending::Blocking(tx) => {
                let _ = tx.send(Err(CallError::Disconnected));
            }
            Pending::Continue(f) => {
                let _ = to_main.send(MainTask::Run(Box::new(move |server| f(server, Err(CallError::Disconnected)))));
            }
        }
    }
    let _ = main_tx.send(Inbound::Closed(reason.clone()));
    if !was_closed && !conn.retiring.load(Ordering::Relaxed) {
        let _ = to_main.send(MainTask::PluginHostDied(reason));
    }
}

/// Drains sync calls that arrived while the main thread was not waiting.
pub fn service_idle(server: &mut Server) {
    let Some(conn) = server.plugins.clone() else { return };
    while let Ok(item) = conn.main_rx.try_recv() {
        match item {
            Inbound::Call { id, method, args } => {
                let (ok, value) = match api::handle(server, &method, &args) {
                    Ok(v) => (true, v),
                    Err(e) => (false, Value::Str(format!("{e:#}"))),
                };
                let _ = conn.send(&Frame::Return { id, ok, value });
            }
            Inbound::Return { id, .. } => log::warn!("Plugin host answered call {id} after it was abandoned"),
            Inbound::Closed(_) => {}
        }
    }
}

fn host_jar(config: &Config) -> Result<PathBuf> {
    let dir = config.resolve(&config.paper.plugins.host_dir);
    let candidates = [dir.join("paper-rs-plugin-host.jar"), dir.join("host").join("target").join("paper-rs-plugin-host.jar")];
    candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| anyhow!("plugin host jar not found (looked for {}); build it with `mvn package` in plugin-host/", candidates[0].display()))
}

/// Forwards the host's console output into the server log. The host prints one record per
/// line as `LEVEL|message`; anything else (a plugin writing to stdout) is logged as-is.
fn pipe_output(stream: impl std::io::Read + Send + 'static, name: &'static str) {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let (level, message) = match line.split_once('|') {
                    Some(("SEVERE", m)) => (log::Level::Error, m),
                    Some(("WARNING", m)) => (log::Level::Warn, m),
                    Some(("INFO", m)) => (log::Level::Info, m),
                    Some(("CONFIG" | "FINE" | "FINER" | "FINEST", m)) => (log::Level::Debug, m),
                    _ => (log::Level::Info, line.as_str()),
                };
                log::log!(target: "plugin", level, "{message}");
            }
        })
        .expect("spawning the plugin output reader");
}

/// Starts the plugin host. It is given the bridge port to connect to and the token it must
/// present, and returns the host process if there is one for the server to manage. It is
/// kept and called again when `/reload` starts a new host.
pub type Launcher = Box<dyn Fn(&Config, u16, &str) -> Result<Option<Child>> + Send + Sync>;

/// The standard launcher: `java [jvm-args] -jar paper-rs-plugin-host.jar --connect ...`.
pub fn java_launcher() -> Launcher {
    Box::new(launch_java)
}

fn launch_java(config: &Config, port: u16, token: &str) -> Result<Option<Child>> {
    let settings = &config.paper.plugins;
    let jar = host_jar(config)?;
    let plugins_dir = config.resolve(&settings.plugins_dir);
    std::fs::create_dir_all(&plugins_dir).with_context(|| format!("creating {}", plugins_dir.display()))?;
    let mut command = Command::new(&settings.java);
    command
        .args(&settings.jvm_args)
        .arg("-jar")
        .arg(&jar)
        .arg("--connect")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--root")
        .arg(&config.root)
        .arg("--plugins")
        .arg(&plugins_dir)
        // The token travels in the environment so it does not show up in process listings.
        .env("PAPER_RS_BRIDGE_TOKEN", token)
        .current_dir(&config.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    log::info!("Starting the plugin host: {} -jar {}", settings.java, jar.display());
    let mut child = command.spawn().with_context(|| format!("starting {} (is Java 25 installed? set plugins.java in config/paper-rs.toml)", settings.java))?;
    pipe_output(child.stdout.take().unwrap(), "Plugin Host Output");
    pipe_output(child.stderr.take().unwrap(), "Plugin Host Errors");
    Ok(Some(child))
}

/// Starts the plugin host with `launcher`, waits for it to connect and loads the plugins
/// (constructing them and running `onLoad`, then enabling `STARTUP` plugins). Call before
/// the server accepts players.
pub fn start(server: &mut Server, launcher: Launcher) -> Result<()> {
    let launcher = Arc::new(launcher);
    server.plugin_data.launcher = Some(launcher.clone());
    start_with(server, &launcher)
}

fn start_with(server: &mut Server, launcher: &Launcher) -> Result<()> {
    let config = server.config.clone();
    let settings = &config.paper.plugins;

    let listener = TcpListener::bind("127.0.0.1:0").context("opening the plugin bridge port")?;
    let port = listener.local_addr()?.port();
    let token: String = (0..32).map(|_| format!("{:x}", rand::random::<u8>() & 15)).collect();
    let mut child = launcher(&config, port, &token)?;

    // Wait for the host to connect, watching for it to die first.
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(settings.startup_timeout_seconds.max(1));
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(child) = child.as_mut() {
                    if let Some(status) = child.try_wait()? {
                        bail!("the plugin host exited during startup ({status})");
                    }
                }
                if Instant::now() > deadline {
                    if let Some(child) = child.as_mut() {
                        let _ = child.kill();
                    }
                    bail!("the plugin host did not connect within {}s", settings.startup_timeout_seconds);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.into()),
        }
    };
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;

    // Handshake: HELLO with the token, answered by WELCOME with the server's description.
    let mut reader = stream.try_clone()?;
    reader.set_read_timeout(Some(Duration::from_secs(settings.startup_timeout_seconds.max(1))))?;
    let hello = match read_frame(&mut reader)? {
        Some(Frame::Hello(v)) => v,
        other => bail!("expected HELLO from the plugin host, got {other:?}"),
    };
    if hello.str("token") != Some(token.as_str()) {
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
        }
        bail!("the process that connected to the plugin bridge did not present the right token");
    }
    if hello.i64("protocol") != Some(PROTOCOL_VERSION) {
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
        }
        bail!("plugin host speaks bridge protocol {:?}, this server speaks {PROTOCOL_VERSION}", hello.i64("protocol"));
    }
    reader.set_read_timeout(None)?;
    let mut writer = BufWriter::with_capacity(64 * 1024, stream);
    write_frame(&mut writer, &Frame::Welcome(api::welcome(server)))?;
    writer.flush()?;

    let (main_tx, main_rx) = crossbeam_channel::unbounded();
    let conn = Arc::new(HostConnection {
        writer: Mutex::new(writer),
        next_id: AtomicU64::new(1),
        main_rx,
        main_calls: Mutex::new(HashSet::new()),
        pending: Mutex::new(HashMap::new()),
        closed: AtomicBool::new(false),
        retiring: AtomicBool::new(false),
        slow_call: Duration::from_millis(settings.slow_call_warning_millis.max(1)),
        child: Mutex::new(child),
        to_main: server.shared.to_main.clone(),
    });
    let to_main = server.shared.to_main.clone();
    let reader_conn = conn.clone();
    std::thread::Builder::new().name("Plugin Bridge Reader".into()).spawn(move || reader_loop(reader_conn, reader, main_tx, to_main))?;
    server.plugins = Some(conn.clone());

    // The host knows the players handed over in WELCOME (after `/reload`).
    for id in server.player_order.clone() {
        if let Some(p) = server.players.get(&id) {
            let state = api::player_state(server, p);
            server.plugin_state_cache.insert(id, state);
        }
    }
    let loaded = conn.call(server, "host.load", Value::Null).map_err(|e| anyhow!("loading plugins failed: {e}"))?;
    events::apply_plugin_list(server, &loaded);
    Ok(())
}

/// `/reload confirm`: disables every plugin and loads them again from `plugins/`.
///
/// Paper reloads plugins inside its own JVM. Here the plugin host is stopped (plugins see
/// `onDisable`) and a new one is started with the same launcher; it loads the plugins from
/// scratch (`onLoad`, `onEnable`) with the online players already known, so no join events
/// fire, then fires `ServerLoadEvent` with `LoadType.RELOAD`. Nothing of the old host's Java
/// state survives: custom inventories are closed and dropped, players go back to the main
/// scoreboard (the main scoreboard itself is kept, since the server owns it), and scheduled
/// tasks and metadata are gone.
pub fn reload(server: &mut Server) -> Result<()> {
    let launcher = server.plugin_data.launcher.clone().ok_or_else(|| anyhow!("Java plugins are not enabled"))?;
    log::info!("Reloading plugins: stopping the plugin host");
    // Windows showing plugin inventories close (their holders lived in the old host).
    for id in server.player_order.clone() {
        let custom = server.players.get(&id).and_then(|p| p.open_menu.as_ref()).is_some_and(|m| matches!(m.kind, crate::server::containers::MenuKind::Custom { .. }));
        if custom {
            server.close_menu_with_reason(id, true, "PLUGIN");
        }
    }
    if let Some(conn) = server.plugins.clone() {
        conn.retiring.store(true, Ordering::Relaxed);
        if !conn.is_closed() {
            if let Err(e) = conn.call(server, "host.shutdown", Value::Null) {
                log::warn!("The plugin host did not shut down cleanly: {e}");
            }
        }
        conn.kill();
    }
    server.plugins = None;
    *server.shared.plugin_host.write() = None;
    server.plugin_commands.clear();
    server.plugin_events.clear();
    server.plugin_state_cache.clear();
    server.plugin_list.clear();
    server.plugin_data.inventories.clear();
    server.reset_plugin_scoreboards();

    log::info!("Reloading plugins: starting a new plugin host");
    server.plugin_data.reloading = true;
    let started = start_with(server, &launcher);
    server.plugin_data.reloading = false;
    if let Err(e) = started {
        if let Some(conn) = server.plugins.take() {
            conn.kill();
        }
        server.resend_command_trees();
        return Err(e);
    }
    post_world(server);
    *server.shared.plugin_host.write() = server.plugins.clone();
    server.resend_command_trees();
    Ok(())
}

/// Enables `POSTWORLD` plugins, registers lifecycle commands and fires `ServerLoadEvent`.
pub fn post_world(server: &mut Server) {
    let Some(conn) = server.plugins.clone() else { return };
    match conn.call(server, "host.post_world", Value::Null) {
        Ok(result) => events::apply_plugin_list(server, &result),
        Err(e) => log::error!("Enabling plugins failed: {e}"),
    }
}

/// Disables all plugins and stops the host.
pub fn shutdown(server: &mut Server) {
    let Some(conn) = server.plugins.clone() else { return };
    conn.retiring.store(true, Ordering::Relaxed);
    if !conn.is_closed() {
        if let Err(e) = conn.call(server, "host.shutdown", Value::Null) {
            log::warn!("The plugin host did not shut down cleanly: {e}");
        }
    }
    conn.kill();
    server.plugins = None;
}

/// Reacts to the host process going away unexpectedly.
pub fn host_died(server: &mut Server, reason: &str) {
    server.plugins = None;
    *server.shared.plugin_host.write() = None;
    server.plugin_commands.clear();
    server.plugin_events.clear();
    server.plugin_state_cache.clear();
    for plugin in &mut server.plugin_list {
        plugin.enabled = false;
    }
    // What the plugins made is gone with them: their windows and their scoreboards.
    for id in server.player_order.clone() {
        let custom = server.players.get(&id).and_then(|p| p.open_menu.as_ref()).is_some_and(|m| matches!(m.kind, crate::server::containers::MenuKind::Custom { .. }));
        if custom {
            server.close_menu_with_reason(id, true, "PLUGIN");
        }
    }
    server.plugin_data.inventories.clear();
    server.reset_plugin_scoreboards();
    match server.config.paper.plugins.on_host_crash {
        OnHostCrash::Shutdown => {
            log::error!("The plugin host stopped unexpectedly ({reason}). Stopping the server so plugins that protect the world are not silently missing.");
            log::error!("Set plugins.on-host-crash = \"continue\" in config/paper-rs.toml to keep running without plugins instead.");
            server.running = false;
        }
        OnHostCrash::Continue => {
            log::error!("The plugin host stopped unexpectedly ({reason}). Continuing without plugins.");
            server.resend_command_trees();
        }
    }
}

/// The host's representation of a text component: its JSON.
pub fn component_value(c: &paper_protocol::text::Component) -> Value {
    Value::Str(c.to_json_string())
}

/// Parses a component sent by the host (JSON text), falling back to plain text.
pub fn component_from(value: &Value) -> Option<paper_protocol::text::Component> {
    let s = value.as_str()?;
    Some(paper_protocol::text::Component::from_json_str(s).unwrap_or_else(|_| paper_protocol::text::Component::text(s)))
}

#[allow(dead_code)]
fn _assert_send() {
    fn is_send<T: Send + Sync>() {}
    is_send::<HostConnection>();
    let _ = bmap! {};
}
