//! 1.7 clients join through the translators: Netty's framing without compression, entity ids
//! as ints, positions as three numbers, chunks as separate id and metadata arrays, items with
//! gzipped NBT, and none of 1.8's blocks, items or mobs.

mod common;

use common::*;
use paper_protocol::codec::{BlockPos, Reader, WriteExt};
use paper_protocol::frame::{FrameDecoder, FrameEncoder};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// 1.7's packet ids (minecraft-data).
mod ids {
    pub const LOGIN_START: i32 = 0x00;
    pub const LOGIN_SUCCESS: i32 = 0x02;
    pub const LOGIN_DISCONNECT: i32 = 0x00;
    pub const KEEP_ALIVE: i32 = 0x00;
    pub const JOIN_GAME: i32 = 0x01;
    pub const CHAT: i32 = 0x02;
    pub const POSITION: i32 = 0x08;
    pub const SPAWN_PLAYER: i32 = 0x0C;
    pub const SPAWN_MOB: i32 = 0x0F;
    pub const MAP_CHUNK: i32 = 0x21;
    pub const BLOCK_CHANGE: i32 = 0x23;
    pub const SET_SLOT: i32 = 0x2F;
    pub const PLAYER_INFO: i32 = 0x38;
    pub const SB_KEEP_ALIVE: i32 = 0x00;
    pub const SB_CHAT: i32 = 0x01;
    pub const SB_POSITION: i32 = 0x04;
    pub const SB_POSITION_LOOK: i32 = 0x06;
}

struct Client17 {
    stream: TcpStream,
    enc: FrameEncoder,
    dec: FrameDecoder,
    backlog: VecDeque<(i32, Vec<u8>)>,
    /// Where the server last put the player (feet).
    position: (f64, f64, f64),
}

impl Client17 {
    fn connect(port: u16) -> Client17 {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        Client17 { stream, enc: FrameEncoder::new(4), dec: FrameDecoder::new(), backlog: VecDeque::new(), position: (0.0, 0.0, 0.0) }
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
            if let Some(frame) = self.dec.next_frame().expect("an uncompressed frame") {
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

    /// Logs in as a 1.7.10 client; returns once the join game packet arrived, with its body.
    fn join(port: u16, name: &str) -> (Client17, Vec<u8>) {
        let mut c = Client17::connect(port);
        let mut hs = Vec::new();
        hs.put_varint(5);
        hs.put_string("localhost");
        hs.put_u16(25565);
        hs.put_varint(2);
        c.send(0, &hs);
        let mut start = Vec::new();
        start.put_string(name);
        c.send(ids::LOGIN_START, &start);
        let deadline = Instant::now() + TIMEOUT;
        let (id, body) = c.read(deadline).expect("the login's answer");
        match id {
            ids::LOGIN_SUCCESS => {
                let mut r = Reader::new(&body);
                assert_eq!(r.string(36).unwrap().len(), 36, "1.7.6 on: a UUID with dashes");
                assert_eq!(r.string(16).unwrap(), name);
                assert!(r.is_empty());
            }
            ids::LOGIN_DISCONNECT => panic!("refused: {}", Reader::new(&body).string(1 << 16).unwrap()),
            other => panic!("login packet {other} (1.7 has no compression)"),
        }
        loop {
            let (id, body) = c.next(TIMEOUT).expect("join game");
            if id == ids::JOIN_GAME {
                return (c, body);
            }
        }
    }

    /// The next packet, after keep-alives and teleports are answered as 1.7 does.
    fn next(&mut self, timeout: Duration) -> Option<(i32, Vec<u8>)> {
        if let Some(p) = self.backlog.pop_front() {
            return Some(p);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let (id, body) = self.read(deadline)?;
            match id {
                ids::KEEP_ALIVE => self.send(ids::SB_KEEP_ALIVE, &body),
                ids::POSITION => {
                    let mut r = Reader::new(&body);
                    let (x, eyes, z) = (r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap());
                    self.position = (x, eyes - 1.62, z);
                    let mut back = Vec::new();
                    back.put_f64(x);
                    back.put_f64(eyes - 1.62);
                    back.put_f64(eyes);
                    back.put_f64(z);
                    back.put_f32(0.0);
                    back.put_f32(0.0);
                    back.put_bool(true);
                    self.send(ids::SB_POSITION_LOOK, &back);
                    return Some((id, body));
                }
                _ => return Some((id, body)),
            }
        }
    }

    fn find(&mut self, timeout: Duration, wanted: i32, pred: impl Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
        if let Some(i) = self.backlog.iter().position(|(id, b)| *id == wanted && pred(b)) {
            return self.backlog.remove(i).map(|p| p.1);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let saved = std::mem::take(&mut self.backlog);
            let next = self.next(left);
            self.backlog = saved;
            let (id, body) = next?;
            if id == wanted && pred(&body) {
                return Some(body);
            }
            self.backlog.push_back((id, body));
        }
    }
}

/// The block at a position of a 1.7 chunk packet (id, metadata), if its section was sent.
fn block_in_chunk(body: &[u8], x: i32, y: i32, z: i32) -> Option<(u8, u8)> {
    let mut r = Reader::new(body);
    r.i32().unwrap();
    r.i32().unwrap();
    assert!(r.bool().unwrap(), "a full chunk");
    let mask = r.u16().unwrap();
    assert_eq!(r.u16().unwrap(), 0, "no block ids above 255");
    let len = r.i32().unwrap() as usize;
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(r.take(len).unwrap()).read_to_end(&mut raw).unwrap();
    let sections = mask.count_ones() as usize;
    assert_eq!(raw.len(), sections * (4096 + 3 * 2048) + 256);
    let section = y / 16;
    if mask & (1 << section) == 0 {
        return None;
    }
    let before = (0..section).filter(|s| mask & (1 << s) != 0).count();
    let index = ((y & 15) * 256 + (z & 15) * 16 + (x & 15)) as usize;
    let at = before * 4096 + index;
    Some((raw[at], raw[sections * 4096 + at / 2] >> (4 * (at % 2)) & 15))
}

#[test]
fn the_server_list_shows_1_7_as_compatible() {
    let server = start_server(|_| {});
    let mut c = Client17::connect(server.port());
    let mut hs = Vec::new();
    hs.put_varint(5);
    hs.put_string("localhost");
    hs.put_u16(25565);
    hs.put_varint(1);
    c.send(0, &hs);
    c.send(0, &[]);
    let (_, body) = c.read(Instant::now() + TIMEOUT).expect("status");
    let status: serde_json::Value = serde_json::from_str(&Reader::new(&body).string(1 << 20).unwrap()).unwrap();
    assert_eq!(status["version"]["protocol"], 5);
    server.stop();
}

#[test]
fn a_1_7_client_joins_and_plays() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Creative);
    // Blocks where 1.7 can see them (its world starts at y 0): a diamond block, granite (stone
    // to 1.7) and stained glass (which 1.7 has).
    let state = |name: &str| paper_data::blocks::default_state(name).unwrap();
    let (diamond, granite, glass) = (state("minecraft:diamond_block"), state("minecraft:granite"), state("minecraft:red_stained_glass"));
    server
        .handle
        .run(move |s| {
            s.load_chunk_now(0, paper_server::world::chunk::ChunkPos { x: 0, z: 0 });
            assert!(s.set_block_and_update(0, BlockPos::new(3, 70, 3), diamond));
            assert!(s.set_block_and_update(0, BlockPos::new(3, 71, 3), granite));
            assert!(s.set_block_and_update(0, BlockPos::new(3, 72, 3), glass));
        })
        .unwrap();
    let (mut client, join) = Client17::join(server.port(), "Old17");
    let mut r = Reader::new(&join);
    assert!(r.i32().unwrap() > 0);
    assert_eq!(r.u8().unwrap(), 1, "creative");
    assert_eq!(r.i8().unwrap(), 0, "the overworld");
    r.u8().unwrap();
    r.u8().unwrap();
    assert_eq!(r.string(16).unwrap(), "flat");
    assert!(r.is_empty(), "no reduced debug info in 1.7");

    let chunk = client
        .find(TIMEOUT, ids::MAP_CHUNK, |b| {
            let mut r = Reader::new(b);
            (r.i32().unwrap(), r.i32().unwrap()) == (0, 0)
        })
        .expect("the spawn chunk");
    assert_eq!(block_in_chunk(&chunk, 3, 70, 3), Some((57, 0)), "a diamond block");
    assert_eq!(block_in_chunk(&chunk, 3, 71, 3), Some((1, 0)), "granite as stone");
    assert_eq!(block_in_chunk(&chunk, 3, 72, 3), Some((95, 14)), "red stained glass");

    // Block changes: a spruce fence (1.8) is a fence.
    client.find(TIMEOUT, ids::POSITION, |_| true).expect("the spawn position");
    std::thread::sleep(Duration::from_millis(300));
    let fence = state("minecraft:spruce_fence");
    server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(4, 70, 4), fence)).unwrap();
    let change = client
        .find(TIMEOUT, ids::BLOCK_CHANGE, |b| {
            let mut r = Reader::new(b);
            (r.i32().unwrap(), r.u8().unwrap(), r.i32().unwrap()) == (4, 70, 4)
        })
        .expect("the block change");
    let mut r = Reader::new(&change[9..]);
    assert_eq!((r.varint().unwrap(), r.u8().unwrap()), (85, 0), "a fence");

    // Chat both ways, as JSON without a position.
    let mut chat = Vec::new();
    chat.put_string("/say hello from 1.7");
    client.send(ids::SB_CHAT, &chat);
    client.find(TIMEOUT, ids::CHAT, |b| Reader::new(b).string(32767).unwrap().contains("hello from 1.7")).expect("the /say message");

    // Mobs: a horse stays a horse (100), a rabbit (1.8) is a chicken.
    for (kind, legacy) in [("minecraft:horse", 100), ("minecraft:rabbit", 93)] {
        let mob = server
            .handle
            .run(move |s| s.spawn_mob(0, kind, paper_server::world::entity::Vec3::new(6.5, -60.0, 6.5), paper_server::server::mobs::SpawnReason::Command).unwrap())
            .unwrap();
        let spawn = client.find(TIMEOUT, ids::SPAWN_MOB, |b| Reader::new(b).varint().unwrap() == mob).unwrap_or_else(|| panic!("the {kind}"));
        let mut r = Reader::new(&spawn);
        r.varint().unwrap();
        assert_eq!(r.u8().unwrap(), legacy, "{kind}");
    }

    // Items: a lead stays (420), an armor stand (1.8) is a stick.
    for (item, legacy) in [("minecraft:lead", 420), ("minecraft:armor_stand", 280)] {
        let id = paper_data::items::id(item).unwrap() as i32;
        server
            .handle
            .run(move |s| {
                let player = *s.players.keys().next().unwrap();
                s.give_item(player, id, 2)
            })
            .unwrap();
        let slot = client
            .find(TIMEOUT, ids::SET_SLOT, |b| {
                let mut r = Reader::new(b);
                r.i8().unwrap();
                r.i16().unwrap();
                r.i16().unwrap() == legacy
            })
            .unwrap_or_else(|| panic!("the {item}"));
        let mut r = Reader::new(&slot);
        assert_eq!(r.i8().unwrap(), 0);
        r.i16().unwrap();
        r.i16().unwrap();
        assert_eq!(r.i8().unwrap(), 2);
        r.i16().unwrap();
        assert_eq!(r.i16().unwrap(), -1, "no NBT");
    }

    // A second 1.7 player: listed by name, spawned with its name and UUID.
    let (mut other, _) = Client17::join(server.port(), "Other17");
    client.find(TIMEOUT, ids::PLAYER_INFO, |b| Reader::new(b).string(16).unwrap() == "Other17").expect("the tab list entry");
    let spawn = client
        .find(TIMEOUT, ids::SPAWN_PLAYER, |b| {
            let mut r = Reader::new(b);
            r.varint().unwrap();
            r.string(36).unwrap();
            r.string(16).unwrap() == "Other17"
        })
        .expect("the other player");
    let mut r = Reader::new(&spawn);
    r.varint().unwrap();
    assert_eq!(r.string(36).unwrap().len(), 36);
    other.next(Duration::from_millis(100));

    // Moving: the server sees the feet.
    let (x, y, z) = client.position;
    let mut moved = Vec::new();
    moved.put_f64(x + 1.5);
    moved.put_f64(y);
    moved.put_f64(y + 1.62);
    moved.put_f64(z);
    moved.put_bool(true);
    client.send(ids::SB_POSITION, &moved);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let pos = server
            .handle
            .run(|s| s.players.values().find(|p| p.profile.name == "Old17").map(|p| (p.loc.x, p.loc.y, p.loc.z)))
            .unwrap()
            .expect("the player");
        if (pos.0 - (x + 1.5)).abs() < 0.01 {
            assert!((pos.1 - y).abs() < 0.01, "the feet: {pos:?} for {y}");
            break;
        }
        assert!(Instant::now() < deadline, "the server did not see the move: {pos:?}");
        client.next(Duration::from_millis(50));
        other.next(Duration::from_millis(10));
    }
    server.stop();
}
