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
    let removed: play::RemoveEntities = alice.expect();
    assert_eq!(removed.entity_ids, vec![spawn.entity_id]);

    // Throwing it (Q) spawns a new item entity in front of her and empties the slot.
    alice.clear_backlog();
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::DROP_ITEM), pos: BlockPos::new(0, 0, 0), face: 0, sequence: VarInt(0) });
    let emptied: play::ContainerSetSlot = alice.find(TIMEOUT, |s: &play::ContainerSetSlot| s.slot == 36).expect("slot emptied");
    assert!(emptied.item.is_empty());
    let thrown: play::AddEntity = alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == item_type()).expect("thrown item");
    assert!(thrown.velocity.2 > 0.1, "thrown forward (yaw 0 looks towards +z): {:?}", thrown.velocity);
    let count = server.handle.run(|s| s.worlds[0].entities.len()).unwrap();
    assert_eq!(count, 1, "the thrown item exists on the server");
    server.stop();
}
