//! Golems built from blocks: a pumpkin on two snow blocks, or on a T of iron blocks, in any
//! orientation, placed by anyone (a dispenser too).

mod common;

use common::*;
use paper_data::blocks;
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use std::time::{Duration, Instant};

fn set(server: &paper_server::RunningServer, pos: BlockPos, block: &'static str) {
    let state = blocks::default_state(block).unwrap();
    server.handle.run(move |s| s.set_block_and_update(0, pos, state)).unwrap();
}

fn block(server: &paper_server::RunningServer, pos: BlockPos) -> &'static str {
    server.handle.run(move |s| blocks::block_of_state(s.worlds[0].block_at(pos.x, pos.y, pos.z).unwrap()).name).unwrap()
}

/// Waits for a mob of the kind near a block, and answers where it stands.
fn wait_for(server: &paper_server::RunningServer, kind: &'static str, near: BlockPos) -> (f64, f64, f64) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let found = server
            .handle
            .run(move |s| s.worlds[0].entities.values().find(|e| e.info.name == kind && (e.pos.x - near.x as f64 - 0.5).abs() < 1.5 && (e.pos.z - near.z as f64 - 0.5).abs() < 1.5).map(|e| (e.pos.x, e.pos.y, e.pos.z)))
            .unwrap();
        if let Some(at) = found {
            return at;
        }
        assert!(Instant::now() < deadline, "no {kind} near {near:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A player near the spawn, so the chunks there are loaded (and stay so).
fn join(server: &paper_server::RunningServer) -> Client {
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    for x in [0, 1] {
        alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == x && c.z == 0).expect("the chunks near the spawn");
    }
    alice
}

fn count(server: &paper_server::RunningServer, kind: &'static str) -> usize {
    server.handle.run(move |s| s.worlds[0].entities.values().filter(|e| e.info.name == kind && !e.removed).count()).unwrap()
}

#[test]
fn pumpkins_on_snow_or_iron_make_golems() {
    let server = start_server(|_| {});
    let _alice = join(&server);
    // A snow golem: the blocks turn into it.
    set(&server, BlockPos::new(10, -60, 8), "minecraft:snow_block");
    set(&server, BlockPos::new(10, -59, 8), "minecraft:snow_block");
    set(&server, BlockPos::new(10, -58, 8), "minecraft:carved_pumpkin");
    let (_, y, _) = wait_for(&server, "minecraft:snow_golem", BlockPos::new(10, -60, 8));
    assert!((y + 59.95).abs() < 0.1, "standing where the lowest block was: {y}");
    for y in -60..=-58 {
        assert_eq!(block(&server, BlockPos::new(10, y, 8)), "minecraft:air");
    }

    // An iron golem from a T along the z axis, finished with a jack o'lantern.
    for at in [(14, -60, 8), (14, -59, 7), (14, -59, 8), (14, -59, 9)] {
        set(&server, BlockPos::new(at.0, at.1, at.2), "minecraft:iron_block");
    }
    set(&server, BlockPos::new(14, -58, 8), "minecraft:jack_o_lantern");
    wait_for(&server, "minecraft:iron_golem", BlockPos::new(14, -60, 8));
    assert_eq!(block(&server, BlockPos::new(14, -59, 7)), "minecraft:air", "its arm is gone");

    // Lying on its side works too: a snow golem along the x axis, head first.
    set(&server, BlockPos::new(19, -60, 12), "minecraft:snow_block");
    set(&server, BlockPos::new(20, -60, 12), "minecraft:snow_block");
    set(&server, BlockPos::new(18, -60, 12), "minecraft:carved_pumpkin");
    wait_for(&server, "minecraft:snow_golem", BlockPos::new(20, -60, 12));

    // Something beside the leg spoils an iron golem.
    for at in [(24, -60, 8), (24, -59, 7), (24, -59, 8), (24, -59, 9), (24, -60, 9)] {
        set(&server, BlockPos::new(at.0, at.1, at.2), "minecraft:iron_block");
    }
    let before = count(&server, "minecraft:iron_golem");
    set(&server, BlockPos::new(24, -58, 8), "minecraft:carved_pumpkin");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(count(&server, "minecraft:iron_golem"), before);
    assert_eq!(block(&server, BlockPos::new(24, -59, 8)), "minecraft:iron_block");
    server.stop();
}

#[test]
fn dispensers_finish_golems() {
    let server = start_server(|_| {});
    let _alice = join(&server);
    // Snow blocks under a dispenser facing down, three blocks up.
    set(&server, BlockPos::new(10, -60, 2), "minecraft:snow_block");
    set(&server, BlockPos::new(10, -59, 2), "minecraft:snow_block");
    let dispenser = BlockPos::new(10, -57, 2);
    let state = blocks::with_property(blocks::default_state("minecraft:dispenser").unwrap(), "facing", "down").unwrap();
    let pumpkin = paper_data::items::id("minecraft:carved_pumpkin").unwrap() as i32;
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, dispenser, state);
            let mut slots = s.container_items(0, dispenser);
            slots[0] = Slot::of(pumpkin, 1);
            s.set_container_items(0, dispenser, &slots);
        })
        .unwrap();
    set(&server, BlockPos::new(10, -56, 2), "minecraft:redstone_block");
    wait_for(&server, "minecraft:snow_golem", BlockPos::new(10, -60, 2));
    let left = server.handle.run(move |s| s.container_items(0, dispenser)[0].clone()).unwrap();
    assert!(left.is_empty(), "the pumpkin was used");
    server.stop();
}
