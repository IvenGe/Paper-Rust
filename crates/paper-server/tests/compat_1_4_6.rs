//! A 1.4.6 client joins through the translators: a login from before Netty (its handshake,
//! the encryption exchange it does even offline, packets without lengths), UTF-16 strings and
//! 1.4.6's own block and item ids.

mod common;

use common::*;
use paper_compat::prenetty::{self, wire};
use paper_protocol::codec::{BlockPos, Reader, WriteExt};
use paper_protocol::crypto::Cfb8;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

struct Client146 {
    stream: TcpStream,
    /// Decrypted bytes not yet split into packets.
    buf: Vec<u8>,
    enc: Option<Cfb8>,
    dec: Option<Cfb8>,
    backlog: VecDeque<Vec<u8>>,
    /// Where the server last put the player (feet).
    position: (f64, f64, f64),
}

impl Client146 {
    fn connect(port: u16) -> Client146 {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        Client146 { stream, buf: Vec::new(), enc: None, dec: None, backlog: VecDeque::new(), position: (0.0, 0.0, 0.0) }
    }

    fn send(&mut self, packet: &[u8]) {
        let mut bytes = packet.to_vec();
        if let Some(cipher) = &mut self.enc {
            cipher.encrypt(&mut bytes);
        }
        self.stream.write_all(&bytes).expect("write");
    }

    /// The next packet (its id first), as a 1.4.6 client finds its end.
    fn read(&mut self, deadline: Instant) -> Option<Vec<u8>> {
        let mut chunk = [0u8; 16384];
        loop {
            match prenetty::split_clientbound(&self.buf) {
                Ok(Some(len)) => return Some(self.buf.drain(..len).collect()),
                Ok(None) => {}
                Err(e) => panic!("a packet 1.4.6 could not read: {e} ({:02x?})", &self.buf[..self.buf.len().min(24)]),
            }
            if Instant::now() >= deadline {
                return None;
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => {
                    let mut data = chunk[..n].to_vec();
                    if let Some(cipher) = &mut self.dec {
                        cipher.decrypt(&mut data);
                    }
                    self.buf.extend_from_slice(&data);
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => return None,
            }
        }
    }

    fn handshake(name: &str, protocol: u8) -> Vec<u8> {
        let mut hs = vec![0x02, protocol];
        wire::put_string(&mut hs, name, 16);
        wire::put_string(&mut hs, "localhost", 255);
        hs.put_i32(25565);
        hs
    }

    /// Logs in as 1.4.6 does; returns once the login packet (0x01) arrived, with it.
    fn join(port: u16, name: &str) -> (Client146, Vec<u8>) {
        let mut c = Client146::connect(port);
        c.send(&Client146::handshake(name, prenetty::PROTOCOL));
        let deadline = Instant::now() + TIMEOUT;
        let auth = c.read(deadline).expect("the encryption request");
        assert_eq!(auth[0], 0xFD, "{auth:02x?}");
        let mut r = Reader::new(&auth[1..]);
        assert_eq!(wire::read_string(&mut r, 20).unwrap(), "-", "an offline server");
        let key = RsaPublicKey::from_public_key_der(wire::read_array(&mut r).unwrap()).expect("an RSA public key");
        let token = wire::read_array(&mut r).unwrap().to_vec();
        let secret: [u8; 16] = rand::random();
        let mut answer = vec![0xFC];
        wire::put_array(&mut answer, &key.encrypt(&mut rsa::rand_core::OsRng, Pkcs1v15Encrypt, &secret).unwrap());
        wire::put_array(&mut answer, &key.encrypt(&mut rsa::rand_core::OsRng, Pkcs1v15Encrypt, &token).unwrap());
        c.send(&answer);
        // Encrypting after the answer, decrypting after the confirmation.
        c.enc = Some(Cfb8::new(&secret));
        assert_eq!(c.read(deadline).expect("the confirmation"), prenetty::SHARED_KEY_ACK);
        assert!(c.buf.is_empty(), "nothing before the client's initial spawn");
        c.dec = Some(Cfb8::new(&secret));
        c.send(&[0xCD, 0]);
        loop {
            let packet = c.next(TIMEOUT).expect("the login");
            match packet[0] {
                0x01 => return (c, packet),
                0xFF => panic!("refused: {}", wire::read_string(&mut Reader::new(&packet[1..]), 256).unwrap()),
                _ => {}
            }
        }
    }

    /// The next packet, after keep-alives and teleports are answered as 1.4.6 does.
    fn next(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        if let Some(p) = self.backlog.pop_front() {
            return Some(p);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let packet = self.read(deadline)?;
            match packet[0] {
                0x00 => self.send(&packet),
                0x0D => {
                    let mut r = Reader::new(&packet[1..]);
                    let (x, eyes, feet, z) = (r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap());
                    assert!((eyes - feet - 1.62).abs() < 1e-6, "the eyes then the feet: {eyes} {feet}");
                    self.position = (x, feet, z);
                    // Back: x, the feet, the eyes, z.
                    let mut back = vec![0x0D];
                    back.put_f64(x);
                    back.put_f64(feet);
                    back.put_f64(eyes);
                    back.put_f64(z);
                    back.put_f32(0.0);
                    back.put_f32(0.0);
                    back.put_bool(true);
                    self.send(&back);
                    return Some(packet);
                }
                _ => return Some(packet),
            }
        }
    }

    fn find(&mut self, timeout: Duration, id: u8, pred: impl Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
        if let Some(i) = self.backlog.iter().position(|p| p[0] == id && pred(&p[1..])) {
            return self.backlog.remove(i);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let saved = std::mem::take(&mut self.backlog);
            let next = self.next(left);
            self.backlog = saved;
            let packet = next?;
            if packet[0] == id && pred(&packet[1..]) {
                return Some(packet);
            }
            self.backlog.push_back(packet);
        }
    }
}

/// The block at a position of a 1.4.6 chunk packet (id, metadata), if its section was sent.
fn block_in_chunk(packet: &[u8], x: i32, y: i32, z: i32) -> Option<(u8, u8)> {
    let mut r = Reader::new(&packet[1..]);
    r.i32().unwrap();
    r.i32().unwrap();
    assert!(r.bool().unwrap(), "a full chunk");
    let mask = r.u16().unwrap();
    assert_eq!(r.u16().unwrap(), 0, "no block ids above 255");
    let len = r.i32().unwrap() as usize;
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(r.take(len).unwrap()).read_to_end(&mut raw).unwrap();
    let sections = mask.count_ones() as usize;
    // Ids, metadata, block light, sky light, biomes.
    assert_eq!(raw.len(), sections * (4096 + 3 * 2048) + 256);
    let section = y / 16;
    if mask & (1 << section) == 0 {
        return None;
    }
    let before = (0..section).filter(|s| mask & (1 << s) != 0).count();
    let index = ((y & 15) * 256 + (z & 15) * 16 + (x & 15)) as usize;
    let at = before * 4096 + index;
    let meta = raw[sections * 4096 + at / 2] >> (4 * (at % 2)) & 15;
    Some((raw[at], meta))
}

fn text(packet: &[u8]) -> String {
    wire::read_string(&mut Reader::new(&packet[1..]), 32767).unwrap()
}

#[test]
fn old_server_list_pings_are_answered() {
    let server = start_server(|c| c.properties.max_players = 7);
    // 1.4 to 1.6: the protocol, version, message and counts.
    let mut c = Client146::connect(server.port());
    c.send(&[0xFE, 0x01]);
    let answer = c.read(Instant::now() + TIMEOUT).expect("an answer");
    assert_eq!(answer[0], 0xFF);
    let fields: Vec<String> = text(&answer).split('\0').map(str::to_owned).collect();
    assert_eq!(fields[0], "§1");
    assert_eq!(fields[1], "51", "1.4.6's protocol, so its list shows the server as compatible");
    assert!(fields[2].contains("1.4.6"), "{fields:?}");
    assert_eq!((fields[4].as_str(), fields[5].as_str()), ("0", "7"));
    // Older ones: the message and counts.
    let mut c = Client146::connect(server.port());
    c.send(&[0xFE]);
    let answer = c.read(Instant::now() + TIMEOUT).expect("an answer");
    assert!(text(&answer).ends_with("§0§7"), "{}", text(&answer));
    server.stop();
}

#[test]
fn other_versions_from_before_netty_are_told_why() {
    let server = start_server(|_| {});
    // 1.5.2 (protocol 61).
    let mut c = Client146::connect(server.port());
    c.send(&Client146::handshake("Newer", 61));
    let kick = c.read(Instant::now() + TIMEOUT).expect("a refusal");
    assert_eq!(kick[0], 0xFF);
    assert!(text(&kick).starts_with("Outdated server!"), "{}", text(&kick));
    assert!(text(&kick).contains("1.4.6-1.4.7"), "{}", text(&kick));
    server.stop();

    // 1.4.6 in online mode: the session servers it would sign in with are gone.
    let server = start_server(|c| c.properties.online_mode = true);
    let mut c = Client146::connect(server.port());
    c.send(&Client146::handshake("Signed", prenetty::PROTOCOL));
    let kick = c.read(Instant::now() + TIMEOUT).expect("a refusal");
    assert_eq!(kick[0], 0xFF);
    assert!(text(&kick).contains("online mode"), "{}", text(&kick));
    server.stop();
}

#[test]
fn a_1_4_6_client_joins_and_plays() {
    let server = start_server(|c| c.properties.gamemode = paper_server::config::GameMode::Creative);
    // Blocks where 1.4.6 can see them (its world starts at y 0): a diamond block, a block 1.4.6
    // has (granite, which it shows as stone) and one it does not (a hay bale: yellow wool).
    let state = |name: &str| paper_data::blocks::default_state(name).unwrap();
    let (diamond, granite, hay) = (state("minecraft:diamond_block"), state("minecraft:granite"), state("minecraft:hay_block"));
    server
        .handle
        .run(move |s| {
            s.load_chunk_now(0, paper_server::world::chunk::ChunkPos { x: 0, z: 0 });
            assert!(s.set_block_and_update(0, BlockPos::new(3, 70, 3), diamond));
            assert!(s.set_block_and_update(0, BlockPos::new(3, 71, 3), granite));
            assert!(s.set_block_and_update(0, BlockPos::new(3, 72, 3), hay));
        })
        .unwrap();
    let (mut client, login) = Client146::join(server.port(), "Old146");
    let mut r = Reader::new(&login[1..]);
    let entity = r.i32().unwrap();
    assert!(entity > 0);
    assert_eq!(wire::read_string(&mut r, 16).unwrap(), "flat");
    assert_eq!(r.u8().unwrap(), 1, "creative");
    assert_eq!(r.i8().unwrap(), 0, "the overworld");
    r.u8().unwrap();
    r.u8().unwrap();
    r.u8().unwrap();
    assert!(r.is_empty());

    // The chunk, in 1.4.6's ids.
    let chunk = client
        .find(TIMEOUT, 0x33, |b| {
            let mut r = Reader::new(b);
            (r.i32().unwrap(), r.i32().unwrap()) == (0, 0)
        })
        .expect("the spawn chunk");
    assert_eq!(block_in_chunk(&chunk, 3, 70, 3), Some((57, 0)), "a diamond block");
    assert_eq!(block_in_chunk(&chunk, 3, 71, 3), Some((1, 0)), "granite as stone");
    assert_eq!(block_in_chunk(&chunk, 3, 72, 3), Some((35, 4)), "a hay bale as yellow wool");

    // Block changes.
    client.find(TIMEOUT, 0x0D, |_| true).expect("the spawn position");
    std::thread::sleep(Duration::from_millis(300));
    let packed_ice = state("minecraft:packed_ice");
    server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(4, 70, 4), packed_ice)).unwrap();
    let change = client
        .find(TIMEOUT, 0x35, |b| {
            let mut r = Reader::new(b);
            (r.i32().unwrap(), r.u8().unwrap(), r.i32().unwrap()) == (4, 70, 4)
        })
        .expect("the block change");
    let mut r = Reader::new(&change[10..]);
    assert_eq!((r.i16().unwrap(), r.u8().unwrap()), (79, 0), "packed ice as ice");

    // Chat both ways: a command, then its output in § text.
    let mut chat = vec![0x03];
    wire::put_string(&mut chat, "/say hello from 1.4.6", 100);
    client.send(&chat);
    client.find(TIMEOUT, 0x03, |b| wire::read_string(&mut Reader::new(b), 119).unwrap().contains("hello from 1.4.6")).expect("the /say message");

    // Mobs as 1.4.6 mobs: a zombie is 54, a rabbit (which came later) a chicken.
    let spawn_mob = |kind: &'static str| {
        server
            .handle
            .run(move |s| s.spawn_mob(0, kind, paper_server::world::entity::Vec3::new(6.5, -60.0, 6.5), paper_server::server::mobs::SpawnReason::Command).unwrap())
            .unwrap()
    };
    for (kind, legacy) in [("minecraft:zombie", 54), ("minecraft:rabbit", 93)] {
        let mob = spawn_mob(kind);
        let spawn = client.find(TIMEOUT, 0x18, |b| Reader::new(b).i32().unwrap() == mob).unwrap_or_else(|| panic!("the {kind}"));
        assert_eq!(spawn[5], legacy, "{kind}");
    }

    // Items as 1.4.6 items: a diamond is 264, a name tag (1.6) paper.
    for (item, legacy) in [("minecraft:diamond", 264), ("minecraft:name_tag", 339)] {
        let id = paper_data::items::id(item).unwrap() as i32;
        server
            .handle
            .run(move |s| {
                let player = *s.players.keys().next().unwrap();
                s.give_item(player, id, 3)
            })
            .unwrap();
        let slot = client
            .find(TIMEOUT, 0x67, |b| {
                let mut r = Reader::new(b);
                r.i8().unwrap();
                r.i16().unwrap();
                r.i16().unwrap() == legacy
            })
            .unwrap_or_else(|| panic!("the {item}"));
        let mut r = Reader::new(&slot[1..]);
        assert_eq!(r.i8().unwrap(), 0, "the player's inventory");
        r.i16().unwrap();
        r.i16().unwrap();
        assert_eq!(r.i8().unwrap(), 3);
    }

    // Moving: the server sees where the client goes (its feet, not its eyes).
    let (x, y, z) = client.position;
    let mut moved = vec![0x0B];
    moved.put_f64(x + 1.5);
    moved.put_f64(y);
    moved.put_f64(y + 1.62);
    moved.put_f64(z);
    moved.put_bool(true);
    client.send(&moved);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let pos = server.handle.run(|s| s.players.values().next().map(|p| (p.loc.x, p.loc.y, p.loc.z))).unwrap().expect("the player");
        if (pos.0 - (x + 1.5)).abs() < 0.01 {
            assert!((pos.1 - y).abs() < 0.01, "the feet: {pos:?} for {y}");
            break;
        }
        assert!(Instant::now() < deadline, "the server did not see the move: {pos:?}");
        client.next(Duration::from_millis(50));
    }

    // Breaking a block in creative, by its position (the hay bale the client sees as wool),
    // from next to it: the world 1.4.6 sees starts at y 0, above the flat world's ground.
    server
        .handle
        .run(|s| {
            let id = *s.players.keys().next().unwrap();
            s.teleport(id, paper_server::server::player::Location { x: 3.5, y: 73.0, z: 4.5, yaw: 0.0, pitch: 0.0 })
        })
        .unwrap();
    client
        .find(TIMEOUT, 0x0D, |b| {
            let mut r = Reader::new(b);
            r.f64().unwrap();
            (r.f64().unwrap() - 74.62).abs() < 1e-6
        })
        .expect("the teleport");
    let mut dig = vec![0x0E, 0];
    dig.put_i32(3);
    dig.put_u8(72);
    dig.put_i32(3);
    dig.put_i8(1);
    client.send(&dig);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let block = server.handle.run(|s| s.worlds[0].block_at(3, 72, 3)).unwrap();
        if block != Some(hay) {
            assert_eq!(block, Some(paper_data::blocks::AIR));
            break;
        }
        assert!(Instant::now() < deadline, "the hay bale is still there");
        client.next(Duration::from_millis(50));
    }
    server.stop();
}
