//! Paper-RS Boost: with `network.boost-mod` on, a client with the mod (simulated here) is
//! asked during login, opens extra connections, and gets its chunks over them, each with a
//! marker on the main connection that keeps it in order; a client without the mod joins as
//! usual. A broken extra connection closes the main one too.

mod common;

use common::*;
use paper_protocol::codec::{Decode, Reader, WriteExt};
use paper_protocol::crypto::Cfb8;
use paper_protocol::packets::{play, Packet};
use paper_server::net::boost;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// A stream as the mod opens it: the header, then the server's yes or no.
fn open_stream(port: u16, token: &[u8; 16], index: u8) -> (TcpStream, bool) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_nodelay(true).unwrap();
    let mut header = boost::MAGIC.to_vec();
    header.push(boost::VERSION as u8);
    header.extend_from_slice(&boost::token_id(token).to_be_bytes());
    header.push(index);
    s.write_all(&header).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut answer = [0u8; 1];
    let ok = s.read_exact(&mut answer).is_ok() && answer[0] == 1;
    (s, ok)
}

/// Reads what a stream has until it has been quiet for a while, decrypted: the chunks it
/// carried as (number, x, z).
fn read_stream(s: &mut TcpStream, cipher: &mut Cfb8, buf: &mut Vec<u8>, threshold: Option<u32>, quiet: Duration) -> Vec<(u32, i32, i32)> {
    s.set_read_timeout(Some(quiet)).unwrap();
    let mut chunk = vec![0u8; 65536];
    loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut data = chunk[..n].to_vec();
                cipher.decrypt(&mut data);
                buf.extend_from_slice(&data);
            }
        }
    }
    let mut out = Vec::new();
    loop {
        let mut r = Reader::new(buf);
        let Ok(len) = r.varint() else { break };
        let head = buf.len() - r.remaining();
        if r.remaining() < len as usize {
            break;
        }
        let message = buf[head..head + len as usize].to_vec();
        buf.drain(..head + len as usize);
        let mut m = Reader::new(&message);
        let seq = m.varint().unwrap() as u32;
        let frame = &message[message.len() - m.remaining()..];
        let payload = paper_protocol::frame::unframe(frame, threshold).expect("a frame");
        let mut p = Reader::new(&payload);
        assert_eq!(p.varint().unwrap(), play::LevelChunkWithLight::ID, "a chunk");
        let packet = play::LevelChunkWithLight::decode(&mut p).expect("the chunk decodes");
        out.push((seq, packet.x, packet.z));
    }
    out
}

#[test]
fn modded_clients_get_chunks_over_extra_connections() {
    let server = start_server(|c| {
        c.paper.network.boost_mod = true;
        c.paper.network.boost_streams = 2;
    });
    let port = server.port();

    // Without the mod: asked, answers as vanilla does, and plays as usual.
    let mut plain = Client::join(port, "Plain");
    plain.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("chunks as usual");
    assert_eq!(server.handle.run(|s| s.players.values().map(|p| p.conn.boost_streams()).sum::<usize>()).unwrap(), 0);

    // With the mod: it takes two streams.
    let mut asked = None;
    let mut modded = Client::join_answering(port, "Modded", |channel, data| {
        assert_eq!(channel, boost::CHANNEL);
        let mut r = Reader::new(data);
        let (version, offered, threshold) = (r.varint().unwrap(), r.varint().unwrap(), r.varint().unwrap());
        let token: [u8; 16] = data[data.len() - 16..].try_into().unwrap();
        asked = Some((version, offered, threshold, token));
        let mut answer = Vec::new();
        answer.put_varint(boost::VERSION);
        answer.put_varint(2);
        Some(answer)
    });
    let (version, offered, threshold, token) = asked.expect("the login query");
    assert_eq!((version, offered), (boost::VERSION, 2));
    let threshold = (threshold >= 0).then_some(threshold as u32);
    let (mut s0, ok0) = open_stream(port, &token, 0);
    let (mut s1, ok1) = open_stream(port, &token, 1);
    assert!(ok0 && ok1, "both streams taken");
    let (_, again) = open_stream(port, &token, 1);
    assert!(!again, "a stream is taken once");
    let id = modded.entity_id;
    let deadline = Instant::now() + TIMEOUT;
    while server.handle.run(move |s| s.players[&id].conn.boost_streams()).unwrap() < 2 {
        assert!(Instant::now() < deadline, "streams attached");
        std::thread::sleep(Duration::from_millis(20));
    }

    // Far away, new chunks: markers here, the chunks over the streams.
    server.handle.run(move |s| s.teleport(id, paper_server::server::player::Location { x: 1000.5, y: 100.0, z: 1000.5, yaw: 0.0, pitch: 0.0 })).unwrap();
    let mut markers = Vec::new();
    let mut direct = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    while markers.len() < 20 && Instant::now() < deadline {
        let Some((pid, frame)) = modded.next_packet(Duration::from_millis(200)) else { continue };
        if pid == play::CustomPayload::ID {
            let p: play::CustomPayload = decode(&frame);
            if p.channel == boost::MARKER {
                markers.push(Reader::new(&p.data.0).varint().unwrap() as u32);
            }
        } else if pid == play::LevelChunkWithLight::ID {
            let c: play::LevelChunkWithLight = decode(&frame);
            if (c.x - 62).abs() <= 2 && (c.z - 62).abs() <= 2 {
                direct += 1;
            }
        }
    }
    assert!(markers.len() >= 20, "markers: {}", markers.len());
    assert_eq!(direct, 0, "the chunks near the new place came over the streams");
    let (mut c0, mut c1) = (Cfb8::new(&boost::stream_key(&token, 0)), Cfb8::new(&boost::stream_key(&token, 1)));
    let (mut b0, mut b1) = (Vec::new(), Vec::new());
    let mut chunks = read_stream(&mut s0, &mut c0, &mut b0, threshold, Duration::from_millis(500));
    let from_second = read_stream(&mut s1, &mut c1, &mut b1, threshold, Duration::from_millis(500));
    assert!(!chunks.is_empty() && !from_second.is_empty(), "both streams carried chunks");
    chunks.extend(from_second);
    for seq in &markers {
        assert!(chunks.iter().any(|(s, ..)| s == seq), "chunk {seq} came");
    }
    assert!(chunks.iter().any(|(_, x, z)| (x - 62).abs() <= 1 && (z - 62).abs() <= 1), "the chunks around the player: {chunks:?}");

    // A stream that goes takes the player with it: its chunks would never come.
    drop(s1);
    let kicked = modded.find(TIMEOUT, |d: &play::Disconnect| d.reason.to_plain().contains("Paper-RS Boost"));
    assert!(kicked.is_some(), "the main connection closes");
    let _ = plain.next_packet(Duration::from_millis(10));
    server.stop();
}
