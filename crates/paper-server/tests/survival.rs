//! Survival: fall damage, death and respawn, eating, fighting mobs, explosions.

mod common;

use common::*;
use paper_data::items;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::{Difficulty, GameMode};
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(300);

fn survival_server() -> paper_server::RunningServer {
    start_server(|c| {
        c.properties.gamemode = GameMode::Survival;
        c.properties.difficulty = Difficulty::Normal;
    })
}

fn join(server: &paper_server::RunningServer, name: &str) -> Client {
    let mut client = Client::join(server.port(), name);
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(QUIET);
    client.clear_backlog();
    client
}

fn damage_type(name: &str) -> i32 {
    paper_data::synced::id("minecraft:damage_type", name).unwrap()
}

/// Sends the moves of a fall from `from_y` to `to_y` (one packet per block, like a client).
fn fall(client: &mut Client, x: f64, from_y: f64, to_y: f64, z: f64) {
    let mut y = from_y;
    while y > to_y {
        client.send(&play::MovePlayerPos { x, y, z, flags: 0 });
        client.send(&play::ClientTickEnd {});
        y -= 1.0;
    }
    client.send(&play::MovePlayerPos { x, y: to_y, z, flags: play::move_flags::ON_GROUND });
    client.send(&play::ClientTickEnd {});
}

#[test]
fn falling_hurts_and_dying_drops_items_until_respawn() {
    let server = survival_server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;

    // A ten-block fall: 10 - 3 = 7 damage.
    fall(&mut alice, 8.5, -50.0, -60.0, 8.5);
    let hurt: play::DamageEvent = alice.find(TIMEOUT, |d: &play::DamageEvent| d.entity_id == id).expect("damage event");
    assert_eq!(hurt.source_type, damage_type("minecraft:fall"));
    let health: play::SetHealth = alice.find(TIMEOUT, |h: &play::SetHealth| h.health < 20.0).expect("health update");
    assert_eq!(health.health, 13.0);

    // Dying drops the inventory and shows the death screen.
    let stick = items::id("minecraft:stick").unwrap() as i32;
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&id).unwrap().inventory[36] = Slot::of(stick, 5);
        })
        .unwrap();
    server.handle.console_command("kill Alice");
    let death: play::PlayerCombatKill = alice.expect();
    assert_eq!(death.player_id.0, id);
    assert!(death.message.to_plain().contains("Alice"), "death message names the player: {}", death.message.to_plain());
    alice.drain(QUIET);
    let (dropped, inventory_empty) = server
        .handle
        .run(move |s| {
            let dropped: i32 = s.worlds[0]
                .entities
                .values()
                .filter_map(|e| match &e.kind {
                    paper_server::world::entity::EntityKind::Item { stack: Slot(Some(stack)), .. } if stack.item == stick => Some(stack.count),
                    _ => None,
                })
                .sum();
            (dropped, s.players[&id].inventory.iter().all(Slot::is_empty))
        })
        .unwrap();
    assert_eq!(dropped, 5, "the sticks are on the ground");
    assert!(inventory_empty);

    // Respawning: full health and food, back at the spawn.
    alice.clear_backlog();
    alice.send(&play::ClientCommand { action: VarInt(0) });
    alice.expect::<play::Respawn>();
    let health: play::SetHealth = alice.find(TIMEOUT, |h: &play::SetHealth| h.health == 20.0).expect("full health");
    assert_eq!(health.food.0, 20);
    let pos: play::PlayerPosition = alice.expect();
    assert!((pos.x - 8.5).abs() < 11.0 && (pos.z - 8.5).abs() < 11.0, "respawned near the world spawn: {pos:?}");
    server.stop();
}

#[test]
fn eating_restores_food() {
    let server = survival_server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let bread = items::id("minecraft:bread").unwrap() as i32;
    server
        .handle
        .run(move |s| {
            let p = s.players.get_mut(&id).unwrap();
            p.food = 10;
            p.saturation = 0.0;
            p.inventory[36] = Slot::of(bread, 2);
            s.resync_inventory(id);
        })
        .unwrap();
    alice.drain(QUIET);
    alice.clear_backlog();

    // Hold right-click: after 1.6 seconds the bread is eaten.
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(1), yaw: 0.0, pitch: 0.0 });
    let done: play::EntityEvent = alice.find(TIMEOUT, |e: &play::EntityEvent| e.status == 9).expect("finished eating");
    assert_eq!(done.entity_id, id);
    let health: play::SetHealth = alice.find(TIMEOUT, |h: &play::SetHealth| h.food.0 > 10).expect("food update");
    assert_eq!(health.food.0, 15, "bread gives 5 food");
    assert!(health.saturation > 0.0);
    let left = server.handle.run(move |s| s.players[&id].inventory[36].clone()).unwrap();
    assert_eq!(left, Slot::of(bread, 1), "one bread eaten");
    server.stop();
}

#[test]
fn players_fight_and_kill_mobs() {
    let server = survival_server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let sword = items::id("minecraft:diamond_sword").unwrap() as i32;
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().inventory[36] = Slot::of(sword, 1)).unwrap();

    // At night (zombies burn in daylight), a zombie right next to Alice.
    server.handle.console_command("time set midnight");
    server.handle.console_command("summon zombie 10.5 -60 8.5");
    let zombie_type = paper_data::entity_types::by_name("minecraft:zombie").unwrap().id;
    let spawn: play::AddEntity = alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == zombie_type).expect("zombie appears");
    let zombie = spawn.entity_id.0;

    // A diamond sword at full strength deals 7; three hits kill a 20-health zombie.
    for hit in 0..3 {
        std::thread::sleep(Duration::from_millis(700));
        alice.send(&play::Attack { entity_id: VarInt(zombie) });
        let event: play::DamageEvent = alice.find(TIMEOUT, |d: &play::DamageEvent| d.entity_id == zombie).unwrap_or_else(|| panic!("hit {hit} lands"));
        assert_eq!(event.source_type, damage_type("minecraft:player_attack"));
        assert_eq!(event.cause_id, Some(id));
    }
    let death: play::EntityEvent = alice.find(TIMEOUT, |e: &play::EntityEvent| e.entity_id == zombie && e.status == 3).expect("death animation");
    assert_eq!(death.status, 3);
    alice.find(TIMEOUT, |r: &play::RemoveEntities| r.entity_ids.contains(&VarInt(zombie))).expect("body removed");
    // Killed by a player: experience orbs.
    let orb_type = paper_data::entity_types::by_name("minecraft:experience_orb").unwrap().id;
    alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == orb_type).expect("experience dropped");
    // The sword wore down.
    let sword_damage = server.handle.run(move |s| s.players[&id].inventory[36].0.as_ref().map(paper_server::server::survival::item_damage)).unwrap();
    assert_eq!(sword_damage, Some(3), "one point of wear per hit");
    server.stop();
}

#[test]
fn creepers_explode_near_players() {
    let server = survival_server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    server.handle.console_command("summon creeper 10.5 -60 8.5");
    let explosion: play::Explode = alice.find(Duration::from_secs(30), |_: &play::Explode| true).expect("the creeper explodes");
    assert!((explosion.center.0 - 10.5).abs() < 3.0, "{:?}", explosion.center);
    assert!(explosion.block_count > 0, "the ground is blown up");
    assert!(explosion.knockback.is_some(), "Alice is pushed");
    let health: play::SetHealth = alice.find(TIMEOUT, |h: &play::SetHealth| h.health < 20.0).expect("Alice is hurt");
    assert!(health.health < 20.0);
    let crater = server.handle.run(|s| s.worlds[0].block_at(10, -61, 8).map(paper_data::blocks::is_air)).unwrap();
    assert_eq!(crater, Some(true), "a crater where the creeper stood");
    let _ = id;
    server.stop();
}

#[test]
fn game_rules_can_turn_damage_off() {
    let server = survival_server();
    let mut alice = join(&server, "Alice");
    server.handle.console_command("gamerule fall_damage false");
    alice.drain(QUIET);
    alice.clear_backlog();
    fall(&mut alice, 8.5, -40.0, -60.0, 8.5);
    alice.drain(Duration::from_millis(500));
    let id = alice.entity_id;
    let health = server.handle.run(move |s| s.players[&id].health).unwrap();
    assert_eq!(health, 20.0, "no fall damage with fall_damage=false");
    let rule = server.handle.run(|s| s.worlds[0].rules.bool("minecraft:fall_damage")).unwrap();
    assert!(!rule);
    server.stop();
}
