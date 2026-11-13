//! Block physics meeting entities: powered TNT, items dropped by blocks that water washes
//! away.

mod common;

use common::*;
use paper_data::blocks;
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::play;
use paper_server::config::GameMode;
use paper_server::world::entity::EntityKind;
use std::time::Duration;

fn join(server: &paper_server::RunningServer) -> Client {
    let mut client = Client::join(server.port(), "Alice");
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(Duration::from_millis(300));
    client.clear_backlog();
    client
}

#[test]
fn redstone_primes_tnt_which_explodes() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Creative);
    let mut alice = join(&server);
    let (tnt, power) = (BlockPos::new(14, -60, 8), BlockPos::new(15, -60, 8));
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, tnt, blocks::default_state("minecraft:tnt").unwrap());
            s.set_block_and_update(0, power, blocks::default_state("minecraft:redstone_block").unwrap());
        })
        .unwrap();
    let tnt_type = paper_data::entity_types::by_name("minecraft:tnt").unwrap().id;
    alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == tnt_type).expect("primed TNT appears");
    let gone = server.handle.run(move |s| s.worlds[0].block_at(tnt.x, tnt.y, tnt.z).map(blocks::is_air)).unwrap();
    assert_eq!(gone, Some(true), "the TNT block becomes the entity");
    // Four seconds later it explodes.
    let explosion: play::Explode = alice.find(Duration::from_secs(10), |_: &play::Explode| true).expect("explosion");
    assert!((explosion.center.0 - 14.5).abs() < 1.0);
    server.stop();
}

#[test]
fn water_washes_away_torches_which_drop() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Creative);
    let _alice = join(&server);
    let (torch, source) = (BlockPos::new(12, -60, 8), BlockPos::new(14, -60, 8));
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, torch, blocks::default_state("minecraft:torch").unwrap());
            s.set_block_and_update(0, source, blocks::default_state("minecraft:water").unwrap());
        })
        .unwrap();
    let torch_item = paper_data::items::id("minecraft:torch").unwrap() as i32;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (washed, dropped) = server
            .handle
            .run(move |s| {
                let here = s.worlds[0].block_at(torch.x, torch.y, torch.z).unwrap();
                let dropped = s.worlds[0].entities.values().any(|e| matches!(&e.kind, EntityKind::Item { stack, .. } if stack.0.as_ref().is_some_and(|i| i.item == torch_item)));
                (blocks::block_of_state(here).name == "minecraft:water", dropped)
            })
            .unwrap();
        if washed && dropped {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "water reaches the torch and it drops (washed: {washed}, dropped: {dropped})");
        std::thread::sleep(Duration::from_millis(100));
    }
    server.stop();
}
