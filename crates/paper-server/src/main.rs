//! The `paper` server binary.

use anyhow::Result;
use paper_server::config::Config;
use std::io::BufRead;
use std::path::PathBuf;

const HELP: &str = "\
Usage: paper [options]

Options:
  --root <dir>      Server directory (default: the current directory)
  --port <port>     Listen on this port instead of server.properties' server-port
  --nogui           Accepted for compatibility with vanilla start scripts
  --version         Print the version and exit
  --help            Print this help and exit
";

fn main() -> Result<()> {
    let mut root = PathBuf::from(".");
    let mut port: Option<u16> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" | "--universe" | "-W" => root = PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("{arg} needs a directory"))?),
            "--port" | "-p" => port = Some(args.next().ok_or_else(|| anyhow::anyhow!("{arg} needs a port number"))?.parse()?),
            "--nogui" | "nogui" => {}
            "--version" | "-v" => {
                println!("Paper-RS {} (Minecraft {}, protocol {})", env!("CARGO_PKG_VERSION"), paper_data::VERSION_NAME, paper_data::PROTOCOL_VERSION);
                return Ok(());
            }
            "--help" | "-h" | "-?" => {
                print!("{HELP}");
                return Ok(());
            }
            other => anyhow::bail!("unknown option {other}\n\n{HELP}"),
        }
    }

    std::fs::create_dir_all(&root)?;
    paper_server::logging::init(Some(&root.join("logs")), log::LevelFilter::Info);
    let mut config = Config::load(&root)?;
    if let Some(port) = port {
        config.properties.server_port = port;
    }

    let server = match paper_server::start(config) {
        Ok(server) => server,
        Err(e) => {
            log::error!("Failed to start the server: {e:#}");
            log::logger().flush();
            std::process::exit(1);
        }
    };

    // Console: every line typed is run as a command.
    let console = server.handle.clone();
    std::thread::Builder::new().name("Console".into()).spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if !line.is_empty() {
                console.console_command(line);
            }
        }
    })?;

    // Ctrl-C and SIGTERM stop the server cleanly.
    let signals = server.handle.clone();
    server.runtime().spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).expect("installing the SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        signals.shutdown();
    });

    server.wait();
    Ok(())
}
