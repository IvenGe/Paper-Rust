//! A 1.8 client joins through the translator: legacy block and item ids, fixed-point entities,
//! JSON chat, and no configuration phase or teleport confirmations of its own.

mod common;

use common::*;
use paper_protocol::codec::{BlockPos, Reader, WriteExt};
use paper_protocol::frame::{FrameDecoder, FrameEncoder};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// 1.8's packet ids (minecraft-data).
mod ids {
    pub const LOGIN_START: i32 = 0x00;
    pub const LOGIN_SUCCESS: i32 = 0x02;
    pub const LOGIN_COMPRESS: i32 = 0x03;
    pub const LOGIN_DISCONNECT: i32 = 0x00;
    pub const KEEP_ALIVE: i32 = 0x00;
    pub const JOIN_GAME: i32 = 0x01;
    pub const CHAT: i32 = 0x02;
    pub const POSITION: i32 = 0x08;
    pub const SPAWN_MOB: i32 = 0x0F;
    pub const MAP_CHUNK: i32 = 0x21;
    pub const BLOCK_CHANGE: i32 = 0x23;
    pub const SET_SLOT: i32 = 0x2F;
    pub const SB_KEEP_ALIVE: i32 = 0x00;
    pub const SB_CHAT: i32 = 0x01;
    pub const SB_POSITION: i32 = 0x04;
    pub const SB_POSITION_LOOK: i32 = 0x06;
}

fn pos18(p: BlockPos) -> i64 {
    ((p.x as i64 & 0x3FF_FFFF) << 38) | ((p.y as i64 & 0xFFF) << 26) | (p.z as i64 & 0x3FF_FFFF)
}

struct Client18 {
    stream: TcpStream,
    enc: FrameEncoder,
    dec: FrameDecoder,
    backlog: std::collections::VecDeque<(i32, Vec<u8>)>,
    position: (f64, f64, f64),
}

impl Client18 {
    fn connect(port: u16) -> Client18 {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        Client18 { stream, enc: FrameEncoder::new(4), dec: FrameDecoder::new(), backlog: Default::default(), position: (0.0, 0.0, 0.0) }
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

    fn handshake(&mut self, intent: i32) {
        let mut body = Vec::new();
        body.put_varint(47);
        body.put_string("localhost");
        body.put_u16(25565);
        body.put_varint(intent);
        self.send(0, &body);
    }

    /// Logs in as a 1.8 client; returns once the join game packet arrived, with its body.
    fn join(port: u16, name: &str) -> (Client18, Vec<u8>) {
        let mut c = Client18::connect(port);
        c.handshake(2);
        let mut start = Vec::new();
        start.put_string(name);
        c.send(ids::LOGIN_START, &start);
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let (id, body) = c.read(deadline).expect("login reply");
            match id {
                ids::LOGIN_COMPRESS => {
                    let threshold = Reader::new(&body).varint().unwrap() as u32;
                    c.enc.set_compression(Some(threshold));
                    c.dec.set_compression(Some(threshold));
                }
                ids::LOGIN_SUCCESS => {
                    let mut r = Reader::new(&body);
                    let uuid = r.string(36).unwrap();
                    assert_eq!(uuid.len(), 36, "a hyphenated uuid: {uuid}");
                    assert_eq!(r.string(16).unwrap(), name);
                    assert!(r.is_empty());
                    break;
                }
                ids::LOGIN_DISCONNECT => panic!("refused: {}", Reader::new(&body).string(1 << 16).unwrap()),
                other => panic!("unexpected login packet {other}"),
            }
        }
        loop {
            let (id, body) = c.next(TIMEOUT).expect("join game");
            if id == ids::JOIN_GAME {
                return (c, body);
            }
        }
    }

    /// The next packet, after keep-alives and teleports are answered as 1.8 does.
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
                    let (x, y, z) = (r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap());
                    self.position = (x, y, z);
                    let mut back = Vec::new();
                    back.put_f64(x);
                    back.put_f64(y);
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

/// The block at a position of a 1.8 chunk packet (`id << 4 | meta`), if its section was sent.
fn block_in_chunk(body: &[u8], x: i32, y: i32, z: i32) -> Option<u16> {
    let mut r = Reader::new(body);
    r.i32().unwrap();
    r.i32().unwrap();
    assert!(r.bool().unwrap(), "a full chunk");
    let mask = r.u16().unwrap();
    let data = r.byte_array(1 << 22).unwrap();
    let section = y / 16;
    if mask & (1 << section) == 0 {
        return None;
    }
    let before = (0..section).filter(|s| mask & (1 << s) != 0).count();
    let index = ((y & 15) * 256 + (z & 15) * 16 + (x & 15)) as usize;
    let at = before * 8192 + index * 2;
    Some(u16::from_le_bytes([data[at], data[at + 1]]))
}

#[test]
fn the_server_list_shows_1_8_as_compatible() {
    let server = start_server(|_| {});
    let mut c = Client18::connect(server.port());
    c.handshake(1);
    c.send(0, &[]);
    let (_, body) = c.read(Instant::now() + TIMEOUT).expect("status");
    let status: serde_json::Value = serde_json::from_str(&Reader::new(&body).string(1 << 20).unwrap()).unwrap();
    assert_eq!(status["version"]["protocol"], 47);
    server.stop();
}

#[test]
fn a_1_8_client_joins_and_plays() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Creative);
    // Blocks where 1.8 can see them (its world starts at y 0).
    let diamond = paper_data::blocks::default_state("minecraft:diamond_block").unwrap();
    let placed = server
        .handle
        .run(move |s| {
            s.load_chunk_now(0, paper_server::world::chunk::ChunkPos { x: 0, z: 0 });
            s.set_block_and_update(0, BlockPos::new(3, 70, 3), diamond)
        })
        .unwrap();
    assert!(placed, "the diamond block is placed");
    let (mut client, join) = Client18::join(server.port(), "Old18");
    let mut r = Reader::new(&join);
    let entity = r.i32().unwrap();
    assert_eq!(r.u8().unwrap(), 1, "creative");
    assert_eq!(r.i8().unwrap(), 0, "the overworld");
    r.u8().unwrap();
    r.u8().unwrap();
    assert_eq!(r.string(16).unwrap(), "flat");
    r.bool().unwrap();
    assert!(r.is_empty());
    assert!(entity > 0);

    // The chunk with the diamond block in 1.8's ids (57:0).
    let chunk = client
        .find(TIMEOUT, ids::MAP_CHUNK, |b| {
            let mut r = Reader::new(b);
            (r.i32().unwrap(), r.i32().unwrap()) == (0, 0)
        })
        .expect("the spawn chunk");
    let mask = {
        let mut r = Reader::new(&chunk);
        r.i32().unwrap();
        r.i32().unwrap();
        r.bool().unwrap();
        r.u16().unwrap()
    };
    assert_eq!(block_in_chunk(&chunk, 3, 70, 3), Some(57 << 4), "sections sent: {mask:016b}");

    // Block changes.
    client.find(TIMEOUT, ids::POSITION, |_| true).expect("the spawn position");
    std::thread::sleep(Duration::from_millis(300));
    let gold = paper_data::blocks::default_state("minecraft:gold_block").unwrap();
    server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(4, 70, 4), gold)).unwrap();
    let change = client.find(TIMEOUT, ids::BLOCK_CHANGE, |b| Reader::new(b).i64().unwrap() == pos18(BlockPos::new(4, 70, 4))).expect("the block change");
    let mut r = Reader::new(&change);
    r.i64().unwrap();
    assert_eq!(r.varint().unwrap(), 41 << 4, "a gold block");

    // Chat both ways (a command, then its output as JSON).
    let mut chat = Vec::new();
    chat.put_string("/say hello from 1.8");
    client.send(ids::SB_CHAT, &chat);
    client.find(TIMEOUT, ids::CHAT, |b| Reader::new(b).string(32767).unwrap().contains("hello from 1.8")).expect("the /say message");

    // Mobs as 1.8 mobs: a zombie is 54.
    let zombie = server
        .handle
        .run(|s| s.spawn_mob(0, "minecraft:zombie", paper_server::world::entity::Vec3::new(6.5, -60.0, 6.5), paper_server::server::mobs::SpawnReason::Command).unwrap())
        .unwrap();
    let spawn = client.find(TIMEOUT, ids::SPAWN_MOB, |b| Reader::new(b).varint().unwrap() == zombie).expect("the zombie");
    let mut r = Reader::new(&spawn);
    r.varint().unwrap();
    assert_eq!(r.u8().unwrap(), 54);

    // Items as 1.8 items: a diamond is 264.
    let item = paper_data::items::id("minecraft:diamond").unwrap() as i32;
    server
        .handle
        .run(move |s| {
            let id = *s.players.keys().next().unwrap();
            s.give_item(id, item, 3)
        })
        .unwrap();
    let slot = client
        .find(TIMEOUT, ids::SET_SLOT, |b| {
            let mut r = Reader::new(b);
            r.i8().unwrap();
            r.i16().unwrap();
            r.i16().unwrap() == 264
        })
        .expect("the diamonds");
    let mut r = Reader::new(&slot);
    assert_eq!(r.i8().unwrap(), 0);
    r.i16().unwrap();
    r.i16().unwrap();
    assert_eq!(r.i8().unwrap(), 3);

    // Moving: the server sees where the client goes.
    let (x, y, z) = client.position;
    let mut moved = Vec::new();
    moved.put_f64(x + 1.5);
    moved.put_f64(y);
    moved.put_f64(z);
    moved.put_bool(true);
    client.send(ids::SB_POSITION, &moved);
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
