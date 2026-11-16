//! Respawning after death the way a real client goes about it: whatever put the player on
//! the death screen (dying here, coming back after leaving dead, a plugin taking all its
//! health), the respawn button brings it back, and the client gets the chunks and entities
//! around it again, in the same world too, as from vanilla.

mod common;

use common::*;
use paper_bridge::bmap;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use paper_server::config::{Difficulty, GameMode};
use paper_server::server::mobs::SpawnReason;
use paper_server::world::entity::Vec3;
use paper_server::RunningServer;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(300);

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("paper-rs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn survival_server(root: Option<PathBuf>) -> RunningServer {
    let server = start_server(|c| {
        c.properties.gamemode = GameMode::Survival;
        c.properties.difficulty = Difficulty::Normal;
        if let Some(root) = root {
            c.root = root;
        }
    });
    // Players join and respawn right at the world spawn.
    server.handle.run(|s| s.set_game_rule(0, "respawn_radius", paper_data::game_rules::Value::Int(0))).unwrap();
    server
}

fn join(server: &RunningServer, name: &str) -> Client {
    let mut client = Client::join(server.port(), name);
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(QUIET);
    client.clear_backlog();
    client
}

fn wait_until(server: &RunningServer, what: &str, f: impl Fn(&mut paper_server::server::Server) -> bool + Send + Sync + Copy + 'static) {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if server.handle.run(f) == Some(true) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn is_dead(server: &RunningServer, id: i32) -> bool {
    server.handle.run(move |s| s.players[&id].dead).unwrap()
}

/// Presses the respawn button: the client is sent back into the world at full health.
fn press_respawn(client: &mut Client) {
    client.clear_backlog();
    client.send(&play::ClientCommand { action: VarInt(0) });
    client.expect::<play::Respawn>();
    client.find(TIMEOUT, |h: &play::SetHealth| h.health == 20.0).expect("full health");
}

/// A plugin call, as the plugin host makes it.
fn plugin_call(server: &RunningServer, method: &'static str, args: paper_bridge::Value) {
    let result = server.handle.run(move |s| paper_server::plugins::api::handle(s, method, &args).map(|_| ()).map_err(|e| e.to_string())).unwrap();
    result.unwrap_or_else(|e| panic!("{method}: {e}"));
}

#[test]
fn players_who_left_dead_respawn_when_they_come_back() {
    let root = temp_root("respawn-rejoin");
    let server = survival_server(Some(root.clone()));
    let mut alice = join(&server, "Alice");
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    // Off the death screen to the title screen, and back later.
    drop(alice);
    wait_until(&server, "Alice to leave", |s| s.players.is_empty());

    let mut alice = Client::join(server.port(), "Alice");
    let id = alice.entity_id;
    let health = alice.expect::<play::SetHealth>();
    assert_eq!(health.health, 0.0, "back on the death screen");
    assert!(is_dead(&server, id), "the server knows it is dead");
    alice.drain(QUIET);
    press_respawn(&mut alice);
    assert!(!is_dead(&server, id));
    let health = server.handle.run(move |s| s.players[&id].health).unwrap();
    assert_eq!(health, 20.0);

    // And it plays on: it can be hurt again.
    server.handle.run(move |s| s.hurt_player(id, paper_server::server::survival::Damage::new("minecraft:generic"), 2.0)).unwrap();
    alice.find(TIMEOUT, |h: &play::SetHealth| h.health == 18.0).expect("hurt after respawning");
    server.stop();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn plugins_taking_all_health_kill() {
    let server = survival_server(None);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let uuid = server.handle.run(move |s| s.players[&id].uuid().to_string()).unwrap();

    // `Player.setHealth(0)` (as EssentialsX's /suicide does): a death like any other, with
    // the death screen, and the respawn button works.
    plugin_call(&server, "player.set_health", bmap! { "uuid" => uuid, "health" => 0.0 });
    let death: play::PlayerCombatKill = alice.expect();
    assert_eq!(death.player_id.0, id);
    assert!(death.message.to_plain().contains("Alice"), "the death message: {}", death.message.to_plain());
    assert!(is_dead(&server, id));
    alice.drain(QUIET);
    press_respawn(&mut alice);
    assert!(!is_dead(&server, id));

    // A mob with no health left dies and goes, rather than standing there for good.
    let zombie = server.handle.run(|s| s.spawn_mob(0, "minecraft:zombie", Vec3::new(5.5, -60.0, 5.5), SpawnReason::Command).unwrap()).unwrap();
    alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_id.0 == zombie).expect("the zombie");
    plugin_call(&server, "entity.set", bmap! { "id" => zombie, "health" => 0.0 });
    alice.find(TIMEOUT, |e: &play::EntityEvent| e.entity_id == zombie && e.status == 3).expect("the zombie dies");
    wait_until(&server, "the zombie to go", move |s| s.worlds[0].entities.get(&zombie).is_none_or(|e| e.removed));
    server.stop();
}

#[test]
fn respawning_sends_the_world_around_again() {
    let server = survival_server(None);
    let mut alice = join(&server, "Alice");
    let pig = server.handle.run(|s| s.spawn_mob(0, "minecraft:pig", Vec3::new(6.5, -60.0, 6.5), SpawnReason::Command).unwrap()).unwrap();
    alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_id.0 == pig).expect("the pig");
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    alice.clear_backlog();

    // As in vanilla, a new player takes the old one's place, here in the same world: the
    // client forgets what the old one saw, then gets the chunks and entities again.
    alice.send(&play::ClientCommand { action: VarInt(0) });
    alice.find(TIMEOUT, |r: &play::RemoveEntities| r.entity_ids.contains(&VarInt(pig))).expect("the pig is forgotten");
    alice.expect::<play::Respawn>();
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("the spawn chunk again");
    alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_id.0 == pig).expect("the pig again");

    // Someone else sees the old body go and the new player come.
    let mut bob = join(&server, "Bob");
    let (alice_id, bob_id) = (alice.entity_id, bob.entity_id);
    wait_until(&server, "Bob to see Alice", move |s| s.players[&bob_id].visible.contains(&alice_id));
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    bob.drain(QUIET);
    bob.clear_backlog();
    press_respawn(&mut alice);
    bob.find(TIMEOUT, |r: &play::RemoveEntities| r.entity_ids.contains(&VarInt(alice_id))).expect("the body goes");
    bob.find(TIMEOUT, |a: &play::AddEntity| a.entity_id.0 == alice_id).expect("Alice comes back");
    server.stop();
}
