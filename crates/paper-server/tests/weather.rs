//! Weather: `/weather` starts and stops rain and thunder for the players of a world, and
//! lightning burns and hurts what it strikes.

mod common;

use common::*;
use paper_protocol::packets::play;
use std::time::Duration;

fn join(server: &paper_server::RunningServer) -> Client {
    let mut client = Client::join(server.port(), "Alice");
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(Duration::from_millis(300));
    client.clear_backlog();
    client
}

#[test]
fn weather_command_starts_and_stops_rain_and_thunder() {
    let server = start_server(|_| {});
    let mut alice = join(&server);
    server.handle.console_command("weather thunder 1d");
    alice.find(TIMEOUT, |g: &play::GameEvent| g.event == play::game_event::START_RAINING).expect("rain starts");
    let thunder: play::GameEvent = alice.find(TIMEOUT, |g: &play::GameEvent| g.event == play::game_event::THUNDER_LEVEL_CHANGE && g.value > 0.5).expect("thunder builds up");
    assert!(thunder.value <= 1.0);
    let weather = server.handle.run(|s| s.worlds[0].weather.clone()).unwrap();
    assert!(weather.raining && weather.thundering && weather.rain_time > 23000, "{weather:?}");
    // A full storm darkens the noon sky enough to sleep.
    let noon_storm = server
        .handle
        .run(|s| {
            let mut w = s.worlds[0].weather.clone();
            (w.rain_level, w.thunder_level) = (1.0, 1.0);
            let saved = std::mem::replace(&mut s.worlds[0].weather, w);
            let time = std::mem::replace(&mut s.worlds[0].day_time, 6000);
            let darken = paper_server::server::mobs::sky_darken(&s.worlds[0]);
            s.worlds[0].weather = saved;
            s.worlds[0].day_time = time;
            darken
        })
        .unwrap();
    assert!(noon_storm >= 4, "sky darken {noon_storm}");

    // A player joining now gets the rain right away.
    let mut bob = Client::join(server.port(), "Bob");
    bob.find(TIMEOUT, |g: &play::GameEvent| g.event == play::game_event::START_RAINING).expect("rain on join");

    server.handle.console_command("weather clear");
    alice.find(Duration::from_secs(10), |g: &play::GameEvent| g.event == play::game_event::STOP_RAINING).expect("rain stops");
    let weather = server.handle.run(|s| s.worlds[0].weather.clone()).unwrap();
    assert!(!weather.raining && weather.clear_time > 0, "{weather:?}");
    server.stop();
}

#[test]
fn lightning_burns_and_hurts() {
    let server = start_server(|c| c.properties.difficulty = paper_server::config::Difficulty::Normal);
    let mut alice = join(&server);
    let zombie = server
        .handle
        .run(|s| s.spawn_mob(0, "minecraft:zombie", paper_server::world::entity::Vec3::new(12.5, -60.0, 8.5), paper_server::server::mobs::SpawnReason::Command))
        .unwrap()
        .expect("a zombie");
    server.handle.run(|s| s.strike_lightning(0, paper_protocol::codec::BlockPos::new(12, -60, 8))).unwrap();
    let bolt_type = paper_data::entity_types::by_name("minecraft:lightning_bolt").unwrap().id;
    alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == bolt_type).expect("the bolt is seen");
    let (health, burning) = server
        .handle
        .run(move |s| {
            let m = s.worlds[0].entities[&zombie].mob().unwrap();
            (m.living.health, m.living.fire_ticks > 0)
        })
        .unwrap();
    // Five damage, less the zombie's natural armour.
    assert!(health < 16.0 && burning, "health {health}, burning {burning}");
    let fire = server.handle.run(|s| s.worlds[0].block_at(12, -60, 8).map(|b| paper_data::blocks::block_of_state(b).name)).unwrap();
    assert_eq!(fire, Some("minecraft:fire"), "the bolt set the ground on fire");
    server.stop();
}
