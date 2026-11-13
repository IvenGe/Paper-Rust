//! Block updates seen by a client: redstone reacting to a player, fluids flowing, placement
//! rules, all over the real protocol.

mod common;

use common::{start_server, Client, TIMEOUT};
use paper_data::blocks;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(400);

fn prop(state: i32, name: &str) -> Option<&'static str> {
    blocks::property(state as u32, name)
}

fn block_name(state: i32) -> &'static str {
    blocks::block_of_state(state as u32).name
}

/// Joins and waits until the spawn chunk arrived and the world is quiet.
fn join(port: u16) -> Client {
    let mut alice = Client::join(port, "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    alice.drain(QUIET);
    alice.clear_backlog();
    alice
}

fn use_on(client: &mut Client, pos: BlockPos, face: i32, sequence: i32) {
    client.send(&play::UseItemOn { hand: VarInt(0), pos, face: VarInt(face), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(sequence) });
}

fn update_at(client: &mut Client, pos: BlockPos, pred: impl Fn(i32) -> bool) -> i32 {
    client.find(TIMEOUT, |u: &play::BlockUpdate| u.pos == pos && pred(u.state.0)).unwrap_or_else(|| panic!("no matching block update at {pos:?}")).state.0
}

#[test]
fn a_player_pulls_a_lever_and_sees_the_lamp_light_up() {
    let server = start_server(|_| {});
    let mut alice = join(server.port());
    let (lever, lamp) = (BlockPos::new(11, -60, 8), BlockPos::new(10, -60, 8));
    server.handle.console_command("setblock 10 -60 8 minecraft:redstone_lamp");
    server.handle.console_command("setblock 11 -60 8 minecraft:lever[face=floor,facing=north]");
    update_at(&mut alice, lever, |s| block_name(s) == "minecraft:lever");
    alice.clear_backlog();

    // Right-click the lever: it flips instead of anything being placed, and the lamp lights.
    use_on(&mut alice, lever, 1, 3);
    let state = update_at(&mut alice, lever, |_| true);
    assert_eq!(prop(state, "powered"), Some("true"));
    let state = update_at(&mut alice, lamp, |_| true);
    assert_eq!((block_name(state), prop(state, "lit")), ("minecraft:redstone_lamp", Some("true")));
    let ack: play::BlockChangedAck = alice.find(TIMEOUT, |a: &play::BlockChangedAck| a.sequence.0 == 3).expect("ack");
    assert_eq!(ack.sequence.0, 3);
    let click = paper_data::registries::id("minecraft:sound_event", "minecraft:block.lever.click").unwrap() as i32;
    let sound: play::Sound = alice.expect();
    assert_eq!(sound.sound, play::SoundRef::Id(click));
    // The lamp lights up at once and goes out 4 ticks after the lever is off again.
    use_on(&mut alice, lever, 1, 4);
    let state = update_at(&mut alice, lever, |s| prop(s, "powered") == Some("false"));
    assert_eq!(prop(state, "face"), Some("floor"));
    let state = update_at(&mut alice, lamp, |s| prop(s, "lit") == Some("false"));
    assert_eq!(block_name(state), "minecraft:redstone_lamp");
    let (lit, powered) = server.handle.run(move |s| (s.worlds[0].block_at(10, -60, 8), s.worlds[0].block_at(11, -60, 8))).unwrap();
    assert_eq!(prop(lit.unwrap() as i32, "lit"), Some("false"));
    assert_eq!(prop(powered.unwrap() as i32, "powered"), Some("false"));
    server.stop();
}

#[test]
fn water_placed_with_setblock_flows() {
    let server = start_server(|_| {});
    let mut alice = join(server.port());
    server.handle.console_command("setblock 4 -60 4 minecraft:water");
    let source = update_at(&mut alice, BlockPos::new(4, -60, 4), |_| true);
    assert_eq!((block_name(source), prop(source, "level")), ("minecraft:water", Some("0")));
    // One block per 5 ticks, one level less per block.
    for (x, level) in [(5, "1"), (6, "2"), (7, "3")] {
        let state = update_at(&mut alice, BlockPos::new(x, -60, 4), |s| block_name(s) == "minecraft:water");
        assert_eq!(prop(state, "level"), Some(level), "water at x {x}");
    }
    let state = update_at(&mut alice, BlockPos::new(4, -60, 0), |s| block_name(s) == "minecraft:water");
    assert_eq!(prop(state, "level"), Some("4"));
    // A torch in the way is washed away.
    server.handle.console_command("setblock 20 -60 4 minecraft:torch");
    server.handle.console_command("setblock 22 -60 4 minecraft:water");
    let state = update_at(&mut alice, BlockPos::new(21, -60, 4), |s| block_name(s) == "minecraft:water");
    assert_eq!(prop(state, "level"), Some("1"));
    let state = update_at(&mut alice, BlockPos::new(20, -60, 4), |s| block_name(s) != "minecraft:torch");
    assert_eq!(block_name(state), "minecraft:water");
    server.stop();
}

#[test]
fn placing_redstone_follows_vanilla_rules() {
    let server = start_server(|_| {});
    let mut alice = join(server.port());
    let redstone = paper_data::items::id("minecraft:redstone").unwrap() as i32;
    alice.send(&play::SetCreativeModeSlot { slot: 36, item: Slot::of(redstone, 64) });
    // A redstone block, then dust placed on the ground next to it: the dust connects to it
    // and carries power.
    server.handle.console_command("setblock 10 -60 10 minecraft:redstone_block");
    update_at(&mut alice, BlockPos::new(10, -60, 10), |_| true);
    use_on(&mut alice, BlockPos::new(11, -61, 10), 1, 5);
    let wire = update_at(&mut alice, BlockPos::new(11, -60, 10), |s| block_name(s) == "minecraft:redstone_wire");
    assert_eq!(prop(wire, "west"), Some("side"), "the dust points at the redstone block");
    let wire = update_at(&mut alice, BlockPos::new(11, -60, 10), |s| prop(s, "power") == Some("15"));
    assert_eq!(prop(wire, "east"), Some("side"), "a line through the block");
    // Dust cannot be placed on the side of a block in mid-air.
    server.handle.console_command("setblock 12 -57 12 minecraft:stone");
    update_at(&mut alice, BlockPos::new(12, -57, 12), |_| true);
    alice.clear_backlog();
    use_on(&mut alice, BlockPos::new(12, -57, 12), 5, 6);
    alice.find(TIMEOUT, |a: &play::BlockChangedAck| a.sequence.0 == 6).expect("ack");
    assert_eq!(server.handle.run(|s| s.worlds[0].block_at(13, -57, 12)), Some(Some(blocks::AIR)));
    server.stop();
}

#[test]
fn note_blocks_send_block_events() {
    let server = start_server(|_| {});
    let mut alice = join(server.port());
    server.handle.console_command("setblock 9 -60 6 minecraft:note_block");
    server.handle.console_command("setblock 9 -60 5 minecraft:lever[face=floor,facing=north]");
    update_at(&mut alice, BlockPos::new(9, -60, 5), |_| true);
    alice.clear_backlog();
    use_on(&mut alice, BlockPos::new(9, -60, 5), 1, 2);
    let event: play::BlockEvent = alice.expect();
    assert_eq!(event.pos, BlockPos::new(9, -60, 6));
    assert_eq!(event.block.0, blocks::id("minecraft:note_block").unwrap() as i32);
    let harp = paper_data::registries::id("minecraft:sound_event", "minecraft:block.note_block.harp").unwrap() as i32;
    alice.find(TIMEOUT, |s: &play::Sound| s.sound == play::SoundRef::Id(harp)).expect("the note");
    server.stop();
}
