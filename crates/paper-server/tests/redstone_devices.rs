//! Redstone devices that react to entities or items: tripwires, crafters, pistons pushing
//! entities, and what dispensers do with special items.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::{Difficulty, GameMode};
use paper_server::server::player::Location;
use std::time::{Duration, Instant};

fn server() -> paper_server::RunningServer {
    let server = start_server(|c| {
        c.properties.gamemode = GameMode::Survival;
        c.properties.difficulty = Difficulty::Peaceful;
    });
    server
        .handle
        .run(|s| {
            use paper_data::game_rules::Value;
            s.set_game_rule(0, "respawn_radius", Value::Int(0));
            s.set_game_rule(0, "spawn_mobs", Value::Bool(false));
            s.set_game_rule(0, "spawn_wandering_traders", Value::Bool(false));
        })
        .unwrap();
    server
}

/// Joins at the middle of chunk 0, 0 and waits until the chunk around spawn has arrived.
fn join(server: &paper_server::RunningServer, name: &str) -> Client {
    let mut client = Client::join(server.port(), name);
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(Duration::from_millis(300));
    client.clear_backlog();
    client
}

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

fn hold(server: &paper_server::RunningServer, player: i32, stack: Slot) {
    server.handle.run(move |s| s.set_inventory_slot(player, 36, stack)).unwrap();
}

fn use_on(pos: BlockPos, face: i32, sequence: i32) -> play::UseItemOn {
    play::UseItemOn { hand: VarInt(0), pos, face: VarInt(face), cursor_x: 0.5, cursor_y: 0.5, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(sequence) }
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn state_at(server: &paper_server::RunningServer, pos: BlockPos) -> u32 {
    server.handle.run(move |s| s.worlds[0].block_at(pos.x, pos.y, pos.z).unwrap()).unwrap()
}

fn prop(state: u32, name: &str) -> Option<&'static str> {
    blocks::property(state, name)
}

fn set(server: &paper_server::RunningServer, pos: BlockPos, state: u32) {
    server.handle.run(move |s| s.set_block_and_update(0, pos, state)).unwrap();
}

#[test]
fn string_between_hooks_powers_them_while_something_is_in_it() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    // Two hooks on stone, facing each other across one block of string, and a lamp above
    // one of them.
    let (stone_a, stone_b) = (BlockPos::new(10, -60, 6), BlockPos::new(10, -60, 10));
    let (hook_a, hook_b, string, lamp) = (BlockPos::new(10, -60, 7), BlockPos::new(10, -60, 9), BlockPos::new(10, -60, 8), BlockPos::new(10, -59, 7));
    for p in [stone_a, stone_b] {
        set(&server, p, blocks::default_state("minecraft:stone").unwrap());
    }
    set(&server, lamp, blocks::default_state("minecraft:redstone_lamp").unwrap());
    hold(&server, id, Slot::of(item("minecraft:tripwire_hook"), 2));
    alice.send(&use_on(stone_a, 3, 1));
    wait_until("the first hook", TIMEOUT, || blocks::block_of_state(state_at(&server, hook_a)).name == "minecraft:tripwire_hook");
    alice.send(&use_on(stone_b, 2, 2));
    wait_until("the second hook", TIMEOUT, || blocks::block_of_state(state_at(&server, hook_b)).name == "minecraft:tripwire_hook");
    assert_eq!((prop(state_at(&server, hook_a), "facing"), prop(state_at(&server, hook_b), "facing")), (Some("south"), Some("north")));
    assert_eq!(prop(state_at(&server, hook_a), "attached"), Some("false"));

    // String between them attaches both.
    hold(&server, id, Slot::of(item("minecraft:string"), 2));
    alice.send(&use_on(BlockPos::new(10, -61, 8), 1, 3));
    wait_until("attached hooks", TIMEOUT, || prop(state_at(&server, hook_a), "attached") == Some("true") && prop(state_at(&server, hook_b), "attached") == Some("true"));
    let wire = state_at(&server, string);
    assert_eq!(blocks::block_of_state(wire).name, "minecraft:tripwire");
    assert_eq!((prop(wire, "attached"), prop(wire, "north"), prop(wire, "south"), prop(wire, "east")), (Some("true"), Some("true"), Some("true"), Some("false")));

    // Walking into the string powers the hooks and lights the lamp; leaving turns them off.
    server.handle.run(move |s| s.teleport(id, Location { x: 10.5, y: -60.0, z: 8.5, yaw: 0.0, pitch: 0.0 })).unwrap();
    wait_until("powered hooks", TIMEOUT, || prop(state_at(&server, hook_a), "powered") == Some("true") && prop(state_at(&server, hook_b), "powered") == Some("true"));
    assert_eq!(prop(state_at(&server, string), "powered"), Some("true"));
    wait_until("the lamp", TIMEOUT, || prop(state_at(&server, lamp), "lit") == Some("true"));
    server.handle.run(move |s| s.teleport(id, Location { x: 8.5, y: -60.0, z: 8.5, yaw: 0.0, pitch: 0.0 })).unwrap();
    wait_until("the hooks to turn off", TIMEOUT, || prop(state_at(&server, hook_a), "powered") == Some("false") && prop(state_at(&server, string), "powered") == Some("false"));
    wait_until("the lamp to go out", TIMEOUT, || prop(state_at(&server, lamp), "lit") == Some("false"));

    // Something dropped into it presses it too.
    server.handle.run(move |s| s.spawn_item(0, paper_server::world::entity::Vec3::new(10.5, -59.9, 8.5), Slot::of(item("minecraft:stone"), 1), paper_server::world::entity::Vec3::ZERO, 40)).unwrap();
    wait_until("the item to set it off", TIMEOUT, || prop(state_at(&server, hook_b), "powered") == Some("true"));
    server.handle.run(|s| s.worlds[0].entities.values_mut().for_each(|e| e.removed = true)).unwrap();
    wait_until("the hooks to turn off again", TIMEOUT, || prop(state_at(&server, hook_b), "powered") == Some("false"));

    // Cut with shears: the hooks detach without a click.
    hold(&server, id, Slot::of(item("minecraft:shears"), 1));
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: string, face: 1, sequence: VarInt(4) });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::STOP_DESTROY_BLOCK), pos: string, face: 1, sequence: VarInt(5) });
    wait_until("detached hooks", TIMEOUT, || prop(state_at(&server, hook_a), "attached") == Some("false"));
    assert_eq!(prop(state_at(&server, hook_a), "powered"), Some("false"));
    assert!(blocks::is_air(state_at(&server, string)));

    // String broken by hand sets them off before they notice it is gone.
    hold(&server, id, Slot::of(item("minecraft:string"), 1));
    alice.send(&use_on(BlockPos::new(10, -61, 8), 1, 6));
    wait_until("attached again", TIMEOUT, || prop(state_at(&server, hook_a), "attached") == Some("true"));
    hold(&server, id, Slot::EMPTY);
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: string, face: 1, sequence: VarInt(7) });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::STOP_DESTROY_BLOCK), pos: string, face: 1, sequence: VarInt(8) });
    wait_until("the hooks set off", TIMEOUT, || prop(state_at(&server, hook_a), "powered") == Some("true"));
    wait_until("the hooks to give up", TIMEOUT, || prop(state_at(&server, hook_a), "attached") == Some("false") && prop(state_at(&server, hook_a), "powered") == Some("false"));
    server.stop();
}

#[test]
fn pistons_push_entities_slime_launches_them_and_honey_carries_them() {
    use paper_server::server::mobs::SpawnReason;
    use paper_server::world::entity::Vec3;
    let server = server();
    let _alice = join(&server, "Alice");
    let piston = |facing: &str| blocks::with_property(blocks::default_state("minecraft:piston").unwrap(), "facing", facing).unwrap();
    let redstone = blocks::default_state("minecraft:redstone_block").unwrap();
    // A piston facing east with a cow in front of it.
    set(&server, BlockPos::new(12, -60, 12), piston("east"));
    let cow = server
        .handle
        .run(|s| {
            let id = s.spawn_mob(0, "minecraft:cow", Vec3::new(13.5, -60.0, 12.5), SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&id).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
            id
        })
        .unwrap();
    // One facing up with a slime block on it and an item on the slime.
    set(&server, BlockPos::new(12, -60, 15), piston("up"));
    set(&server, BlockPos::new(12, -59, 15), blocks::default_state("minecraft:slime_block").unwrap());
    let launched = server.handle.run(|s| s.spawn_item(0, Vec3::new(12.5, -58.0, 15.5), Slot::of(paper_data::items::id("minecraft:stone").unwrap() as i32, 1), Vec3::ZERO, 40).unwrap()).unwrap();
    // One facing east with a honey block in front and an item on the honey (a block up: honey
    // on the ground would stick to it, and the ground is too long a row to push).
    set(&server, BlockPos::new(12, -59, 18), piston("east"));
    set(&server, BlockPos::new(13, -59, 18), blocks::default_state("minecraft:honey_block").unwrap());
    let carried = server.handle.run(|s| s.spawn_item(0, Vec3::new(13.5, -58.0, 18.5), Slot::of(paper_data::items::id("minecraft:dirt").unwrap() as i32, 1), Vec3::ZERO, 40).unwrap()).unwrap();
    std::thread::sleep(Duration::from_millis(500));

    for (y, z) in [(-60, 12), (-60, 15), (-59, 18)] {
        set(&server, BlockPos::new(11, y, z), redstone);
    }
    let pos = |id: i32| server.handle.run(move |s| s.worlds[0].entities.get(&id).map(|e| e.pos)).unwrap();
    wait_until("the cow pushed out of the way", TIMEOUT, || pos(cow).is_some_and(|p| p.x >= 14.45));
    wait_until("the item launched", TIMEOUT, || pos(launched).is_some_and(|p| p.y > -55.0));
    wait_until("the item carried along", TIMEOUT, || pos(carried).is_some_and(|p| p.x > 14.2));
    let p = pos(carried).unwrap();
    assert!(p.x < 14.95 && (p.z - 18.5).abs() < 0.01, "on the honey: {p:?}");
    server.stop();
}

/// Puts a dispenser facing `facing` at `pos` holding `items`, and fires it once by placing
/// (and then removing) a redstone block on top of it.
fn fire_dispenser(server: &paper_server::RunningServer, pos: BlockPos, facing: &str, items: Vec<Slot>) {
    let state = blocks::with_property(blocks::default_state("minecraft:dispenser").unwrap(), "facing", facing).unwrap();
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, pos, state);
            let mut slots = s.container_items(0, pos);
            for (i, item) in items.into_iter().enumerate() {
                slots[i] = item;
            }
            s.set_container_items(0, pos, &slots);
        })
        .unwrap();
    pulse(server, pos);
}

/// Powers a dispenser at `pos` once.
fn pulse(server: &paper_server::RunningServer, pos: BlockPos) {
    let above = pos.offset(0, 1, 0);
    let old = state_at(server, above);
    set(server, above, blocks::default_state("minecraft:redstone_block").unwrap());
    std::thread::sleep(Duration::from_millis(400));
    set(server, above, old);
    std::thread::sleep(Duration::from_millis(150));
}

fn dispenser_items(server: &paper_server::RunningServer, pos: BlockPos) -> Vec<Slot> {
    server.handle.run(move |s| s.container_items(0, pos)).unwrap()
}

fn count_of(slots: &[Slot], name: &str) -> i32 {
    let id = item(name);
    slots.iter().filter_map(|s| s.0.as_ref()).filter(|s| s.item == id).map(|s| s.count).sum()
}

#[test]
fn dispensers_use_bone_meal_shears_bottles_and_buckets() {
    use paper_server::server::mobs::SpawnReason;
    use paper_server::world::entity::Vec3;
    let server = server();
    let _alice = join(&server, "Alice");
    // Bone meal grows the wheat in front; with nothing to grow the dispenser keeps it.
    let (d, crop) = (BlockPos::new(-10, -59, 0), BlockPos::new(-9, -59, 0));
    set(&server, BlockPos::new(-9, -60, 0), blocks::default_state("minecraft:farmland").unwrap());
    set(&server, crop, blocks::default_state("minecraft:wheat").unwrap());
    fire_dispenser(&server, d, "east", vec![Slot::of(item("minecraft:bone_meal"), 3)]);
    wait_until("the wheat to grow", TIMEOUT, || prop(state_at(&server, crop), "age").is_some_and(|a| a != "0"));
    assert_eq!(count_of(&dispenser_items(&server, d), "minecraft:bone_meal"), 2);
    set(&server, crop, blocks::AIR);
    pulse(&server, d);
    assert_eq!(count_of(&dispenser_items(&server, d), "minecraft:bone_meal"), 2, "nothing to grow: kept");

    // Shears shear the sheep in front and wear down.
    let sheep = server
        .handle
        .run(|s| {
            let id = s.spawn_mob(0, "minecraft:sheep", Vec3::new(-7.5, -60.0, 4.5), SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&id).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
            id
        })
        .unwrap();
    // The sheep stands below the dispenser's front: aim it down at the sheep's block instead.
    fire_dispenser(&server, BlockPos::new(-8, -58, 4), "down", vec![Slot::of(item("minecraft:shears"), 1)]);
    wait_until("a shorn sheep", TIMEOUT, || server.handle.run(move |s| s.worlds[0].entities[&sheep].mob().unwrap().sheared).unwrap());
    let shears = dispenser_items(&server, BlockPos::new(-8, -58, 4)).into_iter().find(|s| !s.is_empty()).unwrap();
    assert_eq!(paper_server::server::survival::item_damage(shears.0.as_ref().unwrap()), 1);

    // Glass bottles fill from water: the water bottle goes into the dispenser.
    let d = BlockPos::new(-10, -59, 8);
    set(&server, BlockPos::new(-9, -59, 8), blocks::default_state("minecraft:water").unwrap());
    fire_dispenser(&server, d, "east", vec![Slot::of(item("minecraft:glass_bottle"), 2)]);
    wait_until("a water bottle", TIMEOUT, || count_of(&dispenser_items(&server, d), "minecraft:potion") == 1);
    assert_eq!(count_of(&dispenser_items(&server, d), "minecraft:glass_bottle"), 1);
    // ... and a water bottle makes mud of dirt.
    let water = dispenser_items(&server, d).into_iter().find(|s| s.0.as_ref().is_some_and(|s| s.item == item("minecraft:potion"))).unwrap();
    let d = BlockPos::new(-10, -59, 12);
    set(&server, BlockPos::new(-9, -59, 12), blocks::default_state("minecraft:dirt").unwrap());
    fire_dispenser(&server, d, "east", vec![water]);
    wait_until("mud", TIMEOUT, || blocks::block_of_state(state_at(&server, BlockPos::new(-9, -59, 12))).name == "minecraft:mud");
    assert_eq!(count_of(&dispenser_items(&server, d), "minecraft:glass_bottle"), 1);

    // A bucket of cod lets out water and a cod; powder snow fills the block in front.
    let d = BlockPos::new(-10, -59, 16);
    fire_dispenser(&server, d, "east", vec![Slot::of(item("minecraft:cod_bucket"), 1)]);
    wait_until("a cod", TIMEOUT, || server.handle.run(|s| s.worlds[0].entities.values().any(|e| e.info.name == "minecraft:cod")).unwrap());
    assert_eq!(blocks::block_of_state(state_at(&server, BlockPos::new(-9, -59, 16))).name, "minecraft:water");
    assert_eq!(count_of(&dispenser_items(&server, d), "minecraft:bucket"), 1);
    let d = BlockPos::new(-10, -59, 20);
    fire_dispenser(&server, d, "east", vec![Slot::of(item("minecraft:powder_snow_bucket"), 1)]);
    wait_until("powder snow", TIMEOUT, || blocks::block_of_state(state_at(&server, BlockPos::new(-9, -59, 20))).name == "minecraft:powder_snow");
    server.stop();
}

#[test]
fn dispensers_dress_players_and_mobs_and_shoot_fire_charges() {
    use paper_server::server::mobs::SpawnReason;
    use paper_server::world::entity::Vec3;
    let server = server();
    let alice = join(&server, "Alice");
    let id = alice.entity_id;
    // A helmet goes on Alice's head as she stands in front.
    server.handle.run(move |s| s.teleport(id, Location { x: 10.5, y: -60.0, z: 2.5, yaw: 0.0, pitch: 0.0 })).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    fire_dispenser(&server, BlockPos::new(9, -60, 2), "east", vec![Slot::of(item("minecraft:iron_helmet"), 2)]);
    wait_until("the helmet on", TIMEOUT, || server.handle.run(move |s| s.players[&id].inventory[5].clone()).unwrap() == Slot::of(item("minecraft:iron_helmet"), 1));
    // A second one has nowhere to go and flies out.
    pulse(&server, BlockPos::new(9, -60, 2));
    wait_until("the helmet dropped", TIMEOUT, || count_of(&dispenser_items(&server, BlockPos::new(9, -60, 2)), "minecraft:iron_helmet") == 0);

    // A saddle goes on a pig.
    let pig = server
        .handle
        .run(|s| {
            let id = s.spawn_mob(0, "minecraft:pig", Vec3::new(10.5, -60.0, 6.5), SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&id).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
            id
        })
        .unwrap();
    fire_dispenser(&server, BlockPos::new(9, -60, 6), "east", vec![Slot::of(item("minecraft:saddle"), 1)]);
    wait_until("the pig saddled", TIMEOUT, || server.handle.run(move |s| s.worlds[0].entities[&pig].mob().unwrap().equipment[7].clone()).unwrap() == Slot::of(item("minecraft:saddle"), 1));

    // A fire charge: a small fireball that sets fire where it lands.
    for z in 8..=14 {
        for y in -60..=-57 {
            server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(15, y, z), blocks::default_state("minecraft:stone").unwrap())).unwrap();
        }
    }
    fire_dispenser(&server, BlockPos::new(10, -60, 11), "east", vec![Slot::of(item("minecraft:fire_charge"), 1)]);
    wait_until("fire", TIMEOUT, || {
        server
            .handle
            .run(|s| {
                (11..=14).any(|x| (-60..=-57).any(|y| (8..=14).any(|z| s.worlds[0].block_at(x, y, z).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:fire"))))
            })
            .unwrap()
    });
    server.stop();
}

#[test]
fn crafters_craft_on_a_pulse_fill_evenly_and_switch_slots_off() {
    use paper_server::world::block_updates::{redstone, Level};
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let item_id = |n: &str| item(n);
    // A crafter facing east with a chest in front of it.
    let (crafter, chest) = (BlockPos::new(10, -60, 6), BlockPos::new(11, -60, 6));
    let state = blocks::with_property(blocks::default_state("minecraft:crafter").unwrap(), "orientation", "east_up").unwrap();
    set(&server, crafter, state);
    set(&server, chest, blocks::default_state("minecraft:chest").unwrap());
    let log = Slot::of(item_id("minecraft:oak_log"), 2);
    server
        .handle
        .run(move |s| {
            let mut slots = s.container_items(0, crafter);
            slots[0] = log;
            s.set_container_items(0, crafter, &slots);
        })
        .unwrap();

    // Alice opens it: the result shows last, and she switches the middle slot off.
    alice.clear_backlog();
    alice.send(&use_on(crafter, 1, 1));
    let open: play::OpenScreen = alice.find(TIMEOUT, |_: &play::OpenScreen| true).expect("the crafter's window");
    assert_eq!(open.menu_type.0, paper_data::registries::id("minecraft:menu", "minecraft:crafter_3x3").unwrap() as i32);
    let content: play::ContainerSetContent = alice.find(TIMEOUT, |c: &play::ContainerSetContent| c.window_id == open.window_id.0).expect("contents");
    assert_eq!(content.slots.len(), 46);
    assert_eq!(content.slots[45].0.as_ref().map(|s| (s.item, s.count)), Some((item_id("minecraft:oak_planks"), 4)));
    alice.send(&play::ContainerSlotStateChanged { slot_id: VarInt(4), window_id: open.window_id, new_state: false });
    alice.find(TIMEOUT, |d: &play::ContainerSetData| d.window_id == open.window_id.0 && d.property == 4 && d.value == 1).expect("slot 4 switched off");
    assert_eq!(server.handle.run(move |s| s.crafter_disabled(0, crafter)).unwrap()[4], true);

    // A hopper above fills the empty slots one item each, skipping the switched-off one.
    let hopper = BlockPos::new(10, -59, 6);
    set(&server, hopper, blocks::with_property(blocks::default_state("minecraft:hopper").unwrap(), "facing", "down").unwrap());
    let sticks = Slot::of(item_id("minecraft:stick"), 4);
    server
        .handle
        .run(move |s| {
            let mut slots = s.container_items(0, hopper);
            slots[0] = sticks;
            s.set_container_items(0, hopper, &slots);
        })
        .unwrap();
    wait_until("the sticks in", TIMEOUT, || server.handle.run(move |s| s.container_items(0, hopper)[0].is_empty()).unwrap());
    let slots = server.handle.run(move |s| s.container_items(0, crafter)).unwrap();
    let filled: Vec<usize> = (0..9).filter(|i| slots[*i].0.as_ref().is_some_and(|s| s.item == item_id("minecraft:stick"))).collect();
    assert_eq!(filled, vec![1, 2, 3, 5]);
    // Comparators count the used and switched-off slots.
    let signal = server.handle.run(move |s| redstone::analog_output(&Level::new(&mut s.worlds[0]), state, crafter)).unwrap();
    assert_eq!(signal, 6);
    set(&server, hopper, blocks::AIR);
    server
        .handle
        .run(move |s| {
            let mut slots = s.container_items(0, crafter);
            for i in [1, 2, 3, 5] {
                slots[i] = Slot::EMPTY;
            }
            s.set_container_items(0, crafter, &slots);
        })
        .unwrap();

    // A pulse crafts: the planks go into the chest and one log is used up.
    set(&server, BlockPos::new(9, -60, 6), blocks::default_state("minecraft:redstone_block").unwrap());
    wait_until("planks in the chest", TIMEOUT, || server.handle.run(move |s| s.container_items(0, chest).iter().any(|s| s.0.as_ref().is_some_and(|s| s.count == 4))).unwrap());
    let left = server.handle.run(move |s| s.container_items(0, crafter)[0].clone()).unwrap();
    assert_eq!(left, Slot::of(item_id("minecraft:oak_log"), 1));
    assert_eq!(prop(state_at(&server, crafter), "triggered"), Some("true"));
    wait_until("the craft's glow to end", TIMEOUT, || prop(state_at(&server, crafter), "crafting") == Some("false"));
    // Without a chest the result flies out of the front.
    set(&server, BlockPos::new(9, -60, 6), blocks::AIR);
    set(&server, chest, blocks::AIR);
    server.handle.run(|s| s.worlds[0].entities.values_mut().for_each(|e| e.removed = true)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    set(&server, BlockPos::new(9, -60, 6), blocks::default_state("minecraft:redstone_block").unwrap());
    wait_until("planks on the ground", TIMEOUT, || {
        server
            .handle
            .run(|s| s.worlds[0].entities.values().any(|e| matches!(&e.kind, paper_server::world::entity::EntityKind::Item { stack: Slot(Some(st)), .. } if st.item == items::id("minecraft:oak_planks").unwrap() as i32 && st.count == 4)))
            .unwrap()
    });
    assert!(server.handle.run(move |s| s.container_items(0, crafter)[0].is_empty()).unwrap());

    // Placed by Alice, who looks south: its front faces her.
    hold(&server, id, Slot::of(item_id("minecraft:crafter"), 1));
    alice.send(&use_on(BlockPos::new(12, -61, 10), 1, 2));
    wait_until("a placed crafter", TIMEOUT, || blocks::block_of_state(state_at(&server, BlockPos::new(12, -60, 10))).name == "minecraft:crafter");
    assert_eq!(prop(state_at(&server, BlockPos::new(12, -60, 10)), "orientation"), Some("north_up"));
    server.stop();
}
