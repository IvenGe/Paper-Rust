//! Clients of older versions join through a translator. The client here speaks 26.1 or 26.2
//! with packet ids and block state ids taken from minecraft-data (an independent source of
//! the tables the translator uses).

mod common;

use common::*;
use paper_protocol::codec::{BlockPos, Reader, WriteExt};
use paper_protocol::frame::{FrameDecoder, FrameEncoder};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Packet ids of 26.1 and 26.2 (the same), from minecraft-data's 26.1 protocol.
mod ids {
    pub const LOGIN_HELLO: i32 = 0;
    pub const LOGIN_ACKNOWLEDGED: i32 = 3;
    pub const LOGIN_DISCONNECT: i32 = 0;
    pub const LOGIN_FINISHED: i32 = 2;
    pub const LOGIN_COMPRESSION: i32 = 3;
    pub const CONFIG_FINISH: i32 = 3;
    pub const CONFIG_REGISTRY_DATA: i32 = 7;
    pub const CONFIG_KNOWN_PACKS: i32 = 14;
    pub const CONFIG_DISCONNECT: i32 = 2;
    pub const CONFIG_FINISH_ACK: i32 = 3;
    pub const CONFIG_SELECT_KNOWN_PACKS: i32 = 7;
    pub const PLAY_LOGIN: i32 = 49;
    pub const PLAY_CHUNK: i32 = 45;
    pub const PLAY_BLOCK_UPDATE: i32 = 8;
    pub const PLAY_KEEP_ALIVE: i32 = 44;
    pub const PLAY_POSITION: i32 = 72;
    pub const PLAY_SYSTEM_CHAT: i32 = 121;
    pub const PLAY_CHUNK_BATCH_FINISHED: i32 = 11;
    pub const PLAY_SET_SLOT: i32 = 20;
    pub const PLAY_SET_PLAYER_INVENTORY: i32 = 108;
    pub const PLAY_ADD_ENTITY: i32 = 1;
    pub const PLAY_BLOCK_ENTITY_DATA: i32 = 6;
    pub const PLAY_ENTITY_DATA: i32 = 99;
    pub const SB_TELEPORT_CONFIRM: i32 = 0;
    pub const SB_KEEP_ALIVE: i32 = 28;
    pub const SB_CHUNK_BATCH_RECEIVED: i32 = 11;
    pub const SB_CHAT_COMMAND: i32 = 7;
    pub const SB_POSITION_LOOK: i32 = 31;
}

/// Ids of 26.1 (minecraft-data).
const BEDROCK_26_1: i32 = 85;
const DIAMOND_BLOCK_26_1: i32 = 5309;
const RED_BED_26_1: std::ops::RangeInclusive<i32> = 2155..=2170;
const BED_BLOCK_ENTITY_26_1: i32 = 25;
const DIAMOND_26_1: i32 = 899;
const ZOMBIE_26_1: i32 = 150;
const LEATHER_CHESTPLATE_26_1: i32 = 956;
const SHULKER_BOX_26_1: i32 = 582;
/// Data component types of 26.1.
const CUSTOM_NAME_26_1: i32 = 6;
const ENCHANTMENTS_26_1: i32 = 13;
const DYED_COLOR_26_1: i32 = 44;
const CONTAINER_26_1: i32 = 75;

struct OldClient {
    stream: TcpStream,
    enc: FrameEncoder,
    dec: FrameDecoder,
    /// Play packets not looked at yet.
    backlog: std::collections::VecDeque<(i32, Vec<u8>)>,
    registries: Vec<(String, usize)>,
    offered_packs: Vec<(String, String, String)>,
    position: (f64, f64, f64),
}

impl OldClient {
    fn connect(port: u16) -> OldClient {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        OldClient { stream, enc: FrameEncoder::new(4), dec: FrameDecoder::new(), backlog: Default::default(), registries: Vec::new(), offered_packs: Vec::new(), position: (0.0, 0.0, 0.0) }
    }

    fn send(&mut self, id: i32, body: &[u8]) {
        let mut payload = Vec::new();
        payload.put_varint(id);
        payload.extend_from_slice(body);
        let mut out = Vec::new();
        self.enc.encode(&payload, &mut out).unwrap();
        self.stream.write_all(&out).expect("write");
    }

    fn read(&mut self, deadline: Instant) -> Option<(i32, Vec<u8>)> {
        let mut buf = [0u8; 16384];
        loop {
            if let Some(frame) = self.dec.next_frame().expect("valid frame") {
                let mut r = Reader::new(&frame);
                let id = r.varint().unwrap();
                return Some((id, r.rest().to_vec()));
            }
            if Instant::now() >= deadline {
                return None;
            }
            match self.stream.read(&mut buf) {
                Ok(0) => return None,
                Ok(n) => self.dec.push(&buf[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => return None,
            }
        }
    }

    fn handshake(&mut self, protocol: i32, intent: i32) {
        let mut body = Vec::new();
        body.put_varint(protocol);
        body.put_string("localhost");
        body.put_u16(25565);
        body.put_varint(intent);
        self.send(0, &body);
    }

    fn status(port: u16, protocol: i32) -> serde_json::Value {
        let mut c = OldClient::connect(port);
        c.handshake(protocol, 1);
        c.send(0, &[]);
        let (id, body) = c.read(Instant::now() + TIMEOUT).expect("status");
        assert_eq!(id, 0);
        serde_json::from_str(&Reader::new(&body).string(1 << 20).unwrap()).unwrap()
    }

    /// Logs in and configures as a client of `protocol` (26.2 or 26.1); returns once the play
    /// login packet has arrived, with its body.
    fn join(port: u16, protocol: i32, name: &str) -> (OldClient, Vec<u8>) {
        let mut c = OldClient::connect(port);
        c.handshake(protocol, 2);
        let mut hello = Vec::new();
        hello.put_string(name);
        hello.put_uuid(uuid::Uuid::nil());
        c.send(ids::LOGIN_HELLO, &hello);
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let (id, body) = c.read(deadline).expect("login reply");
            match id {
                ids::LOGIN_COMPRESSION => {
                    let threshold = Reader::new(&body).varint().unwrap() as u32;
                    c.enc.set_compression(Some(threshold));
                    c.dec.set_compression(Some(threshold));
                }
                ids::LOGIN_FINISHED => {
                    let mut r = Reader::new(&body);
                    r.uuid().unwrap();
                    assert_eq!(r.string(16).unwrap(), name);
                    let properties = r.varint().unwrap();
                    for _ in 0..properties {
                        r.string(1024).unwrap();
                        r.string(1 << 15).unwrap();
                        r.option(|r| r.string(1024)).unwrap();
                    }
                    if protocol >= 776 {
                        r.uuid().expect("26.2 has a session id");
                    }
                    assert!(r.is_empty(), "login finished has {} bytes too many", r.remaining());
                    break;
                }
                ids::LOGIN_DISCONNECT => panic!("refused: {}", Reader::new(&body).string(1 << 16).unwrap()),
                other => panic!("unexpected login packet {other}"),
            }
        }
        c.send(ids::LOGIN_ACKNOWLEDGED, &[]);
        loop {
            let (id, body) = c.read(deadline).expect("configuration packet");
            match id {
                ids::CONFIG_KNOWN_PACKS => {
                    let mut r = Reader::new(&body);
                    c.offered_packs = r.list(64, |r| Ok((r.string(256)?, r.string(256)?, r.string(256)?))).unwrap();
                    let mine = if protocol >= 776 { "26.2" } else { "26.1" };
                    let mut reply = Vec::new();
                    reply.put_varint(1);
                    reply.put_string("minecraft");
                    reply.put_string("core");
                    reply.put_string(mine);
                    c.send(ids::CONFIG_SELECT_KNOWN_PACKS, &reply);
                }
                ids::CONFIG_REGISTRY_DATA => {
                    let mut r = Reader::new(&body);
                    let registry = r.string(256).unwrap();
                    let entries = r
                        .list(4096, |r| {
                            r.string(256)?;
                            if r.bool()? {
                                r.nbt()?;
                            }
                            Ok(())
                        })
                        .unwrap();
                    c.registries.push((registry, entries.len()));
                }
                ids::CONFIG_FINISH => {
                    c.send(ids::CONFIG_FINISH_ACK, &[]);
                    break;
                }
                ids::CONFIG_DISCONNECT => panic!("disconnected in configuration"),
                _ => {}
            }
        }
        loop {
            let (id, body) = c.next(TIMEOUT).expect("play login");
            if id == ids::PLAY_LOGIN {
                return (c, body);
            }
        }
    }

    /// The next play packet, after keep-alives, teleports and chunk batches are answered as
    /// a 26.1 client does.
    fn next(&mut self, timeout: Duration) -> Option<(i32, Vec<u8>)> {
        if let Some(p) = self.backlog.pop_front() {
            return Some(p);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let (id, body) = self.read(deadline)?;
            match id {
                ids::PLAY_KEEP_ALIVE => self.send(ids::SB_KEEP_ALIVE, &body),
                ids::PLAY_POSITION => {
                    let mut r = Reader::new(&body);
                    let teleport = r.varint().unwrap();
                    let (x, y, z) = (r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap());
                    self.position = (x, y, z);
                    let mut confirm = Vec::new();
                    confirm.put_varint(teleport);
                    self.send(ids::SB_TELEPORT_CONFIRM, &confirm);
                    self.move_to(x, y, z);
                    return Some((id, body));
                }
                ids::PLAY_CHUNK_BATCH_FINISHED => {
                    let mut ack = Vec::new();
                    ack.put_f32(20.0);
                    self.send(ids::SB_CHUNK_BATCH_RECEIVED, &ack);
                    return Some((id, body));
                }
                _ => return Some((id, body)),
            }
        }
    }

    fn find(&mut self, timeout: Duration, wanted: i32, pred: impl Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
        let mut i = 0;
        while i < self.backlog.len() {
            if self.backlog[i].0 == wanted && pred(&self.backlog[i].1) {
                return self.backlog.remove(i).map(|p| p.1);
            }
            i += 1;
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (id, body) = self.read_next(left)?;
            if id == wanted && pred(&body) {
                return Some(body);
            }
            self.backlog.push_back((id, body));
        }
    }

    fn read_next(&mut self, timeout: Duration) -> Option<(i32, Vec<u8>)> {
        let saved = std::mem::take(&mut self.backlog);
        let next = self.next(timeout);
        self.backlog = saved;
        next
    }

    fn move_to(&mut self, x: f64, y: f64, z: f64) {
        let mut body = Vec::new();
        body.put_f64(x);
        body.put_f64(y);
        body.put_f64(z);
        body.put_f32(0.0);
        body.put_f32(0.0);
        body.put_u8(1);
        self.send(ids::SB_POSITION_LOOK, &body);
    }

    fn command(&mut self, command: &str) {
        let mut body = Vec::new();
        body.put_string(command);
        self.send(ids::SB_CHAT_COMMAND, &body);
    }
}

/// Reads a 26.1/26.2 play login packet: the game modes are bytes, and 26.2 has an
/// online-mode flag that 26.1 lacks.
fn read_login(body: &[u8], protocol: i32) -> (i32, i8) {
    let mut r = Reader::new(body);
    let entity = r.i32().unwrap();
    r.bool().unwrap();
    r.list(64, |r| r.string(256)).unwrap();
    r.varint().unwrap();
    r.varint().unwrap();
    r.varint().unwrap();
    r.bool().unwrap();
    r.bool().unwrap();
    r.bool().unwrap();
    r.varint().unwrap();
    r.string(256).unwrap();
    r.i64().unwrap();
    let game_mode = r.i8().unwrap();
    let previous = r.i8().unwrap();
    assert_eq!(previous, -1, "no previous game mode");
    r.bool().unwrap();
    r.bool().unwrap();
    r.option(|r| Ok((r.string(256)?, r.position()?))).unwrap();
    r.varint().unwrap();
    r.varint().unwrap();
    if protocol >= 776 {
        r.bool().unwrap();
    }
    r.bool().unwrap();
    assert!(r.is_empty(), "the login packet has {} bytes too many", r.remaining());
    (entity, game_mode)
}

/// The palette entries of the bottom section of a 26.1/26.2 chunk packet.
fn bottom_palette(body: &[u8]) -> Vec<i32> {
    let mut r = Reader::new(body);
    r.i32().unwrap();
    r.i32().unwrap();
    r.list(16, |r| Ok((r.varint()?, r.list(1 << 12, |r| r.i64())?))).unwrap();
    let data = r.byte_array(1 << 22).unwrap();
    let mut s = Reader::new(data);
    s.i16().unwrap();
    s.i16().unwrap();
    let bits = s.u8().unwrap();
    if bits == 0 {
        return vec![s.varint().unwrap()];
    }
    assert!(bits <= 8, "a direct palette in the bottom section");
    s.list(256, |r| r.varint()).unwrap()
}

#[test]
fn the_server_list_shows_older_versions_as_compatible() {
    let server = start_server(|_| {});
    let status = OldClient::status(server.port(), 775);
    assert_eq!(status["version"]["protocol"], 775);
    assert!(status["version"]["name"].as_str().unwrap().contains("26.1"), "{}", status["version"]["name"]);
    // A version that cannot join (1.12.2) sees the server's own.
    let status = OldClient::status(server.port(), 340);
    assert_eq!(status["version"]["protocol"], paper_data::PROTOCOL_VERSION);
    server.stop();
}

#[test]
fn a_26_1_client_joins_and_sees_the_world_in_its_ids() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Creative);
    let (mut client, login) = OldClient::join(server.port(), 775, "Old");
    // Its own vanilla pack was offered, registries it lacks were left out.
    assert!(client.offered_packs.iter().any(|(ns, id, v)| ns == "minecraft" && id == "core" && v == "26.1"), "{:?}", client.offered_packs);
    let names: Vec<&str> = client.registries.iter().map(|(n, _)| n.as_str()).collect();
    assert!(!names.contains(&"minecraft:sulfur_cube_archetype") && !names.contains(&"minecraft:decorated_pot_pattern"), "{names:?}");
    let biomes = client.registries.iter().find(|(n, _)| n == "minecraft:worldgen/biome").map(|(_, c)| *c);
    assert_eq!(biomes, Some(65), "26.1's biomes");
    let (_, game_mode) = read_login(&login, 775);
    assert_eq!(game_mode, 1, "creative");

    // The world in 26.1's block states: bedrock is 85 there (88 in 26.3).
    let chunk = client.find(TIMEOUT, ids::PLAY_CHUNK, |_| true).expect("a chunk");
    let palette = bottom_palette(&chunk);
    assert!(palette.contains(&BEDROCK_26_1), "bottom palette {palette:?}");

    // Changes too.
    client.find(TIMEOUT, ids::PLAY_POSITION, |_| true).expect("the spawn position");
    std::thread::sleep(Duration::from_millis(300));
    let diamond = paper_data::blocks::default_state("minecraft:diamond_block").unwrap();
    server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(2, -60, 2), diamond)).unwrap();
    let update = client
        .find(TIMEOUT, ids::PLAY_BLOCK_UPDATE, |b| {
            let mut r = Reader::new(b);
            r.position().unwrap() == BlockPos::new(2, -60, 2)
        })
        .expect("the block update");
    let mut r = Reader::new(&update);
    r.position().unwrap();
    assert_eq!(r.varint().unwrap(), DIAMOND_BLOCK_26_1);

    // Beds come with the block entity 26.1 draws them from.
    let bed = paper_data::blocks::default_state("minecraft:red_bed").unwrap();
    server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(4, -60, 4), bed)).unwrap();
    let update = client.find(TIMEOUT, ids::PLAY_BLOCK_UPDATE, |b| Reader::new(b).position().unwrap() == BlockPos::new(4, -60, 4)).expect("the bed");
    let mut r = Reader::new(&update);
    r.position().unwrap();
    let state = r.varint().unwrap();
    assert!(RED_BED_26_1.contains(&state), "a red bed of 26.1: {state}");
    let entity = client.find(TIMEOUT, ids::PLAY_BLOCK_ENTITY_DATA, |b| Reader::new(b).position().unwrap() == BlockPos::new(4, -60, 4)).expect("the bed's block entity");
    let mut r = Reader::new(&entity);
    r.position().unwrap();
    assert_eq!(r.varint().unwrap(), BED_BLOCK_ENTITY_26_1);

    // Items by their 26.1 ids.
    let diamond = paper_data::items::id("minecraft:diamond").unwrap() as i32;
    server
        .handle
        .run(move |s| {
            let id = *s.players.keys().next().unwrap();
            s.give_item(id, diamond, 3)
        })
        .unwrap();
    let slot_item = |b: &[u8], player_inventory: bool| {
        let mut r = Reader::new(b);
        if player_inventory {
            r.varint().unwrap();
        } else {
            r.varint().unwrap();
            r.varint().unwrap();
            r.i16().unwrap();
        }
        let count = r.varint().unwrap();
        (count > 0).then(|| (r.varint().unwrap(), count))
    };
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let (id, body) = client.next(Duration::from_millis(200)).unwrap_or((-1, Vec::new()));
        let item = match id {
            ids::PLAY_SET_SLOT => slot_item(&body, false),
            ids::PLAY_SET_PLAYER_INVENTORY => slot_item(&body, true),
            _ => None,
        };
        if let Some((item, count)) = item {
            assert_eq!((item, count), (DIAMOND_26_1, 3));
            break;
        }
        assert!(Instant::now() < deadline, "no inventory update with the diamonds");
    }

    // Items with components: their types and the items inside them by 26.1's ids.
    let component = |name: &str| paper_data::registries::id("minecraft:data_component_type", name).unwrap() as i32;
    let item = |name: &str| paper_data::items::id(name).unwrap() as i32;
    let mut chestplate = paper_protocol::types::ItemStack::new(item("minecraft:leather_chestplate"), 1);
    chestplate.added.push((component("minecraft:dyed_color"), 0xFF0000i32.to_be_bytes().to_vec()));
    let mut name = Vec::new();
    paper_nbt::write_network(&mut name, &paper_nbt::Tag::String("Red".into()));
    chestplate.added.push((component("minecraft:custom_name"), name));
    let sharpness = paper_data::synced::id("minecraft:enchantment", "minecraft:sharpness").unwrap();
    let mut enchantments = Vec::new();
    enchantments.put_varint(1);
    enchantments.put_varint(sharpness);
    enchantments.put_varint(2);
    chestplate.added.push((component("minecraft:enchantments"), enchantments));
    let mut shulker = paper_protocol::types::ItemStack::new(item("minecraft:shulker_box"), 1);
    let mut contents = Vec::new();
    contents.put_varint(1);
    contents.put_bool(true);
    contents.put_varint(diamond);
    contents.put_varint(5);
    contents.put_varint(0);
    contents.put_varint(0);
    shulker.added.push((component("minecraft:container"), contents));
    server
        .handle
        .run(move |s| {
            let id = *s.players.keys().next().unwrap();
            s.add_to_inventory(id, &chestplate);
            s.add_to_inventory(id, &shulker);
        })
        .unwrap();
    // (item, [(component type, value bytes)]) of the 26.1 slots that arrive.
    let read_slot = |b: &[u8], player_inventory: bool| -> Option<(i32, Vec<(i32, Vec<u8>)>)> {
        let mut r = Reader::new(b);
        if player_inventory {
            r.varint().unwrap();
        } else {
            r.varint().unwrap();
            r.varint().unwrap();
            r.i16().unwrap();
        }
        if r.varint().unwrap() <= 0 {
            return None;
        }
        let item = r.varint().unwrap();
        let (added, removed) = (r.varint().unwrap(), r.varint().unwrap());
        assert_eq!(removed, 0);
        let mut components = Vec::new();
        for _ in 0..added {
            let ty = r.varint().unwrap();
            let before = r.remaining();
            let value: Vec<u8> = match ty {
                DYED_COLOR_26_1 => r.take(4).unwrap().to_vec(),
                CUSTOM_NAME_26_1 => {
                    let start = r.peek();
                    r.nbt().unwrap();
                    start[..before - r.remaining()].to_vec()
                }
                ENCHANTMENTS_26_1 => {
                    let start = r.peek();
                    r.list(16, |r| Ok((r.varint()?, r.varint()?))).unwrap();
                    start[..before - r.remaining()].to_vec()
                }
                CONTAINER_26_1 => {
                    let start = r.peek();
                    assert_eq!(r.varint().unwrap(), 1);
                    assert!(r.bool().unwrap());
                    assert_eq!(r.varint().unwrap(), DIAMOND_26_1, "the diamond inside, by its 26.1 id");
                    assert_eq!(r.varint().unwrap(), 5);
                    r.varint().unwrap();
                    r.varint().unwrap();
                    start[..before - r.remaining()].to_vec()
                }
                other => panic!("unexpected component type {other}"),
            };
            components.push((ty, value));
        }
        assert!(r.is_empty(), "{} bytes left after the slot", r.remaining());
        Some((item, components))
    };
    let (mut saw_chestplate, mut saw_shulker) = (false, false);
    let deadline = Instant::now() + TIMEOUT;
    while !(saw_chestplate && saw_shulker) {
        assert!(Instant::now() < deadline, "the items did not arrive (chestplate {saw_chestplate}, shulker box {saw_shulker})");
        let Some((id, body)) = client.next(Duration::from_millis(200)) else { continue };
        let slot = match id {
            ids::PLAY_SET_SLOT => read_slot(&body, false),
            ids::PLAY_SET_PLAYER_INVENTORY => read_slot(&body, true),
            _ => None,
        };
        match slot {
            Some((LEATHER_CHESTPLATE_26_1, components)) => {
                let types: Vec<i32> = components.iter().map(|(t, _)| *t).collect();
                assert_eq!(types, vec![DYED_COLOR_26_1, CUSTOM_NAME_26_1, ENCHANTMENTS_26_1]);
                assert_eq!(components[0].1, 0xFF0000i32.to_be_bytes());
                saw_chestplate = true;
            }
            Some((SHULKER_BOX_26_1, components)) => {
                assert_eq!(components.iter().map(|(t, _)| *t).collect::<Vec<_>>(), vec![CONTAINER_26_1]);
                saw_shulker = true;
            }
            _ => {}
        }
    }

    // Entities by their 26.1 types, with their data.
    let zombie = server
        .handle
        .run(|s| s.spawn_mob(0, "minecraft:zombie", paper_server::world::entity::Vec3::new(6.5, -60.0, 6.5), paper_server::server::mobs::SpawnReason::Command).unwrap())
        .unwrap();
    let spawn = client.find(TIMEOUT, ids::PLAY_ADD_ENTITY, |b| Reader::new(b).varint().unwrap() == zombie).expect("the zombie");
    let mut r = Reader::new(&spawn);
    r.varint().unwrap();
    r.uuid().unwrap();
    assert_eq!(r.varint().unwrap(), ZOMBIE_26_1);
    client.find(TIMEOUT, ids::PLAY_ENTITY_DATA, |b| Reader::new(b).varint().unwrap() == zombie).expect("the zombie's data");

    // Commands and chat work both ways.
    client.command("say hello from 26.1");
    let chat = client
        .find(TIMEOUT, ids::PLAY_SYSTEM_CHAT, |b| {
            let tag = Reader::new(b).nbt().unwrap();
            format!("{tag:?}").contains("hello from 26.1")
        })
        .expect("the /say message");
    assert!(!chat.is_empty());

    // Moving: the server sees where the client goes.
    let (x, y, z) = client.position;
    client.move_to(x + 1.5, y, z);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let pos = server.handle.run(|s| s.players.values().next().map(|p| (p.loc.x, p.loc.y, p.loc.z))).unwrap().expect("the player");
        if (pos.0 - (x + 1.5)).abs() < 0.01 {
            break;
        }
        assert!(Instant::now() < deadline, "the server did not see the move: {pos:?}");
        client.next(Duration::from_millis(50));
    }
    server.stop();
}

#[test]
fn a_26_2_client_joins() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Survival);
    let (mut client, login) = OldClient::join(server.port(), 776, "Newer");
    assert!(client.offered_packs.iter().any(|(_, id, v)| id == "core" && v == "26.2"));
    let names: Vec<&str> = client.registries.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"minecraft:sulfur_cube_archetype"), "26.2 has sulfur cubes");
    let (_, game_mode) = read_login(&login, 776);
    assert_eq!(game_mode, 0, "survival");
    client.find(TIMEOUT, ids::PLAY_CHUNK, |_| true).expect("a chunk");
    server.stop();
}
