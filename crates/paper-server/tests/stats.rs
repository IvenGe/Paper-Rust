//! Player statistics (vanilla's `ServerStatsCounter`): counted as players play, shown on the
//! statistics screen, added to scoreboard objectives with statistic criteria, and saved in the
//! main world's `stats/<uuid>.json`.

mod common;

use common::*;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(300);
const CUSTOM: &str = "minecraft:custom";

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("paper-rs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One of Alice's statistics (-1 while she is offline).
fn stat(server: &paper_server::RunningServer, kind: &'static str, key: &'static str) -> i32 {
    server.handle.run(move |s| s.players.values().find(|p| p.name() == "Alice").map_or(-1, |p| p.stats.get(kind, key))).unwrap()
}

/// Alice's score for an objective of the main scoreboard.
fn score(server: &paper_server::RunningServer, objective: &'static str) -> Option<i32> {
    server.handle.run(move |s| s.scoreboards.get(0).and_then(|b| b.scores.get("Alice")).and_then(|m| m.get(objective)).map(|sc| sc.value)).unwrap()
}

/// Waits while reading what the server sends, so keep-alives are answered.
fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(50));
        client.clear_backlog();
    }
}

#[test]
fn statistics_are_counted_shown_scored_and_saved() {
    let root = temp_root("stats");
    let config = |c: &mut paper_server::config::Config| {
        c.root = root.clone();
        c.properties.gamemode = paper_server::config::GameMode::Survival;
    };
    let server = start_server(config);
    server.handle.console_command("scoreboard objectives add jumps minecraft.custom:minecraft.jump");
    server.handle.console_command("scoreboard objectives add placed minecraft.used:minecraft.dirt");
    server.handle.console_command("scoreboard objectives add deaths deathCount");
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    alice.drain(QUIET);
    alice.move_to(8.5, -60.0, 8.5);
    wait_reading(&mut alice, "Alice on the ground", || server.handle.run(|s| s.players.values().all(|p| p.on_ground)).unwrap());
    let walked = stat(&server, CUSTOM, "minecraft:walk_one_cm");

    // Two jumps (off the ground and up), then a walk of three blocks.
    for _ in 0..2 {
        alice.send(&play::MovePlayerPos { x: 8.5, y: -59.58, z: 8.5, flags: 0 });
        alice.send(&play::ClientTickEnd {});
        alice.send(&play::MovePlayerPos { x: 8.5, y: -60.0, z: 8.5, flags: play::move_flags::ON_GROUND });
        alice.send(&play::ClientTickEnd {});
    }
    for x in [9.5, 10.5, 11.5] {
        alice.move_to(x, -60.0, 8.5);
    }
    wait_reading(&mut alice, "the walk", || stat(&server, CUSTOM, "minecraft:walk_one_cm") >= walked + 300);
    assert_eq!(stat(&server, CUSTOM, "minecraft:walk_one_cm"), walked + 300, "centimetres walked on the ground");
    assert_eq!(stat(&server, CUSTOM, "minecraft:jump"), 2);
    assert_eq!(score(&server, "jumps"), Some(2), "objectives with the jump criteria count jumps");
    assert!(stat(&server, CUSTOM, "minecraft:play_time") > 0, "play time counts every tick");

    // A dirt block placed on the grass beside her and broken again by hand.
    server.handle.console_command("give Alice minecraft:dirt 1");
    alice.find(TIMEOUT, |s: &play::ContainerSetSlot| s.slot == 36 && !s.item.is_empty()).expect("dirt in the hotbar");
    let dirt = BlockPos::new(12, -60, 8);
    alice.send(&play::UseItemOn {
        hand: VarInt(0),
        pos: BlockPos::new(12, -61, 8),
        face: VarInt(1),
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        world_border_hit: false,
        sequence: VarInt(1),
    });
    wait_reading(&mut alice, "the dirt placed", || stat(&server, "minecraft:used", "minecraft:dirt") == 1);
    assert_eq!(score(&server, "placed"), Some(1), "objectives follow typed statistics too");
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: dirt, face: 1, sequence: VarInt(2) });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::STOP_DESTROY_BLOCK), pos: dirt, face: 1, sequence: VarInt(3) });
    wait_reading(&mut alice, "the dirt mined", || stat(&server, "minecraft:mined", "minecraft:dirt") == 1);
    // She steps where it was, picks its drop up and throws it away again.
    alice.move_to(12.5, -60.0, 8.5);
    wait_reading(&mut alice, "the dirt picked up", || stat(&server, "minecraft:picked_up", "minecraft:dirt") == 1);
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::DROP_ITEM), pos: BlockPos::new(0, 0, 0), face: 0, sequence: VarInt(0) });
    wait_reading(&mut alice, "the dirt dropped", || stat(&server, "minecraft:dropped", "minecraft:dirt") == 1);
    assert_eq!(stat(&server, CUSTOM, "minecraft:drop"), 1);

    // The statistics screen asks for them.
    alice.clear_backlog();
    alice.send(&play::ClientCommand { action: VarInt(1) });
    let shown: play::AwardStats = alice.expect();
    let id = |registry: &str, name: &str| VarInt(paper_data::registries::id(registry, name).unwrap() as i32);
    let jumps = (id("minecraft:stat_type", CUSTOM), id("minecraft:custom_stat", "minecraft:jump"), VarInt(2));
    let mined = (id("minecraft:stat_type", "minecraft:mined"), id("minecraft:block", "minecraft:dirt"), VarInt(1));
    assert!(shown.stats.contains(&jumps) && shown.stats.contains(&mined), "{:?}", shown.stats);

    // Dying counts, and so does the death criteria.
    server.handle.console_command("kill Alice");
    wait_reading(&mut alice, "Alice's death", || stat(&server, CUSTOM, "minecraft:deaths") == 1);
    assert_eq!(score(&server, "deaths"), Some(1));
    assert_eq!(stat(&server, CUSTOM, "minecraft:time_since_death"), 0);
    let uuid = server.handle.run(|s| s.players.values().find(|p| p.name() == "Alice").unwrap().uuid()).unwrap();
    server.stop();

    // They are saved in the main world's stats folder...
    let file = root.join("world").join("stats").join(format!("{uuid}.json"));
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).expect("stats file")).unwrap();
    assert_eq!(saved["stats"][CUSTOM]["minecraft:jump"], 2, "{saved}");
    assert_eq!(saved["stats"]["minecraft:mined"]["minecraft:dirt"], 1, "{saved}");
    assert!(saved["DataVersion"].as_i64().is_some_and(|v| v > 0));

    // ...read by plugins while she is offline, changed for her, and back when she joins.
    let server = start_server(config);
    assert_eq!(server.handle.run(move |s| s.statistic(uuid, CUSTOM, "minecraft:jump")).unwrap(), 2);
    server.handle.run(move |s| s.set_statistic(uuid, CUSTOM, "minecraft:jump", 7)).unwrap();
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    assert_eq!(stat(&server, CUSTOM, "minecraft:jump"), 7);
    assert_eq!(stat(&server, "minecraft:mined", "minecraft:dirt"), 1);
    assert_eq!(stat(&server, CUSTOM, "minecraft:deaths"), 1);
    server.stop();
}
