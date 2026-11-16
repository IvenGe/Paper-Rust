//! Workstations: the stonecutter cuts what the recipe picked makes, the smithing table
//! upgrades and trims, the grindstone takes enchantments off for experience, and the loom
//! adds a pattern to a banner. Items go in with shift-clicks from the hotbar, recipes and
//! patterns are picked with window buttons, and taking the result uses up the inputs.

mod common;

use common::*;
use paper_data::items;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use paper_protocol::text::Component;
use paper_protocol::types::{HashedSlot, ItemStack, Slot};
use paper_server::config::GameMode;
use paper_server::server::containers::MenuKind;
use paper_server::server::Server;
use std::time::Duration;

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

fn click(window_id: i32, slot: i16, button: i8, mode: i32) -> play::ContainerClick {
    play::ContainerClick { window_id, state_id: 0, slot, button, mode, changed_slots: Vec::new(), carried: HashedSlot(None) }
}

fn component(name: &str) -> i32 {
    paper_data::registries::id("minecraft:data_component_type", name).unwrap() as i32
}

fn has_component(stack: &ItemStack, name: &str) -> bool {
    stack.added.iter().any(|(t, _)| *t == component(name))
}

/// Opens a workstation window for the player, returning its window id.
fn open(server: &paper_server::RunningServer, client: &mut Client, id: i32, kind: MenuKind, menu: &'static str) -> i32 {
    server.handle.run(move |s| assert!(s.open_menu(id, kind, menu, Component::text("station")))).unwrap();
    let screen: play::OpenScreen = client.expect();
    client.drain(Duration::from_millis(100));
    client.clear_backlog();
    screen.window_id.0
}

fn grid(server: &paper_server::RunningServer, id: i32) -> Vec<Slot> {
    server.handle.run(move |s| s.players[&id].open_menu.as_ref().unwrap().craft_grid.clone()).unwrap()
}

fn put(server: &paper_server::RunningServer, id: i32, slot: usize, stack: Slot) {
    server.handle.run(move |s: &mut Server| s.set_inventory_slot(id, slot, stack)).unwrap();
}

/// Sends a click and waits until the server has handled it.
fn send_click(server: &paper_server::RunningServer, client: &mut Client, packet: &play::ContainerClick) {
    client.send(packet);
    client.drain(Duration::from_millis(150));
    server.handle.run(|_| ()).unwrap();
}

#[test]
fn stonecutters_smithing_tables_grindstones_and_looms_make_their_results() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    let id = alice.entity_id;
    let pos = paper_protocol::codec::BlockPos::new(8, -60, 8);

    // The client is told the stonecutter's recipes when it joins.
    let recipes: play::UpdateRecipes = alice.find(TIMEOUT, |_: &play::UpdateRecipes| true).expect("the recipes");
    let stone = item("minecraft:stone");
    assert!(recipes.stonecutter.iter().any(|(inputs, result, count)| inputs.contains(&stone) && *result == item("minecraft:stone_slab") && *count == 2), "stone to slabs is sent");
    assert!(recipes.property_sets.iter().any(|(set, items)| set == "minecraft:smithing_template" && items.contains(&item("minecraft:netherite_upgrade_smithing_template"))));
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();

    // Stonecutter: stone goes in, the slab recipe is picked, two slabs come out per stone.
    let window = open(&server, &mut alice, id, MenuKind::Stonecutter { world: 0, pos }, "minecraft:stonecutter");
    put(&server, id, 36, Slot::of(stone, 3));
    send_click(&server, &mut alice, &click(window, 29, 0, 1));
    assert_eq!(grid(&server, id)[0], Slot::of(stone, 3), "the stone went in");
    let index = server
        .handle
        .run(move |s| s.recipes.as_ref().unwrap().stonecutting_for(&Slot::of(stone, 1)).iter().position(|r| r.result.item == item("minecraft:stone_slab")).unwrap())
        .unwrap();
    alice.send(&play::ContainerButtonClick { window_id: VarInt(window), button_id: VarInt(index as i32) });
    alice.drain(Duration::from_millis(150));
    assert_eq!(grid(&server, id)[1], Slot::of(item("minecraft:stone_slab"), 2), "the result shows");
    send_click(&server, &mut alice, &click(window, 1, 0, 0));
    let carried = server.handle.run(move |s| s.players[&id].carried.clone()).unwrap();
    assert_eq!(carried, Slot::of(item("minecraft:stone_slab"), 2));
    assert_eq!(grid(&server, id)[0], Slot::of(stone, 2), "a stone used up");
    // Shift-clicking the result cuts the rest.
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().carried = Slot::EMPTY).unwrap();
    send_click(&server, &mut alice, &click(window, 1, 0, 1));
    assert!(grid(&server, id)[0].is_empty(), "all the stone cut");
    let slabs = server.handle.run(move |s| s.players[&id].inventory.iter().filter_map(|i| i.0.as_ref()).filter(|i| i.item == item("minecraft:stone_slab")).map(|i| i.count).sum::<i32>()).unwrap();
    assert_eq!(slabs, 4);
    server.handle.run(move |s| s.close_menu(id, true)).unwrap();

    // Smithing table: a netherite upgrade keeps the sword's enchantments.
    let window = open(&server, &mut alice, id, MenuKind::Smithing { world: 0, pos }, "minecraft:smithing");
    let mut sword = ItemStack::new(item("minecraft:diamond_sword"), 1);
    let sharpness = paper_data::synced::id("minecraft:enchantment", "minecraft:sharpness").unwrap();
    paper_server::server::enchanting::enchant(&mut sword, sharpness, 3);
    put(&server, id, 36, Slot(Some(sword)));
    put(&server, id, 37, Slot::of(item("minecraft:netherite_upgrade_smithing_template"), 1));
    put(&server, id, 38, Slot::of(item("minecraft:netherite_ingot"), 1));
    for slot in [31, 32, 33] {
        send_click(&server, &mut alice, &click(window, slot, 0, 1));
    }
    let slots = grid(&server, id);
    assert_eq!(slots[0].0.as_ref().map(|s| s.item), Some(item("minecraft:netherite_upgrade_smithing_template")), "template slot");
    assert_eq!(slots[1].0.as_ref().map(|s| s.item), Some(item("minecraft:diamond_sword")), "base slot");
    assert_eq!(slots[2].0.as_ref().map(|s| s.item), Some(item("minecraft:netherite_ingot")), "addition slot");
    let result = slots[3].0.clone().expect("a netherite sword");
    assert_eq!(result.item, item("minecraft:netherite_sword"));
    assert_eq!(paper_server::server::enchanting::read_enchantments(&result, "minecraft:enchantments"), vec![(sharpness, 3)]);
    send_click(&server, &mut alice, &click(window, 3, 0, 0));
    assert!(grid(&server, id)[..3].iter().all(Slot::is_empty), "the inputs are used up");
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().carried = Slot::EMPTY).unwrap();

    // A trim: the pattern in the material of the addition.
    put(&server, id, 36, Slot::of(item("minecraft:iron_chestplate"), 1));
    put(&server, id, 37, Slot::of(item("minecraft:bolt_armor_trim_smithing_template"), 1));
    put(&server, id, 38, Slot::of(item("minecraft:amethyst_shard"), 1));
    for slot in [31, 32, 33] {
        send_click(&server, &mut alice, &click(window, slot, 0, 1));
    }
    let trimmed = grid(&server, id)[3].0.clone().expect("a trimmed chestplate");
    assert_eq!(trimmed.item, item("minecraft:iron_chestplate"));
    assert!(has_component(&trimmed, "minecraft:trim"), "it has a trim");
    server.handle.run(move |s| s.close_menu(id, true)).unwrap();

    // Grindstone: the enchantments come off, for experience.
    let window = open(&server, &mut alice, id, MenuKind::Grindstone { world: 0, pos }, "minecraft:grindstone");
    let mut sword = ItemStack::new(item("minecraft:diamond_sword"), 1);
    paper_server::server::enchanting::enchant(&mut sword, sharpness, 5);
    put(&server, id, 36, Slot(Some(sword)));
    send_click(&server, &mut alice, &click(window, 30, 0, 1));
    let ground = grid(&server, id)[2].0.clone().expect("a plain sword");
    assert_eq!(ground.item, item("minecraft:diamond_sword"));
    assert!(paper_server::server::enchanting::read_enchantments(&ground, "minecraft:enchantments").is_empty(), "no enchantments left");
    // (The orbs appear at the grindstone, right by the player, who takes them.)
    let xp = move |s: &mut Server| s.players[&id].xp_total + s.worlds[0].entities.values().filter_map(|e| match e.kind {
        paper_server::world::entity::EntityKind::ExperienceOrb { value } if !e.removed => Some(value),
        _ => None,
    }).sum::<i32>();
    let before = server.handle.run(xp).unwrap();
    send_click(&server, &mut alice, &click(window, 2, 0, 0));
    let deadline = std::time::Instant::now() + TIMEOUT;
    while server.handle.run(xp).unwrap() <= before {
        assert!(std::time::Instant::now() < deadline, "experience for the enchantments");
        alice.drain(Duration::from_millis(50));
    }
    assert!(grid(&server, id)[..2].iter().all(Slot::is_empty));
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().carried = Slot::EMPTY).unwrap();
    server.handle.run(move |s| s.close_menu(id, true)).unwrap();

    // Loom: a banner and a dye; picking a pattern adds a layer in the dye's colour.
    let window = open(&server, &mut alice, id, MenuKind::Loom { world: 0, pos }, "minecraft:loom");
    put(&server, id, 36, Slot::of(item("minecraft:white_banner"), 2));
    put(&server, id, 37, Slot::of(item("minecraft:red_dye"), 1));
    send_click(&server, &mut alice, &click(window, 31, 0, 1));
    send_click(&server, &mut alice, &click(window, 32, 0, 1));
    alice.send(&play::ContainerButtonClick { window_id: VarInt(window), button_id: VarInt(0) });
    alice.drain(Duration::from_millis(150));
    let banner = grid(&server, id)[3].0.clone().expect("a patterned banner");
    assert_eq!((banner.item, banner.count), (item("minecraft:white_banner"), 1));
    assert!(has_component(&banner, "minecraft:banner_patterns"));
    send_click(&server, &mut alice, &click(window, 3, 0, 0));
    let slots = grid(&server, id);
    assert_eq!(slots[0], Slot::of(item("minecraft:white_banner"), 1), "one banner used");
    assert!(slots[1].is_empty(), "the dye used");
    server.stop();
}
