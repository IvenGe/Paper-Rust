//! End-to-end tests: a real server instance on a loopback port, driven by a protocol client.

mod common;

use common::{decode, packet_id, start_server, Client, TIMEOUT};
use paper_data::blocks;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::{login, play, Packet};
use paper_protocol::types::Slot;
use paper_server::world::chunk::read_sections;
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(400);

fn biome_count() -> u32 {
    paper_data::synced::entries("minecraft:worldgen/biome").unwrap().len() as u32
}

/// Waits for the chunk packet of a specific chunk.
fn expect_chunk(client: &mut Client, x: i32, z: i32) -> play::LevelChunkWithLight {
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == x && c.z == z).unwrap_or_else(|| panic!("chunk {x},{z} never arrived"))
}

#[test]
fn status_ping() {
    let server = start_server(|c| {
        c.properties.motd = "§aHello §lTests".into();
        c.properties.max_players = 7;
    });
    let status = Client::status(server.port());
    assert_eq!(status["version"]["protocol"], paper_data::PROTOCOL_VERSION);
    assert_eq!(status["players"]["max"], 7);
    assert_eq!(status["players"]["online"], 0);
    assert_eq!(status["description"]["extra"][0]["text"], "Hello ");
    assert_eq!(status["description"]["extra"][0]["color"], "green");
    assert_eq!(status["description"]["extra"][1]["bold"], true);

    // The player count follows joins.
    let _alice = Client::join(server.port(), "Alice");
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let status = Client::status(server.port());
        if status["players"]["online"] == 1 {
            assert_eq!(status["players"]["sample"][0]["name"], "Alice");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "status never showed the player");
        std::thread::sleep(Duration::from_millis(50));
    }
    server.stop();
}

#[test]
fn join_flat_world() {
    let server = start_server(|_| {});
    let mut alice = Client::join(server.port(), "Alice");

    // Configuration phase.
    assert_eq!(alice.brand.as_deref(), Some("Paper-RS"));
    let registry_names: Vec<&str> = alice.registries.iter().map(|(r, _)| r.as_str()).collect();
    for required in ["minecraft:worldgen/biome", "minecraft:dimension_type", "minecraft:damage_type", "minecraft:chat_type", "minecraft:world_clock"] {
        assert!(registry_names.contains(&required), "missing registry {required}");
    }
    assert!(alice.tag_registries.contains(&"minecraft:block".to_owned()));
    assert!(alice.tag_registries.contains(&"minecraft:item".to_owned()));
    let profile = alice.profile.clone().unwrap();
    assert_eq!(profile.uuid, paper_server::net::auth::offline_uuid("Alice"));

    // Login packet.
    let login = alice.login.clone().unwrap();
    assert_eq!(login.spawn.dimension_name, "minecraft:overworld");
    assert_eq!(login.spawn.game_mode, 1, "test config is creative");
    assert!(login.spawn.is_flat);
    assert!(!login.online_mode);
    assert!(!login.enforces_secure_chat);
    assert_eq!(login.view_distance, VarInt(4));
    let dimension_types = &alice.registries.iter().find(|(r, _)| r == "minecraft:dimension_type").unwrap().1;
    assert_eq!(dimension_types[login.spawn.dimension_type as usize], "minecraft:overworld");

    // The spawn teleport, then the world around the player.
    let pos: play::PlayerPosition = alice.expect();
    assert_eq!((pos.x, pos.y, pos.z), (8.5, -60.0, 8.5));
    let chunk = expect_chunk(&mut alice, 0, 0);
    let sections = read_sections(&chunk.data, 24, biome_count()).expect("chunk sections decode");
    let grass = blocks::default_state("minecraft:grass_block").unwrap();
    let bedrock = blocks::default_state("minecraft:bedrock").unwrap();
    // Section 0 covers y -64..-48: bedrock, dirt, dirt, grass, then air.
    assert_eq!(sections[0].get(8, 0, 8), bedrock);
    assert_eq!(sections[0].get(8, 3, 8), grass);
    assert_eq!(sections[0].get(8, 4, 8), blocks::AIR);
    assert_eq!(sections[0].non_air, 4 * 256);
    assert!(sections[1..].iter().all(|s| s.is_empty()));
    // Heightmaps: world surface and motion blocking, 37 longs each at 9 bits.
    assert_eq!(chunk.heightmaps.len(), 2);
    assert!(chunk.heightmaps.iter().all(|(_, data)| data.len() == 37));
    // Light: 26 sections; sky light present from the terrain section upwards.
    assert_eq!(chunk.light.sky_mask.len(), 1);
    assert_eq!(chunk.light.sky_mask[0].count_ones() as usize, chunk.light.sky_arrays.len());
    assert!(chunk.light.sky_arrays.iter().all(|a| a.len() == 2048));
    assert!(chunk.light.block_arrays.is_empty());

    // Every chunk in view arrives (view distance 4), in batches the client acknowledges.
    alice.drain(QUIET);
    assert!(alice.has::<play::ChunkBatchStart>());
    assert!(alice.has::<play::ChunkBatchFinished>());
    assert!(alice.has::<play::Commands>());
    assert!(alice.has::<play::SetTime>());
    assert!(alice.has::<play::SetDefaultSpawnPosition>());
    assert!(alice.has::<play::PlayerInfoUpdate>());
    let event: play::GameEvent = alice.find(QUIET, |e: &play::GameEvent| e.event == play::game_event::START_WAITING_FOR_CHUNKS).expect("start waiting for chunks");
    assert_eq!(event.value, 0.0);
    let inventory: play::ContainerSetContent = alice.expect();
    assert_eq!(inventory.slots.len(), 46);
    let mut chunks = 0;
    while alice.try_expect::<play::LevelChunkWithLight>(Duration::from_millis(50)).is_some() {
        chunks += 1;
    }
    let expected = paper_server::server::player::ChunkView::area(paper_server::world::chunk::ChunkPos::new(0, 0), 4).count() - 1;
    assert_eq!(chunks, expected, "all chunks in view were sent exactly once");

    // The world state on the server agrees.
    let (players, loaded) = server.handle.run(|s| (s.players.len(), s.worlds[0].loaded_chunk_count())).unwrap();
    assert_eq!(players, 1);
    assert!(loaded >= expected + 1);
    server.stop();
}

#[test]
fn join_without_compression() {
    let server = start_server(|c| c.properties.network_compression_threshold = -1);
    let mut alice = Client::join(server.port(), "Alice");
    expect_chunk(&mut alice, 0, 0);
    server.stop();
}

#[test]
fn wrong_protocol_version_is_refused() {
    let server = start_server(|_| {});
    for (version, key) in [(paper_data::PROTOCOL_VERSION - 1, "multiplayer.disconnect.outdated_client"), (paper_data::PROTOCOL_VERSION + 1, "multiplayer.disconnect.incompatible")] {
        let (mut client, frame) = Client::begin_login(server.port(), version, "Old");
        let frame = frame.expect("a disconnect packet");
        assert_eq!(packet_id(&frame), login::LoginDisconnect::ID);
        let disconnect: login::LoginDisconnect = decode(&frame);
        assert_eq!(disconnect.reason.0 .0["translate"], key);
        assert_eq!(disconnect.reason.0 .0["with"][0]["text"], paper_data::VERSION_NAME);
        assert!(client.wait_closed(TIMEOUT));
    }
    // Names with illegal characters are refused too.
    let (_, frame) = Client::begin_login(server.port(), paper_data::PROTOCOL_VERSION, "bad name");
    assert_eq!(packet_id(&frame.unwrap()), login::LoginDisconnect::ID);
    assert_eq!(server.handle.run(|s| s.players.len()), Some(0));
    server.stop();
}

#[test]
fn players_see_each_other() {
    let server = start_server(|_| {});
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    alice.clear_backlog();

    let mut bob = Client::join(server.port(), "Bob");
    bob.expect::<play::PlayerPosition>();

    // Alice learns about Bob: player list entry, join message, entity.
    let info: play::PlayerInfoUpdate = alice.expect();
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].name, "Bob");
    assert_eq!(info.entries[0].game_mode, 1);
    let joined: play::SystemChat = alice.expect();
    assert_eq!(joined.content.to_plain(), "Bob joined the game");
    let spawn: play::AddEntity = alice.expect();
    assert_eq!(spawn.entity_id.0, bob.entity_id);
    assert_eq!(spawn.uuid, bob.profile.as_ref().unwrap().uuid);
    assert_eq!(spawn.entity_type.0, paper_data::registries::id("minecraft:entity_type", "minecraft:player").unwrap() as i32);
    assert_eq!((spawn.x, spawn.y, spawn.z), (8.5, -60.0, 8.5));
    let metadata: play::SetEntityData = alice.expect();
    assert_eq!(metadata.entity_id.0, bob.entity_id);
    assert_eq!(*metadata.metadata.0.last().unwrap(), 0xFF);

    // Bob's player list already contains both, and he sees Alice.
    let info: play::PlayerInfoUpdate = bob.expect();
    let mut names: Vec<&str> = info.entries.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Alice", "Bob"]);
    let spawn: play::AddEntity = bob.expect();
    assert_eq!(spawn.entity_id.0, alice.entity_id);

    // The first movement packet sets Bob on the ground; a change of the on-ground state is
    // sent as an absolute position, as vanilla does.
    alice.clear_backlog();
    bob.move_to(8.5, -60.0, 8.5);
    let sync: play::EntityPositionSync = alice.find(TIMEOUT, |s: &play::EntityPositionSync| s.entity_id == bob.entity_id && s.on_ground).expect("on-ground sync");
    assert_eq!((sync.x, sync.y, sync.z), (8.5, -60.0, 8.5));

    // Movement: a small step is a relative move, with the head following.
    bob.move_to(9.5, -60.0, 8.5);
    let moved: play::MoveEntityPos = alice.find(TIMEOUT, |m: &play::MoveEntityPos| m.entity_id == bob.entity_id).expect("relative move");
    assert_eq!((moved.dx, moved.dy, moved.dz), (4096, 0, 0));
    assert!(moved.on_ground);
    // A long jump (more than 8 blocks) is sent as an absolute position.
    bob.move_to(30.5, -60.0, 8.5);
    let sync: play::EntityPositionSync = alice.find(TIMEOUT, |s: &play::EntityPositionSync| s.entity_id == bob.entity_id && s.x == 30.5).expect("absolute move");
    assert_eq!((sync.y, sync.z), (-60.0, 8.5));
    // Rotation.
    bob.send(&play::MovePlayerRot { yaw: 90.0, pitch: 10.0, flags: 1 });
    let rot: play::MoveEntityRot = alice.find(TIMEOUT, |r: &play::MoveEntityRot| r.entity_id.0 == bob.entity_id).expect("rotation");
    assert_eq!(rot.yaw.0, 90.0);
    let head: play::RotateHead = alice.find(TIMEOUT, |h: &play::RotateHead| h.entity_id.0 == bob.entity_id && h.head_yaw.0 == 90.0).expect("head rotation");
    assert_eq!(head.head_yaw.0, 90.0);

    // Sneaking shows up as metadata.
    bob.send(&play::PlayerInput { flags: play::input_flags::SNEAK });
    let meta: play::SetEntityData = alice.find(TIMEOUT, |m: &play::SetEntityData| m.entity_id.0 == bob.entity_id).expect("sneak metadata");
    assert_eq!(meta.metadata.0[..3], [0, 0, 0x02], "flags byte has the sneaking bit");

    // Chat reaches everyone, including the sender.
    bob.chat("hello   world");
    for client in [&mut alice, &mut bob] {
        let chat: play::SystemChat = client.find(TIMEOUT, |c: &play::SystemChat| c.content.0["translate"] == "chat.type.text").expect("chat line");
        assert_eq!(chat.content.to_plain(), "<Bob> hello world");
        assert!(!chat.overlay);
    }

    // Bob leaves: Alice is told to forget him.
    drop(bob);
    let removed: play::RemoveEntities = alice.expect();
    assert_eq!(removed.entity_ids.len(), 1);
    let info: play::PlayerInfoRemove = alice.expect();
    assert_eq!(info.uuids.len(), 1);
    let left: play::SystemChat = alice.find(TIMEOUT, |c: &play::SystemChat| c.content.0["translate"] == "multiplayer.player.left").expect("leave message");
    assert_eq!(left.content.to_plain(), "Bob left the game");
    assert_eq!(server.handle.run(|s| s.players.len()), Some(1));
    server.stop();
}

#[test]
fn far_apart_players_do_not_see_each_other() {
    let server = start_server(|_| {});
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    let mut bob = Client::join(server.port(), "Bob");
    bob.expect::<play::PlayerPosition>();
    alice.expect::<play::AddEntity>();
    expect_chunk(&mut bob, 0, 0);

    // Bob walks far away: Alice's client is told to remove him; coming back re-adds him.
    alice.clear_backlog();
    bob.move_to(1000.5, -60.0, 8.5);
    let removed: play::RemoveEntities = alice.expect();
    assert_eq!(removed.entity_ids, vec![VarInt(bob.entity_id)]);
    // Bob's view follows him: new centre and chunks around it.
    let center: play::SetChunkCacheCenter = bob.find(TIMEOUT, |c: &play::SetChunkCacheCenter| c.x.0 == 62).expect("view recentred");
    assert_eq!(center.z.0, 0);
    expect_chunk(&mut bob, 62, 0);
    assert!(bob.find(TIMEOUT, |f: &play::ForgetLevelChunk| f.x == 0 && f.z == 0).is_some(), "old chunks are unloaded");

    bob.move_to(8.5, -60.0, 8.5);
    let spawn: play::AddEntity = alice.expect();
    assert_eq!(spawn.entity_id.0, bob.entity_id);
    assert_eq!(spawn.x, 8.5);
    server.stop();
}

#[test]
fn commands_and_permissions() {
    let server = start_server(|_| {});
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    alice.clear_backlog();

    // Not an operator: operator commands are unknown, followed by a line pointing at the
    // offending input.
    alice.command("gamemode survival");
    let reply: play::SystemChat = alice.expect();
    assert_eq!(reply.content.0["translate"], "command.unknown.command");
    let context: play::SystemChat = alice.expect();
    assert_eq!(context.content.to_plain(), "gamemode survival<--[HERE]");
    alice.clear_backlog();

    // Everyone may use /list.
    alice.command("list");
    let reply: play::SystemChat = alice.expect();
    assert_eq!(reply.content.to_plain(), "There are 1 of a max of 20 players online: Alice");

    // The console makes Alice an operator; her client gets the new permission level and
    // command list.
    server.handle.console_command("op Alice");
    let event: play::EntityEvent = alice.expect();
    assert_eq!((event.entity_id, event.status), (alice.entity_id, 28));
    alice.drain(QUIET);
    assert!(alice.has::<play::Commands>(), "the command graph is resent for the new permission level");
    assert_eq!(server.handle.run(|s| s.players.values().next().unwrap().op_level), Some(4));
    alice.clear_backlog();

    alice.command("gamemode survival");
    let event: play::GameEvent = alice.expect();
    assert_eq!((event.event, event.value), (play::game_event::CHANGE_GAME_MODE, 0.0));
    let abilities: play::PlayerAbilities = alice.expect();
    assert_eq!(abilities.flags & play::abilities::ALLOW_FLYING, 0);
    let reply: play::SystemChat = alice.expect();
    assert_eq!(reply.content.to_plain(), "Set own game mode to Survival Mode");

    // /give fills the hotbar first.
    alice.command("give Alice stone 70");
    let stone = paper_data::items::id("minecraft:stone").unwrap() as i32;
    let first: play::ContainerSetSlot = alice.expect();
    assert_eq!((first.slot, first.item.clone()), (36, Slot::of(stone, 64)));
    let second: play::ContainerSetSlot = alice.expect();
    assert_eq!((second.slot, second.item), (37, Slot::of(stone, 6)));

    // /tp to coordinates teleports and re-centres the view.
    alice.command("tp 100 -60 100");
    let pos: play::PlayerPosition = alice.find(TIMEOUT, |p: &play::PlayerPosition| p.x == 100.5).expect("teleport");
    assert_eq!((pos.y, pos.z), (-60.0, 100.5));
    let center: play::SetChunkCacheCenter = alice.expect();
    assert_eq!((center.x.0, center.z.0), (6, 6));

    // /time set is broadcast.
    alice.clear_backlog();
    alice.command("time set noon");
    let time: play::SetTime = alice.find(TIMEOUT, |t: &play::SetTime| t.clocks.first().is_some_and(|c| c.time == 6000)).expect("time update");
    assert_eq!(time.clocks.len(), 1);

    // Tab completion.
    alice.send(&play::CommandSuggestion { transaction_id: VarInt(7), text: "/gamemode c".into() });
    let suggestions: play::CommandSuggestions = alice.expect();
    assert_eq!(suggestions.transaction_id, 7);
    assert_eq!((suggestions.start, suggestions.length), (10, 1));
    assert_eq!(suggestions.matches, vec![("creative".to_owned(), None)]);
    alice.send(&play::CommandSuggestion { transaction_id: VarInt(8), text: "/kick ".into() });
    let suggestions: play::CommandSuggestions = alice.expect();
    assert_eq!(suggestions.matches, vec![("Alice".to_owned(), None)]);

    // /kick closes the connection with the reason.
    server.handle.console_command("kick Alice Too curious");
    let disconnect: play::Disconnect = alice.expect();
    assert_eq!(disconnect.reason.to_plain(), "Too curious");
    assert!(alice.wait_closed(TIMEOUT));
    assert_eq!(server.handle.run(|s| s.players.len()), Some(0));
    server.stop();
}

#[test]
fn breaking_and_placing_blocks() {
    let server = start_server(|_| {});
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    let mut bob = Client::join(server.port(), "Bob");
    bob.expect::<play::PlayerPosition>();
    expect_chunk(&mut alice, 0, 0);
    expect_chunk(&mut bob, 0, 0);
    alice.drain(QUIET);
    bob.drain(QUIET);
    alice.clear_backlog();
    bob.clear_backlog();

    let grass = blocks::default_state("minecraft:grass_block").unwrap();
    let target = BlockPos::new(9, -61, 9);

    // Creative: starting to dig breaks instantly.
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: target, face: 1, sequence: VarInt(5) });
    let update: play::BlockUpdate = alice.expect();
    assert_eq!((update.pos, update.state.0), (target, blocks::AIR as i32));
    let ack: play::BlockChangedAck = alice.expect();
    assert_eq!(ack.sequence.0, 5);
    // Bob sees the block vanish, with the break effect for the old block.
    let update: play::BlockUpdate = bob.expect();
    assert_eq!((update.pos, update.state.0), (target, blocks::AIR as i32));
    let effect: play::LevelEvent = bob.expect();
    assert_eq!((effect.event, effect.pos, effect.data), (2001, target, grass as i32));
    assert!(!alice.has::<play::LevelEvent>(), "the breaker plays the effect itself");
    assert_eq!(server.handle.run(move |s| s.worlds[0].block_at(9, -61, 9)), Some(Some(blocks::AIR)));

    // Out of reach: refused, and the client is told the block is still there.
    let far = BlockPos::new(60, -61, 9);
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: far, face: 1, sequence: VarInt(6) });
    let ack: play::BlockChangedAck = alice.expect();
    assert_eq!(ack.sequence.0, 6);
    assert_eq!(server.handle.run(move |s| s.worlds[0].block_at(60, -61, 9)), Some(Some(grass)));

    // Placing: put oak stairs in the hotbar (creative) and use them on the ground.
    alice.clear_backlog();
    bob.clear_backlog();
    let stairs_item = paper_data::items::id("minecraft:oak_stairs").unwrap() as i32;
    alice.send(&play::SetCreativeModeSlot { slot: 36, item: Slot::of(stairs_item, 1) });
    // Bob sees the item appear in Alice's hand.
    let equipment: play::SetEquipment = bob.expect();
    assert_eq!(equipment.entity_id, alice.entity_id);
    assert_eq!(equipment.equipment[0], (play::equipment_slot::MAIN_HAND, Slot::of(stairs_item, 1)));

    // Alice stands at 8.5,-60,8.5 looking south (yaw 0); click the top of the block at 8,-61,10.
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: BlockPos::new(8, -61, 10), face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(7) });
    let placed_at = BlockPos::new(8, -60, 10);
    let update: play::BlockUpdate = alice.expect();
    assert_eq!(update.pos, placed_at);
    let state = update.state.0 as u32;
    assert_eq!(blocks::block_of_state(state).name, "minecraft:oak_stairs");
    assert_eq!(blocks::property(state, "facing"), Some("south"));
    assert_eq!(blocks::property(state, "half"), Some("bottom"));
    let ack: play::BlockChangedAck = alice.expect();
    assert_eq!(ack.sequence.0, 7);
    let update: play::BlockUpdate = bob.expect();
    assert_eq!((update.pos, update.state.0 as u32), (placed_at, state));
    let swing: play::SwingAnimation = bob.expect();
    assert_eq!(swing.entity_id.0, alice.entity_id);

    // Placing a full block changes sky light below it; both clients get a light update
    // for the chunk.
    alice.clear_backlog();
    let stone_item = paper_data::items::id("minecraft:stone").unwrap() as i32;
    alice.send(&play::SetCreativeModeSlot { slot: 36, item: Slot::of(stone_item, 1) });
    // Build a 1-block overhang two blocks above the ground at 12,-58,12 via the console.
    server.handle.console_command("setblock 12 -58 12 minecraft:stone");
    let update: play::BlockUpdate = alice.find(TIMEOUT, |u: &play::BlockUpdate| u.pos == BlockPos::new(12, -58, 12)).expect("setblock update");
    assert_eq!(blocks::block_of_state(update.state.0 as u32).name, "minecraft:stone");
    let light: play::LightUpdate = alice.expect();
    assert_eq!((light.x.0, light.z.0), (0, 0));
    assert!(!light.light.sky_arrays.is_empty());
    let (below, above) = server.handle.run(|s| {
        let chunk = &s.worlds[0].chunks[&paper_server::world::chunk::ChunkPos::new(0, 0)];
        (chunk.sky_light_at(12, -59, 12), chunk.sky_light_at(12, -57, 12))
    }).unwrap();
    assert_eq!(below, 14, "shadowed cell is lit from its neighbours");
    assert_eq!(above, 15);

    // A player cannot place a block inside another player.
    bob.clear_backlog();
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: BlockPos::new(8, -61, 8), face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(8) });
    let ack: play::BlockChangedAck = alice.find(TIMEOUT, |a: &play::BlockChangedAck| a.sequence.0 == 8).expect("ack");
    assert_eq!(ack.sequence.0, 8);
    assert_eq!(server.handle.run(|s| s.worlds[0].block_at(8, -60, 8)), Some(Some(blocks::AIR)));
    server.stop();
}

#[test]
fn duplicate_login_replaces_the_first_session() {
    let server = start_server(|_| {});
    let mut first = Client::join(server.port(), "Alice");
    first.expect::<play::PlayerPosition>();
    let mut second = Client::join(server.port(), "Alice");
    second.expect::<play::PlayerPosition>();
    let disconnect: play::Disconnect = first.expect();
    assert_eq!(disconnect.reason.0["translate"], "multiplayer.disconnect.duplicate_login");
    assert!(first.wait_closed(TIMEOUT));
    assert_eq!(server.handle.run(|s| s.players.len()), Some(1));
    server.stop();
}

#[test]
fn server_full_and_whitelist() {
    let server = start_server(|c| c.properties.max_players = 1);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    let (_, frame) = Client::begin_login(server.port(), paper_data::PROTOCOL_VERSION, "Bob");
    let disconnect: login::LoginDisconnect = decode(&frame.unwrap());
    assert_eq!(disconnect.reason.0 .0["translate"], "multiplayer.disconnect.server_full");
    server.stop();

    let server = start_server(|c| c.properties.white_list = true);
    let (_, frame) = Client::begin_login(server.port(), paper_data::PROTOCOL_VERSION, "Bob");
    let disconnect: login::LoginDisconnect = decode(&frame.unwrap());
    assert_eq!(disconnect.reason.0 .0["translate"], "multiplayer.disconnect.not_whitelisted");
    server.handle.console_command("whitelist add Bob");
    server.handle.run(|_| ()).unwrap();
    let mut bob = Client::join(server.port(), "Bob");
    bob.expect::<play::PlayerPosition>();
    server.stop();
}

#[test]
fn shutdown_disconnects_players() {
    let server = start_server(|_| {});
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    server.handle.console_command("stop");
    let disconnect: play::Disconnect = alice.expect();
    assert_eq!(disconnect.reason.0["translate"], "multiplayer.disconnect.server_shutdown");
    assert!(alice.wait_closed(TIMEOUT));
    server.wait();
}
