//! Sculk: sensors hear what players do as vibrations (not through wool) and give a redstone
//! signal; calibrated sensors hear only the frequency at their back; shriekers set off by a
//! sensor warn the players near, bring darkness and at the fourth warning call the warden;
//! catalysts bloom where something dies near them and spread sculk with its experience.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::GameMode;
use paper_server::server::player::Location;
use paper_server::server::survival::Damage;
use paper_server::world::entity::Vec3;
use std::time::{Duration, Instant};

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn state_at(server: &paper_server::RunningServer, pos: BlockPos) -> u32 {
    server.handle.run(move |s| s.worlds[0].block_at(pos.x, pos.y, pos.z)).unwrap().unwrap()
}

fn name_at(server: &paper_server::RunningServer, pos: BlockPos) -> &'static str {
    blocks::block_of_state(state_at(server, pos)).name
}

fn prop(server: &paper_server::RunningServer, pos: BlockPos, name: &str) -> String {
    blocks::property(state_at(server, pos), name).unwrap_or("").to_owned()
}

fn put(server: &paper_server::RunningServer, pos: BlockPos, state: &str) {
    let state = blocks::parse_state(state).unwrap();
    server.handle.run(move |s| s.set_block_and_update(0, pos, state)).unwrap();
}

/// Alice, next to `at`, places stone on the ground there (y -61, so the stone is at y -60).
fn place_stone(server: &paper_server::RunningServer, alice: &mut Client, at: (i32, i32), sequence: i32) {
    let id = alice.entity_id;
    let stone = items::id("minecraft:stone").unwrap() as i32;
    server
        .handle
        .run(move |s| {
            s.teleport(id, Location { x: at.0 as f64 + 0.5, y: -60.0, z: at.1 as f64 + 2.5, yaw: 0.0, pitch: 0.0 });
            s.set_inventory_slot(id, 36, Slot::of(stone, 16));
        })
        .unwrap();
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: BlockPos::new(at.0, -61, at.1), face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(sequence) });
    let placed = BlockPos::new(at.0, -60, at.1);
    wait_until("the stone", TIMEOUT, || name_at(server, placed) == "minecraft:stone");
}

#[test]
fn sculk_hears_shrieks_and_spreads() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    let id = alice.entity_id;
    alice.expect::<play::PlayerPosition>();
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    // No animals walking about to set the sensors off.
    server
        .handle
        .run(|s| {
            s.set_game_rule(0, "spawn_mobs", paper_data::game_rules::Value::Bool(false));
            s.worlds[0].entities.values_mut().for_each(|e| e.removed = true);
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));

    // A sensor three blocks from a block placed: active, power 15 - 15 / 8 * 3 = 10, the
    // frequency of `block_place` (13) for comparators; then it cools down and is idle again.
    let sensor = BlockPos::new(8, -60, 8);
    put(&server, sensor, "minecraft:sculk_sensor");
    place_stone(&server, &mut alice, (11, 8), 1);
    wait_until("the sensor to hear it", TIMEOUT, || prop(&server, sensor, "sculk_sensor_phase") == "active");
    assert_eq!(prop(&server, sensor, "power"), "10");
    let frequency = server.handle.run(move |s| s.block_entity_data(0, sensor).and_then(|c| c.i32("last_vibration_frequency"))).unwrap();
    assert_eq!(frequency, Some(13));
    wait_until("the sensor to cool down", TIMEOUT, || prop(&server, sensor, "sculk_sensor_phase") == "cooldown");
    assert_eq!(prop(&server, sensor, "power"), "0");
    wait_until("the sensor to be idle", TIMEOUT, || prop(&server, sensor, "sculk_sensor_phase") == "inactive");

    // Wool in the way: not heard. Without it: heard.
    let muffled = BlockPos::new(8, -60, 30);
    put(&server, muffled, "minecraft:sculk_sensor");
    put(&server, BlockPos::new(10, -60, 30), "minecraft:white_wool");
    place_stone(&server, &mut alice, (11, 30), 2);
    std::thread::sleep(Duration::from_millis(1500));
    let heard = server.handle.run(move |s| s.block_entity_data(0, muffled).cloned()).unwrap();
    assert_eq!(prop(&server, muffled, "sculk_sensor_phase"), "inactive", "wool blocks vibrations: {heard:?}");
    put(&server, BlockPos::new(10, -60, 30), "minecraft:air");
    place_stone(&server, &mut alice, (12, 30), 3);
    wait_until("the sensor to hear past the gap", TIMEOUT, || prop(&server, muffled, "sculk_sensor_phase") == "active");

    // A calibrated sensor with full power at its back hears only frequency 15.
    let calibrated = BlockPos::new(30, -60, 8);
    put(&server, calibrated, "minecraft:calibrated_sculk_sensor[facing=north]");
    put(&server, BlockPos::new(30, -60, 9), "minecraft:redstone_block");
    place_stone(&server, &mut alice, (33, 8), 4);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(prop(&server, calibrated, "sculk_sensor_phase"), "inactive", "tuned to 15, not 13");
    put(&server, BlockPos::new(30, -60, 9), "minecraft:air");
    place_stone(&server, &mut alice, (34, 8), 5);
    wait_until("the untuned sensor to hear", TIMEOUT, || prop(&server, calibrated, "sculk_sensor_phase") == "active");
    assert_eq!(prop(&server, calibrated, "power"), "12", "15 - 15 / 16 * 4");

    // A sensor next to a shrieker that can call the warden: Alice's first warning, then
    // darkness when the shriek ends.
    let shrieker = BlockPos::new(30, -60, 30);
    put(&server, shrieker, "minecraft:sculk_shrieker[can_summon=true]");
    put(&server, BlockPos::new(31, -60, 30), "minecraft:sculk_sensor");
    place_stone(&server, &mut alice, (34, 30), 6);
    wait_until("the shriek", TIMEOUT, || prop(&server, shrieker, "shrieking") == "true");
    let warning = server.handle.run(move |s| s.players[&id].warden.warning_level).unwrap();
    assert_eq!(warning, 1);
    let darkness = paper_data::effects::by_name("minecraft:darkness").unwrap().id;
    wait_until("darkness", TIMEOUT, || server.handle.run(move |s| s.players[&id].effects.iter().any(|e| e.id == darkness)).unwrap());
    assert_eq!(prop(&server, shrieker, "shrieking"), "false");

    // The fourth warning calls the warden.
    server
        .handle
        .run(move |s| {
            let p = s.players.get_mut(&id).unwrap();
            p.warden.warning_level = 3;
            p.warden.cooldown_ticks = 0;
        })
        .unwrap();
    wait_until("the sensor to rest", TIMEOUT, || prop(&server, BlockPos::new(31, -60, 30), "sculk_sensor_phase") == "inactive");
    place_stone(&server, &mut alice, (35, 30), 7);
    wait_until("the second shriek", TIMEOUT, || prop(&server, shrieker, "shrieking") == "true");
    let warden = |server: &paper_server::RunningServer| server.handle.run(|s| s.worlds[0].entities.values().find(|e| e.info.name == "minecraft:warden").map(|e| e.pos)).unwrap();
    wait_until("the warden", TIMEOUT, || warden(&server).is_some());
    let at = warden(&server).unwrap();
    assert!((at.x - 30.5).abs() <= 6.0 && (at.z - 30.5).abs() <= 6.0, "near the shrieker: {at:?}");

    // A cow killed by Alice next to a catalyst: it blooms, no experience drops, and sculk
    // spreads from where the cow died.
    let catalyst = BlockPos::new(-20, -60, -20);
    put(&server, catalyst, "minecraft:sculk_catalyst");
    let cow = server
        .handle
        .run(move |s| {
            s.teleport(id, Location { x: -17.5, y: -60.0, z: -17.5, yaw: 0.0, pitch: 0.0 });
            s.spawn_mob(0, "minecraft:cow", Vec3::new(-17.5, -60.0, -19.5), paper_server::server::mobs::SpawnReason::Command)
        })
        .unwrap()
        .expect("a cow");
    server.handle.run(move |s| s.hurt_entity(0, cow, Damage::by("minecraft:player_attack", id, Vec3::new(-17.5, -60.0, -17.5)), 100.0)).unwrap();
    wait_until("the bloom", TIMEOUT, || prop(&server, catalyst, "bloom") == "true");
    let orbs = server.handle.run(|s| s.worlds[0].entities.values().filter(|e| e.info.name == "minecraft:experience_orb").count()).unwrap();
    assert_eq!(orbs, 0, "the catalyst took the experience");
    let sculk = |server: &paper_server::RunningServer| {
        server
            .handle
            .run(|s| (-24..=-12).flat_map(|x| (-24..=-12).map(move |z| (x, z))).filter(|(x, z)| (-62..=-60).any(|y| s.worlds[0].block_at(*x, y, *z).is_some_and(|b| matches!(blocks::block_of_state(b).name, "minecraft:sculk" | "minecraft:sculk_vein")))).count())
            .unwrap()
    };
    wait_until("sculk", TIMEOUT, || sculk(&server) > 0);
    wait_until("the bloom to end", TIMEOUT, || prop(&server, catalyst, "bloom") == "false");
    server.stop();
}
