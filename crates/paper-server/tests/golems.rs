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
    // (Where it stands, it may already have left some snow.)
    assert!(matches!(block(&server, BlockPos::new(10, -60, 8)), "minecraft:air" | "minecraft:snow"));
    for y in -59..=-58 {
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

#[test]
fn snow_golems_leave_snow_melt_in_rain_and_throw_snowballs() {
    use paper_server::server::mobs::SpawnReason;
    use paper_server::world::entity::Vec3;
    let server = start_server(|c| c.properties.difficulty = paper_server::config::Difficulty::Normal);
    let _alice = join(&server);
    // Where it stands it leaves snow (with mob griefing), however the snow golem came to be.
    let golem = server.handle.run(|s| s.spawn_mob(0, "minecraft:snow_golem", Vec3::new(12.5, -60.0, 14.5), SpawnReason::Command).unwrap()).unwrap();
    let deadline = Instant::now() + TIMEOUT;
    while block(&server, BlockPos::new(12, -60, 14)) != "minecraft:snow" {
        assert!(Instant::now() < deadline, "no snow under the golem");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Without mob griefing, none.
    server
        .handle
        .run(move |s| {
            s.set_game_rule(0, "mob_griefing", paper_data::game_rules::Value::Bool(false));
            let e = s.worlds[0].entities.get_mut(&golem).unwrap();
            e.pos = Vec3::new(16.5, -60.0, 14.5);
            e.mob_mut().unwrap().brain.idle_ticks = 1_000_000;
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(block(&server, BlockPos::new(16, -60, 14)), "minecraft:air");

    // A monster in sight gets snowballs thrown at it.
    let zombie = server
        .handle
        .run(|s| {
            let zombie = s.spawn_mob(0, "minecraft:zombie", Vec3::new(22.5, -60.0, 14.5), SpawnReason::Command).unwrap();
            let e = s.worlds[0].entities.get_mut(&zombie).unwrap();
            e.flags.no_ai = true;
            zombie
        })
        .unwrap();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let thrown = server
            .handle
            .run(move |s| {
                s.worlds[0].entities.values().any(|e| e.info.name == "minecraft:snowball" && matches!(e.kind, paper_server::world::entity::EntityKind::Thrown { owner: Some(o), .. } if o == golem))
                    || s.worlds[0].entities.get(&golem).and_then(|e| e.mob()).is_some_and(|m| m.brain.target == Some(zombie) && m.brain.attack_cooldown > 0)
            })
            .unwrap();
        if thrown {
            break;
        }
        assert!(Instant::now() < deadline, "no snowball at the zombie");
        std::thread::sleep(Duration::from_millis(20));
    }

    // Rain hurts it.
    let health = server.handle.run(move |s| s.worlds[0].entities[&golem].mob().unwrap().living.health).unwrap();
    server.handle.console_command("weather rain");
    let deadline = Instant::now() + TIMEOUT;
    while server.handle.run(move |s| s.worlds[0].entities.get(&golem).and_then(|e| e.mob()).map_or(0.0, |m| m.living.health)).unwrap() >= health {
        assert!(Instant::now() < deadline, "the rain does not hurt the golem");
        std::thread::sleep(Duration::from_millis(50));
    }
    server.stop();
}
