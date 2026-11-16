//! Rust plugins: one linked into the server binary (as a server of one's own does), and the
//! `hello_plugin` example of `paper-plugin` loaded from the plugins folder as a library. They
//! hear of joins and block breaks (and may cancel them), add commands with completions that
//! show in the command tree as their operator level allows, call the server's API, and are
//! called every tick.

mod common;

use common::*;
use paper_plugin::{bmap, CommandInfo, Event, Info, Plugin, Sender, Server};
use paper_protocol::packets::play::{self, NodeKind};
use std::time::Duration;

#[derive(Default)]
struct TestPlugin {
    ticks: u64,
}

impl Plugin for TestPlugin {
    fn info(&self) -> Info {
        Info::new("RustTest", "0.1")
            .listen("event.join")
            .listen("BlockBreakEvent")
            .command("rhello", "Says hello and puts down a gold block")
            .command("rticks", "Ticks seen")
            .command_with(CommandInfo { name: "rsecret".into(), op_level: 4, ..CommandInfo::default() })
            .ticks()
    }

    fn event(&mut self, server: &mut Server, event: &mut Event) {
        if event.is("PlayerJoinEvent") {
            let player = event.player().expect("the player who joins");
            player.send_message(server, &format!("Hi {} from Rust", player.name));
            event.set("message", "Someone joined (rust)");
        } else if event.is("BlockBreakEvent") && event.get("state").as_str() == Some("minecraft:diamond_block") {
            event.cancel();
        }
    }

    fn command(&mut self, server: &mut Server, sender: &Sender, label: &str, args: &[String]) -> bool {
        match label {
            "rhello" => {
                server.set_block("world", 3, -60, 3, "minecraft:gold_block").expect("set_block");
                let state = server.call("player.state", bmap! { "uuid" => sender.player().unwrap().uuid.clone() }).unwrap();
                sender.send_message(server, &format!("Hello {} at y {}", state.str("name").unwrap_or("?"), state.f64("y").unwrap_or(0.0).floor()));
                true
            }
            "rticks" => {
                sender.send_message(server, &format!("ticks {}", self.ticks));
                true
            }
            _ => !args.is_empty(),
        }
    }

    fn complete(&mut self, server: &mut Server, _sender: &Sender, _label: &str, args: &[String]) -> Vec<String> {
        let typed = args.last().cloned().unwrap_or_default();
        server.players().into_iter().map(|p| p.name).filter(|n| n.starts_with(&typed)).collect()
    }

    fn tick(&mut self, _server: &mut Server) {
        self.ticks += 1;
    }
}

fn literals(commands: &play::Commands) -> Vec<String> {
    commands.nodes[commands.root_index as usize]
        .children
        .iter()
        .filter_map(|i| match &commands.nodes[*i as usize].kind {
            NodeKind::Literal(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn next_text(client: &mut Client) -> String {
    client.expect::<play::SystemChat>().content.to_plain()
}

#[test]
fn a_linked_rust_plugin_hears_events_and_adds_commands() {
    let config = |c: &mut paper_server::config::Config| c.properties.gamemode = paper_server::config::GameMode::Creative;
    let server = start_server_with_natives(config, vec![paper_plugin::entry::<TestPlugin>()]);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    // The join: a message from the plugin, and the join message it set.
    assert_eq!(next_text(&mut alice), "Hi Alice from Rust");
    assert_eq!(next_text(&mut alice), "Someone joined (rust)");
    // Its commands are in the tree, but not the one for operators.
    let tree: play::Commands = alice.expect();
    let labels = literals(&tree);
    for label in ["rhello", "rusttest:rhello", "rticks"] {
        assert!(labels.contains(&label.to_owned()), "{label} missing from {labels:?}");
    }
    assert!(!labels.contains(&"rsecret".to_owned()), "an operator's command is hidden");
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();

    // A command that calls the server's API.
    alice.command("rhello");
    assert_eq!(next_text(&mut alice), "Hello Alice at y -60");
    let block = server.handle.run(|s| s.worlds[0].block_at(3, -60, 3).map(paper_data::blocks::state_to_string)).unwrap();
    assert_eq!(block.as_deref(), Some("minecraft:gold_block"));
    // Completions of its arguments.
    alice.send(&play::CommandSuggestion { transaction_id: paper_protocol::codec::VarInt(7), text: "/rhello Al".into() });
    let suggestions: play::CommandSuggestions = alice.find(TIMEOUT, |s: &play::CommandSuggestions| s.transaction_id == 7).expect("suggestions");
    assert_eq!(suggestions.matches.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(), vec!["Alice"]);
    // Called every tick.
    std::thread::sleep(Duration::from_millis(300));
    alice.clear_backlog();
    alice.command("rticks");
    let ticks: u64 = next_text(&mut alice).strip_prefix("ticks ").unwrap().parse().unwrap();
    assert!(ticks > 0, "ticks seen: {ticks}");

    // It cancels breaking diamond blocks, not other blocks.
    let id = alice.entity_id;
    let (diamond, stone) = server
        .handle
        .run(move |s| {
            let pos = paper_protocol::codec::BlockPos::new(5, -61, 5);
            let diamond = paper_data::blocks::default_state("minecraft:diamond_block").unwrap();
            let stone = paper_data::blocks::default_state("minecraft:stone").unwrap();
            (paper_server::plugins::events::on_block_break(s, id, 0, pos, diamond), paper_server::plugins::events::on_block_break(s, id, 0, pos, stone))
        })
        .unwrap();
    assert!(!diamond && stone, "diamond kept: {}, stone broken: {stone}", !diamond);

    // `/plugins` lists it.
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();
    alice.command("plugins");
    let mut lines = Vec::new();
    while let Some(chat) = alice.find(Duration::from_millis(500), |_: &play::SystemChat| true) {
        lines.push(chat.content.to_plain());
    }
    assert!(lines.iter().any(|l| l.contains("Rust Plugins")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("RustTest")), "{lines:?}");
    server.stop();
}

/// The example plugin, as `cargo build -p paper-plugin --example hello_plugin` makes it.
fn example_library() -> Option<std::path::PathBuf> {
    let file = if cfg!(windows) {
        "hello_plugin.dll"
    } else if cfg!(target_os = "macos") {
        "libhello_plugin.dylib"
    } else {
        "libhello_plugin.so"
    };
    let target = std::env::var("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| repo_root().join("target"));
    ["debug", "release"].iter().map(|profile| target.join(profile).join("examples").join(file)).find(|p| p.is_file())
}

#[test]
fn a_rust_plugin_library_is_loaded_from_the_plugins_folder() {
    let Some(library) = example_library() else {
        eprintln!("skipping: build the example plugin first (cargo build -p paper-plugin --example hello_plugin)");
        return;
    };
    let dir = std::env::temp_dir().join(format!("paper-rs-native-plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::copy(&library, plugins.join(library.file_name().unwrap())).unwrap();
    let server = start_server(|c| {
        c.root = dir.clone();
        c.paper.plugins.plugins_dir = plugins.display().to_string();
    });
    let names = server.handle.run(|s| s.native.list.iter().map(|p| (p.name.clone(), p.enabled)).collect::<Vec<_>>()).unwrap();
    assert_eq!(names, vec![("HelloRust".to_owned(), true)]);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    assert_eq!(next_text(&mut alice), "Welcome, from a Rust plugin!");
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();
    alice.command("hello");
    assert_eq!(next_text(&mut alice), "Hello, Alice, from Rust!");
    // Its alias.
    alice.command("hi-rust Bob");
    assert_eq!(next_text(&mut alice), "Hello, Bob, from Rust!");
    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
