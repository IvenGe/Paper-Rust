//! Vanilla world generation in the running server: worlds are generated, decorated and
//! finished before players see them, and unfinished chunks survive a restart.
//!
//! These tests need the vanilla jar (see `vanilla::test_jar`); without it they pass without
//! checking anything.

mod common;

use common::*;
use paper_data::blocks;
use paper_protocol::packets::play;
use paper_server::world::chunk::ChunkPos;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn have_jar() -> bool {
    let found = paper_server::vanilla::test_jar().is_some();
    if !found {
        eprintln!("skipping: no vanilla jar");
    }
    found
}

fn vanilla(c: &mut paper_server::config::Config) {
    c.properties.level_type = "minecraft:normal".into();
}

fn wait_for(server: &paper_server::RunningServer, what: &str, timeout: Duration, f: impl Fn(&mut paper_server::server::Server) -> bool + Send + Sync + Copy + 'static) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if server.handle.run(f) == Some(true) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

/// Counts blocks of the given names in the finished chunks of a world.
fn count_blocks(s: &paper_server::server::Server, world: usize, names: &[&str]) -> usize {
    let w = &s.worlds[world];
    let mut count = 0;
    for chunk in w.chunks.values() {
        for y in w.min_y..w.min_y + w.height {
            for z in 0..16 {
                for x in 0..16 {
                    if names.contains(&blocks::block_of_state(chunk.get_block(x, y, z)).name) {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

#[test]
fn players_join_a_decorated_vanilla_world() {
    if !have_jar() {
        return;
    }
    let server = start_server(vanilla);
    let name = server.handle.run(|s| s.worlds[0].generator.name()).unwrap();
    assert_eq!(name, "vanilla");
    let spawn = server.handle.run(|s| s.worlds[0].spawn).unwrap();

    let mut alice = Client::join(server.port(), "Alice");
    let at: play::PlayerPosition = alice.expect();
    assert!((at.x - (spawn.x as f64 + 0.5)).abs() <= 11.0 && (at.z - (spawn.z as f64 + 0.5)).abs() <= 11.0, "near the spawn {spawn:?}: {at:?}");
    let (cx, cz) = (spawn.x >> 4, spawn.z >> 4);
    alice.find(Duration::from_secs(60), |c: &play::LevelChunkWithLight| c.x == cx && c.z == cz).expect("the spawn chunk arrives");

    // The spawn stands on solid ground with room above.
    let ground = server.handle.run(move |s| s.worlds[0].block_at(spawn.x, spawn.y - 1, spawn.z)).unwrap();
    assert!(ground.is_some_and(blocks::has_collision), "solid ground under the spawn: {:?}", ground.map(|g| blocks::block_of_state(g).name));

    // Chunks players see are finished: decorated, with ores from the features stage; the
    // unfinished ones around them are kept apart.
    wait_for(&server, "the view to be lit", Duration::from_secs(60), move |s| (-2..=2).all(|dz| (-2..=2).all(|dx| s.worlds[0].is_ready(ChunkPos::new(cx + dx, cz + dz)))));
    let (ores, unfinished_overlap) = server
        .handle
        .run(|s| {
            let ores = count_blocks(s, 0, &["minecraft:coal_ore", "minecraft:iron_ore", "minecraft:copper_ore", "minecraft:deepslate_iron_ore"]);
            let overlap = s.worlds[0].proto.keys().any(|p| s.worlds[0].chunks.contains_key(p));
            (ores, overlap)
        })
        .unwrap();
    assert!(ores > 50, "ores are placed: {ores}");
    assert!(!unfinished_overlap, "a chunk is either finished or not");
    server.stop();
}

#[test]
fn the_nether_and_the_end_are_generated_like_vanilla() {
    if !have_jar() {
        return;
    }
    let server = start_server(|_| {});
    let names = server.handle.run(|s| s.worlds.iter().map(|w| w.generator.name()).collect::<Vec<_>>()).unwrap();
    assert_eq!(names, vec!["flat", "vanilla", "vanilla"], "the flat test world still has a vanilla Nether and End");
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");

    // In the End: the main island with its open exit portal.
    let end = server.handle.run(|s| s.world_of_dimension("minecraft:the_end")).unwrap().expect("an End");
    server
        .handle
        .run(move |s| {
            for cz in -1..=0 {
                for cx in -1..=0 {
                    s.worlds[end].add_ticket(ChunkPos::new(cx, cz));
                }
            }
        })
        .unwrap();
    wait_for(&server, "the End around the origin to be finished", Duration::from_secs(60), move |s| (-1..=0).all(|cz| (-1..=0).all(|cx| s.worlds[end].is_ready(ChunkPos::new(cx, cz)))));
    let (portal, end_stone) = server.handle.run(move |s| (count_blocks(s, end, &["minecraft:end_portal"]), count_blocks(s, end, &["minecraft:end_stone"]))).unwrap();
    assert!(end_stone > 1000, "the main island: {end_stone}");
    assert_eq!(portal, 20, "the exit portal is open");
    server.stop();
}

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("paper-rs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn unfinished_chunks_are_saved_and_finished_after_a_restart() {
    if !have_jar() {
        return;
    }
    let root = temp_root("worldgen-restart");
    let server = start_server(|c| {
        vanilla(c);
        c.root = root.clone();
        c.properties.allow_nether = false;
        c.paper.world.allow_end = false;
    });
    let spawn = server.handle.run(|s| s.worlds[0].spawn).unwrap();
    let (cx, cz) = (spawn.x >> 4, spawn.z >> 4);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    wait_for(&server, "the spawn area to be lit", Duration::from_secs(60), move |s| s.worlds[0].is_ready(ChunkPos::new(cx, cz)));
    // Snapshot a finished chunk and note which chunks are unfinished.
    let (blocks_before, unfinished) = server
        .handle
        .run(move |s| {
            let chunk = &s.worlds[0].chunks[&ChunkPos::new(cx, cz)];
            let states: Vec<u32> = (s.worlds[0].min_y..s.worlds[0].min_y + s.worlds[0].height).map(|y| chunk.get_block(7, y, 7)).collect();
            (states, s.worlds[0].proto.len())
        })
        .unwrap();
    assert!(unfinished > 0, "the edge of the generated area is unfinished");
    drop(alice);
    server.stop();

    // The region files hold finished and unfinished chunks with vanilla's statuses.
    let storage = paper_server::world::storage::WorldStorage::open(&root.join("world"), paper_server::world::storage::ChunkIo::start()).unwrap();
    let mut statuses = std::collections::HashMap::new();
    for dz in -12..=12 {
        for dx in -12..=12 {
            if let Ok(Some(chunk)) = storage.load_chunk(ChunkPos::new(cx + dx, cz + dz), -64, 384, true) {
                *statuses.entry(format!("{:?}", chunk.status)).or_insert(0) += 1;
            }
        }
    }
    assert!(statuses.contains_key("Full") && (statuses.contains_key("Decorated") || statuses.contains_key("Terrain")), "{statuses:?}");
    drop(storage);

    // After a restart the finished chunk is the same, and generation carries on.
    let server = start_server(|c| {
        vanilla(c);
        c.root = root.clone();
        c.properties.allow_nether = false;
        c.paper.world.allow_end = false;
    });
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    wait_for(&server, "the spawn area to be lit again", Duration::from_secs(60), move |s| (-2..=2).all(|dz| (-2..=2).all(|dx| s.worlds[0].is_ready(ChunkPos::new(cx + dx, cz + dz)))));
    let blocks_after: Vec<u32> = server
        .handle
        .run(move |s| {
            let chunk = &s.worlds[0].chunks[&ChunkPos::new(cx, cz)];
            (s.worlds[0].min_y..s.worlds[0].min_y + s.worlds[0].height).map(|y| chunk.get_block(7, y, 7)).collect()
        })
        .unwrap();
    assert_eq!(blocks_before, blocks_after);
    drop(alice);
    server.stop();
    let _ = std::fs::remove_dir_all(&root);
}
