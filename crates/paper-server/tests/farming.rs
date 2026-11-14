//! Farming and random ticks: hoes till, seeds need farmland, crops grow on random ticks and
//! with bone meal, farmland gets wet near water and is trampled, unattached leaves decay.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::GameMode;
use std::time::{Duration, Instant};

fn join(server: &paper_server::RunningServer) -> Client {
    let mut client = Client::join(server.port(), "Alice");
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(Duration::from_millis(300));
    client.clear_backlog();
    client
}

fn use_on(pos: BlockPos, sequence: i32) -> play::UseItemOn {
    play::UseItemOn { hand: VarInt(0), pos, face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(sequence) }
}

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

fn name_at(server: &paper_server::RunningServer, pos: BlockPos) -> String {
    server.handle.run(move |s| s.worlds[0].block_at(pos.x, pos.y, pos.z).map(|b| blocks::block_of_state(b).name.to_owned())).unwrap().unwrap_or_default()
}

fn state_at(server: &paper_server::RunningServer, pos: BlockPos) -> u32 {
    server.handle.run(move |s| s.worlds[0].block_at(pos.x, pos.y, pos.z)).unwrap().unwrap()
}

fn hold(server: &paper_server::RunningServer, id: i32, stack: Slot) {
    server.handle.run(move |s| s.set_inventory_slot(id, 36, stack)).unwrap();
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn hoes_till_seeds_grow_and_farmland_is_trampled() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = join(&server);
    let id = alice.entity_id;
    let (soil, crop) = (BlockPos::new(10, -61, 8), BlockPos::new(10, -60, 8));

    // Seeds need farmland: on grass nothing happens.
    hold(&server, id, Slot::of(item("minecraft:wheat_seeds"), 4));
    alice.send(&use_on(soil, 1));
    alice.drain(Duration::from_millis(300));
    assert_eq!(name_at(&server, crop), "minecraft:air", "no wheat on grass");

    // A hoe tills the grass; then the seeds go in.
    hold(&server, id, Slot::of(item("minecraft:iron_hoe"), 1));
    alice.send(&use_on(soil, 2));
    wait_until("the grass to be tilled", TIMEOUT, || name_at(&server, soil) == "minecraft:farmland");
    hold(&server, id, Slot::of(item("minecraft:wheat_seeds"), 4));
    alice.send(&use_on(soil, 3));
    wait_until("the seeds to be planted", TIMEOUT, || name_at(&server, crop) == "minecraft:wheat");

    // Bone meal makes it grow by two to five stages.
    hold(&server, id, Slot::of(item("minecraft:bone_meal"), 1));
    alice.send(&use_on(crop, 4));
    wait_until("bone meal to work", TIMEOUT, || blocks::property(state_at(&server, crop), "age").is_some_and(|a| a != "0"));
    let age: i32 = blocks::property(state_at(&server, crop), "age").unwrap().parse().unwrap();
    assert!((2..=5).contains(&age), "grew by {age}");

    // Random ticks do the rest (many per tick here); water nearby wets the farmland.
    let water = BlockPos::new(12, -61, 8);
    server
        .handle
        .run(move |s| {
            s.set_game_rule(0, "random_tick_speed", paper_data::game_rules::Value::Int(4096));
            s.set_block_and_update(0, water, blocks::default_state("minecraft:water").unwrap());
        })
        .unwrap();
    wait_until("the wheat to be ripe", Duration::from_secs(30), || blocks::property(state_at(&server, crop), "age") == Some("7"));
    wait_until("the farmland to get wet", Duration::from_secs(10), || blocks::property(state_at(&server, soil), "moisture") == Some("7"));

    // Leaves far from any log decay.
    let leaf = BlockPos::new(6, -60, 4);
    server.handle.run(move |s| s.set_block_and_update(0, leaf, blocks::parse_state("oak_leaves[distance=7,persistent=false]").unwrap())).unwrap();
    wait_until("the leaves to decay", Duration::from_secs(10), || name_at(&server, leaf) == "minecraft:air");

    // Jumping onto farmland tramples it (and the crop pops off).
    let (x, z) = (10.5, 8.5);
    alice.send(&play::MovePlayerPos { x, y: -58.5, z, flags: 0 });
    alice.send(&play::ClientTickEnd {});
    alice.send(&play::MovePlayerPos { x, y: -61.0 + 15.0 / 16.0, z, flags: 1 });
    alice.send(&play::ClientTickEnd {});
    // (Grass grows back onto the dirt quickly at this tick speed.)
    wait_until("the farmland to be trampled", TIMEOUT, || matches!(name_at(&server, soil).as_str(), "minecraft:dirt" | "minecraft:grass_block"));
    wait_until("the wheat to pop off", TIMEOUT, || name_at(&server, crop) == "minecraft:air");
    server.stop();
}

#[test]
fn axes_strip_logs_and_shovels_make_paths() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = join(&server);
    let id = alice.entity_id;
    let (log, grass) = (BlockPos::new(10, -60, 8), BlockPos::new(9, -61, 6));
    server.handle.run(move |s| s.set_block_and_update(0, log, blocks::parse_state("oak_log[axis=x]").unwrap())).unwrap();
    hold(&server, id, Slot::of(item("minecraft:iron_axe"), 1));
    alice.send(&use_on(log, 1));
    wait_until("the log to be stripped", TIMEOUT, || name_at(&server, log) == "minecraft:stripped_oak_log");
    assert_eq!(blocks::property(state_at(&server, log), "axis"), Some("x"), "the axis stays");
    hold(&server, id, Slot::of(item("minecraft:iron_shovel"), 1));
    alice.send(&use_on(grass, 2));
    wait_until("a path", TIMEOUT, || name_at(&server, grass) == "minecraft:dirt_path");
    server.stop();
}

#[test]
fn saplings_grow_into_trees() {
    if paper_server::vanilla::test_jar().is_none() {
        eprintln!("skipping: no vanilla jar");
        return;
    }
    // A flat world: the tree features come from the vanilla jar all the same.
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = join(&server);
    let id = alice.entity_id;

    // A sapling at its second stage grows into an oak on random ticks (in daylight).
    let oak = BlockPos::new(12, -60, 12);
    server
        .handle
        .run(move |s| {
            s.worlds[0].day_time = 1000;
            s.set_game_rule(0, "random_tick_speed", paper_data::game_rules::Value::Int(4096));
            s.set_block_and_update(0, oak, blocks::parse_state("oak_sapling[stage=1]").unwrap());
        })
        .unwrap();
    wait_until("the oak to grow", Duration::from_secs(30), || matches!(name_at(&server, oak).as_str(), "minecraft:oak_log"));
    let leaves = server
        .handle
        .run(move |s| (-3..=3).any(|dx| (2..10).any(|dy| s.worlds[0].block_at(oak.x + dx, oak.y + dy, oak.z).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:oak_leaves"))))
        .unwrap();
    assert!(leaves, "the oak has leaves");
    server.handle.run(|s| s.set_game_rule(0, "random_tick_speed", paper_data::game_rules::Value::Int(0))).unwrap();

    // Dark oaks need four saplings in a square; bone meal grows them.
    let corner = BlockPos::new(4, -60, 20);
    server
        .handle
        .run(move |s| {
            // Within reach (the spawn point is spread at random).
            s.teleport(id, paper_server::server::player::Location { x: 7.5, y: -60.0, z: 21.5, yaw: 0.0, pitch: 0.0 });
            for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                s.set_block_and_update(0, corner.offset(dx, 0, dz), blocks::parse_state("dark_oak_sapling[stage=1]").unwrap());
            }
        })
        .unwrap();
    for sequence in 1..40 {
        hold(&server, id, Slot::of(item("minecraft:bone_meal"), 1));
        alice.send(&use_on(corner, sequence));
        alice.drain(Duration::from_millis(100));
        if name_at(&server, corner) == "minecraft:dark_oak_log" {
            break;
        }
    }
    assert_eq!(name_at(&server, corner), "minecraft:dark_oak_log", "a dark oak grew");
    assert_eq!(name_at(&server, corner.offset(1, 0, 1)), "minecraft:dark_oak_log", "with a 2x2 trunk");
    server.stop();
}
