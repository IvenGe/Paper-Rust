//! The ender dragon fight: the dragon appears above a new End's island with its boss bar,
//! flies, takes damage only from players and explosions (a quarter of it away from the
//! head), and once killed flies to the portal, dies, and opens the exit portal, the egg and a
//! gateway.

mod common;

use common::*;
use paper_data::blocks;
use paper_protocol::packets::play;
use paper_server::config::GameMode;
use paper_server::server::player::Location;
use paper_server::server::survival::Damage;
use paper_server::world::chunk::ChunkPos;
use paper_server::world::entity::Vec3;
use std::time::{Duration, Instant};

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_dragon_fight_from_spawn_to_death() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Creative);
    let mut alice = Client::join(server.port(), "Alice");
    let id = alice.entity_id;
    alice.expect::<play::PlayerPosition>();
    let end = server.handle.run(|s| s.world_of_dimension("minecraft:the_end")).unwrap().expect("an End");
    // Into the End, onto the island beside the portal.
    server.handle.run(move |s| (-1..=1).for_each(|cx| (-1..=1).for_each(|cz| s.worlds[end].add_ticket(ChunkPos::new(cx, cz))))).unwrap();
    wait_until("the island", Duration::from_secs(60), || server.handle.run(move |s| (-1..=1).all(|cx| (-1..=1).all(|cz| s.worlds[end].is_ready(ChunkPos::new(cx, cz))))).unwrap());
    let ground = server.handle.run(move |s| s.worlds[end].top_y(8, 8).unwrap()).unwrap();
    server.handle.run(move |s| s.change_dimension(id, end, Location { x: 8.5, y: ground as f64, z: 8.5, yaw: 0.0, pitch: 0.0 })).unwrap();

    // The dragon appears with its boss bar, and the exit portal is closed.
    let bar: play::BossEvent = alice.find(Duration::from_secs(60), |b: &play::BossEvent| matches!(b.action, play::BossEventAction::Add { .. })).expect("the dragon's bar");
    let play::BossEventAction::Add { flags, color, .. } = bar.action else { unreachable!() };
    assert_eq!((flags, color), (6, 0), "boss music and fog, pink");
    let find = |server: &paper_server::RunningServer| server.handle.run(move |s| s.worlds[end].entities.values().find(|e| e.info.name == "minecraft:ender_dragon").map(|e| (e.id, e.pos))).unwrap();
    wait_until("the dragon", Duration::from_secs(60), || find(&server).is_some());
    let dragon = find(&server).unwrap();
    assert!(dragon.1.y > 100.0, "high above the island: {:?}", dragon.1);
    let portal_blocks = |server: &paper_server::RunningServer| {
        server.handle.run(move |s| (-3..=3).map(|x| (-3..=3).filter(|z| (40..90).any(|y| s.worlds[end].block_at(x, y, *z).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:end_portal"))).count()).sum::<usize>()).unwrap()
    };
    assert_eq!(portal_blocks(&server), 0);
    // It flies.
    wait_until("the dragon to fly", TIMEOUT, || server.handle.run(move |s| s.worlds[end].entities.get(&dragon.0).is_some_and(|e| e.pos.distance_squared(dragon.1) > 4.0)).unwrap());

    // Only players and explosions hurt it; away from the head a quarter (plus one).
    let health = |server: &paper_server::RunningServer| server.handle.run(move |s| s.worlds[end].entities.get(&dragon.0).and_then(|e| e.mob()).map(|m| m.living.health)).unwrap();
    assert_eq!(health(&server), Some(200.0));
    server.handle.run(move |s| s.hurt_entity(end, dragon.0, Damage::new("minecraft:magic"), 20.0)).unwrap();
    assert_eq!(health(&server), Some(200.0), "magic does nothing");
    server.handle.run(move |s| s.hurt_entity(end, dragon.0, Damage::by("minecraft:player_attack", id, Vec3::new(8.5, 70.0, 8.5)), 20.0)).unwrap();
    assert_eq!(health(&server), Some(194.0), "20 on the body: 20 / 4 + 1");

    // A deadly blow sends it to the portal to die; it rises for ten seconds and the fight is
    // won: the portal opens, the egg and a gateway appear, and the bar goes.
    server
        .handle
        .run(move |s| {
            if let Some(m) = s.worlds[end].entities.get_mut(&dragon.0).and_then(|e| e.mob_mut()) {
                m.living.invulnerable_time = 0;
            }
            s.hurt_entity(end, dragon.0, Damage::by("minecraft:player_attack", id, Vec3::new(8.5, 70.0, 8.5)), 1000.0)
        })
        .unwrap();
    assert_eq!(health(&server), Some(1.0), "it lives on until it reaches the portal");
    wait_until("the dragon to die", Duration::from_secs(60), || health(&server).is_none());
    assert_eq!(portal_blocks(&server), 20, "the exit portal is open");
    let egg = server.handle.run(move |s| (60..90).any(|y| s.worlds[end].block_at(0, y, 0).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:dragon_egg"))).unwrap();
    assert!(egg, "the dragon egg");
    let gateways = |server: &paper_server::RunningServer| {
        server
            .handle
            .run(move |s| {
                (0..20)
                    .filter(|i| {
                        let p = paper_server::world::dragon_fight::gateway_position(*i);
                        s.worlds[end].block_at(p.x, p.y, p.z).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:end_gateway")
                    })
                    .count()
            })
            .unwrap()
    };
    wait_until("a gateway", Duration::from_secs(60), || gateways(&server) == 1);
    alice.find(TIMEOUT, |b: &play::BossEvent| b.uuid == bar.uuid && b.action == play::BossEventAction::Remove).expect("the bar goes");
    let orbs = server.handle.run(move |s| s.worlds[end].entities.values().filter(|e| e.info.name == "minecraft:experience_orb").count()).unwrap();
    assert!(orbs > 0, "experience");
    // The gateway leads out to the islands beyond (found and loaded when first used), where
    // a gateway leads back.
    let gateway = server
        .handle
        .run(move |s| {
            (0..20).map(paper_server::world::dragon_fight::gateway_position).find(|p| s.worlds[end].block_at(p.x, p.y, p.z).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:end_gateway"))
        })
        .unwrap()
        .unwrap();
    server.handle.run(move |s| s.teleport(id, Location { x: gateway.x as f64 + 0.5, y: gateway.y as f64, z: gateway.z as f64 + 0.5, yaw: 0.0, pitch: 0.0 })).unwrap();
    wait_until("Alice out on the islands", Duration::from_secs(120), || server.handle.run(move |s| s.players[&id].loc.x.hypot(s.players[&id].loc.z) > 500.0).unwrap());
    let back = server
        .handle
        .run(move |s| {
            let exit = s.block_entity_data(end, gateway).and_then(|n| n.get("exit_portal").and_then(|t| t.as_int_array().map(|v| v.to_vec())))?;
            let block = s.worlds[end].block_at(exit[0], exit[1], exit[2])?;
            Some(blocks::block_of_state(block).name)
        })
        .unwrap();
    assert_eq!(back, Some("minecraft:end_gateway"), "a gateway back");

    // The fight is remembered.
    let saved = server.handle.run(move |s| s.worlds[end].level_data(1, false).dragon_fight).unwrap().expect("saved fight");
    assert_eq!((saved.bool("DragonKilled"), saved.bool("PreviouslyKilled")), (Some(true), Some(true)));
    server.stop();
}
