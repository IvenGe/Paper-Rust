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

fn state(name: &str) -> u32 {
    blocks::default_state(name).unwrap()
}

fn with(s: u32, property: &str, value: &str) -> u32 {
    blocks::with_property(s, property, value).unwrap()
}

fn item(name: &str) -> i32 {
    paper_data::items::id(name).unwrap() as i32
}

fn wait_until(server: &paper_server::RunningServer, what: &str, f: impl Fn(&mut paper_server::server::Server) -> bool + Send + Sync + Copy + 'static) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if server.handle.run(f) == Some(true) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn hoppers_move_items_between_chests_and_pick_up_items() {
    use paper_protocol::types::Slot;
    let server = start_server(|c| c.properties.gamemode = GameMode::Creative);
    let _alice = join(&server);
    // A chest on top of a hopper that feeds a chest beside it.
    let (top, hopper, side) = (BlockPos::new(12, -59, 8), BlockPos::new(12, -60, 8), BlockPos::new(13, -60, 8));
    let diamond = item("minecraft:diamond");
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, side, state("minecraft:chest"));
            s.set_block_and_update(0, hopper, with(state("minecraft:hopper"), "facing", "east"));
            s.set_block_and_update(0, top, state("minecraft:chest"));
            let mut items = vec![Slot::EMPTY; 27];
            items[0] = Slot::of(diamond, 3);
            s.set_container_items(0, top, &items);
        })
        .unwrap();
    // Eight ticks per item: three diamonds pass through in a few seconds.
    wait_until(&server, "the diamonds to reach the side chest", move |s| s.container_items(0, side).iter().filter_map(|i| i.0.as_ref()).map(|i| i.count).sum::<i32>() == 3);
    assert!(server.handle.run(move |s| s.container_items(0, top).iter().all(Slot::is_empty)).unwrap());

    // An item dropped on an open hopper is collected.
    let (open_hopper, below) = (BlockPos::new(15, -60, 8), BlockPos::new(15, -61, 8));
    let stick = item("minecraft:stick");
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, below, state("minecraft:barrel"));
            s.set_block_and_update(0, open_hopper, with(state("minecraft:hopper"), "facing", "down"));
            s.spawn_item(0, paper_server::world::entity::Vec3::new(15.5, -58.5, 8.5), Slot::of(stick, 2), paper_server::world::entity::Vec3::ZERO, 0);
        })
        .unwrap();
    wait_until(&server, "the sticks to fall through into the barrel", move |s| s.container_items(0, below).iter().filter_map(|i| i.0.as_ref()).filter(|i| i.item == stick).map(|i| i.count).sum::<i32>() == 2);
    server.stop();
}

#[test]
fn powered_droppers_and_dispensers_fire() {
    use paper_protocol::types::Slot;
    let server = start_server(|c| c.properties.gamemode = GameMode::Creative);
    let mut alice = join(&server);
    // A dropper facing up with one stick, powered by placing a redstone block next to it.
    let (dropper, power) = (BlockPos::new(12, -60, 6), BlockPos::new(13, -60, 6));
    let stick = item("minecraft:stick");
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, dropper, with(state("minecraft:dropper"), "facing", "up"));
            s.set_container_items(0, dropper, &[Slot::of(stick, 1), Slot::EMPTY, Slot::EMPTY, Slot::EMPTY, Slot::EMPTY, Slot::EMPTY, Slot::EMPTY, Slot::EMPTY, Slot::EMPTY]);
            s.set_block_and_update(0, power, state("minecraft:redstone_block"));
        })
        .unwrap();
    let item_type = paper_data::entity_types::by_name("minecraft:item").unwrap().id;
    alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == item_type && (a.x - 12.5).abs() < 0.5 && (a.z - 6.5).abs() < 0.5).expect("the stick flies out");
    assert!(server.handle.run(move |s| s.container_items(0, dropper).iter().all(Slot::is_empty)).unwrap());

    // A dispenser shoots arrows.
    let (dispenser, power2) = (BlockPos::new(12, -60, 11), BlockPos::new(11, -60, 11));
    let arrow = item("minecraft:arrow");
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, dispenser, with(state("minecraft:dispenser"), "facing", "east"));
            let mut items = vec![Slot::EMPTY; 9];
            items[4] = Slot::of(arrow, 5);
            s.set_container_items(0, dispenser, &items);
            s.set_block_and_update(0, power2, state("minecraft:redstone_block"));
        })
        .unwrap();
    let arrow_type = paper_data::entity_types::by_name("minecraft:arrow").unwrap().id;
    let shot: play::AddEntity = alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == arrow_type).expect("an arrow is shot");
    assert!(shot.velocity.0 > 0.5, "flying east: {:?}", shot.velocity);
    let left = server.handle.run(move |s| s.container_items(0, dispenser)[4].clone()).unwrap();
    assert_eq!(left, Slot::of(arrow, 4));
    server.stop();
}

#[test]
fn pressure_plates_power_redstone_while_pressed() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Creative);
    let mut alice = join(&server);
    let (plate, lamp) = (BlockPos::new(10, -60, 8), BlockPos::new(11, -60, 8));
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, lamp, state("minecraft:redstone_lamp"));
            s.set_block_and_update(0, plate, state("minecraft:oak_pressure_plate"));
        })
        .unwrap();
    alice.move_to(10.5, -60.0, 8.5);
    wait_until(&server, "the lamp to light", move |s| s.worlds[0].block_at(lamp.x, lamp.y, lamp.z).and_then(|b| blocks::property(b, "lit")) == Some("true"));
    // Stepping off: the plate releases after its delay and the lamp goes out.
    alice.move_to(8.5, -60.0, 8.5);
    wait_until(&server, "the lamp to go out", move |s| s.worlds[0].block_at(lamp.x, lamp.y, lamp.z).and_then(|b| blocks::property(b, "lit")) == Some("false"));
    server.stop();
}
