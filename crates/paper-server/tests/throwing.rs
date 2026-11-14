//! Thrown items: snowballs and eggs, ender pearls, splash and lingering potions, bottles o'
//! enchanting, dispensers throwing them; and spawn eggs.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::{LpVec3, Slot};
use paper_server::config::{Difficulty, GameMode};
use paper_server::server::mobs::SpawnReason;
use paper_server::server::player::Location;
use paper_server::world::entity::{EntityKind, Vec3};
use std::time::{Duration, Instant};

fn server() -> paper_server::RunningServer {
    let server = start_server(|c| {
        c.properties.gamemode = GameMode::Survival;
        c.properties.difficulty = Difficulty::Normal;
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

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn hold(server: &paper_server::RunningServer, player: i32, stack: Slot) {
    server.handle.run(move |s| s.set_inventory_slot(player, 36, stack)).unwrap();
}

fn count(server: &paper_server::RunningServer, name: &'static str) -> usize {
    server.handle.run(move |s| s.worlds[0].entities.values().filter(|e| e.info.name == name && !e.removed).count()).unwrap()
}

fn use_item(yaw: f32, pitch: f32) -> play::UseItem {
    play::UseItem { hand: VarInt(0), sequence: VarInt(1), yaw, pitch }
}

/// A potion stack (`potion_contents` with just a potion).
fn potion(kind: &str, potion: &str) -> Slot {
    let id = paper_data::registries::id("minecraft:potion", potion).unwrap() as i32;
    let mut stack = paper_protocol::types::ItemStack::new(item(kind), 1);
    let mut data = vec![1u8];
    paper_protocol::codec::Encode::encode(&VarInt(id), &mut data);
    data.extend_from_slice(&[0, 0, 0]);
    let ty = paper_data::registries::id("minecraft:data_component_type", "minecraft:potion_contents").unwrap() as i32;
    stack.added.push((ty, data));
    Slot(Some(stack))
}

#[test]
fn snowballs_fly_land_and_hurt_blazes_and_eggs_hatch_chicks() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    // A thrown snowball shows its item, flies, and is gone when it lands; one was used.
    hold(&server, id, Slot::of(item("minecraft:snowball"), 16));
    alice.send(&use_item(0.0, -20.0));
    let snowball_type = paper_data::entity_types::by_name("minecraft:snowball").unwrap().id;
    alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == snowball_type).expect("the snowball");
    wait_until("the snowball to land", TIMEOUT, || count(&server, "minecraft:snowball") == 0);
    assert_eq!(server.handle.run(move |s| s.players[&id].inventory[36].clone()).unwrap(), Slot::of(item("minecraft:snowball"), 15));

    // Snowballs hurt blazes.
    let blaze = server
        .handle
        .run(|s| {
            let id = s.spawn_mob(0, "minecraft:blaze", Vec3::new(0.5, -60.0, 8.5), SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&id).unwrap().no_gravity = true;
            id
        })
        .unwrap();
    server
        .handle
        .run(move |s| {
            let item = Slot::of(paper_data::items::id("minecraft:snowball").unwrap() as i32, 1);
            s.spawn_thrown(0, "minecraft:snowball", Vec3::new(0.5, -59.0, 4.5), Vec3::new(0.0, 0.0, 1.0), 1.5, 0.0, 0.0, item, None);
        })
        .unwrap();
    wait_until("the blaze to be hit", TIMEOUT, || server.handle.run(move |s| s.worlds[0].entities.get(&blaze).and_then(|e| e.mob()).map(|m| m.living.health)).unwrap() == Some(17.0));

    // Eggs: one in eight hatches.
    server
        .handle
        .run(|s| {
            let egg = paper_data::items::id("minecraft:egg").unwrap() as i32;
            for i in 0..64 {
                let x = 20.5 + (i % 8) as f64;
                let z = 20.5 + (i / 8) as f64;
                s.spawn_thrown(0, "minecraft:egg", Vec3::new(x, -57.0, z), Vec3::new(0.0, -1.0, 0.0), 0.5, 0.0, 0.03, Slot::of(egg, 1), None);
            }
        })
        .unwrap();
    wait_until("the eggs to land", TIMEOUT, || count(&server, "minecraft:egg") == 0);
    let chicks = server.handle.run(|s| s.worlds[0].entities.values().filter(|e| e.info.name == "minecraft:chicken" && e.mob().is_some_and(|m| m.baby)).count()).unwrap();
    assert!(chicks >= 1, "some egg hatched");
    server.stop();
}

#[test]
fn ender_pearls_take_their_thrower_along() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    hold(&server, id, Slot::of(item("minecraft:ender_pearl"), 2));
    alice.send(&use_item(0.0, -30.0));
    // The pearl cools down for a second.
    alice.find(TIMEOUT, |c: &play::Cooldown| c.group == "minecraft:ender_pearl" && c.duration.0 == 20).expect("the cooldown");
    let teleport: play::PlayerPosition = alice.find(TIMEOUT, |p: &play::PlayerPosition| p.z > 4.0).expect("the pearl takes Alice along");
    assert!(teleport.z > 4.0);
    // It hurts a little.
    wait_until("the landing to hurt", TIMEOUT, || server.handle.run(move |s| s.players[&id].health).unwrap() <= 15.0);
    // A second pearl right away does nothing.
    alice.send(&use_item(0.0, -30.0));
    alice.drain(Duration::from_millis(300));
    assert_eq!(count(&server, "minecraft:ender_pearl"), 0);
    assert_eq!(server.handle.run(move |s| s.players[&id].inventory[36].clone()).unwrap(), Slot::of(item("minecraft:ender_pearl"), 1));
    server.stop();
}

#[test]
fn potions_splash_and_linger_and_bottles_give_experience() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let speed = paper_data::effects::by_name("minecraft:speed").unwrap().id;
    // A splash potion at her feet: she runs faster, for less than a drink would give.
    let at = server.handle.run(move |s| s.players[&id].loc).unwrap();
    server
        .handle
        .run(move |s| {
            s.spawn_thrown(0, "minecraft:splash_potion", Vec3::new(at.x, at.y + 1.0, at.z), Vec3::new(0.0, -1.0, 0.0), 0.5, 0.0, 0.05, potion("minecraft:splash_potion", "minecraft:swiftness"), None);
        })
        .unwrap();
    wait_until("swiftness", TIMEOUT, || server.handle.run(move |s| s.players[&id].effects.iter().any(|e| e.id == speed && e.duration > 20 && e.duration <= 3600)).unwrap());

    // A lingering potion breaking on the ground beside a cow leaves a cloud that gives it
    // regeneration.
    let regeneration = paper_data::effects::by_name("minecraft:regeneration").unwrap().id;
    let cow = server
        .handle
        .run(|s| {
            let id = s.spawn_mob(0, "minecraft:cow", Vec3::new(12.5, -60.0, 12.5), SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&id).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
            s.spawn_thrown(0, "minecraft:lingering_potion", Vec3::new(14.0, -58.0, 12.5), Vec3::new(0.0, -1.0, 0.0), 0.5, 0.0, 0.05, potion("minecraft:lingering_potion", "minecraft:regeneration"), None);
            id
        })
        .unwrap();
    wait_until("a cloud", TIMEOUT, || count(&server, "minecraft:area_effect_cloud") == 1);
    let cloud = server.handle.run(|s| s.worlds[0].entities.values().find(|e| e.info.name == "minecraft:area_effect_cloud").map(|e| e.pos)).unwrap().unwrap();
    assert!((cloud.y + 60.0).abs() < 0.01, "on the ground: {cloud:?}");
    wait_until("the cow's regeneration", TIMEOUT, || server.handle.run(move |s| s.worlds[0].entities[&cow].mob().unwrap().living.effects.iter().any(|(e, _, _)| *e == regeneration)).unwrap());

    // A bottle o' enchanting breaks into experience.
    hold(&server, id, Slot::of(item("minecraft:experience_bottle"), 1));
    server.handle.run(move |s| s.teleport(id, Location { x: -10.5, y: -60.0, z: -10.5, yaw: 0.0, pitch: 60.0 })).unwrap();
    alice.drain(Duration::from_millis(200));
    alice.send(&use_item(0.0, 60.0));
    wait_until("experience orbs", TIMEOUT, || count(&server, "minecraft:experience_orb") > 0);
    server.stop();
}

#[test]
fn spawn_eggs_and_dispensers() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    // A spawn egg on the ground (players join at the middle of chunk 0, 0): a cow on top of
    // the clicked block.
    hold(&server, id, Slot::of(item("minecraft:cow_spawn_egg"), 2));
    let ground = BlockPos::new(9, -61, 10);
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: ground, face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(1) });
    wait_until("a cow", TIMEOUT, || count(&server, "minecraft:cow") == 1);
    let cow = server.handle.run(|s| s.worlds[0].entities.values().find(|e| e.info.name == "minecraft:cow").map(|e| e.pos)).unwrap().unwrap();
    assert!((cow.x - 9.5).abs() < 0.6 && (cow.z - 10.5).abs() < 0.6 && (cow.y + 60.0).abs() < 0.01, "on the block: {cow:?}");
    let left = server.handle.run(move |s| s.players[&id].inventory[36].clone()).unwrap();
    assert_eq!(left, Slot::of(item("minecraft:cow_spawn_egg"), 1));
    // The same egg on the cow: a calf.
    let cow_id = server.handle.run(|s| s.worlds[0].entities.values().find(|e| e.info.name == "minecraft:cow").map(|e| e.id)).unwrap().unwrap();
    alice.send(&play::Interact { entity_id: VarInt(cow_id), hand: VarInt(0), target: LpVec3(0.0, 0.5, 0.0), sneaking: false });
    wait_until("a calf", TIMEOUT, || count(&server, "minecraft:cow") == 2);
    let calves = server.handle.run(|s| s.worlds[0].entities.values().filter(|e| e.mob().is_some_and(|m| m.baby && m.age < 0)).count()).unwrap();
    assert_eq!(calves, 1);
    hold(&server, id, Slot::of(item("minecraft:cow_spawn_egg"), 1));
    // On a spawner it sets what spawns there.
    let spawner = BlockPos::new(10, -60, 7);
    server.handle.run(move |s| s.set_block_and_update(0, spawner, blocks::default_state("minecraft:spawner").unwrap())).unwrap();
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: spawner, face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(2) });
    alice.find(TIMEOUT, |b: &play::BlockEntityData| b.pos == spawner).expect("the spawner's new mob");
    let spawn_data = server.handle.run(move |s| s.block_entity_data(0, spawner).and_then(|n| n.compound("SpawnData")?.compound("entity")?.string("id").map(str::to_owned))).unwrap();
    assert_eq!(spawn_data.as_deref(), Some("minecraft:cow"));

    // A powered dispenser throws a snowball.
    let dispenser = BlockPos::new(-6, -60, -6);
    server
        .handle
        .run(move |s| {
            let state = blocks::with_property(blocks::default_state("minecraft:dispenser").unwrap(), "facing", "north").unwrap();
            s.set_block_and_update(0, dispenser, state);
            let mut items = s.container_items(0, dispenser);
            items[0] = Slot::of(paper_data::items::id("minecraft:snowball").unwrap() as i32, 4);
            s.set_container_items(0, dispenser, &items);
            s.set_block_and_update(0, BlockPos::new(-5, -60, -6), blocks::default_state("minecraft:redstone_block").unwrap());
        })
        .unwrap();
    wait_until("a dispensed snowball", TIMEOUT, || server.handle.run(|s| s.worlds[0].entities.values().any(|e| matches!(e.kind, EntityKind::Thrown { .. }))).unwrap());
    let left = server.handle.run(move |s| s.container_items(0, dispenser)[0].clone()).unwrap();
    assert_eq!(left, Slot::of(item("minecraft:snowball"), 3));
    server.stop();
}
