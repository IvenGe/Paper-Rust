//! Ghasts and blazes shoot fireballs; a ghast's fireball explodes, and a player can strike it
//! back.

mod common;

use common::*;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use paper_server::config::GameMode;
use paper_server::server::mobs::SpawnReason;
use paper_server::world::entity::{EntityKind, Vec3};
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(300);

/// Waits while reading what the server sends, so keep-alives are answered.
fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(50));
        client.clear_backlog();
    }
}

fn join(server: &paper_server::RunningServer) -> (Client, i32) {
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    alice.clear_backlog();
    let id = alice.entity_id;
    (alice, id)
}

fn spawn(server: &paper_server::RunningServer, name: &'static str, at: Vec3) -> i32 {
    server.handle.run(move |s| s.spawn_mob(0, name, at, SpawnReason::Command).unwrap()).unwrap()
}

/// The fireballs of a kind in the world, with their owners.
fn fireballs(server: &paper_server::RunningServer, kind: &'static str) -> Vec<(i32, Option<i32>)> {
    server
        .handle
        .run(move |s| {
            s.worlds[0]
                .entities
                .values()
                .filter(|e| e.info.name == kind && !e.removed)
                .map(|e| (e.id, match &e.kind {
                    EntityKind::Thrown { owner, .. } => *owner,
                    _ => None,
                }))
                .collect()
        })
        .unwrap()
}

#[test]
fn ghasts_shoot_fireballs_at_players() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let (mut alice, _) = join(&server);
    // A ghast hovering a little above Alice's height, twenty blocks off.
    let ghast = spawn(&server, "minecraft:ghast", Vec3::new(8.5, -58.0, 28.5));
    wait_reading(&mut alice, "the ghast's fireball", || fireballs(&server, "minecraft:fireball").iter().any(|(_, owner)| *owner == Some(ghast)));
    server.stop();
}

#[test]
fn players_strike_ghast_fireballs_back() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let (mut alice, id) = join(&server);
    let ghast = spawn(&server, "minecraft:ghast", Vec3::new(8.5, -58.0, 40.5));
    // A ghast's fireball three blocks south of Alice, coming her way (north).
    let shot = server
        .handle
        .run(move |s| s.spawn_thrown(0, "minecraft:fireball", Vec3::new(8.5, -58.5, 11.5), Vec3::new(0.0, 0.0, -1.0), 0.1, 0.0, 0.0, paper_protocol::types::Slot::EMPTY, Some(ghast)).unwrap())
        .unwrap();
    // Alice looks south (yaw 0) and hits it: it flies the way she looks, and is hers now.
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().loc.yaw = 0.0).unwrap();
    alice.send(&play::Attack { entity_id: VarInt(shot) });
    let state = move |s: &mut paper_server::server::Server| s.worlds[0].entities.get(&shot).map(|e| (e.vel, matches!(e.kind, EntityKind::Thrown { owner: Some(o), .. } if o == id)));
    wait_reading(&mut alice, "the fireball struck back", || server.handle.run(state).unwrap().is_some_and(|(_, hers)| hers));
    let (vel, _) = server.handle.run(state).unwrap().unwrap();
    assert!(vel.z > 0.0, "back to the south: {vel:?}");
    server.stop();
}

#[test]
fn ghast_fireballs_explode_where_they_land() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let (mut alice, _) = join(&server);
    // A fireball going straight down onto the grass beside Alice.
    server
        .handle
        .run(|s| {
            s.spawn_thrown(0, "minecraft:fireball", Vec3::new(14.5, -55.0, 8.5), Vec3::new(0.0, -1.0, 0.0), 0.5, 0.0, 0.0, paper_protocol::types::Slot::EMPTY, None);
        })
        .unwrap();
    let crater = |s: &mut paper_server::server::Server| s.worlds[0].block_at(14, -61, 8).is_some_and(paper_data::blocks::is_air);
    wait_reading(&mut alice, "the explosion", || server.handle.run(crater).unwrap());
    assert!(fireballs(&server, "minecraft:fireball").is_empty(), "the fireball is gone");
    server.stop();
}

#[test]
fn blazes_shoot_bursts_of_three_fireballs() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let (mut alice, _) = join(&server);
    // Sixteen blocks off, so each fireball is in flight for a while.
    let blaze = spawn(&server, "minecraft:blaze", Vec3::new(8.5, -60.0, 24.5));
    // The burst is over within a second: look every tick or two.
    let mut seen = std::collections::HashSet::new();
    let deadline = Instant::now() + TIMEOUT;
    while seen.len() < 3 {
        assert!(Instant::now() < deadline, "timed out waiting for a burst of three ({} seen)", seen.len());
        for (fireball, owner) in fireballs(&server, "minecraft:small_fireball") {
            if owner == Some(blaze) {
                seen.insert(fireball);
            }
        }
        alice.drain(Duration::from_millis(10));
        alice.clear_backlog();
    }
    server.stop();
}
