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

    // An end crystal on each side of the exit portal calls the dragon back: the portal
    // closes, the crystals beam at the sky and then at each pillar, which is torn down and
    // built again with a new crystal, and a new dragon comes as the four crystals blow up.
    let portal = server.handle.run(move |s| s.worlds[end].dragon_fight.as_ref().and_then(|f| f.exit_portal)).unwrap().expect("the exit portal");
    let crystal = paper_data::items::id("minecraft:end_crystal").unwrap() as i32;
    // (The dragon's experience would be picked up; orbs in the way stop a crystal.)
    server.handle.run(move |s| s.worlds[end].entities.values_mut().filter(|e| e.info.name == "minecraft:experience_orb").for_each(|e| e.removed = true)).unwrap();
    let crystals_near = move |server: &paper_server::RunningServer| {
        server
            .handle
            .run(move |s| s.worlds[end].entities.values().filter(|e| e.info.name == "minecraft:end_crystal" && !e.removed && (e.pos.x - portal.x as f64).abs() < 5.0 && (e.pos.z - portal.z as f64).abs() < 5.0).map(|e| e.id).collect::<Vec<_>>())
            .unwrap()
    };
    for (i, (dx, dz)) in [(0, -3), (3, 0), (0, 3), (-3, 0)].into_iter().enumerate() {
        let target = portal.offset(dx, 0, dz);
        server
            .handle
            .run(move |s| {
                let loc = Location { x: target.x as f64 + 0.5 + dx as f64 * 0.7, y: portal.y as f64 + 1.0, z: target.z as f64 + 0.5 + dz as f64 * 0.7, yaw: 0.0, pitch: 0.0 };
                s.teleport(id, loc);
                s.set_inventory_slot(id, 36, paper_protocol::types::Slot::of(crystal, 1));
            })
            .unwrap();
        alice.send(&play::UseItemOn {
            hand: paper_protocol::codec::VarInt(0),
            pos: target,
            face: paper_protocol::codec::VarInt(1),
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: paper_protocol::codec::VarInt(100 + i as i32),
        });
        let placed = Instant::now();
        while crystals_near(&server).len() != i + 1 {
            if placed.elapsed() > TIMEOUT {
                let around = server
                    .handle
                    .run(move |s| {
                        let near: Vec<(String, Vec3)> = s.worlds[end].entities.values().filter(|e| e.pos.distance_squared(Vec3::new(target.x as f64, target.y as f64, target.z as f64)) < 16.0).map(|e| (e.info.name.clone(), e.pos)).collect();
                        let p = &s.players[&id];
                        (near, (p.loc.x, p.loc.y, p.loc.z), s.worlds[end].block_at(target.x, target.y, target.z).map(|b| blocks::block_of_state(b).name), s.worlds[end].block_at(target.x, target.y + 1, target.z).map(|b| blocks::block_of_state(b).name))
                    })
                    .unwrap();
                panic!("no crystal at {target:?}: {around:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let respawning = |server: &paper_server::RunningServer| server.handle.run(move |s| s.worlds[end].dragon_fight.as_ref().is_some_and(|f| f.respawn.is_some())).unwrap();
    wait_until("the respawn to start", TIMEOUT, || respawning(&server));
    assert_eq!(portal_blocks(&server), 0, "the portal closes");
    let beams = move |server: &paper_server::RunningServer| {
        server
            .handle
            .run(move |s| {
                s.worlds[end]
                    .entities
                    .values()
                    .filter_map(|e| match &e.kind {
                        paper_server::world::entity::EntityKind::Crystal { beam_target, show_bottom } if (e.pos.x - portal.x as f64).abs() < 5.0 && (e.pos.z - portal.z as f64).abs() < 5.0 => Some((*beam_target, *show_bottom)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    wait_until("the beams", TIMEOUT, || beams(&server).iter().all(|(b, _)| b.is_some()));
    let now = beams(&server);
    assert_eq!(now.len(), 4);
    assert!(now.iter().all(|(b, bottom)| *b == Some(paper_protocol::codec::BlockPos::new(0, 128, 0)) && !bottom), "beaming at the sky, placed without their base: {now:?}");
    let started = Instant::now();
    let mut last = Instant::now();
    while !find(&server).is_some_and(|d| d.0 != dragon.0) {
        if last.elapsed() > Duration::from_secs(10) {
            last = Instant::now();
            let state = server.handle.run(move |s| (s.tick, s.worlds[end].dragon_fight.as_ref().map(|f| format!("{:?}", f.respawn.as_ref().map(|r| (r.stage, r.time)))))).unwrap();
            eprintln!("{:?}: {state:?}", started.elapsed());
        }
        assert!(started.elapsed() < Duration::from_secs(180), "timed out waiting for the new dragon");
        // Alice answers keep-alives meanwhile: without anyone near, the respawn waits.
        alice.drain(Duration::from_millis(100));
    }
    assert!(!respawning(&server));
    assert!(crystals_near(&server).is_empty(), "the four crystals blew up");
    let pillars = server
        .handle
        .run(move |s| s.worlds[end].entities.values().filter(|e| matches!(e.kind, paper_server::world::entity::EntityKind::Crystal { .. }) && !e.removed && e.pos.y > 70.0).map(|e| e.flags.invulnerable).collect::<Vec<_>>())
        .unwrap();
    assert_eq!(pillars.len(), 10, "a crystal on every pillar");
    assert!(pillars.iter().all(|i| !i), "breakable again");
    let killed = server.handle.run(move |s| s.worlds[end].dragon_fight.as_ref().map(|f| f.dragon_killed)).unwrap();
    assert_eq!(killed, Some(false));
    server.stop();
}
