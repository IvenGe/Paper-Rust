//! Worlds and players survive a restart: what one server instance changes, the next one
//! started on the same directory loads.

mod common;

use common::*;
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(300);

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("paper-rs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn wait_for(server: &paper_server::RunningServer, what: &str, f: impl Fn(&mut paper_server::server::Server) -> bool + Send + Sync + Copy + 'static) {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if server.handle.run(f) == Some(true) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn worlds_and_players_survive_a_restart() {
    let root = temp_root("persistence");
    let gold = paper_data::blocks::default_state("minecraft:gold_block").unwrap();
    let diamond = paper_data::items::id("minecraft:diamond").unwrap() as i32;

    // --- first run: change the world, the time and a player ---
    let server = start_server(|c| c.root = root.clone());
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    wait_for(&server, "the spawn chunk", |s| s.worlds[0].block_at(10, -61, 10).is_some());
    server.handle.console_command("setblock 10 -60 10 minecraft:gold_block");
    server.handle.console_command("give Alice diamond 5");
    server.handle.console_command("gamemode survival Alice");
    server.handle.console_command("time set 1000");
    wait_for(&server, "the commands", move |s| s.worlds[0].block_at(10, -60, 10) == Some(gold) && s.players.values().all(|p| p.game_mode == paper_server::config::GameMode::Survival));
    alice.move_to(12.5, -60.0, 12.5);
    wait_for(&server, "the move", |s| s.players.values().all(|p| p.loc.x == 12.5));
    // Explore a little so chunks away from spawn are generated (and must be saved too).
    alice.move_to(60.5, -60.0, 12.5);
    wait_for(&server, "the second move", |s| s.players.values().all(|p| p.loc.x == 60.5));
    alice.move_to(12.5, -60.0, 12.5);
    wait_for(&server, "the move back", |s| s.players.values().all(|p| p.loc.x == 12.5));
    let generated = server.handle.run(|s| s.worlds[0].loaded_chunk_count()).unwrap();
    server.stop();

    let world = root.join("world");
    assert!(world.join("level.dat").is_file(), "level.dat is written");
    assert!(world.join("region/r.0.0.mca").is_file(), "the region with the spawn chunks is written");
    assert!(world.join("playerdata").read_dir().unwrap().count() == 1, "the player's data is written");
    assert!(!world.join("session.lock").metadata().unwrap().permissions().readonly());

    // --- second run on the same directory ---
    let server = start_server(|c| c.root = root.clone());
    let (age, day_time) = server.handle.run(|s| (s.worlds[0].age, s.worlds[0].day_time)).unwrap();
    assert!(day_time >= 1000, "the time continues from level.dat (day time {day_time})");
    assert!(age > 0);

    let mut alice = Client::join(server.port(), "Alice");
    let login = alice.login.clone().expect("login packet");
    assert_eq!(login.spawn.game_mode, paper_server::config::GameMode::Survival.id(), "the game mode is restored");
    let pos: play::PlayerPosition = alice.expect();
    assert_eq!((pos.x, pos.y, pos.z), (12.5, -60.0, 12.5), "the player is where they logged out");
    let inventory: play::ContainerSetContent = alice.expect();
    assert_eq!(inventory.slots[36], Slot::of(diamond, 5), "the inventory is restored");

    // The changed block comes back from the region file, not from the generator.
    wait_for(&server, "the chunk to load", |s| s.worlds[0].block_at(10, -60, 10).is_some());
    assert_eq!(server.handle.run(|s| s.worlds[0].block_at(10, -60, 10)).unwrap(), Some(gold));
    assert!(generated > 0);
    server.stop();

    // A second server cannot open a world that is in use.
    let first = start_server(|c| c.root = root.clone());
    let second = paper_server::start({
        let mut c = paper_server::config::Config::for_tests(&root);
        c.root = root.clone();
        c
    });
    assert!(second.is_err(), "the world is locked by the first server");
    first.stop();
    let _ = std::fs::remove_dir_all(&root);
}
