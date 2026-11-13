//! Dropped items: blocks broken in survival drop loot, players pick items up and throw them.

mod common;

use common::*;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(300);

fn item_type() -> i32 {
    paper_data::entity_types::by_name("minecraft:item").unwrap().id
}

#[test]
fn broken_blocks_drop_items_that_players_pick_up() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    alice.clear_backlog();

    // Break the grass block under Alice's feet.
    let below = BlockPos::new(8, -61, 8);
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: below, face: 1, sequence: VarInt(1) });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::STOP_DESTROY_BLOCK), pos: below, face: 1, sequence: VarInt(2) });
    let spawn: play::AddEntity = alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == item_type()).expect("an item entity appears");
    // A real client falls into the hole; this one has to be told.
    alice.move_to(8.5, -61.0, 8.5);

    // It is picked up once its pickup delay is over, and lands in the hotbar.
    let take: play::TakeItemEntity = alice.expect();
    assert_eq!((take.collected_id.0, take.collector_id.0, take.count.0), (spawn.entity_id.0, alice.entity_id, 1));
    let slot: play::ContainerSetSlot = alice.find(TIMEOUT, |s: &play::ContainerSetSlot| s.slot == 36).expect("hotbar slot updated");
    let item = slot.item.0.as_ref().unwrap();
    let name = paper_data::items::get(item.item as u32).unwrap().name;
    assert!(name == "minecraft:dirt" || name == "minecraft:grass_block", "grass drops dirt (or itself without loot tables), got {name}");
    let removed: play::RemoveEntities = alice.find(TIMEOUT, |r: &play::RemoveEntities| r.entity_ids.contains(&spawn.entity_id)).expect("item removed");
    assert_eq!(removed.entity_ids, vec![spawn.entity_id]);

    // Throwing it (Q) spawns a new item entity in front of her and empties the slot.
    alice.clear_backlog();
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::DROP_ITEM), pos: BlockPos::new(0, 0, 0), face: 0, sequence: VarInt(0) });
    let emptied: play::ContainerSetSlot = alice.find(TIMEOUT, |s: &play::ContainerSetSlot| s.slot == 36).expect("slot emptied");
    assert!(emptied.item.is_empty());
    let thrown: play::AddEntity = alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == item_type()).expect("thrown item");
    assert!(thrown.velocity.2 > 0.1, "thrown forward (yaw 0 looks towards +z): {:?}", thrown.velocity);
    let count = server.handle.run(|s| s.worlds[0].entities.values().filter(|e| matches!(e.kind, paper_server::world::entity::EntityKind::Item { .. })).count()).unwrap();
    assert_eq!(count, 1, "the thrown item exists on the server");
    server.stop();
}

#[test]
fn blocks_take_their_mining_time() {
    use paper_protocol::types::Slot;
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    alice.drain(QUIET);
    let id = alice.entity_id;
    let stone = paper_data::blocks::default_state("minecraft:stone").unwrap();
    let (a, b) = (BlockPos::new(10, -60, 8), BlockPos::new(10, -60, 9));
    server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, a, stone);
            s.set_block_and_update(0, b, stone);
        })
        .unwrap();
    // Standing on the ground (mining in the air is five times slower).
    alice.move_to(8.5, -60.0, 8.5);
    // A hacked client claiming to break stone by hand at once: the stone stays (by hand it
    // takes seven and a half seconds).
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: a, face: 1, sequence: VarInt(1) });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::STOP_DESTROY_BLOCK), pos: a, face: 1, sequence: VarInt(2) });
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(server.handle.run(move |s| s.worlds[0].block_at(a.x, a.y, a.z)).unwrap(), Some(stone), "not broken yet");

    // A diamond pickaxe needs about six ticks: the break goes through once they are over.
    let pickaxe = paper_data::items::id("minecraft:diamond_pickaxe").unwrap() as i32;
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().inventory[36] = Slot::of(pickaxe, 1)).unwrap();
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: b, face: 1, sequence: VarInt(3) });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::STOP_DESTROY_BLOCK), pos: b, face: 1, sequence: VarInt(4) });
    std::thread::sleep(Duration::from_millis(1000));
    let broken = server.handle.run(move |s| s.worlds[0].block_at(b.x, b.y, b.z).map(paper_data::blocks::is_air)).unwrap();
    assert_eq!(broken, Some(true), "broken with the pickaxe");
    server.stop();
}
