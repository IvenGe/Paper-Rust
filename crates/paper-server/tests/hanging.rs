//! Item frames and paintings: hung on block faces, frames take an item that turns and is
//! knocked out, paintings take the largest variant that fits, and both break when hit or
//! left without the block behind them, dropping themselves.

mod common;

use common::*;
use paper_data::items;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::{LpVec3, Slot};
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

fn use_on(pos: BlockPos, face: i32, sequence: i32) -> play::UseItemOn {
    play::UseItemOn { hand: VarInt(0), pos, face: VarInt(face), cursor_x: 0.5, cursor_y: 0.5, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(sequence) }
}

#[derive(Debug, Clone, PartialEq)]
struct Hung {
    id: i32,
    kind: String,
    block: BlockPos,
    facing: u8,
    item: Option<i32>,
    rotation: u8,
    variant: String,
}

/// The item frames and paintings still in the overworld.
fn hung(server: &paper_server::RunningServer) -> Vec<Hung> {
    server
        .handle
        .run(|s| {
            let mut out: Vec<Hung> = s.worlds[0]
                .entities
                .values()
                .filter(|e| !e.removed)
                .filter_map(|e| match &e.kind {
                    EntityKind::Hanging(h) => Some(Hung { id: e.id, kind: e.info.name.clone(), block: h.block, facing: h.facing, item: h.item.0.as_ref().map(|i| i.item), rotation: h.rotation, variant: h.variant.clone() }),
                    _ => None,
                })
                .collect();
            out.sort_by_key(|h| h.id);
            out
        })
        .unwrap()
}

/// Whether an item lies in the world, or Alice picked it up.
fn dropped(server: &paper_server::RunningServer, player: i32, wanted: i32) -> bool {
    server
        .handle
        .run(move |s| {
            let lying = s.worlds[0].entities.values().any(|e| !e.removed && matches!(&e.kind, EntityKind::Item { stack, .. } if stack.0.as_ref().is_some_and(|i| i.item == wanted)));
            lying || s.players[&player].inventory.iter().any(|slot| slot.0.as_ref().is_some_and(|i| i.item == wanted))
        })
        .unwrap()
}

#[test]
fn item_frames_take_turn_and_give_back_their_item() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();
    let id = alice.entity_id;
    let wall = BlockPos::new(8, -60, 11);
    let stone = paper_data::blocks::default_state("minecraft:stone").unwrap();
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, wall, stone);
            s.set_inventory_slot(id, 36, Slot::of(item("minecraft:item_frame"), 1));
        })
        .unwrap();

    // Hung on the north side of the stone: it takes up the block in front, facing north, and
    // clients get it at that block.
    alice.send(&use_on(wall, 2, 1));
    wait_reading(&mut alice, "a frame", || !hung(&server).is_empty());
    let frame = hung(&server)[0].clone();
    assert_eq!((frame.kind.as_str(), frame.block, frame.facing), ("minecraft:item_frame", BlockPos::new(8, -60, 10), 2));
    let used = server.handle.run(move |s| s.players[&id].inventory[36].is_empty()).unwrap();
    assert!(used, "the frame item is used up");
    let frame_type = paper_data::entity_types::by_name("minecraft:item_frame").unwrap().id;
    let mut bob = Client::join(server.port(), "Bob");
    let shown: play::AddEntity = bob.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == frame_type).expect("Bob sees the frame");
    assert_eq!((shown.x, shown.y, shown.z, shown.data.0), (8.0, -60.0, 10.0, 2));
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();

    // A diamond goes in (one of them), and each further click turns it.
    let diamond = item("minecraft:diamond");
    server.handle.run(move |s| s.set_inventory_slot(id, 36, Slot::of(diamond, 2))).unwrap();
    let click = |client: &mut Client| client.send(&play::Interact { entity_id: VarInt(frame.id), hand: VarInt(0), target: LpVec3(0.0, 0.0, 0.0), sneaking: false });
    click(&mut alice);
    wait_reading(&mut alice, "the diamond in the frame", || hung(&server)[0].item == Some(diamond));
    let left = server.handle.run(move |s| s.players[&id].inventory[36].0.as_ref().map(|i| i.count)).unwrap();
    assert_eq!(left, Some(1), "one diamond goes in");
    click(&mut alice);
    click(&mut alice);
    wait_reading(&mut alice, "the diamond turned", || hung(&server)[0].rotation == 2);

    // A hit knocks the diamond out; the next breaks the frame, which drops itself.
    alice.send(&play::Attack { entity_id: VarInt(frame.id) });
    wait_reading(&mut alice, "the diamond out", || hung(&server)[0].item.is_none());
    assert!(dropped(&server, id, diamond) && hung(&server).len() == 1, "the diamond drops and the frame stays");
    std::thread::sleep(Duration::from_millis(600));
    alice.send(&play::Attack { entity_id: VarInt(frame.id) });
    wait_reading(&mut alice, "the frame broken", || hung(&server).is_empty());
    assert!(dropped(&server, id, item("minecraft:item_frame")), "the frame drops");
    server.stop();
}

#[test]
fn paintings_fill_the_wall_and_fall_without_it() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();
    let id = alice.entity_id;
    // A wall four blocks wide and two high, north of which Alice stands.
    let stone = paper_data::blocks::default_state("minecraft:stone").unwrap();
    server
        .handle
        .run(move |s| {
            for x in 6..=9 {
                for y in -60..=-59 {
                    s.set_block_and_update(0, BlockPos::new(x, y, 12), stone);
                }
            }
            s.set_inventory_slot(id, 36, Slot::of(item("minecraft:painting"), 2));
        })
        .unwrap();
    // Not on the floor.
    alice.send(&use_on(BlockPos::new(8, -61, 9), 1, 1));
    alice.drain(Duration::from_millis(300));
    assert!(hung(&server).is_empty(), "paintings hang on walls only");

    // Clicked at its third block, the whole wall takes a painting four wide and two high
    // (each reaches left and up from the block clicked).
    alice.send(&use_on(BlockPos::new(8, -60, 12), 2, 2));
    wait_reading(&mut alice, "a painting", || !hung(&server).is_empty());
    let painting = hung(&server)[0].clone();
    assert_eq!((painting.kind.as_str(), painting.block, painting.facing), ("minecraft:painting", BlockPos::new(8, -60, 11), 2));
    assert!(["minecraft:fighters", "minecraft:changing", "minecraft:finding", "minecraft:lowmist", "minecraft:passage"].contains(&painting.variant.as_str()), "a 4x2 painting: {}", painting.variant);

    // Without a block of the wall it falls within the check interval, and drops itself.
    server.handle.run(|s| s.set_block_and_update(0, BlockPos::new(6, -59, 12), paper_data::blocks::AIR)).unwrap();
    wait_reading(&mut alice, "the painting down", || hung(&server).is_empty());
    assert!(dropped(&server, id, item("minecraft:painting")), "the painting drops");
    server.stop();
}
