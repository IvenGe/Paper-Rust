//! Fishing: a cast bobber lands in water and bobs; a fish is lured and bites, and reeling in
//! then catches something from the fishing loot table with experience, wearing the rod.
//! Reeling in with nothing on the line just brings the bobber back.

mod common;

use common::*;
use paper_data::items;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::GameMode;
use paper_server::world::entity::{EntityKind, HookState};
use std::time::{Duration, Instant};

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

/// Waits while reading what the server sends, so keep-alives are answered.
fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(20));
        client.clear_backlog();
    }
}

/// The bobber out: (id, state, biting, nibble).
fn bobber(server: &paper_server::RunningServer) -> Option<(i32, HookState, bool, i32)> {
    server
        .handle
        .run(|s| {
            s.worlds[0].entities.values().find_map(|e| match &e.kind {
                EntityKind::Fishing(h) if !e.removed => Some((e.id, h.state, h.biting, h.nibble)),
                _ => None,
            })
        })
        .unwrap()
}

#[test]
fn a_bobber_in_open_water_catches_a_fish() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();
    let id = alice.entity_id;
    // A pond south of Alice, three blocks deep, with nothing over it.
    let water = paper_data::blocks::default_state("minecraft:water").unwrap();
    server
        .handle
        .run(move |s| {
            for x in 2..=16 {
                for z in 11..=25 {
                    for y in -63..=-61 {
                        s.worlds[0].set_block(x, y, z, water);
                    }
                }
            }
            s.set_inventory_slot(id, 36, Slot::of(item("minecraft:fishing_rod"), 1));
        })
        .unwrap();
    // She looks south, a little down, and casts.
    alice.send(&play::MovePlayerRot { yaw: 0.0, pitch: 20.0, flags: 0 });
    alice.drain(Duration::from_millis(200));
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(1), yaw: 0.0, pitch: 20.0 });
    let bobber_type = paper_data::entity_types::by_name("minecraft:fishing_bobber").unwrap().id;
    let shown: play::AddEntity = alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == bobber_type).expect("the bobber is seen");
    assert_eq!(shown.data.0, id, "a bobber names its player");
    wait_reading(&mut alice, "the bobber in the water", || bobber(&server).is_some_and(|(_, state, ..)| state == HookState::Bobbing));

    // A fish comes soon (the hook's waits as a plugin might shorten them) and bites.
    let (hook, ..) = bobber(&server).unwrap();
    server
        .handle
        .run(move |s| {
            if let Some(EntityKind::Fishing(h)) = s.worlds[0].entities.get_mut(&hook).map(|e| &mut e.kind) {
                (h.min_wait, h.max_wait, h.min_lure, h.max_lure) = (5, 10, 5, 10);
                h.time_until_lured = 0;
            }
        })
        .unwrap();
    wait_reading(&mut alice, "a bite", || bobber(&server).is_some_and(|(_, _, biting, nibble)| biting && nibble > 5));
    let open = server.handle.run(move |s| matches!(s.worlds[0].entities.get(&hook).map(|e| &e.kind), Some(EntityKind::Fishing(h)) if h.open_water)).unwrap();
    assert!(open, "the pond is open water");

    // Reeling in catches it: an item flies to Alice, experience with it, and the rod wears.
    let items_before = server.handle.run(|s| s.worlds[0].entities.values().filter(|e| matches!(e.kind, EntityKind::Item { .. })).count()).unwrap();
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(2), yaw: 0.0, pitch: 20.0 });
    wait_reading(&mut alice, "the bobber back", || bobber(&server).is_none());
    let (items_after, orbs) = server
        .handle
        .run(|s| {
            let count = |f: fn(&EntityKind) -> bool| s.worlds[0].entities.values().filter(|e| f(&e.kind)).count();
            (count(|k| matches!(k, EntityKind::Item { .. })), count(|k| matches!(k, EntityKind::ExperienceOrb { .. })))
        })
        .unwrap();
    let rod = server.handle.run(move |s| s.players[&id].inventory[36].clone()).unwrap();
    let caught_in_hand = server.handle.run(move |s| s.players[&id].inventory.iter().filter(|i| !i.is_empty()).count() > 1).unwrap();
    assert!(items_after > items_before || caught_in_hand, "something was caught");
    let xp = server.handle.run(move |s| s.players[&id].xp_total).unwrap();
    assert!(orbs > 0 || xp > 0, "experience comes with it");
    assert!(format!("{:?}", rod.0.as_ref().unwrap().added).len() > 2, "the rod wears: {rod:?}");

    // Cast and reeled straight in: nothing caught, nothing worn.
    std::thread::sleep(Duration::from_millis(300));
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(3), yaw: 0.0, pitch: 20.0 });
    wait_reading(&mut alice, "a second bobber", || bobber(&server).is_some());
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(4), yaw: 0.0, pitch: 20.0 });
    wait_reading(&mut alice, "it back", || bobber(&server).is_none());
    let again = server.handle.run(move |s| s.players[&id].inventory[36].clone()).unwrap();
    assert_eq!(again, rod, "reeling in nothing does not wear the rod");

    // The bobber goes when she puts the rod away.
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(5), yaw: 0.0, pitch: 20.0 });
    wait_reading(&mut alice, "a third bobber", || bobber(&server).is_some());
    alice.send(&play::SetCarriedItem { slot: 1 });
    wait_reading(&mut alice, "it gone", || bobber(&server).is_none());
    server.stop();
}
