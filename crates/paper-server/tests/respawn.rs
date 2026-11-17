//! Respawning after death the way a real client goes about it: whatever put the player on
//! the death screen (dying here, coming back after leaving dead, a plugin taking all its
//! health), the respawn button brings it back, and the client gets the chunks and entities
//! around it again, in the same world too, as from vanilla.

mod common;

use common::*;
use paper_bridge::{bmap, Value};
use paper_data::blocks;
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

// --- what the audit of respawning found -----------------------------------------------------

/// Kicks whoever respawns, in `PlayerRespawnEvent` (the server once fell over this).
#[derive(Default)]
struct KickOnRespawn;

impl paper_plugin::Plugin for KickOnRespawn {
    fn info(&self) -> paper_plugin::Info {
        paper_plugin::Info::new("KickOnRespawn", "1").listen("PlayerRespawnEvent")
    }

    fn event(&mut self, server: &mut paper_plugin::Server, event: &mut paper_plugin::Event) {
        if let Some(player) = event.player() {
            let _ = server.call("player.kick", paper_plugin::bmap! { "uuid" => player.uuid, "reason" => "{\"text\":\"bye\"}" });
        }
    }
}

/// Sends whoever respawns to the Nether, and says so once the player is back.
#[derive(Default)]
struct RespawnInTheNether;

impl paper_plugin::Plugin for RespawnInTheNether {
    fn info(&self) -> paper_plugin::Info {
        paper_plugin::Info::new("RespawnInTheNether", "1").listen("PlayerRespawnEvent").listen("PlayerPostRespawnEvent")
    }

    fn event(&mut self, server: &mut paper_plugin::Server, event: &mut paper_plugin::Event) {
        if event.is("PlayerRespawnEvent") {
            event.set("world", "world_nether");
            event.set("x", 5.5);
            event.set("y", 70.0);
            event.set("z", 5.5);
        } else if let Some(player) = event.player() {
            let world = event.get("world").as_str().unwrap_or("?").to_owned();
            player.send_message(server, &format!("back in {world}"));
        }
    }
}

#[test]
fn a_plugin_kicking_on_respawn_does_not_bring_the_server_down() {
    let server = start_server_with_natives(|c| c.properties.gamemode = GameMode::Survival, vec![paper_plugin::entry::<KickOnRespawn>()]);
    let mut alice = join(&server, "Alice");
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    alice.send(&play::ClientCommand { action: VarInt(0) });
    assert!(alice.find(TIMEOUT, |d: &play::Disconnect| d.reason.to_plain().contains("bye")).is_some(), "kicked");
    // The server goes on: someone else can join and play.
    let _bob = join(&server, "Bob");
    assert_eq!(server.handle.run(|s| s.players.len()).unwrap(), 1);
    server.stop();
}

#[test]
fn plugins_choose_the_world_the_player_respawns_in() {
    let server = start_server_with_natives(|c| c.properties.gamemode = GameMode::Survival, vec![paper_plugin::entry::<RespawnInTheNether>()]);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    alice.clear_backlog();
    alice.send(&play::ClientCommand { action: VarInt(0) });
    let respawn: play::Respawn = alice.expect();
    assert_eq!(respawn.spawn.dimension_name, "minecraft:the_nether");
    let (world, x, z) = server.handle.run(move |s| { let p = &s.players[&id]; (s.worlds[p.world].name.clone(), p.loc.x, p.loc.z) }).unwrap();
    assert_eq!((world.as_str(), x, z), ("world_nether", 5.5, 5.5));
    let said: play::SystemChat = alice.find(TIMEOUT, |c: &play::SystemChat| c.content.to_plain().starts_with("back in")).expect("PlayerPostRespawnEvent");
    assert_eq!(said.content.to_plain(), "back in world_nether");
    server.stop();
}

#[test]
fn a_bed_in_a_chunk_no_one_has_loaded_is_still_found() {
    let root = temp_root("respawn-far-bed");
    let server = survival_server(Some(root.clone()));
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    // A bed far from spawn, set as Alice's respawn point.
    let head = blocks::parse_state("minecraft:red_bed[part=head,facing=north]").unwrap();
    let foot = blocks::parse_state("minecraft:red_bed[part=foot,facing=north]").unwrap();
    plugin_call(&server, "world.set_block", bmap! { "x" => 300, "y" => -60, "z" => 300, "state" => head as i64, "physics" => false });
    plugin_call(&server, "world.set_block", bmap! { "x" => 300, "y" => -60, "z" => 301, "state" => foot as i64, "physics" => false });
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().respawn = Some((0, paper_protocol::codec::BlockPos::new(300, -60, 300)))).unwrap();
    drop(alice);
    server.stop();

    // After a restart nothing keeps the bed's chunk loaded: dying still takes Alice to it.
    let server = survival_server(Some(root.clone()));
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    assert!(!server.handle.run(|s| s.worlds[0].chunks.contains_key(&paper_server::world::chunk::ChunkPos::of_block(300, 300))).unwrap(), "the bed's chunk is not loaded");
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    press_respawn(&mut alice);
    let (x, z, respawn) = server.handle.run(move |s| { let p = &s.players[&id]; (p.loc.x, p.loc.z, p.respawn) }).unwrap();
    assert!((x - 300.5).abs() < 3.0 && (z - 300.5).abs() < 3.0, "next to the bed: {x}, {z}");
    assert!(respawn.is_some(), "the respawn point is kept");
    server.stop();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn respawning_far_away_tells_the_client_everything_it_needs() {
    let server = survival_server(None);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    // A forced respawn point far away (`setRespawnLocation(location, true)`): no bed needed.
    let uuid = server.handle.run(move |s| s.players[&id].uuid().to_string()).unwrap();
    plugin_call(&server, "player.set", bmap! { "uuid" => uuid.clone(), "respawn" => Value::List(vec!["world".into(), 1000.into(), (-60).into(), 1000.into()]), "respawn_forced" => true });
    // Something to keep: armour (its attributes come back after the respawn).
    server.handle.run(|s| s.set_game_rule(0, "keep_inventory", paper_data::game_rules::Value::Bool(true))).unwrap();
    let chestplate = paper_data::items::id("minecraft:iron_chestplate").unwrap() as i32;
    server.handle.run(move |s| { s.players.get_mut(&id).unwrap().inventory[6] = paper_protocol::types::Slot::of(chestplate, 1); s.resync_inventory(id); }).unwrap();
    alice.drain(QUIET);
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    alice.clear_backlog();
    alice.send(&play::ClientCommand { action: VarInt(0) });

    // Old clients keep chunks until told: the spawn chunks are forgotten.
    alice.find(TIMEOUT, |f: &play::ForgetLevelChunk| f.x == 0 && f.z == 0).expect("the spawn chunk is forgotten");
    let respawn: play::Respawn = alice.expect();
    // Where Alice died, for the recovery compass.
    let (dimension, pos) = respawn.spawn.death_location.expect("the death location");
    assert_eq!(dimension, "minecraft:overworld");
    assert!(pos.x.abs() < 20 && pos.z.abs() < 20, "{pos:?}");
    let at: play::PlayerPosition = alice.expect();
    assert_eq!((at.x, at.z), (1000.5, 1000.5), "the forced point");
    // The client starts the new player from base values: the armour is sent again.
    alice.find(TIMEOUT, |a: &play::UpdateAttributes| a.entity_id == id).expect("attributes after the respawn");
    server.stop();
}

#[test]
fn the_crafting_grid_result_does_not_drop_on_death() {
    let server = survival_server(None);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let (log, planks) = (paper_data::items::id("minecraft:oak_log").unwrap() as i32, paper_data::items::id("minecraft:oak_planks").unwrap() as i32);
    // A log in the 2x2 grid, with what it would make in the result slot.
    server.handle.run(move |s| { let p = s.players.get_mut(&id).unwrap(); p.inventory[1] = paper_protocol::types::Slot::of(log, 1); p.inventory[0] = paper_protocol::types::Slot::of(planks, 4); }).unwrap();
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    let count = |item: i32| server.handle.run(move |s| s.worlds[0].entities.values().filter_map(|e| match &e.kind { paper_server::world::entity::EntityKind::Item { stack: paper_protocol::types::Slot(Some(st)), .. } if st.item == item => Some(st.count), _ => None }).sum::<i32>()).unwrap();
    assert_eq!((count(log), count(planks)), (1, 0), "the log drops, the planks it would make do not");
    server.stop();
}

#[test]
fn spigot_respawn_and_plugin_worlds_of_their_own() {
    let server = survival_server(None);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let uuid = server.handle.run(move |s| s.players[&id].uuid().to_string()).unwrap();
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    alice.clear_backlog();
    // `player.spigot().respawn()`, as auto-respawn plugins do: the host gets the state back,
    // so the player is alive for the plugin at once.
    let args = bmap! { "uuid" => uuid.clone() };
    let state = server.handle.run(move |s| paper_server::plugins::api::handle(s, "player.respawn", &args).map_err(|e| e.to_string())).unwrap().unwrap();
    alice.expect::<play::Respawn>();
    assert!(!is_dead(&server, id));
    assert_eq!(state.get("health").as_f64(), Some(20.0), "the state the host gets: {state:?}");
    // A living player is not respawned.
    alice.drain(QUIET);
    alice.clear_backlog();
    plugin_call(&server, "player.respawn", bmap! { "uuid" => uuid });
    assert!(alice.try_expect::<play::Respawn>(QUIET).is_none(), "alive: no respawn");

    // A world a plugin made is a dimension of its own for the client, so going there from
    // the overworld is a change of world (not the same one with the old chunks kept).
    let sky = server.handle.run(|s| s.create_world("skyland", "NORMAL", Some(1), "FLAT", false, None).unwrap()).unwrap();
    alice.drain(QUIET);
    alice.clear_backlog();
    server.handle.run(move |s| s.change_dimension(id, sky, paper_server::server::player::Location { x: 0.5, y: -60.0, z: 0.5, yaw: 0.0, pitch: 0.0 })).unwrap();
    let respawn: play::Respawn = alice.expect();
    assert_eq!(respawn.spawn.dimension_name, "minecraft:skyland");
    server.stop();
}

/// A point a plugin set with force holds without a bed where a player can stand: on carpet
/// too (not solid), facing the way the plugin's location did.
#[test]
fn forced_respawn_points_hold_on_carpet() {
    let server = survival_server(None);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let uuid = server.handle.run(move |s| s.players[&id].uuid().to_string()).unwrap();
    let carpet = blocks::default_state("minecraft:white_carpet").unwrap();
    server.handle.run(move |s| s.set_block_with_flags(0, paper_protocol::codec::BlockPos::new(20, -60, 20), carpet, paper_server::world::block_updates::flags::ALL)).unwrap();
    let point = Value::List(vec!["world".into(), 20i64.into(), (-60i64).into(), 20i64.into(), 90.0f64.into(), 30.0f64.into()]);
    plugin_call(&server, "player.set", bmap! { "uuid" => uuid.clone(), "respawn" => point, "respawn_forced" => true });
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    press_respawn(&mut alice);
    let at = server.handle.run(move |s| s.players[&id].loc).unwrap();
    assert!((at.x, at.z, at.yaw, at.pitch) == (20.5, 20.5, 90.0, 30.0) && (at.y - -59.9).abs() < 1e-6, "on the carpet, facing east and down: {at:?}");

    // A pressure plate is solid for fluids, yet a point: vanilla lets players respawn on it.
    let plate = blocks::default_state("minecraft:stone_pressure_plate").unwrap();
    server.handle.run(move |s| s.set_block_with_flags(0, paper_protocol::codec::BlockPos::new(24, -60, 24), plate, paper_server::world::block_updates::flags::ALL)).unwrap();
    let point = Value::List(vec!["world".into(), 24i64.into(), (-60i64).into(), 24i64.into(), 0.0f64.into(), 0.0f64.into()]);
    plugin_call(&server, "player.set", bmap! { "uuid" => uuid.clone(), "respawn" => point, "respawn_forced" => true });
    // The plugin API tells where the player would respawn, as the server works it out.
    let args = bmap! { "uuid" => uuid };
    let peek = server.handle.run(move |s| paper_server::plugins::api::handle(s, "player.respawn_location", &args).map_err(|e| e.to_string())).unwrap().unwrap();
    assert_eq!((peek.at(1).as_f64(), peek.at(3).as_f64()), (Some(24.5), Some(24.5)), "{peek:?}");
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    press_respawn(&mut alice);
    let at = server.handle.run(move |s| s.players[&id].loc).unwrap();
    assert!((at.x, at.z) == (24.5, 24.5), "on the pressure plate: {at:?}");
    server.stop();
}

/// A bed puts a player beside it, on the side the player did not look towards when the point
/// was set, facing the bed (`BedBlock.findStandUpPosition`, `RespawnPosAngle.of`). The plugin
/// API tells the same, for a player who has left too: from the point the plugin's Player
/// object still has.
#[test]
fn beds_put_players_beside_them_facing_them() {
    let server = survival_server(None);
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let uuid = server.handle.run(move |s| s.players[&id].uuid().to_string()).unwrap();
    let head = blocks::parse_state("minecraft:red_bed[part=head,facing=north]").unwrap();
    let foot = blocks::parse_state("minecraft:red_bed[part=foot,facing=north]").unwrap();
    plugin_call(&server, "world.set_block", bmap! { "x" => 10, "y" => -60, "z" => 10, "state" => head as i64, "physics" => false });
    plugin_call(&server, "world.set_block", bmap! { "x" => 10, "y" => -60, "z" => 11, "state" => foot as i64, "physics" => false });
    let point = |world: &str, x: i64, z: i64, yaw: f64, pitch: f64| Value::List(vec![world.into(), x.into(), (-60i64).into(), z.into(), yaw.into(), pitch.into()]);
    let peek = |point: Value, forced: bool| {
        let args = bmap! { "uuid" => uuid.clone(), "respawn" => point, "respawn_forced" => forced };
        let at = server.handle.run(move |s| paper_server::plugins::api::handle(s, "player.respawn_location", &args).map_err(|e| e.to_string())).unwrap().unwrap();
        (1..6).map(|i| at.at(i).as_f64()).collect::<Option<Vec<f64>>>()
    };

    // Set looking south (yaw 0): beside the head to the east, looking west at it.
    plugin_call(&server, "player.set", bmap! { "uuid" => uuid.clone(), "respawn" => point("world", 10, 10, 0.0, 0.0) });
    assert_eq!(peek(Value::Null, false), Some(vec![11.5, -60.0, 10.5, 90.0, 0.0]));
    server.handle.console_command("kill Alice");
    alice.expect::<play::PlayerCombatKill>();
    alice.drain(QUIET);
    press_respawn(&mut alice);
    let at = server.handle.run(move |s| s.players[&id].loc).unwrap();
    assert_eq!((at.x, at.y, at.z, at.yaw, at.pitch), (11.5, -60.0, 10.5, 90.0, 0.0));
    // Set looking east: beside it to the west, looking east at it.
    plugin_call(&server, "player.set", bmap! { "uuid" => uuid.clone(), "respawn" => point("world", 10, 10, -90.0, 0.0) });
    assert_eq!(peek(Value::Null, false), Some(vec![9.5, -60.0, 10.5, -90.0, 0.0]));
    // Both sides taken: at the foot's side.
    let stone = blocks::default_state("minecraft:stone").unwrap();
    for x in [9, 11] {
        plugin_call(&server, "world.set_block", bmap! { "x" => x, "y" => -60, "z" => 10, "state" => stone as i64, "physics" => false });
    }
    assert_eq!(peek(Value::Null, false), Some(vec![9.5, -60.0, 11.5, -135.0, 0.0]));

    // Alice leaves: the point her Player object still has is asked about instead.
    drop(alice);
    wait_until(&server, "Alice gone", |s| s.players.is_empty());
    assert_eq!(peek(point("world", 10, 10, 0.0, 0.0), false), Some(vec![11.5, -60.0, 11.5, 135.0, 0.0]));
    assert_eq!(peek(point("world", 20, 20, 30.0, 15.0), true), Some(vec![20.5, -59.9, 20.5, 30.0, 15.0]), "forced, in the air");
    assert_eq!(peek(point("world", 20, 20, 30.0, 15.0), false), None, "not forced, no bed");
    assert_eq!(peek(point("nowhere", 10, 10, 0.0, 0.0), false), None, "a world that is gone");
    server.stop();
}
