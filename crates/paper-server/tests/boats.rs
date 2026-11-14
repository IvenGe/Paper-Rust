//! Boats: placed on water from the item, boarded, steered by the player's client, floating,
//! and broken into their item.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::{LpVec3, Slot};
use paper_server::config::GameMode;
use paper_server::world::entity::EntityKind;
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(300);

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

/// Waits while reading what the server sends, so keep-alives are answered.
fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(50));
        client.clear_backlog();
    }
}

/// The boat (id and position), if there is one.
fn boat(server: &paper_server::RunningServer) -> Option<(i32, f64, f64, f64)> {
    server.handle.run(|s| s.worlds[0].entities.values().find(|e| matches!(e.kind, EntityKind::Boat { .. }) && !e.removed).map(|e| (e.id, e.pos.x, e.pos.y, e.pos.z))).unwrap()
}

#[test]
fn boats_are_placed_on_water_ridden_steered_and_broken() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    alice.clear_backlog();
    let id = alice.entity_id;
    // A pool south of Alice.
    let water = blocks::default_state("minecraft:water").unwrap();
    server
        .handle
        .run(move |s| {
            for x in 7..=10 {
                for z in 11..=14 {
                    s.set_block_and_update(0, BlockPos::new(x, -61, z), water);
                }
            }
        })
        .unwrap();
    // Placed by looking down at the water (yaw 0 is south).
    server.handle.run(move |s| s.set_inventory_slot(id, 36, Slot::of(item("minecraft:oak_boat"), 1))).unwrap();
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(1), yaw: 0.0, pitch: 23.0 });
    wait_reading(&mut alice, "a boat", || boat(&server).is_some());
    let held = server.handle.run(move |s| s.players[&id].inventory[36].clone()).unwrap();
    assert!(held.is_empty(), "the boat item is used up");
    // It floats a little below the surface (the top of the water is at -60.11).
    let (b, ..) = boat(&server).unwrap();
    wait_reading(&mut alice, "the boat to settle", || boat(&server).is_some_and(|(_, _, y, _)| (y + 60.48).abs() < 0.1));

    // Alice gets in and steers: her client says where the boat went.
    alice.send(&play::Interact { entity_id: VarInt(b), hand: VarInt(0), target: LpVec3(0.0, 0.3, 0.0), sneaking: false });
    wait_reading(&mut alice, "Alice in the boat", || server.handle.run(move |s| s.players[&id].vehicle).unwrap() == Some(b));
    assert_eq!(server.handle.run(move |s| s.controller_of(0, b)).unwrap(), Some(id));
    alice.send(&play::PaddleBoat { left: true, right: true });
    alice.send(&play::MoveVehicle { x: 9.0, y: -60.48, z: 13.5, yaw: 0.0, pitch: 0.0, on_ground: false });
    wait_reading(&mut alice, "the boat moved", || boat(&server).is_some_and(|(_, x, _, z)| (x - 9.0).abs() < 0.01 && (z - 13.5).abs() < 0.01));

    // Out again, and one good blow with a sword breaks it into its item.
    alice.send(&play::PlayerInput { flags: play::input_flags::SNEAK });
    wait_reading(&mut alice, "Alice out", || server.handle.run(move |s| s.players[&id].vehicle).unwrap().is_none());
    alice.send(&play::PlayerInput { flags: 0 });
    server.handle.run(move |s| s.set_inventory_slot(id, 36, Slot::of(item("minecraft:iron_sword"), 1))).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    alice.send(&play::Attack { entity_id: VarInt(b) });
    wait_reading(&mut alice, "the boat broken", || boat(&server).is_none());
    // Alice stands next to it, so once its pickup delay is over she may have the item already.
    let boat_item = item("minecraft:oak_boat");
    let dropped = server
        .handle
        .run(move |s| {
            let lying = s.worlds[0].entities.values().any(|e| matches!(&e.kind, EntityKind::Item { stack, .. } if stack.0.as_ref().is_some_and(|i| i.item == boat_item)));
            lying || s.players[&id].inventory.iter().any(|slot| slot.0.as_ref().is_some_and(|i| i.item == boat_item))
        })
        .unwrap();
    assert!(dropped, "the boat item drops");
    server.stop();
}
