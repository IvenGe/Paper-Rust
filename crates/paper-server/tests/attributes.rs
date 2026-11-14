//! Attributes: equipment and effects change players' and mobs' attributes, the client hears
//! about it, max health limits healing, and saved modifiers survive a rejoin.

mod common;

use common::*;
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::GameMode;
use paper_server::world::attributes::{Modifier, Operation};
use std::time::{Duration, Instant};

fn item(name: &str) -> i32 {
    paper_data::items::id(name).unwrap() as i32
}

fn attribute_id(name: &str) -> i32 {
    paper_data::registries::id("minecraft:attribute", name).unwrap() as i32
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn equipment_and_effects_change_attributes() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();
    let id = alice.entity_id;
    let value = |name: &'static str| server_value(&server, id, name);
    fn server_value(server: &paper_server::RunningServer, id: i32, name: &'static str) -> f64 {
        server.handle.run(move |s| s.players[&id].attributes.value(name)).unwrap()
    }

    // An iron chestplate gives six armour points, and the client is told.
    server.handle.run(move |s| s.set_inventory_slot(id, 6, Slot::of(item("minecraft:iron_chestplate"), 1))).unwrap();
    let update: play::UpdateAttributes = alice
        .find(TIMEOUT, |u: &play::UpdateAttributes| u.entity_id == id && u.attributes.iter().any(|(a, _, m)| *a == attribute_id("minecraft:armor") && !m.is_empty()))
        .expect("an armour update");
    let (_, base, modifiers) = update.attributes.iter().find(|(a, ..)| *a == attribute_id("minecraft:armor")).unwrap();
    assert_eq!((*base, modifiers[0].0.as_str(), modifiers[0].1), (0.0, "minecraft:armor.chestplate", 6.0));
    assert_eq!(value("armor"), 6.0);

    // A sword in hand: attack damage 1 + 5 for iron.
    server.handle.run(move |s| s.set_inventory_slot(id, 36, Slot::of(item("minecraft:iron_sword"), 1))).unwrap();
    wait_until("the sword's damage", || value("attack_damage") == 6.0);

    // Health boost raises max health, and healing goes up to it.
    server.handle.console_command("effect give Alice minecraft:health_boost 60 0");
    wait_until("health boost", || value("max_health") == 24.0);
    server.handle.run(move |s| s.heal_player(id, 100.0)).unwrap();
    assert_eq!(server.handle.run(move |s| s.players[&id].health).unwrap(), 24.0);
    server.handle.console_command("effect clear Alice");
    wait_until("health boost to end", || value("max_health") == 20.0);
    assert_eq!(server.handle.run(move |s| s.players[&id].health).unwrap(), 20.0, "health is capped again");

    // Mobs: a zombie with an iron sword hits for 3 + 5.
    let zombie = server
        .handle
        .run(|s| {
            let id = s.spawn_mob(0, "minecraft:zombie", paper_server::world::entity::Vec3::new(5.5, -60.0, 5.5), paper_server::server::mobs::SpawnReason::Command).unwrap();
            if let Some(m) = s.worlds[0].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                m.equipment[0] = Slot::of(item("minecraft:iron_sword"), 1);
            }
            id
        })
        .unwrap();
    wait_until("the zombie's sword", || server.handle.run(move |s| s.worlds[0].entities[&zombie].mob().unwrap().attributes.value("attack_damage")).unwrap() == 8.0);
    server.stop();
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("paper-rs-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn saved_modifiers_survive_a_rejoin() {
    let dir = temp_root("attributes-rejoin");
    let config = {
        let dir = dir.clone();
        move |c: &mut paper_server::config::Config| {
            c.root = dir.clone();
            c.properties.gamemode = GameMode::Survival;
        }
    };
    let server = start_server(config.clone());
    let alice = Client::join(server.port(), "Alice");
    let id = alice.entity_id;
    server
        .handle
        .run(move |s| {
            let p = s.players.get_mut(&id).unwrap();
            p.attributes.add_modifier("max_health", Modifier::new("test:bonus", 10.0, Operation::AddValue), true);
            p.attributes.add_modifier("movement_speed", Modifier::new("test:passing", 0.5, Operation::AddMultipliedBase), false);
        })
        .unwrap();
    drop(alice);
    server.stop();

    let server = start_server(config);
    let alice = Client::join(server.port(), "Alice");
    let id = alice.entity_id;
    let (max, speed) = server.handle.run(move |s| (s.players[&id].attributes.value("max_health"), s.players[&id].attributes.value("movement_speed"))).unwrap();
    assert_eq!(max, 30.0, "the saved modifier is back");
    assert!((speed - 0.1).abs() < 1e-6, "the transient one is not: {speed}");
    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
