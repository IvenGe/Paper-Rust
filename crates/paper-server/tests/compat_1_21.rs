//! 1.21.5 to 1.21.11 clients join through the translator. The client here speaks each version
//! with packet, block, item and entity ids from minecraft-data (an independent source of the
//! tables the translator uses), and checks what changed between the versions: the chunk
//! sections' fluid count (26.1), the time (26.1), the world border's timing (1.21.11), where a
//! spawned entity's velocity goes (1.21.9) and the one interaction packet of before 26.1.

mod common;

use common::*;
use paper_protocol::codec::{BlockPos, Reader, WriteExt};
use paper_protocol::frame::{FrameDecoder, FrameEncoder};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// What a version's client needs to know: ids from minecraft-data.
#[derive(Clone, Copy, Debug)]
struct V {
    protocol: i32,
    /// Its vanilla data pack's version.
    pack: &'static str,
    login: i32,
    chunk: i32,
    block_update: i32,
    keep_alive: i32,
    position: i32,
    system_chat: i32,
    chunk_batch_finished: i32,
    set_player_inventory: i32,
    add_entity: i32,
    set_time: i32,
    border_lerp: i32,
    sb_keep_alive: i32,
    sb_chunk_batch_received: i32,
    sb_chat_command: i32,
    sb_position_look: i32,
    sb_interact: i32,
    diamond_block: i32,
    diamond: i32,
    zombie: i32,
}

const V1_21_11: V = V {
    protocol: 774,
    pack: "1.21.11",
    login: 48,
    chunk: 44,
    block_update: 8,
    keep_alive: 43,
    position: 70,
    system_chat: 119,
    chunk_batch_finished: 11,
    set_player_inventory: 106,
    add_entity: 1,
    set_time: 111,
    border_lerp: 87,
    sb_keep_alive: 27,
    sb_chunk_batch_received: 10,
    sb_chat_command: 6,
    sb_position_look: 30,
    sb_interact: 25,
    diamond_block: 5108,
    diamond: 898,
    zombie: 150,
};
const V1_21_9: V = V { protocol: 773, pack: "1.21.9", zombie: 147, ..V1_21_11 };
const V1_21_7: V = V {
    protocol: 772,
    pack: "1.21.8",
    login: 43,
    chunk: 39,
    keep_alive: 38,
    position: 65,
    system_chat: 114,
    set_player_inventory: 101,
    set_time: 106,
    border_lerp: 82,
    diamond_block: 4340,
    diamond: 862,
    zombie: 145,
    ..V1_21_11
};
const V1_21_6: V = V { protocol: 771, pack: "1.21.6", ..V1_21_7 };
const V1_21_5: V = V {
    protocol: 770,
    pack: "1.21.5",
    sb_keep_alive: 26,
    sb_chunk_batch_received: 9,
    sb_chat_command: 5,
    sb_position_look: 29,
    sb_interact: 24,
    diamond: 845,
    zombie: 144,
    ..V1_21_6
};

/// Login and configuration ids, the same in all of them.
mod ids {
    pub const LOGIN_HELLO: i32 = 0;
    pub const LOGIN_ACKNOWLEDGED: i32 = 3;
    pub const LOGIN_DISCONNECT: i32 = 0;
    pub const LOGIN_FINISHED: i32 = 2;
    pub const LOGIN_COMPRESSION: i32 = 3;
    pub const CONFIG_FINISH: i32 = 3;
    pub const CONFIG_KNOWN_PACKS: i32 = 14;
    pub const CONFIG_DISCONNECT: i32 = 2;
    pub const CONFIG_FINISH_ACK: i32 = 3;
    pub const CONFIG_SELECT_KNOWN_PACKS: i32 = 7;
    pub const SB_TELEPORT_CONFIRM: i32 = 0;
}

struct Client {
    v: V,
    stream: TcpStream,
    enc: FrameEncoder,
    dec: FrameDecoder,
    backlog: std::collections::VecDeque<(i32, Vec<u8>)>,
    position: (f64, f64, f64),
}

impl Client {
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

    fn join(port: u16, v: V, name: &str) -> (Client, Vec<u8>) {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut c = Client { v, stream, enc: FrameEncoder::new(4), dec: FrameDecoder::new(), backlog: Default::default(), position: (0.0, 0.0, 0.0) };
        let mut hs = Vec::new();
        hs.put_varint(v.protocol);
        hs.put_string("localhost");
        hs.put_u16(25565);
        hs.put_varint(2);
        c.send(0, &hs);
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
                    assert!(r.is_empty(), "login finished has {} bytes too many (no session id before 26.2)", r.remaining());
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
                    let offered = r.list(64, |r| Ok((r.string(256)?, r.string(256)?, r.string(256)?))).unwrap();
                    assert!(offered.iter().any(|(ns, id, version)| ns == "minecraft" && id == "core" && version == v.pack), "{} is offered: {offered:?}", v.pack);
                    let mut reply = Vec::new();
                    reply.put_varint(1);
                    reply.put_string("minecraft");
                    reply.put_string("core");
                    reply.put_string(v.pack);
                    c.send(ids::CONFIG_SELECT_KNOWN_PACKS, &reply);
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
            if id == v.login {
                return (c, body);
            }
        }
    }

    /// The next play packet, after keep-alives, teleports and chunk batches are answered.
    fn next(&mut self, timeout: Duration) -> Option<(i32, Vec<u8>)> {
        if let Some(p) = self.backlog.pop_front() {
            return Some(p);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let (id, body) = self.read(deadline)?;
            if id == self.v.keep_alive {
                self.send(self.v.sb_keep_alive, &body);
            } else if id == self.v.position {
                let mut r = Reader::new(&body);
                let teleport = r.varint().unwrap();
                let (x, y, z) = (r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap());
                self.position = (x, y, z);
                let mut confirm = Vec::new();
                confirm.put_varint(teleport);
                self.send(ids::SB_TELEPORT_CONFIRM, &confirm);
                let mut moved = Vec::new();
                moved.put_f64(x);
                moved.put_f64(y);
                moved.put_f64(z);
                moved.put_f32(0.0);
                moved.put_f32(0.0);
                moved.put_u8(1);
                self.send(self.v.sb_position_look, &moved);
                return Some((id, body));
            } else if id == self.v.chunk_batch_finished {
                let mut ack = Vec::new();
                ack.put_f32(20.0);
                self.send(self.v.sb_chunk_batch_received, &ack);
                return Some((id, body));
            } else {
                return Some((id, body));
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

    fn command(&mut self, command: &str) {
        let mut body = Vec::new();
        body.put_string(command);
        self.send(self.v.sb_chat_command, &body);
    }
}

/// Reads past a paletted container as 1.21.5 and later send it (no length before the data):
/// returns its palette (the single value, or the indirect palette; empty for a direct one).
fn container(r: &mut Reader<'_>, entries: usize, max_indirect: u8) -> Vec<i32> {
    let bits = r.u8().unwrap();
    if bits == 0 {
        return vec![r.varint().unwrap()];
    }
    let palette = if bits <= max_indirect { r.list(4096, |r| r.varint()).unwrap() } else { Vec::new() };
    let per_long = 64 / bits as usize;
    r.take(entries.div_ceil(per_long) * 8).unwrap();
    palette
}

/// The palettes of a chunk's sections, read to the end as `v` sends them.
fn sections(v: V, body: &[u8], count: usize) -> Vec<Vec<i32>> {
    let mut r = Reader::new(body);
    r.i32().unwrap();
    r.i32().unwrap();
    r.list(16, |r| Ok((r.varint()?, r.list(1 << 12, |r| r.i64())?))).unwrap();
    let data = r.byte_array(1 << 22).unwrap();
    let mut s = Reader::new(data);
    let mut palettes = Vec::new();
    for _ in 0..count {
        s.i16().unwrap();
        if v.protocol >= 775 {
            s.i16().unwrap();
        }
        palettes.push(container(&mut s, 4096, 8));
        container(&mut s, 64, 3);
    }
    assert!(s.is_empty(), "{} bytes after {count} sections: the sections are not {}'s", s.remaining(), v.pack);
    palettes
}

fn joins_and_plays(v: V) {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Creative);
    let (mut client, login) = Client::join(server.port(), v, "Old121");
    let mut r = Reader::new(&login);
    r.i32().unwrap();
    r.bool().unwrap();
    r.list(64, |r| r.string(256)).unwrap();
    for _ in 0..3 {
        r.varint().unwrap();
    }
    for _ in 0..3 {
        r.bool().unwrap();
    }
    r.varint().unwrap();
    r.string(256).unwrap();
    r.i64().unwrap();
    assert_eq!(r.i8().unwrap(), 1, "creative");
    assert_eq!(r.i8().unwrap(), -1, "no previous game mode");
    r.bool().unwrap();
    r.bool().unwrap();
    r.option(|r| Ok((r.string(256)?, r.position()?))).unwrap();
    r.varint().unwrap();
    r.varint().unwrap();
    r.bool().unwrap();
    assert!(r.is_empty(), "the login packet has {} bytes too many", r.remaining());

    // A chunk: 24 sections from y -64, read to the end in the version's format.
    let chunk = client
        .find(TIMEOUT, v.chunk, |b| {
            let mut r = Reader::new(b);
            (r.i32().unwrap(), r.i32().unwrap()) == (0, 0)
        })
        .expect("the spawn chunk");
    let palettes = sections(v, &chunk, 24);
    assert!(palettes[0].contains(&85), "bedrock (85) at the bottom: {:?}", palettes[0]);

    // A block change in the version's ids.
    client.find(TIMEOUT, v.position, |_| true).expect("the spawn position");
    std::thread::sleep(Duration::from_millis(300));
    let diamond = paper_data::blocks::default_state("minecraft:diamond_block").unwrap();
    server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(4, -60, 4), diamond)).unwrap();
    let change = client.find(TIMEOUT, v.block_update, |b| Reader::new(b).position().unwrap() == BlockPos::new(4, -60, 4)).expect("the block change");
    let mut r = Reader::new(&change);
    r.position().unwrap();
    assert_eq!(r.varint().unwrap(), v.diamond_block);

    // Chat both ways.
    client.command("say hello from the past");
    client.find(TIMEOUT, v.system_chat, |b| format!("{:?}", Reader::new(b).nbt().unwrap()).contains("hello from the past")).expect("the /say message");

    // An item in the version's ids.
    let item = paper_data::items::id("minecraft:diamond").unwrap() as i32;
    server
        .handle
        .run(move |s| {
            let id = *s.players.keys().next().unwrap();
            s.give_item(id, item, 2)
        })
        .unwrap();
    // As a slot of the inventory window (20 in all of them) or of the player's inventory.
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let (id, body) = client.next(Duration::from_millis(200)).unwrap_or((-1, Vec::new()));
        let mut r = Reader::new(&body);
        let stack = if id == 20 {
            r.varint().unwrap();
            r.varint().unwrap();
            r.i16().unwrap();
            Some((r.varint().unwrap(), r.varint().unwrap_or(-1)))
        } else if id == v.set_player_inventory {
            r.varint().unwrap();
            Some((r.varint().unwrap(), r.varint().unwrap_or(-1)))
        } else {
            None
        };
        if let Some((count, item)) = stack.filter(|(count, _)| *count > 0) {
            assert_eq!((item, count), (v.diamond, 2), "two diamonds in {}'s ids", v.pack);
            break;
        }
        assert!(Instant::now() < deadline, "no inventory update with the diamonds");
    }

    // A zombie, its spawn read to the end: the velocity is three shorts at the end before
    // 1.21.9, a vector after the position from it.
    let zombie = server
        .handle
        .run(|s| s.spawn_mob(0, "minecraft:zombie", paper_server::world::entity::Vec3::new(6.5, -60.0, 6.5), paper_server::server::mobs::SpawnReason::Command).unwrap())
        .unwrap();
    let spawn = client.find(TIMEOUT, v.add_entity, |b| Reader::new(b).varint().unwrap() == zombie).expect("the zombie");
    let mut r = Reader::new(&spawn);
    r.varint().unwrap();
    r.uuid().unwrap();
    assert_eq!(r.varint().unwrap(), v.zombie);
    for _ in 0..3 {
        r.f64().unwrap();
    }
    if v.protocol >= 773 {
        r.lp_vec3().unwrap();
    }
    r.take(3).unwrap();
    r.varint().unwrap();
    if v.protocol < 773 {
        r.take(6).unwrap();
    }
    assert!(r.is_empty(), "{} bytes after the spawn", r.remaining());

    // Hitting it: before 26.1 an attack is one kind of `interact`.
    let health = |server: &paper_server::RunningServer| server.handle.run(move |s| s.worlds[0].entities.get(&zombie).and_then(|e| e.mob()).map(|m| m.living.health)).unwrap();
    let full = health(&server).expect("the zombie");
    let mut attack = Vec::new();
    attack.put_varint(zombie);
    attack.put_varint(1);
    attack.put_bool(false);
    client.send(v.sb_interact, &attack);
    let deadline = Instant::now() + TIMEOUT;
    while health(&server).is_some_and(|h| h >= full) {
        assert!(Instant::now() < deadline, "the zombie was not hit");
        client.next(Duration::from_millis(50));
    }

    let console = |server: &paper_server::RunningServer, line: &'static str| {
        server.handle.run(move |s| s.dispatch_command(&paper_server::server::commands::CommandSource::Console, line)).unwrap();
    };
    // The time: before 26.1 the time of day and whether it runs, no clocks.
    console(&server, "time set 1000");
    let time = client
        .find(TIMEOUT, v.set_time, |b| {
            let mut r = Reader::new(b);
            r.i64().unwrap();
            r.remaining() == 9 && r.i64().unwrap() == 1000
        })
        .expect("the time of day");
    assert_eq!(time.len(), 17);

    // The world border's change: milliseconds before 1.21.11, ticks from it.
    console(&server, "worldborder set 100 10");
    let lerp = client.find(TIMEOUT, v.border_lerp, |_| true).expect("the border moving");
    let mut r = Reader::new(&lerp);
    r.f64().unwrap();
    r.f64().unwrap();
    let time = r.varlong().unwrap();
    assert_eq!(time, if v.protocol >= 774 { 200 } else { 10_000 });
    server.stop();
}

#[test]
fn a_1_21_11_client_joins_and_plays() {
    joins_and_plays(V1_21_11);
}

#[test]
fn a_1_21_9_client_joins_and_plays() {
    joins_and_plays(V1_21_9);
}

#[test]
fn a_1_21_7_client_joins_and_plays() {
    joins_and_plays(V1_21_7);
}

#[test]
fn a_1_21_6_client_joins_and_plays() {
    joins_and_plays(V1_21_6);
}

#[test]
fn a_1_21_5_client_joins_and_plays() {
    joins_and_plays(V1_21_5);
}
