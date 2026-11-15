//! Crossbows: charged and released they load an arrow (one used up), show as loaded, and fire
//! it critical on the next use; with Multishot they fire three spread apart, only one of
//! which can be picked up; an off-hand rocket is loaded first and flies straight.

mod common;

use common::*;
use paper_data::items;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::GameMode;
use paper_server::world::entity::EntityKind;
use std::time::{Duration, Instant};

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(20));
        client.clear_backlog();
    }
}

/// Charges for 1.5 seconds and lets go.
fn charge(alice: &mut Client, sequence: i32) {
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(sequence), yaw: 0.0, pitch: 0.0 });
    std::thread::sleep(Duration::from_millis(1500));
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::RELEASE_USE_ITEM), pos: BlockPos::new(0, 0, 0), face: 0, sequence: VarInt(0) });
}

fn loaded(server: &paper_server::RunningServer, id: i32) -> usize {
    server.handle.run(move |s| s.players[&id].inventory[36].0.as_ref().map_or(0, |c| paper_server::server::crossbows::charged_projectiles(c).len())).unwrap()
}

/// Arrows in flight: (pickup, critical); rockets: shot at an angle.
fn flying(server: &paper_server::RunningServer) -> (Vec<(bool, bool)>, Vec<bool>) {
    server
        .handle
        .run(|s| {
            let arrows = s.worlds[0].entities.values().filter(|e| !e.removed).filter_map(|e| match &e.kind {
                EntityKind::Arrow { pickup, critical, .. } => Some((*pickup, *critical)),
                _ => None,
            });
            let rockets = s.worlds[0].entities.values().filter(|e| !e.removed).filter_map(|e| match &e.kind {
                EntityKind::Firework(f) => Some(f.shot_at_angle),
                _ => None,
            });
            (arrows.collect(), rockets.collect())
        })
        .unwrap()
}

#[test]
fn crossbows_load_and_fire_arrows_multishot_and_rockets() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();
    let id = alice.entity_id;
    server
        .handle
        .run(move |s| {
            s.set_inventory_slot(id, 36, Slot::of(item("minecraft:crossbow"), 1));
            s.set_inventory_slot(id, 37, Slot::of(item("minecraft:arrow"), 2));
        })
        .unwrap();

    // Charged and released: an arrow loaded, one used up.
    charge(&mut alice, 1);
    wait_reading(&mut alice, "the crossbow loaded", || loaded(&server, id) == 1);
    let arrows = server.handle.run(move |s| s.players[&id].inventory[37].0.as_ref().map(|a| a.count)).unwrap();
    assert_eq!(arrows, Some(1));
    // Fired: a critical arrow that can be picked up; the crossbow is empty and wears.
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(2), yaw: 0.0, pitch: 0.0 });
    wait_reading(&mut alice, "an arrow", || !flying(&server).0.is_empty());
    assert_eq!(flying(&server).0, vec![(true, true)]);
    assert_eq!(loaded(&server, id), 0);
    let worn = server.handle.run(move |s| paper_server::server::survival::item_damage(s.players[&id].inventory[36].0.as_ref().unwrap())).unwrap();
    assert_eq!(worn, 1);
    server.handle.run(|s| s.worlds[0].entities.values_mut().filter(|e| matches!(e.kind, EntityKind::Arrow { .. })).for_each(|e| e.removed = true)).unwrap();

    // With Multishot: three arrows for one, only one of them to pick up.
    server
        .handle
        .run(move |s| {
            let mut crossbow = paper_protocol::types::ItemStack::new(item("minecraft:crossbow"), 1);
            let multishot = paper_data::synced::id("minecraft:enchantment", "minecraft:multishot").unwrap();
            paper_server::server::enchanting::enchant(&mut crossbow, multishot, 1);
            s.set_inventory_slot(id, 36, Slot(Some(crossbow)));
        })
        .unwrap();
    charge(&mut alice, 3);
    wait_reading(&mut alice, "three loaded", || loaded(&server, id) == 3);
    let arrows = server.handle.run(move |s| s.players[&id].inventory[37].0.as_ref().map(|a| a.count)).unwrap();
    assert_eq!(arrows, None, "the last arrow used up");
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(4), yaw: 0.0, pitch: 0.0 });
    wait_reading(&mut alice, "three arrows", || flying(&server).0.len() == 3);
    let pickups = flying(&server).0.iter().filter(|(pickup, _)| *pickup).count();
    assert_eq!(pickups, 1, "only the arrow used up can be picked up");
    server.handle.run(|s| s.worlds[0].entities.values_mut().filter(|e| matches!(e.kind, EntityKind::Arrow { .. })).for_each(|e| e.removed = true)).unwrap();

    // A rocket in the off hand is loaded instead, and flies straight; it wears three.
    server
        .handle
        .run(move |s| {
            s.set_inventory_slot(id, 36, Slot::of(item("minecraft:crossbow"), 1));
            s.set_inventory_slot(id, 45, Slot::of(item("minecraft:firework_rocket"), 1));
            s.set_inventory_slot(id, 37, Slot::of(item("minecraft:arrow"), 5));
        })
        .unwrap();
    charge(&mut alice, 5);
    wait_reading(&mut alice, "a rocket loaded", || loaded(&server, id) == 1);
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(6), yaw: 0.0, pitch: 0.0 });
    wait_reading(&mut alice, "a rocket", || !flying(&server).1.is_empty());
    assert_eq!(flying(&server).1, vec![true], "shot at an angle");
    let (worn, rockets, arrows) = server
        .handle
        .run(move |s| {
            let p = &s.players[&id];
            (paper_server::server::survival::item_damage(p.inventory[36].0.as_ref().unwrap()), p.inventory[45].is_empty(), p.inventory[37].0.as_ref().map(|a| a.count))
        })
        .unwrap();
    assert_eq!((worn, rockets, arrows), (3, true, Some(5)));
    server.stop();
}
