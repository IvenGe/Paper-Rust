//! Many block changes at once, as WorldEdit makes them through the plugin host: a batch of
//! changes without physics (`world.edit_batch`), the side effects WorldEdit applies after an
//! edit, and clients told per section with the light per chunk rather than block by block.

mod common;

use common::*;
use paper_bridge::{bmap, Value};
use paper_data::blocks;
use paper_protocol::packets::{play, Packet};
use paper_server::RunningServer;
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(300);

/// One `world.edit_batch` entry: kind (0 change, 1 side effects), position, state, old
/// state, flags.
fn entry(out: &mut Vec<u8>, kind: i32, (x, y, z): (i32, i32, i32), state: u32, old: u32, flags: i32) {
    for v in [kind, x, y, z, state as i32, old as i32, flags] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn call(server: &RunningServer, method: &'static str, args: Value) -> Value {
    server.handle.run(move |s| paper_server::plugins::api::handle(s, method, &args).map_err(|e| e.to_string())).unwrap().unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn name_at(server: &RunningServer, x: i32, y: i32, z: i32) -> &'static str {
    server.handle.run(move |s| blocks::block_of_state(s.worlds[0].block_at(x, y, z).unwrap()).name).unwrap()
}

fn join(server: &RunningServer) -> Client {
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    alice.drain(QUIET);
    alice.clear_backlog();
    alice
}

#[test]
fn batches_change_blocks_and_tell_clients_per_section() {
    let server = start_server(|_| {});
    let mut alice = join(&server);
    let stone = blocks::default_state("minecraft:stone").unwrap();

    // 8 x 8 x 8 blocks over two sections (y -56..-49 and -48..-41 are in sections -4 and -3).
    let mut edits = Vec::new();
    for x in 0..8 {
        for y in -52..-44 {
            for z in 0..8 {
                entry(&mut edits, 0, (x, y, z), stone, 0, 0);
            }
        }
    }
    let applied = call(&server, "world.edit_batch", bmap! { "edits" => edits });
    assert_eq!(applied, Value::Int(512));
    assert_eq!((name_at(&server, 0, -52, 0), name_at(&server, 7, -45, 7)), ("minecraft:stone", "minecraft:stone"));
    assert_eq!(server.handle.run(|s| s.events_suppressed).unwrap(), 0, "plugins hear events again after the batch");

    // The client gets a packet per section, not 512 block updates and light updates.
    let deadline = Instant::now() + Duration::from_secs(2);
    let (mut sections, mut singles, mut lights, mut blocks_told) = (0, 0, 0, 0);
    while Instant::now() < deadline {
        let Some((id, frame)) = alice.next_packet(Duration::from_millis(100)) else { continue };
        if id == play::SectionBlocksUpdate::ID {
            sections += 1;
            blocks_told += decode::<play::SectionBlocksUpdate>(&frame).blocks.len();
        } else if id == play::BlockUpdate::ID {
            // Only the edited blocks count: the world goes on meanwhile (a sheep eats grass).
            let p = decode::<play::BlockUpdate>(&frame).pos;
            if (0..8).contains(&p.x) && (-52..-44).contains(&p.y) && (0..8).contains(&p.z) {
                singles += 1;
            }
        } else if id == play::LightUpdate::ID {
            lights += 1;
        }
    }
    assert_eq!((sections, singles, blocks_told), (2, 0, 512), "one packet per section with every block");
    assert!(lights <= 4, "the light goes per chunk, not per block: {lights} light updates");

    // `world.section` tells the state ids and where block entities are.
    let chest = blocks::default_state("minecraft:chest").unwrap();
    call(&server, "world.set_block", bmap! { "x" => 3, "y" => -50, "z" => 4, "state" => chest as i64, "physics" => false });
    let section = call(&server, "world.section", bmap! { "x" => 3, "y" => -50, "z" => 4 });
    let ids: Vec<i64> = section.get("ids").as_list().unwrap().iter().filter_map(Value::as_i64).collect();
    assert!(ids.contains(&(stone as i64)) && ids.contains(&(chest as i64)), "{ids:?}");
    let entities: Vec<i64> = section.get("block_entities").as_list().unwrap().iter().filter_map(Value::as_i64).collect();
    assert_eq!(entities, vec![((-50 - -64) % 16) << 8 | 4 << 4 | 3], "the chest");
    server.stop();
}

#[test]
fn side_effects_wait_for_the_edit_to_finish() {
    let server = start_server(|_| {});
    let _alice = join(&server);
    let water = blocks::default_state("minecraft:water").unwrap();
    let air = blocks::default_state("minecraft:air").unwrap();

    // Water put in the air without physics stays put, as WorldEdit's edits leave it...
    let mut edits = Vec::new();
    entry(&mut edits, 0, (5, -55, 5), water, 0, 0);
    call(&server, "world.edit_batch", bmap! { "edits" => edits });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(name_at(&server, 5, -56, 5), "minecraft:air", "no flow without side effects");

    // ...until the side effects come (its onPlace and its neighbours' updates): it flows.
    let mut edits = Vec::new();
    entry(&mut edits, 1, (5, -55, 5), water, air, 1 | 2 | 4);
    call(&server, "world.edit_batch", bmap! { "edits" => edits });
    let deadline = Instant::now() + TIMEOUT;
    while name_at(&server, 5, -56, 5) != "minecraft:water" {
        assert!(Instant::now() < deadline, "the water flows down");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Side effects are for the block as it is when they run (an earlier one's may have
    // changed it, as a wire's connections), not as the host last saw it.
    let stone = blocks::default_state("minecraft:stone").unwrap();
    let mut edits = Vec::new();
    entry(&mut edits, 0, (9, -55, 9), water, 0, 0);
    entry(&mut edits, 1, (9, -55, 9), stone, air, 1 | 2);
    call(&server, "world.edit_batch", bmap! { "edits" => edits });
    let deadline = Instant::now() + TIMEOUT;
    while name_at(&server, 9, -56, 9) != "minecraft:water" {
        assert!(Instant::now() < deadline, "the water there flows, though the entry said stone");
        std::thread::sleep(Duration::from_millis(50));
    }

    // A bad batch is refused whole: nothing of it is made.
    let result = server.handle.run(|s| paper_server::plugins::api::handle(s, "world.edit_batch", &bmap! { "edits" => vec![1u8, 2, 3] }).is_err()).unwrap();
    assert!(result, "28 bytes an entry");
    let mut edits = Vec::new();
    entry(&mut edits, 0, (12, -55, 12), stone, 0, 0);
    entry(&mut edits, 0, (13, -55, 12), u32::MAX, 0, 0);
    let refused = server.handle.run(move |s| paper_server::plugins::api::handle(s, "world.edit_batch", &bmap! { "edits" => edits }).is_err()).unwrap();
    assert!(refused, "an invalid state");
    assert_eq!(name_at(&server, 12, -55, 12), "minecraft:air", "the good entry before it was not made either");
    assert_eq!(server.handle.run(|s| s.events_suppressed).unwrap(), 0);
    server.stop();
}

#[test]
fn the_player_state_says_where_the_player_is() {
    let server = start_server(|_| {});
    let alice = join(&server);
    let id = alice.entity_id;
    let water = blocks::default_state("minecraft:water").unwrap();
    let state = move |s: &mut paper_server::server::Server| paper_server::plugins::api::player_state(s, &s.players[&id]);
    let dry = server.handle.run(state).unwrap();
    assert_eq!((dry.bool("in_water"), dry.bool("under_water")), (Some(false), Some(false)));

    // Water at the feet and around the eyes.
    let (x, y, z) = server.handle.run(move |s| { let l = s.players[&id].loc; (l.x.floor() as i32, l.y.floor() as i32, l.z.floor() as i32) }).unwrap();
    for dy in 0..2 {
        call(&server, "world.set_block", bmap! { "x" => x, "y" => y + dy, "z" => z, "state" => water as i64, "physics" => false });
    }
    let wet = server.handle.run(state).unwrap();
    assert_eq!((wet.bool("in_water"), wet.bool("under_water"), wet.bool("in_lava")), (Some(true), Some(true), Some(false)));
    server.stop();
}
