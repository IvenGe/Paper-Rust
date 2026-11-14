//! Signs: the editor opens when a sign is placed, the text written is kept and shown to
//! others, dyes and glow ink sacs work on it, and waxed signs cannot be edited.

mod common;

use common::*;
use paper_data::items;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_server::config::GameMode;
use std::time::{Duration, Instant};

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

/// Waits while reading what the server sends, so keep-alives are answered.
fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(20));
        client.clear_backlog();
    }
}

fn use_on(pos: BlockPos, face: i32, sequence: i32) -> play::UseItemOn {
    play::UseItemOn { hand: VarInt(0), pos, face: VarInt(face), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(sequence) }
}

fn side(server: &paper_server::RunningServer, pos: BlockPos, key: &'static str) -> Option<paper_nbt::Compound> {
    server.handle.run(move |s| s.block_entity_data(0, pos).and_then(|d| d.compound(key).cloned())).unwrap()
}

#[test]
fn signs_are_written_on_dyed_made_to_glow_and_waxed() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    let mut bob = Client::join(server.port(), "Bob");
    bob.expect::<play::PlayerPosition>();
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();
    let id = alice.entity_id;
    // Alice puts a sign on the grass in front of her: its editor opens on the front.
    server.handle.run(move |s| s.set_inventory_slot(id, 36, paper_protocol::types::Slot::of(item("minecraft:oak_sign"), 1))).unwrap();
    let ground = BlockPos::new(8, -61, 10);
    let sign = BlockPos::new(8, -60, 10);
    alice.send(&use_on(ground, 1, 1));
    let editor: play::OpenSignEditor = alice.expect();
    assert_eq!((editor.pos, editor.side.0), (sign, 1));

    // What she writes is kept, and Bob sees it.
    bob.clear_backlog();
    alice.send(&play::SignUpdate { pos: sign, lines: ["Hello".into(), "\u{a7}cworld".into(), String::new(), String::new()], front: true });
    wait_reading(&mut alice, "the text", || side(&server, sign, "front_text").is_some_and(|t| format!("{t:?}").contains("Hello")));
    let front = side(&server, sign, "front_text").unwrap();
    let lines = format!("{:?}", front.get("messages").unwrap());
    assert!(lines.contains("\"world\"") && !lines.contains("\u{a7}"), "formatting codes are taken out: {lines}");
    let shown: play::BlockEntityData = bob.find(TIMEOUT, |b: &play::BlockEntityData| b.pos == sign).expect("Bob sees the sign");
    assert!(format!("{:?}", shown.data).contains("Hello"));

    // A red dye colours the front; a glow ink sac makes it glow.
    server.handle.run(move |s| s.set_inventory_slot(id, 36, paper_protocol::types::Slot::of(item("minecraft:red_dye"), 1))).unwrap();
    alice.send(&use_on(sign, 1, 2));
    wait_reading(&mut alice, "red text", || side(&server, sign, "front_text").is_some_and(|t| t.string("color") == Some("red")));
    server.handle.run(move |s| s.set_inventory_slot(id, 36, paper_protocol::types::Slot::of(item("minecraft:glow_ink_sac"), 1))).unwrap();
    alice.send(&use_on(sign, 1, 3));
    wait_reading(&mut alice, "glowing text", || side(&server, sign, "front_text").is_some_and(|t| t.bool("has_glowing_text") == Some(true)));
    let used_up = server.handle.run(move |s| s.players[&id].inventory[36].is_empty()).unwrap();
    assert!(used_up, "the glow ink sac is used");

    // Honeycomb waxes it: clicking it then opens no editor.
    server.handle.run(move |s| s.set_inventory_slot(id, 36, paper_protocol::types::Slot::of(item("minecraft:honeycomb"), 1))).unwrap();
    alice.send(&use_on(sign, 1, 4));
    wait_reading(&mut alice, "the sign waxed", || server.handle.run(move |s| s.block_entity_data(0, sign).and_then(|d| d.bool("is_waxed"))).unwrap() == Some(true));
    alice.clear_backlog();
    alice.send(&use_on(sign, 1, 5));
    assert!(alice.try_expect::<play::OpenSignEditor>(Duration::from_millis(500)).is_none(), "a waxed sign cannot be edited");
    server.stop();
}
