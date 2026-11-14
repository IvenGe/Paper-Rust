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

    // In the End: the main island with its exit portal, closed until the dragon dies.
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
    let (portal, end_stone, bedrock) = server.handle.run(move |s| (count_blocks(s, end, &["minecraft:end_portal"]), count_blocks(s, end, &["minecraft:end_stone"]), count_blocks(s, end, &["minecraft:bedrock"]))).unwrap();
    assert!(end_stone > 1000, "the main island: {end_stone}");
    assert_eq!(portal, 0, "the exit portal is closed");
    assert!(bedrock > 20, "the exit portal's bedrock: {bedrock}");
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
    // What the generator knows about structure starts, to compare with what gets saved.
    let expected: Vec<(ChunkPos, paper_nbt::Compound)> = server
        .handle
        .run(move |s| {
            let g = s.worlds[0].generator.staged().unwrap();
            let chunks: Vec<ChunkPos> = (-6..=6).flat_map(|dz| (-6..=6).map(move |dx| ChunkPos::new(cx + dx, cz + dz))).filter(|p| s.worlds[0].chunks.contains_key(p)).collect();
            chunks.into_iter().map(|p| (p, g.inner.save_structures(p.x, p.z))).collect()
        })
        .unwrap();
    assert!(!expected.is_empty());
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
    // Structure starts and references are saved with each chunk, in vanilla's layout.
    for (pos, structures) in &expected {
        let saved = storage.load_structures(*pos).unwrap().expect("a saved structures compound");
        assert!(saved.compound("starts").is_some() && saved.compound("References").is_some(), "chunk {pos:?}");
        assert_eq!(&saved, structures, "chunk {pos:?} saves the generator's starts");
    }
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

#[test]
fn structures_are_located_and_eyes_of_ender_lead_to_strongholds() {
    if !have_jar() {
        return;
    }
    let server = start_server(vanilla);
    let mut alice = Client::join(server.port(), "Alice");
    let at: play::PlayerPosition = alice.expect();
    server.handle.console_command("op Alice");
    alice.drain(Duration::from_millis(500));
    alice.clear_backlog();

    // /locate finds the nearest village, as vanilla reports it.
    alice.command("locate structure village_plains");
    let found: play::SystemChat = alice.find(Duration::from_secs(60), |m: &play::SystemChat| m.content.to_plain().contains("village_plains")).expect("a locate result");
    let text = format!("{:?}", found.content);
    assert!(text.contains("commands.locate.structure.success"), "{text}");
    alice.command("locate structure #minecraft:village");
    let tagged: play::SystemChat = alice.find(Duration::from_secs(60), |m: &play::SystemChat| m.content.to_plain().contains("#minecraft:village")).expect("a tag result");
    assert!(format!("{:?}", tagged.content).contains("commands.locate.structure.success"), "{:?}", tagged.content);

    // A thrown eye of ender flies towards the nearest stronghold.
    let stronghold = server
        .handle
        .run(move |s| s.worlds[0].generator.staged().and_then(|g| g.inner.locate("minecraft:stronghold", at.x.floor() as i32, at.z.floor() as i32, 100)).map(|l| (l.x, l.z)))
        .unwrap()
        .expect("a stronghold");
    let id = alice.entity_id;
    let eye = paper_data::items::id("minecraft:ender_eye").unwrap() as i32;
    server.handle.run(move |s| s.set_inventory_slot(id, 36, paper_protocol::types::Slot::of(eye, 1))).unwrap();
    alice.send(&play::UseItem { hand: paper_protocol::codec::VarInt(0), sequence: paper_protocol::codec::VarInt(1), yaw: 0.0, pitch: 0.0 });
    wait_for(&server, "the eye to fly", Duration::from_secs(10), |s| s.worlds[0].entities.values().any(|e| e.info.name == "minecraft:eye_of_ender" && e.age > 10));
    let (start, now) = server
        .handle
        .run(move |s| {
            let e = s.worlds[0].entities.values().find(|e| e.info.name == "minecraft:eye_of_ender").unwrap();
            ((at.x, at.z), (e.pos.x, e.pos.z))
        })
        .unwrap();
    let towards = ((stronghold.0 as f64 - start.0), (stronghold.1 as f64 - start.1));
    let moved = (now.0 - start.0, now.1 - start.1);
    assert!(towards.0 * moved.0 + towards.1 * moved.1 > 0.0, "the eye moves towards the stronghold at {stronghold:?}: {moved:?}");

    // The village is there when its chunks generate, with its villagers.
    let village = server
        .handle
        .run(move |s| s.worlds[0].generator.staged().and_then(|g| g.inner.locate("minecraft:village_plains", at.x.floor() as i32, at.z.floor() as i32, 100)).map(|l| (l.x, l.z)))
        .unwrap()
        .expect("a village");
    server
        .handle
        .run(move |s| {
            s.set_game_mode(id, paper_server::config::GameMode::Spectator);
            s.teleport(id, paper_server::server::player::Location { x: village.0 as f64 + 8.0, y: 120.0, z: village.1 as f64 + 8.0, yaw: 0.0, pitch: 0.0 });
        })
        .unwrap();
    wait_for(&server, "villagers", Duration::from_secs(120), move |s| {
        s.worlds[0].entities.values().any(|e| e.info.name == "minecraft:villager" && (e.pos.x - village.0 as f64).abs() < 96.0 && (e.pos.z - village.1 as f64).abs() < 96.0)
    });
    server.stop();

    // Without structures there is nothing to find.
    let server = start_server(|c| {
        vanilla(c);
        c.properties.generate_structures = false;
    });
    let none = server.handle.run(|s| s.worlds[0].generator.staged().and_then(|g| g.inner.locate("minecraft:village_plains", 0, 0, 100))).unwrap();
    assert!(none.is_none(), "{none:?}");
    server.stop();
}
