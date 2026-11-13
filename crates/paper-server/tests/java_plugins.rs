//! Real Java plugins end to end: the Rust server starts the actual plugin host JVM, which
//! loads the two test plugins in `plugin-host/test-plugins` (a `plugin.yml` plugin and a
//! `paper-plugin.yml` plugin with a bootstrapper), and a protocol client checks what players
//! see.
//!
//! Needs Java 25 and the built jars (`mvn package` in `plugin-host/`). Without them the test
//! prints why and passes, so `cargo test` works on machines without Java. Point
//! `PAPER_RS_TEST_JAVA` at a Java 25 executable if it is not found automatically.

mod common;

use common::*;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play::{self, NodeKind};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(300);

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// A Java 25+ executable, if one can be found.
fn find_java() -> Option<String> {
    let mut candidates = Vec::new();
    if let Ok(java) = std::env::var("PAPER_RS_TEST_JAVA") {
        candidates.push(java);
    }
    if let Ok(home) = std::env::var("JAVA_HOME") {
        candidates.push(format!("{home}/bin/java"));
    }
    candidates.push("/opt/jdk25/bin/java".into());
    candidates.push("java".into());
    candidates.into_iter().find(|java| {
        let Ok(out) = Command::new(java).arg("-version").output() else { return false };
        let text = String::from_utf8_lossy(&out.stderr);
        // `openjdk version "25.0.1" ...`
        text.split('"').nth(1).and_then(|v| v.split('.').next()).and_then(|major| major.parse::<u32>().ok()).is_some_and(|major| major >= 25)
    })
}

struct Setup {
    java: String,
    host_dir: PathBuf,
    plugins: Vec<PathBuf>,
}

fn setup() -> Option<Setup> {
    let root = repo_root().join("plugin-host");
    let host_dir = root.join("host/target");
    let plugins = vec![root.join("test-plugins/bukkit-test-plugin/target/bukkit-test-plugin.jar"), root.join("test-plugins/paper-test-plugin/target/paper-test-plugin.jar")];
    let Some(java) = find_java() else {
        eprintln!("skipping: no Java 25 found (set PAPER_RS_TEST_JAVA)");
        return None;
    };
    if !host_dir.join("paper-rs-plugin-host.jar").is_file() || plugins.iter().any(|p| !p.is_file()) {
        eprintln!("skipping: build the plugin host and test plugins first (mvn package in plugin-host/)");
        return None;
    }
    Some(Setup { java, host_dir, plugins })
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
fn bukkit_and_paper_plugins_run_against_the_rust_server() {
    let Some(setup) = setup() else { return };
    let dir = std::env::temp_dir().join(format!("paper-rs-java-plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let plugins_dir = dir.join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();
    for jar in &setup.plugins {
        std::fs::copy(jar, plugins_dir.join(jar.file_name().unwrap())).unwrap();
    }

    let server = start_server_with_host(
        |c| {
            c.root = dir.clone();
            c.paper.plugins.java = setup.java.clone();
            c.paper.plugins.host_dir = setup.host_dir.display().to_string();
            c.paper.plugins.plugins_dir = plugins_dir.display().to_string();
            c.paper.plugins.jvm_args = vec!["-Xmx512M".into()];
            c.paper.plugins.startup_timeout_seconds = 120;
        },
        paper_server::plugins::java_launcher(),
    );
    let plugins = server.handle.run(|s| s.plugin_names()).unwrap();
    assert_eq!(plugins, vec![("BukkitTestPlugin".to_owned(), true), ("PaperTestPlugin".to_owned(), true)], "both plugins are enabled");

    // PlayerJoinEvent: a message to the player and a replaced join message. The command tree
    // then holds the plugin commands this player may use.
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    assert_eq!(next_text(&mut alice), "Welcome, Alice, from a Bukkit plugin");
    assert_eq!(next_text(&mut alice), "Alice joined (bukkit-test)");
    let tree: play::Commands = alice.expect();
    let labels = literals(&tree);
    for label in ["greet", "hi", "bukkittestplugin:greet", "dice", "papertestplugin:dice", "where"] {
        assert!(labels.contains(&label.to_owned()), "{label} missing from {labels:?}");
    }
    assert!(!labels.contains(&"secret".to_owned()), "operator-only command hidden: {labels:?}");
    alice.drain(QUIET);
    alice.clear_backlog();

    // A plugin.yml command with configuration, an alias, and a permission check.
    alice.command("greet");
    assert_eq!(next_text(&mut alice), "Hello, Alice!");
    alice.command("hi Bob");
    assert_eq!(next_text(&mut alice), "Hello, Bob!");
    alice.command("secret");
    assert!(next_text(&mut alice).starts_with("I'm sorry, but you do not have permission"));

    // Brigadier commands: one registered by a bootstrapper, one BasicCommand from onEnable.
    alice.command("dice 6");
    assert_eq!(next_text(&mut alice), "You rolled a d6");
    alice.command("dice");
    assert_eq!(next_text(&mut alice), "Usage: /dice <sides>");
    alice.command("where");
    assert!(next_text(&mut alice).starts_with("You are in "));

    // Tab completion from a TabCompleter and from a BasicCommand (which, as on Paper, is not
    // filtered by what was typed).
    alice.send(&play::CommandSuggestion { transaction_id: VarInt(1), text: "/greet ".into() });
    let suggestions: play::CommandSuggestions = alice.expect();
    assert_eq!((suggestions.start, suggestions.length), (7, 0));
    assert_eq!(suggestions.matches.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), vec!["world", "everyone"]);
    alice.send(&play::CommandSuggestion { transaction_id: VarInt(2), text: "/where t".into() });
    let suggestions: play::CommandSuggestions = alice.expect();
    assert_eq!((suggestions.start, suggestions.length), (7, 1));
    assert_eq!(suggestions.matches.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), vec!["here", "there"]);

    // Becoming an operator re-evaluates permissions and the command tree.
    alice.clear_backlog();
    server.handle.console_command("op Alice");
    let tree: play::Commands = alice.find(TIMEOUT, |t: &play::Commands| literals(t).contains(&"secret".to_owned())).expect("tree with /secret");
    assert!(literals(&tree).contains(&"secret".to_owned()));
    alice.drain(QUIET);
    alice.clear_backlog();
    alice.command("secret");
    assert_eq!(next_text(&mut alice), "The secret is 42");

    // AsyncChatEvent with a custom renderer.
    alice.chat("hello there");
    assert_eq!(next_text(&mut alice), "[paper] Alice: hello there");

    // BlockBreakEvent: the plugin protects grass and tells the player.
    alice.drain(QUIET);
    alice.clear_backlog();
    let grass = BlockPos::new(8, -61, 8);
    let before = server.handle.run(move |s| s.worlds[0].block_at(8, -61, 8)).unwrap();
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: grass, face: 1, sequence: VarInt(3) });
    let protected: play::SystemChat = alice.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain() == "This block is protected").expect("protection message");
    assert_eq!(protected.content.to_plain(), "This block is protected");
    let update: play::BlockUpdate = alice.expect();
    assert_eq!((update.pos, Some(update.state.0 as u32)), (grass, before));
    assert_eq!(server.handle.run(move |s| s.worlds[0].block_at(8, -61, 8)).unwrap(), before, "the block is still there");

    // /plugins lists them by kind.
    alice.clear_backlog();
    alice.command("plugins");
    alice.drain(QUIET);
    let mut lines = Vec::new();
    while let Some(m) = alice.try_expect::<play::SystemChat>(Duration::from_millis(10)) {
        lines.push(m.content.to_plain());
    }
    assert!(lines.iter().any(|l| l.contains("PaperTestPlugin")) && lines.iter().any(|l| l.contains("BukkitTestPlugin")), "{lines:?}");

    // PlayerQuitEvent replaces the quit message other players see.
    let mut bob = Client::join(server.port(), "Bob");
    bob.expect::<play::PlayerPosition>();
    bob.drain(QUIET);
    bob.clear_backlog();
    server.handle.console_command("kick Alice");
    bob.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain() == "Alice left (bukkit-test)").expect("custom quit message");

    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
