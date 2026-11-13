//! The Nether and the End: lighting a Nether portal and travelling both ways, opening an
//! End portal with the last eye, leaving the End through the exit portal; buckets.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::GameMode;
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(300);

fn join(server: &paper_server::RunningServer) -> Client {
    let mut client = Client::join(server.port(), "Alice");
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(QUIET);
    client.clear_backlog();
    client
}

fn use_on(pos: BlockPos, face: i32, sequence: i32) -> play::UseItemOn {
    play::UseItemOn { hand: VarInt(0), pos, face: VarInt(face), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(sequence) }
}

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

fn give(server: &paper_server::RunningServer, id: i32, slot: usize, stack: Slot) {
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&id).unwrap().inventory[slot] = stack;
            s.resync_inventory(id);
        })
        .unwrap();
}

fn world_of(server: &paper_server::RunningServer, id: i32) -> String {
    server.handle.run(move |s| s.worlds[s.players[&id].world].dimension.clone()).unwrap()
}

#[test]
fn nether_portals_lead_to_the_nether_and_back() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Creative);
    let mut alice = join(&server);
    let id = alice.entity_id;
    assert_eq!(server.handle.run(|s| s.worlds.len()).unwrap(), 3, "overworld, nether and end");

    // An obsidian frame standing on the ground: inside 2 wide, 3 high, along x.
    server
        .handle
        .run(|s| {
            let obsidian = blocks::default_state("minecraft:obsidian").unwrap();
            for x in 12..=15 {
                for y in -60..=-56 {
                    if x == 12 || x == 15 || y == -60 || y == -56 {
                        s.set_block_and_update(0, BlockPos::new(x, y, 8), obsidian);
                    }
                }
            }
        })
        .unwrap();
    give(&server, id, 36, Slot::of(item("minecraft:flint_and_steel"), 1));
    alice.drain(QUIET);

    // Lighting the inside fills it with portal blocks.
    alice.send(&use_on(BlockPos::new(13, -60, 8), 1, 1));
    alice.drain(QUIET);
    let portal = server
        .handle
        .run(|s| (13..=14).all(|x| (-59..=-57).all(|y| s.worlds[0].block_at(x, y, 8).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:nether_portal"))))
        .unwrap();
    assert!(portal, "the frame lights up");

    // Stepping in takes Alice to the Nether (at once in creative).
    alice.clear_backlog();
    alice.move_to(13.5, -59.0, 8.5);
    let respawn: play::Respawn = alice.find(TIMEOUT, |_: &play::Respawn| true).expect("dimension change");
    assert_eq!(respawn.spawn.dimension_name, "minecraft:the_nether");
    let arrived: play::PlayerPosition = alice.expect();
    assert!((arrived.x - 1.7).abs() < 20.0 && (arrived.z - 1.0).abs() < 20.0, "coordinates divided by eight: {arrived:?}");
    assert_eq!(world_of(&server, id), "minecraft:the_nether");
    // A portal was built where she arrived.
    let (x, y, z) = (arrived.x.floor() as i32, arrived.y.floor() as i32, arrived.z.floor() as i32);
    let built = server
        .handle
        .run(move |s| {
            let nether = s.world_of_dimension("minecraft:the_nether").unwrap();
            s.worlds[nether].block_at(x, y, z).map(|b| blocks::block_of_state(b).name)
        })
        .unwrap();
    assert_eq!(built, Some("minecraft:nether_portal"));

    // Out of the portal and back in: home again, to the portal she came through.
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == (x >> 4) && c.z == (z >> 4)).expect("nether chunks arrive");
    alice.move_to(arrived.x + 3.0, arrived.y + 2.0, arrived.z + 3.0);
    std::thread::sleep(Duration::from_millis(800));
    alice.clear_backlog();
    alice.move_to(arrived.x, arrived.y, arrived.z);
    let back: play::Respawn = alice.find(TIMEOUT, |_: &play::Respawn| true).expect("back to the overworld");
    assert_eq!(back.spawn.dimension_name, "minecraft:overworld");
    let home: play::PlayerPosition = alice.expect();
    assert!((home.x - 13.5).abs() < 2.0 && (home.z - 8.5).abs() < 2.0, "arrives at the original portal: {home:?}");
    server.stop();
}

#[test]
fn the_last_eye_opens_the_end_and_the_exit_portal_leads_home() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Creative);
    let mut alice = join(&server);
    let id = alice.entity_id;
    // Twelve frames around the square x 11..13, z 7..9, facing inward; one without an eye.
    let center = BlockPos::new(12, -60, 8);
    server
        .handle
        .run(move |s| {
            let frame = blocks::default_state("minecraft:end_portal_frame").unwrap();
            for (facing, (dx, dz)) in [("south", (0, -1)), ("north", (0, 1)), ("east", (-1, 0)), ("west", (1, 0))] {
                for side in -1..=1 {
                    let pos = if dx == 0 { center.offset(side, 0, dz * 2) } else { center.offset(dx * 2, 0, side) };
                    let with_eye = !(facing == "west" && side == 0);
                    let state = blocks::with_property(blocks::with_property(frame, "facing", facing).unwrap(), "eye", if with_eye { "true" } else { "false" }).unwrap();
                    s.set_block_and_update(0, pos, state);
                }
            }
        })
        .unwrap();
    give(&server, id, 36, Slot::of(item("minecraft:ender_eye"), 1));
    alice.drain(QUIET);
    alice.send(&use_on(center.offset(2, 0, 0), 1, 1));
    alice.drain(QUIET);
    let open = server.handle.run(move |s| (-1..=1).all(|dx| (-1..=1).all(|dz| s.worlds[0].block_at(center.x + dx, center.y, center.z + dz).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:end_portal")))).unwrap();
    assert!(open, "the portal opens");

    // Into the End: onto the obsidian platform.
    alice.clear_backlog();
    alice.move_to(12.5, -60.0, 8.5);
    let respawn: play::Respawn = alice.find(TIMEOUT, |_: &play::Respawn| true).expect("into the end");
    assert_eq!(respawn.spawn.dimension_name, "minecraft:the_end");
    let arrived: play::PlayerPosition = alice.expect();
    assert_eq!((arrived.x, arrived.y, arrived.z), (100.5, 49.0, 0.5));
    let platform = server
        .handle
        .run(|s| {
            let end = s.world_of_dimension("minecraft:the_end").unwrap();
            (s.worlds[end].block_at(100, 48, 0).map(|b| blocks::block_of_state(b).name), s.worlds[end].block_at(100, 49, 0).map(blocks::is_air))
        })
        .unwrap();
    assert_eq!(platform, (Some("minecraft:obsidian"), Some(true)));

    // The exit portal (open, since the dragon fight is not implemented) rolls the credits.
    // Alice flies over to the centre of the island first (the test view distance is short).
    alice.move_to(4.5, 80.0, 0.5);
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("the island centre arrives");
    let exit_y = server
        .handle
        .run(|s| {
            let end = s.world_of_dimension("minecraft:the_end").unwrap();
            (40..90).find(|y| s.worlds[end].block_at(1, *y, 0).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:end_portal"))
        })
        .unwrap()
        .expect("exit portal");
    alice.clear_backlog();
    alice.move_to(1.5, exit_y as f64, 0.5);
    let win: play::GameEvent = alice.find(TIMEOUT, |g: &play::GameEvent| g.event == play::game_event::WIN_GAME).expect("credits");
    assert_eq!(win.value, 1.0, "first time: show the credits");
    alice.send(&play::ClientCommand { action: VarInt(0) });
    let home: play::Respawn = alice.find(TIMEOUT, |_: &play::Respawn| true).expect("home");
    assert_eq!(home.spawn.dimension_name, "minecraft:overworld");
    assert_eq!(home.data_kept, 3, "everything is kept after the credits");
    assert_eq!(world_of(&server, id), "minecraft:overworld");
    server.stop();
}

#[test]
fn buckets_scoop_up_and_pour_water() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = join(&server);
    let id = alice.entity_id;
    let source = BlockPos::new(10, -61, 8);
    server.handle.run(move |s| s.set_block_and_update(0, source, blocks::default_state("minecraft:water").unwrap())).unwrap();
    give(&server, id, 36, Slot::of(item("minecraft:bucket"), 1));
    alice.drain(QUIET);

    // Look at the water and right-click.
    let look = |target: (f64, f64, f64)| {
        let eye = (8.5, -60.0 + 1.62, 8.5);
        let (dx, dy, dz) = (target.0 - eye.0, target.1 - eye.1, target.2 - eye.2);
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        ((-dx).atan2(dz).to_degrees() as f32, (-dy / len).asin().to_degrees() as f32)
    };
    let (yaw, pitch) = look((10.5, -60.1, 8.5));
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(1), yaw, pitch });
    alice.drain(QUIET);
    let (held, gone) = server
        .handle
        .run(move |s| {
            let held = s.players[&id].inventory[36].0.as_ref().map(|i| items::get(i.item as u32).unwrap().name);
            (held, s.worlds[0].block_at(source.x, source.y, source.z).map(|b| blocks::block_of_state(b).name))
        })
        .unwrap();
    assert_eq!(held, Some("minecraft:water_bucket"));
    assert_ne!(gone, Some("minecraft:water"), "the source is scooped up");

    // Pour it onto the grass two blocks away.
    let (yaw, pitch) = look((8.5, -60.5, 10.5));
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(2), yaw, pitch });
    alice.drain(QUIET);
    let (held, poured) = server
        .handle
        .run(move |s| {
            let held = s.players[&id].inventory[36].0.as_ref().map(|i| items::get(i.item as u32).unwrap().name);
            (held, s.worlds[0].block_at(8, -60, 10).map(|b| blocks::block_of_state(b).name))
        })
        .unwrap();
    assert_eq!(held, Some("minecraft:bucket"));
    assert_eq!(poured, Some("minecraft:water"));
    server.stop();
}
