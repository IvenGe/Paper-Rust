//! Boats: placed on water from the item, boarded, steered by the player's client, floating,
//! and broken into their item.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::text::Component;
use paper_protocol::types::{HashedSlot, LpVec3, Slot};
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

fn menu(name: &str) -> i32 {
    paper_data::registries::id("minecraft:menu", name).unwrap() as i32
}

/// The chest boat (id) and its chest's slots, if there is one.
fn chest_boat(server: &paper_server::RunningServer) -> Option<(i32, Vec<Slot>)> {
    server.handle.run(|s| s.worlds[0].entities.values().find(|e| !e.removed && e.chest().is_some()).map(|e| (e.id, e.chest().unwrap().items.clone()))).unwrap()
}

/// Whether an item lies in the world or is in the player's inventory.
fn has_item(server: &paper_server::RunningServer, player: i32, item: i32) -> bool {
    server
        .handle
        .run(move |s| {
            let lying = s.worlds[0].entities.values().any(|e| matches!(&e.kind, EntityKind::Item { stack, .. } if stack.0.as_ref().is_some_and(|i| i.item == item)));
            lying || s.players[&player].inventory.iter().any(|slot| slot.0.as_ref().is_some_and(|i| i.item == item))
        })
        .unwrap()
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

#[test]
fn chest_boats_open_their_chest_and_spill_it_when_broken() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    alice.clear_backlog();
    let id = alice.entity_id;
    let water = blocks::default_state("minecraft:water").unwrap();
    let diamond = item("minecraft:diamond");
    server
        .handle
        .run(move |s| {
            for x in 7..=10 {
                for z in 11..=14 {
                    s.set_block_and_update(0, BlockPos::new(x, -61, z), water);
                }
            }
            s.set_inventory_slot(id, 36, Slot::of(item("minecraft:oak_chest_boat"), 1));
            s.set_inventory_slot(id, 37, Slot::of(diamond, 5));
        })
        .unwrap();
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(1), yaw: 0.0, pitch: 23.0 });
    wait_reading(&mut alice, "a chest boat", || chest_boat(&server).is_some());
    let (b, slots) = chest_boat(&server).unwrap();
    assert_eq!(slots.len(), 27);

    // One seat; from it the inventory key opens the chest, named after the boat.
    alice.send(&play::Interact { entity_id: VarInt(b), hand: VarInt(0), target: LpVec3(0.0, 0.3, 0.0), sneaking: false });
    wait_reading(&mut alice, "Alice in the boat", || server.handle.run(move |s| s.players[&id].vehicle).unwrap() == Some(b));
    alice.clear_backlog();
    alice.send(&play::PlayerCommand { entity_id: VarInt(id), action: VarInt(play::player_command::OPEN_VEHICLE_INVENTORY), jump_boost: VarInt(0) });
    let open: play::OpenScreen = alice.expect();
    assert_eq!(open.menu_type.0, menu("minecraft:generic_9x3"));
    assert_eq!(open.title, Component::translatable("entity.minecraft.oak_chest_boat", vec![]));
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(open.window_id.0) });

    // Out again; a sneaking click opens it from outside, and shift-clicked diamonds go in.
    // (Getting off leaves the player not sneaking, as `rideTick` does: sneak again.)
    alice.send(&play::PlayerInput { flags: play::input_flags::SNEAK });
    wait_reading(&mut alice, "Alice out", || server.handle.run(move |s| s.players[&id].vehicle).unwrap().is_none());
    alice.send(&play::PlayerInput { flags: 0 });
    alice.send(&play::PlayerInput { flags: play::input_flags::SNEAK });
    wait_reading(&mut alice, "Alice sneaking", || server.handle.run(move |s| s.players[&id].sneaking).unwrap());
    alice.clear_backlog();
    alice.send(&play::Interact { entity_id: VarInt(b), hand: VarInt(0), target: LpVec3(0.0, 0.3, 0.0), sneaking: true });
    let open: play::OpenScreen = alice.expect();
    let window = open.window_id.0;
    let content: play::ContainerSetContent = alice.find(TIMEOUT, |c: &play::ContainerSetContent| c.window_id == window).expect("window content");
    assert_eq!(content.slots.len(), 27 + 36);
    // Hotbar slot 1 is window slot 27 + 27 + 1.
    alice.send(&play::ContainerClick { window_id: window, state_id: 0, slot: 55, button: 0, mode: 1, changed_slots: Vec::new(), carried: HashedSlot(None) });
    wait_reading(&mut alice, "the diamonds in the chest", || chest_boat(&server).is_some_and(|(_, items)| items[0] == Slot::of(diamond, 5)));
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(window) });
    alice.send(&play::PlayerInput { flags: 0 });

    // Broken, it drops itself and spills its chest.
    server.handle.run(move |s| s.set_inventory_slot(id, 36, Slot::of(item("minecraft:iron_sword"), 1))).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    alice.send(&play::Attack { entity_id: VarInt(b) });
    wait_reading(&mut alice, "the chest boat broken", || chest_boat(&server).is_none());
    assert!(has_item(&server, id, item("minecraft:oak_chest_boat")), "the chest boat item drops");
    assert!(has_item(&server, id, diamond), "the diamonds spill");
    server.stop();
}

#[test]
fn hoppers_fill_and_empty_chest_boats() {
    use paper_server::server::mobs::SpawnReason;
    use paper_server::world::entity::Vec3;
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    // A chest boat (made as /summon makes one) on a hopper, under another hopper pointing
    // down into it.
    let (below, above) = (BlockPos::new(3, -61, 3), BlockPos::new(3, -59, 3));
    let hopper = blocks::default_state("minecraft:hopper").unwrap();
    let (diamond, emerald) = (item("minecraft:diamond"), item("minecraft:emerald"));
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, below, hopper);
            s.set_block_and_update(0, above, hopper);
            let b = s.spawn_mob(0, "minecraft:oak_chest_boat", Vec3::new(3.5, -60.0, 3.5), SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&b).unwrap().chest_mut().unwrap().items[0] = Slot::of(diamond, 2);
            let mut items = s.container_items(0, above);
            items[0] = Slot::of(emerald, 2);
            s.set_container_items(0, above, &items);
        })
        .unwrap();
    let count = |pos: BlockPos, what: i32| server.handle.run(move |s| s.container_items(0, pos).iter().filter_map(|s| s.0.as_ref()).filter(|i| i.item == what).map(|i| i.count).sum::<i32>()).unwrap();
    wait_reading(&mut alice, "the diamonds in the lower hopper", || count(below, diamond) == 2);
    wait_reading(&mut alice, "the emeralds through the boat", || count(below, emerald) == 2);
    assert_eq!(count(above, emerald), 0);
    server.stop();
}
