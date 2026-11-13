//! A minimal protocol client for driving a real server over TCP in integration tests.
#![allow(dead_code)]

use paper_protocol::codec::{Decode, Encode, Reader, VarInt};
use paper_protocol::frame::{FrameDecoder, FrameEncoder};
use paper_protocol::packets::common::ClientSettings;
use paper_protocol::packets::handshake::{Intent, Intention};
use paper_protocol::packets::{configuration, encode_packet, login, play, status, Packet};
use paper_protocol::types::{GameProfile, KnownPack};
use paper_server::config::Config;
use paper_server::RunningServer;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Starts a server on an ephemeral port with the test configuration, after `tweak`.
pub fn start_server(tweak: impl FnOnce(&mut Config)) -> RunningServer {
    paper_server::logging::init(None, log::LevelFilter::Info);
    let mut config = Config::for_tests(std::path::Path::new(""));
    tweak(&mut config);
    paper_server::start(config).expect("server starts")
}

pub const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Client {
    stream: TcpStream,
    enc: FrameEncoder,
    dec: FrameDecoder,
    pub profile: Option<GameProfile>,
    pub entity_id: i32,
    pub login: Option<play::Login>,
    /// Position after the last server teleport.
    pub position: (f64, f64, f64),
    /// Play packets received but not yet consumed by `expect`.
    backlog: std::collections::VecDeque<(i32, Vec<u8>)>,
    /// Registries received during configuration: (registry, entry ids).
    pub registries: Vec<(String, Vec<String>)>,
    pub tag_registries: Vec<String>,
    pub brand: Option<String>,
}

impl Client {
    pub fn connect(port: u16) -> Client {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        Client {
            stream,
            enc: FrameEncoder::new(4),
            dec: FrameDecoder::new(),
            profile: None,
            entity_id: 0,
            login: None,
            position: (0.0, 0.0, 0.0),
            backlog: Default::default(),
            registries: Vec::new(),
            tag_registries: Vec::new(),
            brand: None,
        }
    }

    pub fn send<P: Packet + Encode>(&mut self, packet: &P) {
        let mut out = Vec::new();
        self.enc.encode(&encode_packet(packet), &mut out).unwrap();
        self.stream.write_all(&out).expect("write");
    }

    /// Reads one frame, or `None` if nothing arrives before `deadline` or the peer closed.
    fn read_frame(&mut self, deadline: Instant) -> Option<Vec<u8>> {
        let mut buf = [0u8; 16384];
        loop {
            if let Some(frame) = self.dec.next_frame().expect("valid frame") {
                return Some(frame);
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

    fn handshake(&mut self, protocol: i32, intent: Intent) {
        self.send(&Intention { protocol_version: protocol, server_address: "localhost".into(), server_port: 25565, intent });
    }

    /// Performs a server list ping and returns the status document.
    pub fn status(port: u16) -> serde_json::Value {
        let mut c = Client::connect(port);
        c.handshake(paper_data::PROTOCOL_VERSION, Intent::Status);
        c.send(&status::StatusRequest {});
        let frame = c.read_frame(Instant::now() + TIMEOUT).expect("status response");
        let response: status::StatusResponse = decode(&frame);
        c.send(&status::PingRequest { payload: 0x1234_5678 });
        let frame = c.read_frame(Instant::now() + TIMEOUT).expect("pong");
        let pong: status::PongResponse = decode(&frame);
        assert_eq!(pong.payload, 0x1234_5678);
        serde_json::from_str(&response.json).expect("status json")
    }

    /// Starts a login with the given protocol version and returns the first reply frame.
    pub fn begin_login(port: u16, protocol: i32, name: &str) -> (Client, Option<Vec<u8>>) {
        let mut c = Client::connect(port);
        c.handshake(protocol, Intent::Login);
        c.send(&login::Hello { name: name.into(), uuid: uuid::Uuid::nil() });
        let frame = c.read_frame(Instant::now() + TIMEOUT);
        (c, frame)
    }

    /// Logs in (offline mode) and completes configuration; returns once the play Login
    /// packet has arrived.
    pub fn join(port: u16, name: &str) -> Client {
        Client::join_with(port, name, ClientSettings::default())
    }

    pub fn join_with(port: u16, name: &str, settings: ClientSettings) -> Client {
        let (mut c, first) = Client::begin_login(port, paper_data::PROTOCOL_VERSION, name);
        let mut frame = first.expect("login reply");
        // Optional compression, then login success.
        if packet_id(&frame) == login::LoginCompression::ID {
            let p: login::LoginCompression = decode(&frame);
            c.enc.set_compression(Some(p.threshold.0 as u32));
            c.dec.set_compression(Some(p.threshold.0 as u32));
            frame = c.read_frame(Instant::now() + TIMEOUT).expect("login finished");
        }
        if packet_id(&frame) == login::LoginDisconnect::ID {
            let p: login::LoginDisconnect = decode(&frame);
            panic!("login refused: {}", p.reason.0.to_plain());
        }
        let finished: login::LoginFinished = decode(&frame);
        assert_eq!(finished.profile.name, name);
        c.profile = Some(finished.profile);
        c.send(&login::LoginAcknowledged {});

        // Configuration.
        c.send(&configuration::ClientInformation { settings });
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let frame = c.read_frame(deadline).expect("configuration packet");
            let id = packet_id(&frame);
            if id == configuration::SelectKnownPacks::ID {
                let p: configuration::SelectKnownPacks = decode(&frame);
                assert_eq!(p.packs, vec![KnownPack { namespace: "minecraft".into(), id: "core".into(), version: paper_data::VERSION_NAME.into() }]);
                c.send(&configuration::ServerboundSelectKnownPacks { packs: p.packs });
            } else if id == configuration::RegistryData::ID {
                let p: configuration::RegistryData = decode(&frame);
                assert!(p.entries.iter().all(|(_, data)| data.is_none()), "known-pack entries carry no data");
                c.registries.push((p.registry, p.entries.into_iter().map(|(id, _)| id).collect()));
            } else if id == configuration::UpdateTags::ID {
                let p: configuration::UpdateTags = decode(&frame);
                c.tag_registries = p.registries.into_iter().map(|r| r.registry).collect();
            } else if id == configuration::CustomPayload::ID {
                let p: configuration::CustomPayload = decode(&frame);
                if p.channel == "minecraft:brand" {
                    c.brand = Reader::new(&p.data.0).string(256).ok();
                }
            } else if id == configuration::FinishConfiguration::ID {
                c.send(&configuration::FinishConfigurationAck {});
                break;
            } else if id == configuration::Disconnect::ID {
                let p: configuration::Disconnect = decode(&frame);
                panic!("disconnected during configuration: {}", p.reason.to_plain());
            }
        }

        // Play: the first packet is Login.
        let login: play::Login = c.expect();
        c.entity_id = login.entity_id;
        c.login = Some(login);
        c
    }

    /// Housekeeping every real client performs: answer keep-alives, confirm teleports,
    /// acknowledge chunk batches. Returns whether the packet was consumed.
    fn auto_handle(&mut self, id: i32, frame: &[u8]) -> bool {
        if id == play::KeepAlive::ID {
            let p: play::KeepAlive = decode(frame);
            self.send(&play::ServerboundKeepAlive { id: p.id });
            return true;
        }
        if id == play::PlayerPosition::ID {
            let p: play::PlayerPosition = decode(frame);
            self.position = (p.x, p.y, p.z);
            self.send(&play::AcceptTeleportation { teleport_id: p.teleport_id, x: p.x, y: p.y, z: p.z, yaw: p.yaw, pitch: p.pitch });
            return false; // tests may also want to see it
        }
        if id == play::ChunkBatchFinished::ID {
            self.send(&play::ChunkBatchReceived { chunks_per_tick: 20.0 });
            return false;
        }
        false
    }

    /// Next play packet (after housekeeping), or `None` on timeout / disconnect.
    pub fn next_packet(&mut self, timeout: Duration) -> Option<(i32, Vec<u8>)> {
        if let Some(p) = self.backlog.pop_front() {
            return Some(p);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let frame = self.read_frame(deadline)?;
            let id = packet_id(&frame);
            if !self.auto_handle(id, &frame) {
                return Some((id, frame));
            }
        }
    }

    /// Waits for the next packet of type `P`, keeping other packets for later calls.
    pub fn expect<P: Packet + Decode>(&mut self) -> P {
        self.try_expect::<P>(TIMEOUT).unwrap_or_else(|| panic!("timed out waiting for {}", P::NAME))
    }

    pub fn try_expect<P: Packet + Decode>(&mut self, timeout: Duration) -> Option<P> {
        self.find(timeout, |_: &P| true)
    }

    /// Waits for a packet of type `P` satisfying `pred`; other packets (including
    /// non-matching `P`s) are kept in order for later calls.
    pub fn find<P: Packet + Decode>(&mut self, timeout: Duration, pred: impl Fn(&P) -> bool) -> Option<P> {
        // Look through what is already buffered first.
        let mut i = 0;
        while i < self.backlog.len() {
            if self.backlog[i].0 == P::ID {
                let p: P = decode(&self.backlog[i].1);
                if pred(&p) {
                    self.backlog.remove(i);
                    return Some(p);
                }
            }
            i += 1;
        }
        let deadline = Instant::now() + timeout;
        loop {
            let frame = self.read_frame(deadline)?;
            let id = packet_id(&frame);
            if self.auto_handle(id, &frame) {
                continue;
            }
            if id == P::ID {
                let p: P = decode(&frame);
                if pred(&p) {
                    return Some(p);
                }
            }
            self.backlog.push_back((id, frame));
        }
    }

    /// Reads until the server has been quiet for `quiet`, keeping everything in the backlog.
    pub fn drain(&mut self, quiet: Duration) {
        loop {
            let Some(frame) = self.read_frame(Instant::now() + quiet) else { return };
            let id = packet_id(&frame);
            if !self.auto_handle(id, &frame) {
                self.backlog.push_back((id, frame));
            }
        }
    }

    /// Whether a packet of type `P` is waiting in the backlog.
    pub fn has<P: Packet>(&self) -> bool {
        self.backlog.iter().any(|(id, _)| *id == P::ID)
    }

    pub fn clear_backlog(&mut self) {
        self.backlog.clear();
    }

    /// Waits until the server closes the connection (or the timeout passes).
    pub fn wait_closed(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline {
            match self.stream.read(&mut buf) {
                Ok(0) => return true,
                Ok(n) => self.dec.push(&buf[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => return true,
            }
        }
        false
    }

    /// Sends a chat message the way an unsigned client does.
    pub fn chat(&mut self, message: &str) {
        self.send(&play::Chat { message: message.into(), timestamp: 0, salt: 0, signed: false });
    }

    pub fn command(&mut self, command: &str) {
        self.send(&play::ChatCommand { command: command.into() });
    }

    pub fn move_to(&mut self, x: f64, y: f64, z: f64) {
        self.position = (x, y, z);
        self.send(&play::MovePlayerPos { x, y, z, flags: 1 });
        self.send(&play::ClientTickEnd {});
    }
}

pub fn packet_id(frame: &[u8]) -> i32 {
    Reader::new(frame).varint().expect("packet id")
}

/// Decodes a payload as `P`, panicking with a useful message on mismatch.
pub fn decode<P: Packet + Decode>(frame: &[u8]) -> P {
    let mut r = Reader::new(frame);
    let id = r.varint().expect("packet id");
    assert_eq!(id, P::ID, "expected {} but got packet id {id:#x}", P::NAME);
    let packet = P::decode(&mut r).unwrap_or_else(|e| panic!("decoding {}: {e}", P::NAME));
    r.finish().unwrap_or_else(|e| panic!("decoding {}: {e}", P::NAME));
    packet
}

#[allow(dead_code)]
fn _unused(_: VarInt) {}
