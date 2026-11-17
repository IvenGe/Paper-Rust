//! Popular plugins on the real plugin host: EssentialsX and WorldEdit, which are not part of
//! this repository. Point `PAPER_RS_TEST_PLUGINS` at a folder holding their jars
//! (`EssentialsX-*.jar`, `worldedit-bukkit-*.jar` or `worldedit.jar`) to run these tests;
//! without it, or without Java 25 and the built plugin host, they print why and pass.

mod common;

use common::*;
use paper_data::blocks;
use paper_protocol::packets::play;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// The plugin jars whose file names start with any of `prefixes`, if the folder is set.
fn jars(prefixes: &[&str]) -> Option<Vec<PathBuf>> {
    let Ok(dir) = std::env::var("PAPER_RS_TEST_PLUGINS") else {
        eprintln!("skipping: set PAPER_RS_TEST_PLUGINS to a folder with the plugin jars");
        return None;
    };
    let host = repo_root().join("plugin-host/host/target/paper-rs-plugin-host.jar");
    if !host.is_file() || !Path::new("/opt/jdk25/bin/java").is_file() && std::env::var("PAPER_RS_TEST_JAVA").is_err() {
        eprintln!("skipping: build the plugin host (mvn package in plugin-host/) and have Java 25");
        return None;
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.ends_with(".jar") && prefixes.iter().any(|p| name.starts_with(p)) {
            out.push(entry.path());
        }
    }
    if out.len() < prefixes.len() {
        eprintln!("skipping: not all of {prefixes:?} are in {dir}");
        return None;
    }
    Some(out)
}

/// A server with the plugins in a folder of its own (`name`: tests run side by side).
fn start(name: &str, jars: &[PathBuf]) -> (paper_server::RunningServer, PathBuf) {
    let dir = std::env::temp_dir().join(format!("paper-rs-third-party-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let plugins_dir = dir.join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();
    for jar in jars {
        std::fs::copy(jar, plugins_dir.join(jar.file_name().unwrap())).unwrap();
    }
    let java = std::env::var("PAPER_RS_TEST_JAVA").unwrap_or_else(|_| "/opt/jdk25/bin/java".into());
    let host_dir = repo_root().join("plugin-host/host/target");
    let server = start_server_with_host(
        |c| {
            c.root = dir.clone();
            c.paper.plugins.java = java;
            c.paper.plugins.host_dir = host_dir.display().to_string();
            c.paper.plugins.plugins_dir = plugins_dir.display().to_string();
            c.paper.plugins.jvm_args = vec!["-Xmx1G".into()];
            c.paper.plugins.startup_timeout_seconds = 180;
        },
        paper_server::plugins::java_launcher(),
    );
    (server, dir)
}

/// Runs a command and returns what the player was told until the server went quiet.
fn run(client: &mut Client, command: &str) -> Vec<String> {
    client.clear_backlog();
    client.command(command);
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        if let Some(chat) = client.try_expect::<play::SystemChat>(Duration::from_millis(100)) {
            out.push(chat.content.to_plain());
        }
    }
    out
}

fn block_name(server: &paper_server::RunningServer, x: i32, y: i32, z: i32) -> &'static str {
    server.handle.run(move |s| blocks::block_of_state(s.worlds[0].block_at(x, y, z).unwrap()).name).unwrap()
}

#[test]
fn worldedit_edits_the_world_through_the_paper_rs_adapter() {
    let Some(jars) = jars(&["worldedit"]) else { return };
    let (server, _dir) = start("worldedit", &jars);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    server.handle.console_command("op Alice");
    alice.drain(Duration::from_millis(500));

    // `//pos1` is the command `/pos1`: the second slash is part of its name.
    assert_eq!(run(&mut alice, "/pos1 0,-61,0"), ["First position set to (0, -61, 0)."]);
    assert!(run(&mut alice, "/pos2 4,-59,4")[0].starts_with("Second position set to (4, -59, 4)"));
    assert_eq!(run(&mut alice, "/set stone"), ["Operation completed (75 blocks affected)."]);
    assert_eq!((block_name(&server, 0, -61, 0), block_name(&server, 4, -59, 4)), ("minecraft:stone", "minecraft:stone"));
    assert_eq!(run(&mut alice, "/replace stone oak_stairs[facing=east]"), ["75 blocks have been replaced."]);
    let stairs = server.handle.run(|s| blocks::state_to_string(s.worlds[0].block_at(2, -60, 2).unwrap())).unwrap();
    assert!(stairs.starts_with("minecraft:oak_stairs[facing=east"), "{stairs}");
    assert_eq!(run(&mut alice, "/undo"), ["Undid 1 available edits."]);
    assert_eq!(block_name(&server, 2, -60, 2), "minecraft:stone");
    assert_eq!(run(&mut alice, "/undo"), ["Undid 1 available edits."]);
    // The flat world's grass and air are back.
    assert_eq!((block_name(&server, 0, -61, 0), block_name(&server, 0, -60, 0)), ("minecraft:grass_block", "minecraft:air"));
    server.stop();
}

/// What a command answers, waiting up to `limit` for a message with `done` in it.
fn run_until(client: &mut Client, command: &str, done: &str, limit: Duration) -> (Duration, Option<String>) {
    client.clear_backlog();
    let start = Instant::now();
    client.command(command);
    let answer = client.find(limit, |c: &play::SystemChat| c.content.to_plain().contains(done)).map(|c| c.content.to_plain());
    (start.elapsed(), answer)
}

#[test]
fn worldedit_large_edits_are_quick_and_apply_their_side_effects() {
    let Some(jars) = jars(&["worldedit"]) else { return };
    let (server, _dir) = start("worldedit-large", &jars);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    server.handle.console_command("op Alice");
    alice.drain(Duration::from_millis(500));

    // 30 x 30 x 30 blocks: each change once crossed the plugin bridge on its own (17 seconds
    // of a frozen server); now they go in batches.
    run(&mut alice, "/pos1 0,-61,0");
    run(&mut alice, "/pos2 29,-32,29");
    let (took, answer) = run_until(&mut alice, "/set tnt", "Operation completed", Duration::from_secs(60));
    assert_eq!(answer.as_deref(), Some("Operation completed (27000 blocks affected)."));
    assert!(took < Duration::from_secs(5), "the edit took {took:?}");
    let tnt = blocks::default_state("minecraft:tnt").unwrap();
    let left = server.handle.run(move |s| (0..30).flat_map(|x| (0..30).flat_map(move |z| (-61..-31).map(move |y| (x, y, z)))).filter(|&(x, y, z)| s.worlds[0].block_at(x, y, z) != Some(tnt)).count()).unwrap();
    assert_eq!(left, 0, "every block is TNT");

    let (took, answer) = run_until(&mut alice, "/undo", "Undid", Duration::from_secs(60));
    assert_eq!(answer.as_deref(), Some("Undid 1 available edits."));
    assert!(took < Duration::from_secs(5), "the undo took {took:?}");
    assert_eq!((block_name(&server, 29, -61, 29), block_name(&server, 29, -32, 29)), ("minecraft:grass_block", "minecraft:air"));

    // Water set in the air flows once the edit is done, as WorldEdit applies the side effects
    // (here the water's onPlace and its neighbours' updates) after the edit.
    run(&mut alice, "/pos1 40,-55,40");
    run(&mut alice, "/pos2 40,-55,40");
    run(&mut alice, "/set water");
    let deadline = Instant::now() + TIMEOUT;
    while block_name(&server, 40, -56, 40) != "minecraft:water" {
        assert!(Instant::now() < deadline, "the water flows down");
        std::thread::sleep(Duration::from_millis(50));
    }
    server.stop();
}

#[test]
fn essentials_teleports_heals_and_sets_homes() {
    let Some(jars) = jars(&["essentialsx"]) else { return };
    let (server, _dir) = start("essentials", &jars);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    server.handle.console_command("op Alice");
    alice.drain(Duration::from_millis(500));

    assert_eq!(run(&mut alice, "sethome"), ["Home set to current location."]);
    assert_eq!(run(&mut alice, "tppos 200 -60 200"), ["Teleporting..."]);
    let x = server.handle.run(|s| s.player_by_name("Alice").unwrap().loc.x).unwrap();
    assert!((x - 200.5).abs() < 1.0, "teleported to {x}");
    assert_eq!(run(&mut alice, "home"), ["Teleporting to home."]);
    let x = server.handle.run(|s| s.player_by_name("Alice").unwrap().loc.x).unwrap();
    assert!(x < 50.0, "home again at {x}");
    assert_eq!(run(&mut alice, "heal"), ["You have been healed."]);
    assert_eq!(run(&mut alice, "speed 2"), ["Set walking speed to 2 for Alice."]);
    assert!(run(&mut alice, "weather storm")[0].contains("storm"));
    let raining = server.handle.run(|s| s.worlds[0].weather.raining).unwrap();
    assert!(raining, "Essentials made it rain");
    server.stop();
}

/// A big edit's side effects all land, whatever batches its changes go in (a batch never
/// splits one block's side effects): a field of 10,000 fences connects everywhere. Block
/// entities that hold items are emptied before WorldEdit replaces them, as on Paper: a
/// brewing stand drops nothing, and //undo brings it back with its potion, once.
#[test]
fn worldedit_side_effects_land_in_big_edits_and_holders_are_emptied_first() {
    use paper_nbt::{Compound, Tag};
    let Some(jars) = jars(&["worldedit"]) else { return };
    let (server, _dir) = start("worldedit-fences", &jars);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    server.handle.console_command("op Alice");
    alice.drain(Duration::from_millis(500));

    run(&mut alice, "/pos1 0,-60,0");
    run(&mut alice, "/pos2 99,-60,99");
    let (_, answer) = run_until(&mut alice, "/set oak_fence", "Operation completed", Duration::from_secs(60));
    assert_eq!(answer.as_deref(), Some("Operation completed (10000 blocks affected)."));
    let gaps = server
        .handle
        .run(|s| {
            (1..99)
                .flat_map(|x| (1..99).map(move |z| (x, z)))
                .filter(|&(x, z)| {
                    let state = s.worlds[0].block_at(x, -60, z).unwrap();
                    ["north", "east", "south", "west"].iter().any(|side| blocks::property(state, side) != Some("true"))
                })
                .count()
        })
        .unwrap();
    assert_eq!(gaps, 0, "every fence inside the field connects all round");

    let stand = blocks::default_state("minecraft:brewing_stand").unwrap();
    server
        .handle
        .run(move |s| {
            s.set_block_with_flags(0, paper_protocol::codec::BlockPos::new(120, -60, 120), stand, paper_server::world::block_updates::flags::ALL);
            let chunk = s.worlds[0].chunks.get_mut(&paper_server::world::chunk::ChunkPos::of_block(120, 120)).unwrap();
            let potion = Compound::new().with("Slot", Tag::Byte(0)).with("id", Tag::String("minecraft:potion".into())).with("count", Tag::Int(1));
            chunk.block_entities.get_mut(&(-60, 120 & 15, 120 & 15)).expect("the stand's block entity").insert("Items", Tag::List(vec![Tag::Compound(potion)]));
        })
        .unwrap();
    let items_near = |server: &paper_server::RunningServer| server.handle.run(|s| s.worlds[0].entities.values().filter(|e| e.info.name == "minecraft:item" && (e.pos.x - 120.5).abs() < 4.0 && (e.pos.z - 120.5).abs() < 4.0).count()).unwrap();
    run(&mut alice, "/pos1 120,-60,120");
    run(&mut alice, "/pos2 120,-60,120");
    run(&mut alice, "/set air");
    assert_eq!(block_name(&server, 120, -60, 120), "minecraft:air");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(items_near(&server), 0, "the stand was emptied, nothing dropped");
    assert_eq!(run(&mut alice, "/undo"), ["Undid 1 available edits."]);
    let potions = server.handle.run(|s| s.worlds[0].chunks[&paper_server::world::chunk::ChunkPos::of_block(120, 120)].block_entities.get(&(-60, 120 & 15, 120 & 15)).and_then(|b| b.list("Items").map(|l| l.len()))).unwrap();
    assert_eq!((block_name(&server, 120, -60, 120), potions), ("minecraft:brewing_stand", Some(1)), "back with its potion");
    server.stop();
}
