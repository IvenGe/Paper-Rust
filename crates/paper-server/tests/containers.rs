//! Containers: chests (single and double) keep items, windows apply clicks on the server,
//! the 2x2 grid crafts, and broken containers drop what they held.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::{HashedSlot, Slot};
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(300);

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

fn menu(name: &str) -> i32 {
    paper_data::registries::id("minecraft:menu", name).unwrap() as i32
}

fn click(window_id: i32, slot: i16, button: i8, mode: i32) -> play::ContainerClick {
    play::ContainerClick { window_id, state_id: 0, slot, button, mode, changed_slots: Vec::new(), carried: HashedSlot(None) }
}

/// Joins and waits until the chunk around spawn has arrived (clients only interact with
/// blocks they have).
fn join(server: &paper_server::RunningServer, name: &str) -> Client {
    let mut client = Client::join(server.port(), name);
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(QUIET);
    client.clear_backlog();
    client
}

fn use_on(pos: BlockPos, face: i32, sequence: i32) -> play::UseItemOn {
    play::UseItemOn { hand: VarInt(0), pos, face: VarInt(face), cursor_x: 0.5, cursor_y: 0.5, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(sequence) }
}

#[test]
fn chest_window_moves_items_between_chest_and_inventory() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    let mut alice = join(&server, "Alice");

    // A chest with five diamonds next to Alice.
    let chest = BlockPos::new(10, -60, 8);
    let diamond = item("minecraft:diamond");
    server
        .handle
        .run(move |s| {
            let state = blocks::with_property(blocks::default_state("minecraft:chest").unwrap(), "facing", "west").unwrap();
            let change = s.worlds[0].set_block(chest.x, chest.y, chest.z, state).unwrap();
            s.publish_block_change(0, &change);
            let mut items = vec![Slot::EMPTY; 27];
            items[0] = Slot::of(diamond, 5);
            s.set_container_items(0, chest, &items);
        })
        .unwrap();

    alice.send(&use_on(chest, 4, 1));
    let open: play::OpenScreen = alice.expect();
    assert_eq!(open.menu_type.0, menu("minecraft:generic_9x3"));
    let window = open.window_id.0;
    let content: play::ContainerSetContent = alice.find(TIMEOUT, |c: &play::ContainerSetContent| c.window_id == window).expect("window content");
    assert_eq!(content.slots.len(), 27 + 36);
    assert_eq!(content.slots[0], Slot::of(diamond, 5));
    let lid: play::BlockEvent = alice.expect();
    assert_eq!((lid.pos, lid.action, lid.param), (chest, 1, 1), "the lid opens for one viewer");

    // Shift-clicking the diamonds moves them to the end of the hotbar (window slot 62).
    alice.send(&click(window, 0, 0, 1));
    let moved: play::ContainerSetSlot = alice.find(TIMEOUT, |s: &play::ContainerSetSlot| s.window_id == window && s.slot == 62).expect("hotbar slot updated");
    assert_eq!(moved.item, Slot::of(diamond, 5));
    let (chest_slot, hotbar) = server.handle.run(move |s| (s.container_items(0, chest)[0].clone(), s.players.values().next().unwrap().inventory[44].clone())).unwrap();
    assert!(chest_slot.is_empty(), "taken out of the chest");
    assert_eq!(hotbar, Slot::of(diamond, 5));

    // Picking two up and putting one back by right-clicking leaves one in the chest.
    alice.send(&click(window, 62, 0, 0));
    alice.send(&click(window, 13, 1, 0));
    alice.send(&click(window, 62, 0, 0));
    alice.drain(QUIET);
    let (chest_slot, hotbar, carried) =
        server.handle.run(move |s| (s.container_items(0, chest)[13].clone(), s.players.values().next().unwrap().inventory[44].clone(), s.players.values().next().unwrap().carried.clone())).unwrap();
    assert_eq!(chest_slot, Slot::of(diamond, 1));
    assert_eq!(hotbar, Slot::of(diamond, 4));
    assert!(carried.is_empty());

    // Closing the window closes the lid.
    alice.clear_backlog();
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(window) });
    let lid: play::BlockEvent = alice.expect();
    assert_eq!((lid.action, lid.param), (1, 0));

    // The chest's contents are saved in its block entity but not sent to clients.
    let saved = server.handle.run(move |s| s.container_items(0, chest)).unwrap();
    assert_eq!(saved[13], Slot::of(diamond, 1));
    server.stop();
}

#[test]
fn inventory_grid_crafts_planks_from_a_log() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    if server.handle.run(|s| s.recipes.is_none()).unwrap() {
        eprintln!("skipping: no vanilla data for recipes");
        server.stop();
        return;
    }
    let mut alice = join(&server, "Alice");
    let log = item("minecraft:oak_log");
    let planks = item("minecraft:oak_planks");
    let id = alice.entity_id;
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&id).unwrap().inventory[36] = Slot::of(log, 2);
            s.resync_inventory(id);
        })
        .unwrap();
    alice.drain(QUIET);
    alice.clear_backlog();

    // Pick up both logs and put one into the grid.
    alice.send(&click(0, 36, 0, 0));
    alice.send(&click(0, 1, 1, 0));
    let result: play::ContainerSetSlot = alice.find(TIMEOUT, |s: &play::ContainerSetSlot| s.window_id == 0 && s.slot == 0 && !s.item.is_empty()).expect("craft result shown");
    assert_eq!(result.item, Slot::of(planks, 4));
    alice.send(&click(0, 36, 0, 0));

    // Shift-clicking the result crafts once per log in the grid.
    alice.send(&click(0, 0, 0, 1));
    alice.drain(QUIET);
    let inventory = server.handle.run(move |s| s.players[&id].inventory.clone()).unwrap();
    let count = |item: i32| inventory[9..45].iter().filter_map(|s| s.0.as_ref()).filter(|s| s.item == item).map(|s| s.count).sum::<i32>();
    assert_eq!(count(planks), 4);
    assert_eq!(count(log), 1, "one log was used");
    assert!(inventory[0..5].iter().all(Slot::is_empty), "grid and result are empty: {:?}", &inventory[0..5]);
    server.stop();
}

#[test]
fn chests_join_into_a_double_chest_and_drop_their_contents_when_broken() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let chest_item = item("minecraft:chest");
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&id).unwrap().inventory[36] = Slot::of(chest_item, 2);
            s.resync_inventory(id);
        })
        .unwrap();
    alice.drain(QUIET);
    alice.clear_backlog();

    // Two chests placed side by side on the ground in front of Alice.
    let (a, b) = (BlockPos::new(8, -60, 10), BlockPos::new(9, -60, 10));
    alice.send(&use_on(a.offset(0, -1, 0), 1, 1));
    alice.send(&use_on(b.offset(0, -1, 0), 1, 2));
    alice.drain(QUIET);
    let types = server
        .handle
        .run(move |s| {
            [a, b].map(|p| {
                let state = s.worlds[0].block_at(p.x, p.y, p.z).unwrap();
                (blocks::block_of_state(state).name, blocks::property(state, "type").map(str::to_owned))
            })
        })
        .unwrap();
    assert_eq!(types[0].0, "minecraft:chest");
    assert_eq!(types[1].0, "minecraft:chest");
    let mut halves = [types[0].1.clone().unwrap(), types[1].1.clone().unwrap()];
    halves.sort();
    assert_eq!(halves, ["left".to_owned(), "right".to_owned()], "the second chest joins the first");

    // Opening either half shows both.
    alice.clear_backlog();
    alice.send(&use_on(a, 1, 3));
    let open: play::OpenScreen = alice.expect();
    assert_eq!(open.menu_type.0, menu("minecraft:generic_9x6"));
    let content: play::ContainerSetContent = alice.expect();
    assert_eq!(content.slots.len(), 54 + 36);

    // Put a stick into each half, then break one half.
    let stick = item("minecraft:stick");
    server
        .handle
        .run(move |s| {
            for p in [a, b] {
                let mut items = s.container_items(0, p);
                items[0] = Slot::of(stick, 3);
                s.set_container_items(0, p, &items);
            }
        })
        .unwrap();
    alice.clear_backlog();
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: a, face: 1, sequence: VarInt(4) });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::STOP_DESTROY_BLOCK), pos: a, face: 1, sequence: VarInt(5) });
    let closed: play::ContainerClose = alice.expect();
    assert_eq!(closed.window_id.0, open.window_id.0, "the window closes when the chest goes");
    alice.drain(QUIET);
    let (sticks_on_ground, other_type, other_items) = server
        .handle
        .run(move |s| {
            let sticks: i32 = s.worlds[0]
                .entities
                .values()
                .filter_map(|e| match &e.kind {
                    paper_server::world::entity::EntityKind::Item { stack: Slot(Some(stack)), .. } if stack.item == stick => Some(stack.count),
                    _ => None,
                })
                .sum();
            // Alice may already have picked them up.
            let picked: i32 = s.players[&id].inventory.iter().filter_map(|i| i.0.as_ref()).filter(|i| i.item == stick).map(|i| i.count).sum();
            let state = s.worlds[0].block_at(b.x, b.y, b.z).unwrap();
            (sticks + picked, blocks::property(state, "type").map(str::to_owned), s.container_items(0, b))
        })
        .unwrap();
    assert_eq!(sticks_on_ground, 3, "the broken half drops its sticks");
    assert_eq!(other_type.as_deref(), Some("single"), "the other half becomes a single chest");
    assert_eq!(other_items[0], Slot::of(stick, 3), "and keeps its own items");
    server.stop();
}

#[test]
fn furnaces_smelt_with_fuel_over_time() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    if server.handle.run(|s| s.recipes.is_none()).unwrap() {
        eprintln!("skipping: no vanilla data for recipes");
        server.stop();
        return;
    }
    let mut alice = join(&server, "Alice");
    let furnace = BlockPos::new(10, -60, 8);
    let (raw_iron, coal, ingot) = (item("minecraft:raw_iron"), item("minecraft:coal"), item("minecraft:iron_ingot"));
    server
        .handle
        .run(move |s| {
            let state = blocks::default_state("minecraft:furnace").unwrap();
            let change = s.worlds[0].set_block(furnace.x, furnace.y, furnace.z, state).unwrap();
            s.publish_block_change(0, &change);
            s.set_container_items(0, furnace, &[Slot::of(raw_iron, 2), Slot::of(coal, 1), Slot::EMPTY]);
        })
        .unwrap();

    // Watching it: the window shows the furnace's progress.
    alice.send(&use_on(furnace, 4, 1));
    let open: play::OpenScreen = alice.expect();
    assert_eq!(open.menu_type.0, menu("minecraft:furnace"));
    let progress: play::ContainerSetData = alice.find(TIMEOUT, |d: &play::ContainerSetData| d.property == 2 && d.value > 0).expect("cooking progress");
    assert_eq!(progress.window_id, open.window_id.0);

    // Skip most of the ten seconds one item takes.
    server
        .handle
        .run(move |s| {
            let chunk = s.worlds[0].chunks.get_mut(&paper_server::world::chunk::ChunkPos::of_block(furnace.x, furnace.z)).unwrap();
            let nbt = chunk.block_entities.get_mut(&(furnace.y, (furnace.z & 15) as u8, (furnace.x & 15) as u8)).unwrap();
            nbt.insert("cooking_time_spent", paper_nbt::Tag::Short(195));
        })
        .unwrap();
    let output: play::ContainerSetSlot = alice.find(TIMEOUT, |s: &play::ContainerSetSlot| s.slot == 2 && !s.item.is_empty()).expect("an ingot comes out");
    assert_eq!(output.item, Slot::of(ingot, 1));
    let (items, lit) = server
        .handle
        .run(move |s| {
            let state = s.worlds[0].block_at(furnace.x, furnace.y, furnace.z).unwrap();
            (s.container_items(0, furnace), blocks::property(state, "lit").map(str::to_owned))
        })
        .unwrap();
    assert_eq!(items[0], Slot::of(raw_iron, 1), "one raw iron used");
    assert!(items[1].is_empty(), "the coal burns");
    assert_eq!(lit.as_deref(), Some("true"));

    // Taking the ingot out with a shift-click puts it in the inventory.
    alice.send(&click(open.window_id.0, 2, 0, 1));
    alice.drain(QUIET);
    let id = alice.entity_id;
    let held = server.handle.run(move |s| s.players[&id].inventory.iter().filter_map(|s| s.0.clone()).find(|s| s.item == ingot).map(|s| s.count)).unwrap();
    assert_eq!(held, Some(1));
    server.stop();
}
