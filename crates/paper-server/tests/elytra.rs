//! Elytra and firework rockets: a player in the air glides when wearing an elytra (and is
//! told not to without one), the elytra wears while gliding, a rocket used while gliding is
//! attached to the player, rockets used on a block fly up, and gliding stops on landing.

mod common;

use common::*;
use paper_data::items;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::metadata::flags;
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::GameMode;
use paper_server::world::entity::EntityKind;
use std::time::{Duration, Instant};

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

/// Waits while reading what the server sends, so keep-alives are answered.
fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(20));
        client.clear_backlog();
    }
}

fn gliding(server: &paper_server::RunningServer, id: i32) -> bool {
    server.handle.run(move |s| s.players[&id].gliding).unwrap()
}

/// The shared flags of the player's own entity as their client last heard them.
fn flags_heard(client: &mut Client, id: i32) -> i8 {
    let data: play::SetEntityData = client.find(TIMEOUT, |d: &play::SetEntityData| d.entity_id.0 == id).expect("the player's data");
    // Index 0, type byte (0), then the flags.
    assert_eq!(&data.metadata.0[..2], &[0, 0]);
    data.metadata.0[2] as i8
}

#[test]
fn elytra_glide_rockets_boost_and_landing_stops_it() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();
    let id = alice.entity_id;
    let start = |client: &mut Client| client.send(&play::PlayerCommand { entity_id: VarInt(id), action: VarInt(play::player_command::START_ELYTRA_FLYING), jump_boost: VarInt(0) });

    // In the air without an elytra: the client is told it does not glide.
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().on_ground = false).unwrap();
    start(&mut alice);
    assert_eq!(flags_heard(&mut alice, id) & flags::FALL_FLYING, 0, "no elytra, no gliding");
    assert!(!gliding(&server, id));

    // With one it glides, and those who see it learn so.
    server
        .handle
        .run(move |s| {
            s.set_inventory_slot(id, 6, Slot::of(item("minecraft:elytra"), 1));
            s.set_inventory_slot(id, 36, Slot::of(item("minecraft:firework_rocket"), 3));
            s.players.get_mut(&id).unwrap().on_ground = false;
        })
        .unwrap();
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();
    start(&mut alice);
    assert_ne!(flags_heard(&mut alice, id) & flags::FALL_FLYING, 0, "gliding");
    assert!(gliding(&server, id));

    // A rocket used now is attached to her, and one is used up.
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(1), yaw: 0.0, pitch: 0.0 });
    let find = |server: &paper_server::RunningServer| {
        server
            .handle
            .run(move |s| s.worlds[0].entities.values().find_map(|e| match &e.kind {
                EntityKind::Firework(f) if !e.removed => Some((e.id, f.attached, f.owner)),
                _ => None,
            }))
            .unwrap()
    };
    wait_reading(&mut alice, "a rocket", || find(&server).is_some());
    let (rocket, attached, owner) = find(&server).unwrap();
    assert_eq!((attached, owner), (Some(id), Some(id)));
    let left = server.handle.run(move |s| s.players[&id].inventory[36].0.as_ref().map(|i| i.count)).unwrap();
    assert_eq!(left, Some(2), "a rocket is used up");
    // It burns out after its flight; the elytra wears a point a second meanwhile.
    wait_reading(&mut alice, "the rocket gone", || server.handle.run(move |s| s.worlds[0].entities.get(&rocket).is_none_or(|e| e.removed)).unwrap());
    let worn = |server: &paper_server::RunningServer| server.handle.run(move |s| paper_server::server::survival::item_damage(s.players[&id].inventory[6].0.as_ref().unwrap())).unwrap();
    wait_reading(&mut alice, "the elytra worn", || worn(&server) >= 1);
    assert!(gliding(&server, id), "still gliding");

    // Landing ends it.
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().on_ground = true).unwrap();
    wait_reading(&mut alice, "gliding to stop", || !gliding(&server, id));

    // A rocket used on the ground flies up from there.
    alice.send(&play::UseItemOn {
        hand: VarInt(0),
        pos: BlockPos::new(8, -61, 10),
        face: VarInt(1),
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        world_border_hit: false,
        sequence: VarInt(2),
    });
    wait_reading(&mut alice, "a rocket rising", || {
        server
            .handle
            .run(|s| s.worlds[0].entities.values().any(|e| !e.removed && e.pos.y > -59.0 && matches!(&e.kind, EntityKind::Firework(f) if f.attached.is_none())))
            .unwrap()
    });
    let left = server.handle.run(move |s| s.players[&id].inventory[36].0.as_ref().map(|i| i.count)).unwrap();
    assert_eq!(left, Some(1));
    server.stop();
}
