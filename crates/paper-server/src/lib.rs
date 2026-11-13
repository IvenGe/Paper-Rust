//! The paper-rs server.
//!
//! [`start`] wires the pieces together: it builds the game state, opens the listener on a
//! multi-threaded I/O runtime and runs the tick loop on a dedicated main thread.

pub mod access;
pub mod config;
pub mod logging;
pub mod net;
pub mod plugins;
pub mod server;
pub mod world;

use anyhow::{Context, Result};
use config::Config;
use server::{Server, ServerHandle};
use std::sync::atomic::Ordering;
use std::time::Instant;

/// A server that is up and accepting connections.
pub struct RunningServer {
    pub handle: ServerHandle,
    main: std::thread::JoinHandle<()>,
    runtime: tokio::runtime::Runtime,
}

impl RunningServer {
    /// The port the server listens on.
    pub fn port(&self) -> u16 {
        self.handle.port()
    }

    /// The I/O runtime, for spawning auxiliary tasks such as signal handlers.
    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }

    /// Blocks until the server has stopped (via `/stop`, a signal, or [`ServerHandle::shutdown`]).
    pub fn wait(self) {
        let _ = self.main.join();
        self.runtime.shutdown_timeout(std::time::Duration::from_secs(5));
    }

    /// Stops the server and waits for it to finish.
    pub fn stop(self) {
        self.handle.shutdown();
        self.wait();
    }
}

/// Starts a server with the given configuration.
pub fn start(mut config: Config) -> Result<RunningServer> {
    // No plugin jars, no JVM.
    if config.paper.plugins.enabled {
        let dir = config.resolve(&config.paper.plugins.plugins_dir);
        let has_jars = std::fs::read_dir(&dir).map(|entries| entries.flatten().any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("jar")))).unwrap_or(false);
        if !has_jars {
            log::info!("No plugins in {}; not starting the Java plugin host", dir.display());
            let _ = std::fs::create_dir_all(&dir);
            config.paper.plugins.enabled = false;
        }
    }
    start_with_plugin_launcher(config, plugins::java_launcher())
}

/// Starts a server whose plugin host is started by `launcher` instead of the standard
/// `java -jar` command (used by tests and by setups that run the host elsewhere).
pub fn start_with_plugin_launcher(config: Config, launcher: plugins::Launcher) -> Result<RunningServer> {
    let started = Instant::now();
    log::info!("Starting Paper-RS {} for Minecraft {} (protocol {})", env!("CARGO_PKG_VERSION"), paper_data::VERSION_NAME, paper_data::PROTOCOL_VERSION);
    let io_threads = config.io_threads();
    let worker_threads = config.worker_threads();
    let props = config.properties.clone();
    if !props.online_mode {
        log::warn!("**** SERVER IS RUNNING IN OFFLINE/INSECURE MODE!");
        log::warn!("The server will make no attempt to authenticate usernames. Beware.");
        log::warn!("To change this, set \"online-mode\" to \"true\" in the server.properties file.");
    }

    let plugins_enabled = config.paper.plugins.enabled;
    let (mut server, handle) = Server::new(config)?;
    log::info!("Using {io_threads} network I/O thread(s) and {worker_threads} worker thread(s)");

    // Java plugins load and enable before the server accepts players, as on Paper.
    if plugins_enabled {
        match plugins::start(&mut server, launcher) {
            Ok(()) => {
                plugins::post_world(&mut server);
                *handle.shared.plugin_host.write() = server.plugins.clone();
            }
            Err(e) => {
                log::error!("Java plugins are unavailable: {e:#}");
                if let Some(conn) = server.plugins.take() {
                    conn.kill();
                }
            }
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(io_threads).thread_name("Network IO").enable_all().build().context("creating the network runtime")?;
    let bind_ip = if props.server_ip.is_empty() { "0.0.0.0".to_owned() } else { props.server_ip.clone() };
    log::info!("Starting Minecraft server on {}:{}", if props.server_ip.is_empty() { "*" } else { &props.server_ip }, props.server_port);
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind((bind_ip.as_str(), props.server_port)))
        .with_context(|| format!("**** FAILED TO BIND TO PORT! {bind_ip}:{} - perhaps a server is already running on that port?", props.server_port))?;
    let port = listener.local_addr().context("reading the listener address")?.port();
    handle.shared.bound_port.store(port as i32, Ordering::Relaxed);
    runtime.spawn(net::accept_loop(listener, handle.shared.clone()));

    let main = std::thread::Builder::new()
        .name("Server thread".into())
        .spawn(move || {
            log::info!("Done ({:.3}s)! For help, type \"help\"", started.elapsed().as_secs_f64());
            server.run();
        })
        .context("starting the server thread")?;
    Ok(RunningServer { handle, main, runtime })
}
