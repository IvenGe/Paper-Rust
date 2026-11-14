//! Falling blocks in the running server: unsupported gravel falls onto the ground, and an
//! anvil hurts what it lands on.

mod common;

use common::*;
use paper_data::blocks;
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::play;
use paper_server::config::GameMode;
use paper_server::server::mobs::SpawnReason;
use paper_server::world::entity::Vec3;
use std::time::{Duration, Instant};

/// Waits while reading what the server sends, so keep-alives are answered.
fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(20));
        client.clear_backlog();
    }
}

fn block_name(server: &paper_server::RunningServer, pos: BlockPos) -> &'static str {
    server.handle.run(move |s| blocks::block_of_state(s.worlds[0].block_at(pos.x, pos.y, pos.z).unwrap()).name).unwrap()
}

#[test]
fn gravel_falls_to_the_ground_and_anvils_hurt_what_they_land_on() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(Duration::from_millis(300));

    // Gravel set five blocks up falls onto the grass.
    let gravel = blocks::default_state("minecraft:gravel").unwrap();
    server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(6, -55, 6), gravel)).unwrap();
    wait_reading(&mut alice, "the gravel on the ground", || block_name(&server, BlockPos::new(6, -60, 6)) == "minecraft:gravel");
    assert_eq!(block_name(&server, BlockPos::new(6, -55, 6)), "minecraft:air");

    // An anvil dropped ten blocks onto a cow kills it (2 damage per block fallen).
    let cow = server
        .handle
        .run(|s| {
            let id = s.spawn_mob(0, "minecraft:cow", Vec3::new(10.5, -60.0, 10.5), SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&id).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
            id
        })
        .unwrap();
    let anvil = blocks::default_state("minecraft:anvil").unwrap();
    server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(10, -50, 10), anvil)).unwrap();
    wait_reading(&mut alice, "the anvil to land", || block_name(&server, BlockPos::new(10, -60, 10)).ends_with("anvil"));
    let alive = server.handle.run(move |s| s.worlds[0].entities.get(&cow).is_some_and(|e| e.is_alive() && !e.removed)).unwrap();
    assert!(!alive, "the cow under the anvil is killed");
    server.stop();
}
