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
    client.find_block_update(TIMEOUT, pos, pred).unwrap_or_else(|| panic!("no matching block update at {pos:?}"))
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

#[test]
fn players_open_doors_and_set_repeaters_by_hand() {
    let server = start_server(|_| {});
    let mut alice = join(server.port());
    let stone = paper_data::items::id("minecraft:stone").unwrap() as i32;
    alice.send(&play::SetCreativeModeSlot { slot: 36, item: Slot::of(stone, 64) });
    for command in [
        "setblock 10 -60 6 minecraft:oak_door[half=lower]",
        "setblock 10 -59 6 minecraft:oak_door[half=upper]",
        "setblock 6 -60 6 minecraft:iron_door[half=lower]",
        "setblock 6 -59 6 minecraft:iron_door[half=upper]",
        "setblock 8 -60 11 minecraft:repeater",
    ] {
        server.handle.console_command(command);
    }
    update_at(&mut alice, BlockPos::new(8, -60, 11), |_| true);
    alice.clear_backlog();

    // Holding stone, a click on the wooden door opens it (both halves); nothing is placed.
    use_on(&mut alice, BlockPos::new(10, -59, 6), 2, 10);
    let upper = update_at(&mut alice, BlockPos::new(10, -59, 6), |_| true);
    assert_eq!(prop(upper, "open"), Some("true"));
    let lower = update_at(&mut alice, BlockPos::new(10, -60, 6), |_| true);
    assert_eq!(prop(lower, "open"), Some("true"));
    alice.find(TIMEOUT, |a: &play::BlockChangedAck| a.sequence.0 == 10).expect("ack");
    assert_eq!(server.handle.run(|s| s.worlds[0].block_at(10, -59, 5)), Some(Some(blocks::AIR)), "no stone placed");

    // An iron door ignores the click: the stone is placed against it instead.
    use_on(&mut alice, BlockPos::new(6, -60, 6), 2, 11);
    alice.find(TIMEOUT, |a: &play::BlockChangedAck| a.sequence.0 == 11).expect("ack");
    let iron = server.handle.run(|s| s.worlds[0].block_at(6, -60, 6)).unwrap().unwrap();
    assert_eq!(prop(iron as i32, "open"), Some("false"));
    assert_eq!(server.handle.run(|s| s.worlds[0].block_at(6, -60, 5)), Some(Some(blocks::default_state("minecraft:stone").unwrap())));

    // Clicking a repeater cycles its delay; sneaking with an item in hand places instead.
    use_on(&mut alice, BlockPos::new(8, -60, 11), 1, 12);
    let repeater = update_at(&mut alice, BlockPos::new(8, -60, 11), |_| true);
    assert_eq!(prop(repeater, "delay"), Some("2"));
    alice.send(&play::PlayerInput { flags: play::input_flags::SNEAK });
    use_on(&mut alice, BlockPos::new(8, -60, 11), 1, 13);
    alice.find(TIMEOUT, |a: &play::BlockChangedAck| a.sequence.0 == 13).expect("ack");
    let (repeater, above) = server.handle.run(|s| (s.worlds[0].block_at(8, -60, 11), s.worlds[0].block_at(8, -59, 11))).unwrap();
    assert_eq!(prop(repeater.unwrap() as i32, "delay"), Some("2"), "sneaking does not use the repeater");
    assert_eq!(above, Some(blocks::default_state("minecraft:stone").unwrap()));
    server.stop();
}

/// Two pistons whose block events run in the same tick, the second pushing into the spot the
/// first one's pull leaves: players are told of that spot as the push finds it (air), not as
/// it ends up (the pushed block on its way), so their own replay of the push works; after the
/// push they are told how it ends up, and then of the stone when the move lands.
#[test]
fn block_changes_go_out_as_later_block_events_find_them() {
    use paper_protocol::packets::Packet;
    use paper_server::world::block_updates::{flags, Level};
    let server = start_server(|_| {});
    let mut alice = join(server.port());
    let state = |s: &str| blocks::parse_state(s).unwrap();
    let (a, b, spot) = (BlockPos::new(2, -57, 4), BlockPos::new(4, -57, 2), BlockPos::new(4, -57, 4));
    let setup = [
        (a, state("minecraft:sticky_piston[facing=east,extended=true]")),
        (BlockPos::new(3, -57, 4), state("minecraft:piston_head[facing=east,type=sticky]")),
        (spot, state("minecraft:gold_block")),
        (b, state("minecraft:piston[facing=south]")),
        (BlockPos::new(4, -57, 3), state("minecraft:stone")),
        (BlockPos::new(4, -57, 1), state("minecraft:redstone_block")),
    ];
    server
        .handle
        .run(move |s| {
            for (pos, state) in setup {
                Level::new(&mut s.worlds[0]).set_block(pos, state, flags::CLIENTS | flags::KNOWN_SHAPE | flags::SKIP_ON_PLACE);
            }
            s.publish_block_updates(0);
        })
        .unwrap();
    update_at(&mut alice, BlockPos::new(4, -57, 1), |_| true);
    alice.drain(QUIET);
    alice.clear_backlog();
    let (sticky, piston) = (blocks::id("minecraft:sticky_piston").unwrap(), blocks::id("minecraft:piston").unwrap());
    server
        .handle
        .run(move |s| {
            let mut level = Level::new(&mut s.worlds[0]);
            // The sticky piston pulls the gold block back; then the other pushes the stone
            // into the gold block's spot.
            level.block_event(a, sticky, 1, 5);
            level.block_event(b, piston, 0, 3);
        })
        .unwrap();
    let local = ((spot.x & 15) as u8, (spot.y & 15) as u8, (spot.z & 15) as u8);
    // What players are told of the spot, in order, with `None` where the push arrives.
    let (mut told, mut events) = (Vec::new(), 0);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let Some((id, frame)) = alice.next_packet(Duration::from_millis(100)) else { continue };
        if id == play::BlockUpdate::ID {
            let update: play::BlockUpdate = common::decode(&frame);
            if update.pos == spot {
                told.push(Some(update.state.0 as u32));
            }
        } else if id == play::SectionBlocksUpdate::ID {
            let update: play::SectionBlocksUpdate = common::decode(&frame);
            if (update.section_x, update.section_y, update.section_z) == (spot.x >> 4, spot.y >> 4, spot.z >> 4) {
                if let Some(&(_, _, _, state)) = update.blocks.iter().rev().find(|&&(x, y, z, _)| (x, y, z) == local) {
                    told.push(Some(state));
                }
            }
        } else if id == play::BlockEvent::ID {
            let event: play::BlockEvent = common::decode(&frame);
            events += 1;
            if event.pos == b {
                told.push(None);
            }
        }
    }
    assert_eq!(events, 2, "both moves went out");
    let names: Vec<_> = told.iter().map(|s| s.map(|s| blocks::block_of_state(s).name)).collect();
    let push = names.iter().position(Option::is_none).expect("the push");
    assert_eq!(names[..push].last(), Some(&Some("minecraft:air")), "the spot is air when the push arrives: {names:?}");
    assert_eq!(names.get(push + 1), Some(&Some("minecraft:moving_piston")), "then the stone on its way: {names:?}");
    assert_eq!(names.last(), Some(&Some("minecraft:stone")), "and the stone when it lands: {names:?}");
    server.stop();
}

/// An effect published ahead of a block's own packet may change the block again (a dispenser
/// pours water where a piston's pull left air, in the same tick): players are told of the
/// block as it is when its packet goes out, the water, not as it was before the effects ran.
#[test]
fn block_packets_tell_the_block_as_it_is_when_they_go() {
    use paper_protocol::packets::Packet;
    use paper_server::world::block_updates::Effect;
    let server = start_server(|_| {});
    let mut alice = join(server.port());
    let (dispenser, front) = (BlockPos::new(6, -60, 4), BlockPos::new(7, -60, 4));
    let water_bucket = paper_data::items::id("minecraft:water_bucket").unwrap() as i32;
    server
        .handle
        .run(move |s| {
            let state = blocks::parse_state("minecraft:dispenser[facing=east]").unwrap();
            s.set_block_and_update(0, dispenser, state);
            let mut items = vec![Slot::EMPTY; 9];
            items[0] = Slot::of(water_bucket, 1);
            s.set_container_items(0, dispenser, &items);
        })
        .unwrap();
    update_at(&mut alice, dispenser, |_| true);
    alice.drain(QUIET);
    alice.clear_backlog();
    // One tick's effects: the dispenser fires (its scheduled tick), then the block in front
    // is pulled away (a later block event); the dispenser pours its water there first.
    let stone = blocks::default_state("minecraft:stone").unwrap();
    server
        .handle
        .run(move |s| {
            let effects = &mut s.worlds[0].updates.effects;
            effects.push(Effect::Dispense { pos: dispenser });
            effects.push(Effect::Block(paper_server::world::BlockChange { pos: front, old: stone, new: blocks::AIR, light: Default::default() }));
            s.publish_block_updates(0);
        })
        .unwrap();
    assert_eq!(server.handle.run(move |s| s.worlds[0].block_at(front.x, front.y, front.z)).unwrap().map(block_name_u32), Some("minecraft:water"));
    let local = ((front.x & 15) as u8, (front.y & 15) as u8, (front.z & 15) as u8);
    let mut told = None;
    let deadline = std::time::Instant::now() + Duration::from_millis(1500);
    while std::time::Instant::now() < deadline {
        let Some((id, frame)) = alice.next_packet(Duration::from_millis(100)) else { continue };
        if id == play::BlockUpdate::ID {
            let update: play::BlockUpdate = common::decode(&frame);
            if update.pos == front {
                told = Some(update.state.0 as u32);
            }
        } else if id == play::SectionBlocksUpdate::ID {
            let update: play::SectionBlocksUpdate = common::decode(&frame);
            if (update.section_x, update.section_y, update.section_z) == (front.x >> 4, front.y >> 4, front.z >> 4) {
                if let Some(&(_, _, _, state)) = update.blocks.iter().rev().find(|&&(x, y, z, _)| (x, y, z) == local) {
                    told = Some(state);
                }
            }
        }
    }
    assert_eq!(told.map(block_name_u32), Some("minecraft:water"), "players end on the water");
    server.stop();
}

fn block_name_u32(state: u32) -> &'static str {
    blocks::block_of_state(state).name
}
