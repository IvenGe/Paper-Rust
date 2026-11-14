//! Mobs with their own ways: endermen (stares, teleports, water, carried blocks), riding
//! (saddles, steering, boosts, getting off, horses that buck until tamed) and pets (taming,
//! sitting, following, collars).

mod common;

use common::*;
use paper_data::blocks;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::types::{LpVec3, Slot};
use paper_protocol::packets::play;
use paper_server::config::{Difficulty, GameMode};
use paper_server::server::mobs::SpawnReason;
use paper_server::server::player::Location;
use paper_server::server::survival::Damage;
use paper_server::world::entity::Vec3;
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
            // Night, so daylight does not chase the endermen away.
            s.worlds[0].day_time = 18000;
            s.worlds[0].daylight_cycle = false;
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

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// [`wait_until`] that keeps reading what the server sends (answering keep-alives), so a
/// long wait under load does not time the player out.
fn wait_reading(client: &mut Client, what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(50));
        client.clear_backlog();
    }
}

fn spawn(server: &paper_server::RunningServer, name: &'static str, at: Vec3) -> i32 {
    server.handle.run(move |s| s.spawn_mob(0, name, at, SpawnReason::Command).unwrap()).unwrap()
}

fn position(server: &paper_server::RunningServer, id: i32) -> Option<Vec3> {
    server.handle.run(move |s| s.worlds[0].entities.get(&id).map(|e| e.pos)).unwrap()
}

#[test]
fn endermen_turn_on_players_who_stare_and_dodge_arrows() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let enderman = spawn(&server, "minecraft:enderman", Vec3::new(0.5, -60.0, 8.5));
    // Hold it in place (no wandering) and look it in the eyes from eight blocks away.
    server
        .handle
        .run(move |s| {
            if let Some(m) = s.worlds[0].entities.get_mut(&enderman).and_then(|e| e.mob_mut()) {
                m.brain.idle_ticks = 1_000_000;
            }
            let eye = 2.55 - 1.62;
            let pitch = -(f64::atan2(eye, 8.0).to_degrees()) as f32;
            s.teleport(id, Location { x: 0.5, y: -60.0, z: 0.5, yaw: 0.0, pitch });
        })
        .unwrap();
    assert!(server.handle.run(move |s| s.stares_at(0, enderman, id)).unwrap(), "Alice looks it in the eyes");
    let target = |server: &paper_server::RunningServer| server.handle.run(move |s| s.worlds[0].entities.get(&enderman).and_then(|e| e.mob()).and_then(|m| m.brain.target)).unwrap();
    wait_until("the enderman to get angry", TIMEOUT, || target(&server) == Some(id));
    let creepy = server.handle.run(move |s| s.worlds[0].entities[&enderman].mob().unwrap().creepy).unwrap();
    assert!(creepy, "it screams");
    // Clients see the scream (DATA_CREEPY, index 17, a boolean: serializer 8).
    alice.find(TIMEOUT, |m: &play::SetEntityData| m.entity_id.0 == enderman && m.metadata.0.windows(3).any(|w| w == [17, 8, 1])).expect("the scream");

    // Arrows do not hit it: it teleports away unharmed.
    let before = position(&server, enderman).unwrap();
    let hurt = server
        .handle
        .run(move |s| {
            let mut arrow = Damage::new("minecraft:arrow");
            arrow.attacker = Some(id);
            s.hurt_entity(0, enderman, arrow, 5.0)
        })
        .unwrap();
    assert!(!hurt, "the arrow misses");
    let (after, health) = server.handle.run(move |s| s.worlds[0].entities.get(&enderman).map(|e| (e.pos, e.mob().unwrap().living.health))).unwrap().unwrap();
    assert!(after.distance_squared(before) > 1.0, "it teleported: {before:?} -> {after:?}");
    assert_eq!(health, 40.0);
    server.stop();
}

#[test]
fn endermen_flee_water_and_move_blocks() {
    let server = server();
    let _alice = join(&server, "Alice");
    // In water it gets hurt and teleports out.
    let water = blocks::default_state("minecraft:water").unwrap();
    server
        .handle
        .run(move |s| {
            for x in 18..21 {
                for z in 18..21 {
                    for y in -60..-57 {
                        s.set_block_and_update(0, BlockPos::new(x, y, z), water);
                    }
                }
            }
        })
        .unwrap();
    let wet = spawn(&server, "minecraft:enderman", Vec3::new(19.5, -60.0, 19.5));
    wait_until("the enderman to leave the water", TIMEOUT, || position(&server, wet).is_some_and(|p| !(18.0..21.0).contains(&p.x) || !(18.0..21.0).contains(&p.z) || p.y > -57.0));

    // With mob griefing it takes a block next to it, which it carries.
    let carrier = spawn(&server, "minecraft:enderman", Vec3::new(-9.5, -60.0, -9.5));
    let dirt = blocks::default_state("minecraft:dirt").unwrap();
    server
        .handle
        .run(move |s| {
            if let Some(m) = s.worlds[0].entities.get_mut(&carrier).and_then(|e| e.mob_mut()) {
                m.brain.idle_ticks = 1_000_000;
            }
            for x in -12..-7 {
                for z in -12..-7 {
                    if (x, z) != (-10, -10) {
                        s.set_block_and_update(0, BlockPos::new(x, -60, z), dirt);
                    }
                }
            }
        })
        .unwrap();
    let carried = |server: &paper_server::RunningServer| server.handle.run(move |s| s.worlds[0].entities.get(&carrier).and_then(|e| e.mob()).and_then(|m| m.carried)).unwrap();
    wait_until("the enderman to pick up dirt", Duration::from_secs(30), || carried(&server) == Some(dirt));
    // It is saved with the enderman.
    let nbt = server.handle.run(move |s| paper_server::world::entity_nbt::entity_to_nbt(&s.worlds[0].entities[&carrier]).unwrap()).unwrap();
    let back = paper_server::world::entity_nbt::entity_from_nbt(1, &nbt).unwrap();
    assert_eq!(back.mob().unwrap().carried, Some(dirt));
    server.stop();
}

fn item(name: &str) -> i32 {
    paper_data::items::id(name).unwrap() as i32
}

fn interact(entity: i32) -> play::Interact {
    play::Interact { entity_id: VarInt(entity), hand: VarInt(0), target: LpVec3(0.0, 0.5, 0.0), sneaking: false }
}

fn hold(server: &paper_server::RunningServer, player: i32, stack: Slot) {
    server.handle.run(move |s| s.set_inventory_slot(player, 36, stack)).unwrap();
}

fn mob<T: Send + 'static>(server: &paper_server::RunningServer, id: i32, f: impl FnOnce(&paper_server::world::entity::Mob) -> T + Send + 'static) -> T {
    server.handle.run(move |s| f(s.worlds[0].entities[&id].mob().unwrap())).unwrap()
}

fn vehicle(server: &paper_server::RunningServer, player: i32) -> Option<i32> {
    server.handle.run(move |s| s.players[&player].vehicle).unwrap()
}

/// Puts the player next to an entity and holds the entity still.
fn next_to(server: &paper_server::RunningServer, player: i32, id: i32) {
    server
        .handle
        .run(move |s| {
            let e = s.worlds[0].entities.get_mut(&id).unwrap();
            let at = e.pos;
            if let Some(m) = e.mob_mut() {
                m.brain.idle_ticks = 1_000_000;
                m.brain.path.clear();
            }
            s.teleport(player, Location { x: at.x + 1.5, y: at.y, z: at.z, yaw: 90.0, pitch: 0.0 });
        })
        .unwrap();
}

#[test]
fn pigs_are_saddled_ridden_steered_boosted_and_left() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let pig = spawn(&server, "minecraft:pig", Vec3::new(4.5, -60.0, 0.5));
    next_to(&server, id, pig);
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();

    // A saddle goes on, and shows.
    hold(&server, id, Slot::of(item("minecraft:saddle"), 1));
    alice.send(&interact(pig));
    alice.find(TIMEOUT, |e: &play::SetEquipment| e.entity_id == pig && e.equipment.iter().any(|(slot, s)| *slot == 7 && *s == Slot::of(item("minecraft:saddle"), 1))).expect("the saddle on the pig");
    assert!(mob(&server, pig, |m| !m.equipment[7].is_empty()));

    // An empty hand gets on.
    hold(&server, id, Slot::EMPTY);
    alice.send(&interact(pig));
    alice.find(TIMEOUT, |p: &play::SetPassengers| p.vehicle == pig && p.passengers == vec![id]).expect("Alice rides the pig");
    assert_eq!(vehicle(&server, id), Some(pig));

    // With a carrot on a stick she steers: the pig goes where her client says, she with it.
    hold(&server, id, Slot::of(item("minecraft:carrot_on_a_stick"), 1));
    assert_eq!(server.handle.run(move |s| s.controller_of(0, pig)).unwrap(), Some(id));
    alice.send(&play::MoveVehicle { x: 6.5, y: -60.0, z: 0.5, yaw: 270.0, pitch: 0.0, on_ground: true });
    wait_until("the pig to move", TIMEOUT, || position(&server, pig).is_some_and(|p| (p.x - 6.5).abs() < 1e-6));
    let rider = server.handle.run(move |s| s.players[&id].loc).unwrap();
    assert!((rider.x - 6.5).abs() < 1e-6 && rider.y > -59.5, "Alice sits on the pig: {rider:?}");
    // Too far in one go is refused.
    alice.send(&play::MoveVehicle { x: 60.5, y: -60.0, z: 0.5, yaw: 270.0, pitch: 0.0, on_ground: true });
    alice.find(TIMEOUT, |m: &play::ClientboundMoveVehicle| (m.x - 6.5).abs() < 1e-6).expect("the pig is put back");

    // The stick boosts, once at a time, and wears.
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(1), yaw: 270.0, pitch: 0.0 });
    wait_until("a boost", TIMEOUT, || mob(&server, pig, |m| m.boost > 0));
    let stick = server.handle.run(move |s| s.players[&id].inventory[36].clone()).unwrap();
    assert_eq!(paper_server::server::survival::item_damage(stick.0.as_ref().unwrap()), 7);

    // The sneak key gets off, beside the pig.
    alice.send(&play::PlayerInput { flags: play::input_flags::SNEAK });
    alice.find(TIMEOUT, |p: &play::SetPassengers| p.vehicle == pig && p.passengers.is_empty()).expect("Alice gets off");
    assert_eq!(vehicle(&server, id), None);
    let (at, pig_at) = server.handle.run(move |s| (s.players[&id].loc, s.worlds[0].entities[&pig].pos)).unwrap();
    assert!(at.y < -59.5 && Vec3::new(at.x, at.y, at.z).distance_squared(pig_at) < 4.0, "next to the pig: {at:?}");
    server.stop();
}

#[test]
fn horses_buck_until_tamed_and_then_take_saddles() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let horse = spawn(&server, "minecraft:horse", Vec3::new(4.5, -60.0, 0.5));
    // Horses are born with their own health, speed and jump.
    let (health, speed) = mob(&server, horse, |m| (m.attributes.value("max_health"), m.attributes.value("movement_speed")));
    assert!((15.0..=30.0).contains(&health) && (0.1125..=0.3375).contains(&speed), "{health} {speed}");

    // Wheat raises its temper.
    next_to(&server, id, horse);
    hold(&server, id, Slot::of(item("minecraft:wheat"), 3));
    alice.send(&interact(horse));
    wait_until("the horse to eat", TIMEOUT, || mob(&server, horse, |m| m.temper == 3));

    // Without a good temper it throws its rider off and gets angrier.
    hold(&server, id, Slot::EMPTY);
    alice.send(&interact(horse));
    wait_until("Alice to get on", TIMEOUT, || vehicle(&server, id) == Some(horse));
    wait_reading(&mut alice, "the horse to buck", Duration::from_secs(30), || vehicle(&server, id).is_none());
    assert_eq!(mob(&server, horse, |m| (m.tamed, m.temper)), (false, 8));

    // At the most temper it gives in: tamed by Alice, who stays on.
    server.handle.run(move |s| s.worlds[0].entities.get_mut(&horse).unwrap().mob_mut().unwrap().temper = 100).unwrap();
    next_to(&server, id, horse);
    alice.send(&interact(horse));
    wait_until("Alice to get on again", TIMEOUT, || vehicle(&server, id) == Some(horse));
    wait_reading(&mut alice, "the horse to be tamed", Duration::from_secs(30), || mob(&server, horse, |m| m.tamed));
    let uuid = server.handle.run(move |s| s.players[&id].uuid()).unwrap();
    assert_eq!(mob(&server, horse, |m| m.owner), Some(uuid));
    assert_eq!(vehicle(&server, id), Some(horse));
    // Unsaddled it does not let her steer; saddled it does.
    assert_eq!(server.handle.run(move |s| s.controller_of(0, horse)).unwrap(), None);
    alice.send(&play::PlayerInput { flags: play::input_flags::SNEAK });
    wait_until("Alice to get off", TIMEOUT, || vehicle(&server, id).is_none());
    // She lets go of the sneak key (sneaking and clicking a tamed horse opens its inventory).
    alice.send(&play::PlayerInput { flags: 0 });
    wait_until("Alice to stand up", TIMEOUT, || !server.handle.run(move |s| s.players[&id].sneaking).unwrap());
    next_to(&server, id, horse);
    hold(&server, id, Slot::of(item("minecraft:saddle"), 1));
    alice.send(&interact(horse));
    wait_until("the saddle", TIMEOUT, || mob(&server, horse, |m| !m.equipment[7].is_empty()));
    hold(&server, id, Slot::EMPTY);
    alice.send(&interact(horse));
    wait_until("Alice to ride", TIMEOUT, || vehicle(&server, id) == Some(horse));
    assert_eq!(server.handle.run(move |s| s.controller_of(0, horse)).unwrap(), Some(id));
    // The tamed horse, its saddle and coat are saved.
    let nbt = server.handle.run(move |s| paper_server::world::entity_nbt::entity_to_nbt(&s.worlds[0].entities[&horse]).unwrap()).unwrap();
    let back = paper_server::world::entity_nbt::entity_from_nbt(1, &nbt).unwrap();
    let m = back.mob().unwrap();
    assert!(m.tamed && m.owner == Some(uuid) && !m.equipment[7].is_empty());
    server.stop();
}

#[test]
fn wolves_are_tamed_with_bones_sit_follow_and_wear_collars() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let wolf = spawn(&server, "minecraft:wolf", Vec3::new(4.5, -60.0, 0.5));
    next_to(&server, id, wolf);
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();
    // Bones until it takes to her (one in three).
    hold(&server, id, Slot::of(item("minecraft:bone"), 64));
    for _ in 0..40 {
        if mob(&server, wolf, |m| m.tamed) {
            break;
        }
        alice.send(&interact(wolf));
        std::thread::sleep(Duration::from_millis(60));
    }
    let uuid = server.handle.run(move |s| s.players[&id].uuid()).unwrap();
    assert_eq!(mob(&server, wolf, |m| (m.tamed, m.owner, m.sitting)), (true, Some(uuid), true));
    assert_eq!(mob(&server, wolf, |m| m.living.max_health), 40.0, "a tamed wolf is tougher");
    // Clients see it sit as a tamed wolf (DATA_FLAGS_ID 18: sitting and tame).
    alice.find(TIMEOUT, |m: &play::SetEntityData| m.entity_id.0 == wolf && m.metadata.0.windows(3).any(|w| w == [18, 0, 5])).expect("a sitting tamed wolf");

    // A dye colours its collar.
    hold(&server, id, Slot::of(item("minecraft:blue_dye"), 1));
    alice.send(&interact(wolf));
    wait_until("a blue collar", TIMEOUT, || mob(&server, wolf, |m| m.collar == 11));

    // Sitting, it stays when she walks off; told to stand, it comes after her.
    server.handle.run(move |s| s.teleport(id, Location { x: 25.5, y: -60.0, z: 0.5, yaw: 0.0, pitch: 0.0 })).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(position(&server, wolf).unwrap().x < 10.0, "the sitting wolf stays");
    next_to(&server, id, wolf);
    hold(&server, id, Slot::EMPTY);
    alice.send(&interact(wolf));
    wait_until("the wolf to stand", TIMEOUT, || mob(&server, wolf, |m| !m.sitting));
    server.handle.run(move |s| s.teleport(id, Location { x: 30.5, y: -60.0, z: 0.5, yaw: 0.0, pitch: 0.0 })).unwrap();
    wait_until("the wolf to catch up", TIMEOUT, || position(&server, wolf).is_some_and(|p| p.x > 20.0));

    // Its owner may hit it without it biting back.
    server
        .handle
        .run(move |s| {
            let at = s.worlds[0].entities[&wolf].pos;
            s.hurt_entity(0, wolf, Damage::by("minecraft:player_attack", id, at), 1.0)
        })
        .unwrap();
    assert_eq!(mob(&server, wolf, |m| m.brain.target), None);
    // Saved as a pet.
    let nbt = server.handle.run(move |s| paper_server::world::entity_nbt::entity_to_nbt(&s.worlds[0].entities[&wolf]).unwrap()).unwrap();
    let back = paper_server::world::entity_nbt::entity_from_nbt(1, &nbt).unwrap();
    let m = back.mob().unwrap();
    assert_eq!((m.tamed, m.owner, m.collar), (true, Some(uuid), 11));
    server.stop();
}
