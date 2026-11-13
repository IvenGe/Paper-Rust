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

/// Gives the chest at `pos` a loot table (as world generation does for dungeon chests).
fn set_loot_table(s: &mut paper_server::server::Server, pos: BlockPos, table: &str, seed: i64) {
    let chunk = paper_server::world::chunk::ChunkPos::of_block(pos.x, pos.z);
    let nbt = paper_nbt::Compound::new()
        .with("id", paper_nbt::Tag::String("minecraft:chest".into()))
        .with("LootTable", paper_nbt::Tag::String(table.into()))
        .with("LootTableSeed", paper_nbt::Tag::Long(seed));
    s.worlds[0].chunks.get_mut(&chunk).unwrap().block_entities.insert((pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8), nbt);
}

#[test]
fn loot_chests_fill_when_opened_and_drop_their_loot_when_broken() {
    if paper_server::vanilla::test_jar().is_none() {
        eprintln!("skipping: no vanilla jar");
        return;
    }
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    let mut alice = join(&server, "Alice");
    let (opened, broken) = (BlockPos::new(10, -60, 8), BlockPos::new(10, -60, 5));
    server
        .handle
        .run(move |s| {
            let chest = blocks::default_state("minecraft:chest").unwrap();
            for pos in [opened, broken] {
                s.set_block_and_update(0, pos, chest);
                set_loot_table(s, pos, "minecraft:chests/simple_dungeon", 12345);
            }
        })
        .unwrap();

    // Opening rolls the dungeon loot into the chest, once.
    alice.send(&use_on(opened, 1, 1));
    alice.find(TIMEOUT, |_: &play::OpenScreen| true).expect("the chest opens");
    let (table_gone, filled) = server
        .handle
        .run(move |s| {
            let table_gone = s.block_entity_data(0, opened).is_some_and(|n| n.get("LootTable").is_none());
            (table_gone, s.container_items(0, opened).iter().filter(|i| !i.is_empty()).count())
        })
        .unwrap();
    assert!(table_gone, "the loot table is used up");
    assert!(filled >= 2, "the dungeon loot is in the chest: {filled} stacks");

    // Breaking a chest nobody opened drops its loot.
    server.handle.run(move |s| s.set_block_and_update(0, broken, blocks::AIR)).unwrap();
    let dropped = server
        .handle
        .run(|s| s.worlds[0].entities.values().filter(|e| matches!(e.kind, paper_server::world::entity::EntityKind::Item { .. })).count())
        .unwrap();
    assert!(dropped >= 2, "loot dropped: {dropped}");
    server.stop();
}

/// A stack with a `minecraft:damage` component.
fn damaged(name: &str, damage: i32) -> Slot {
    use paper_protocol::codec::Encode;
    let mut data = Vec::new();
    VarInt(damage).encode(&mut data);
    let ty = paper_data::registries::id("minecraft:data_component_type", "minecraft:damage").unwrap() as i32;
    let mut stack = paper_protocol::types::ItemStack::new(item(name), 1);
    stack.added.push((ty, data));
    Slot(Some(stack))
}

fn components(slot: &Slot) -> Vec<&'static str> {
    slot.0.as_ref().map_or(Vec::new(), |s| s.added.iter().filter_map(|(t, _)| paper_data::registries::name("minecraft:data_component_type", *t as u32)).collect())
}

#[test]
fn enchanting_tables_enchant_for_levels_and_lapis() {
    if paper_server::vanilla::test_jar().is_none() {
        eprintln!("skipping: no vanilla jar");
        return;
    }
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let table = BlockPos::new(10, -60, 8);
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, table, blocks::default_state("minecraft:enchanting_table").unwrap());
            // Fifteen bookshelves in the ring two blocks out.
            let shelf = blocks::default_state("minecraft:bookshelf").unwrap();
            let mut placed = 0;
            for dy in 0..=1 {
                for dz in -2i32..=2 {
                    for dx in -2i32..=2 {
                        if (dx.abs() == 2 || dz.abs() == 2) && placed < 15 && !(dx == -2 && dz == 0) {
                            s.set_block_and_update(0, table.offset(dx, dy, dz), shelf);
                            placed += 1;
                        }
                    }
                }
            }
            let p = s.players.get_mut(&id).unwrap();
            p.inventory[36] = Slot::of(item("minecraft:diamond_sword"), 1);
            p.inventory[37] = Slot::of(item("minecraft:lapis_lazuli"), 5);
            p.xp_level = 30;
        })
        .unwrap();
    assert_eq!(server.handle.run(move |s| s.bookshelves(0, table)).unwrap(), 15);
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: table, face: VarInt(1), cursor_x: 0.5, cursor_y: 0.5, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(1) });
    let screen: play::OpenScreen = alice.find(TIMEOUT, |_: &play::OpenScreen| true).expect("the table opens");
    assert_eq!(screen.menu_type.0, menu("minecraft:enchantment"));
    let w = screen.window_id.0;
    // The sword into slot 0, the lapis into slot 1 (hotbar 0 and 1 are window slots 29, 30).
    for (from, to) in [(29, 0), (30, 1)] {
        alice.send(&click(w, from, 0, 0));
        alice.send(&click(w, to, 0, 0));
    }
    // The best offer with fifteen shelves costs 30 levels.
    let offer: play::ContainerSetData = alice.find(TIMEOUT, |d: &play::ContainerSetData| d.property == 2 && d.value > 0).expect("three offers");
    assert_eq!(offer.value, 30);
    alice.send(&play::ContainerButtonClick { window_id: VarInt(w), button_id: VarInt(2) });
    alice.drain(QUIET);
    let (sword, lapis, level) = server
        .handle
        .run(move |s| {
            let p = &s.players[&id];
            let m = p.open_menu.as_ref().unwrap();
            (m.craft_grid[0].clone(), m.craft_grid[1].clone(), p.xp_level)
        })
        .unwrap();
    assert!(components(&sword).contains(&"minecraft:enchantments"), "the sword is enchanted: {sword:?}");
    assert_eq!(lapis, Slot::of(item("minecraft:lapis_lazuli"), 2), "three lapis used");
    assert_eq!(level, 27, "three levels paid");
    server.stop();
}

#[test]
fn anvils_repair_and_rename_for_levels() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let anvil = BlockPos::new(10, -60, 8);
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, anvil, blocks::default_state("minecraft:anvil").unwrap());
            let p = s.players.get_mut(&id).unwrap();
            p.inventory[36] = damaged("minecraft:diamond_sword", 1000);
            p.inventory[37] = Slot::of(item("minecraft:diamond"), 3);
            p.xp_level = 10;
        })
        .unwrap();
    alice.send(&use_on(anvil, 1, 1));
    let screen: play::OpenScreen = alice.find(TIMEOUT, |_: &play::OpenScreen| true).expect("the anvil opens");
    assert_eq!(screen.menu_type.0, menu("minecraft:anvil"));
    let w = screen.window_id.0;
    // The sword left, the diamonds right (hotbar 0 and 1 are window slots 30, 31).
    for (from, to) in [(30, 0), (31, 1)] {
        alice.send(&click(w, from, 0, 0));
        alice.send(&click(w, to, 0, 0));
    }
    // Three diamonds repair 1000 damage fully, for three levels.
    let cost: play::ContainerSetData = alice.find(TIMEOUT, |d: &play::ContainerSetData| d.property == 0 && d.value > 0).expect("a cost");
    assert_eq!(cost.value, 3);
    // Taking the result pays for it and uses up the inputs.
    alice.send(&click(w, 2, 0, 0));
    alice.drain(QUIET);
    let (carried, inputs, level) = server
        .handle
        .run(move |s| {
            let p = &s.players[&id];
            let m = p.open_menu.as_ref().unwrap();
            (p.carried.clone(), (m.craft_grid[0].clone(), m.craft_grid[1].clone()), p.xp_level)
        })
        .unwrap();
    assert_eq!(level, 7);
    assert_eq!(inputs, (Slot::EMPTY, Slot::EMPTY));
    let damage_left = carried.0.as_ref().and_then(|st| {
        let ty = paper_data::registries::id("minecraft:data_component_type", "minecraft:damage").unwrap() as i32;
        st.added.iter().find(|(t, _)| *t == ty).map(|(_, d)| paper_protocol::codec::Reader::new(d).varint().unwrap())
    });
    assert_eq!(damage_left, Some(0), "fully repaired");

    // Renaming alone costs one level.
    alice.send(&click(w, 0, 0, 0));
    alice.send(&play::RenameItem { name: "Excalibur".into() });
    let cost: play::ContainerSetData = alice.find(TIMEOUT, |d: &play::ContainerSetData| d.property == 0 && d.value > 0).expect("a rename cost");
    assert_eq!(cost.value, 1 + 1, "one for the name, plus the repair penalty of the first use");
    let named = server.handle.run(move |s| s.players[&id].open_menu.as_ref().unwrap().craft_grid[2].clone()).unwrap();
    assert!(components(&named).contains(&"minecraft:custom_name"));
    server.stop();
}
