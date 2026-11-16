//! Clients from before Netty (1.4.6 to 1.6.4, see [`Era`]): no packet lengths, strings as
//! UTF-16, items with gzipped NBT, their own block and item ids, and a login of their own (a
//! handshake, then an encryption exchange even when the server is offline).
//!
//! The translator works through the 1.8 one: the server's 26.3 packets become 1.8's there and
//! 1.4.6's here, and the client's go the other way. What 1.4.6's packets are follows
//! ViaLegacy (GPL-3.0 or later). The connection does the first part itself: it tells a client
//! from before Netty by its first byte, reads its [`Handshake`], runs the encryption exchange
//! ([`server_auth_data`], [`read_shared_key`], [`SHARED_KEY_ACK`]) and finds packets with
//! [`split_serverbound`]; the translator takes over with the client's first packet after that.

pub(crate) mod clientbound;
pub mod ids;
mod serverbound;
pub mod wire;

pub use wire::{split_clientbound, split_clientbound_1_5, split_clientbound_1_6, split_serverbound, split_serverbound_1_6};

use super::{Legacy18, Phase};
use crate::io::Result;
use crate::{Outcome, Translator, Version};
use anyhow::bail;
use paper_protocol::codec::{Reader, WriteExt};
use std::collections::HashMap;

/// 1.4.6 and 1.4.7's protocol, as their handshake carries it.
pub const PROTOCOL: u8 = 51;

/// The versions from before Netty the server takes, by what sets them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Era {
    /// 1.4.6 and 1.4.7 (protocol 51).
    V1_4,
    /// 1.5 to 1.5.2 (protocols 60 and 61): hoppers and the rest of 1.5's blocks, scoreboards,
    /// particles, minecarts as one type, mobs' names.
    V1_5,
    /// 1.6.2 to 1.6.4 (protocols 74 and 78): JSON chat, horses and leads, health and
    /// abilities as floats, the entity data 1.7 has, the server opening sign editors.
    V1_6,
}

type Splitter = fn(&[u8]) -> std::result::Result<Option<usize>, String>;

impl Era {
    /// The era of a handshake's protocol, if the server takes it.
    pub fn of(protocol: u8) -> Option<Era> {
        match protocol {
            51 => Some(Era::V1_4),
            60 | 61 => Some(Era::V1_5),
            74 | 78 => Some(Era::V1_6),
            _ => None,
        }
    }

    /// The highest block id of the era.
    fn last_block(self) -> u16 {
        match self {
            Era::V1_4 => 145,
            Era::V1_5 => 158,
            Era::V1_6 => 173,
        }
    }

    /// How to find the end of what such a client sends.
    pub fn split_serverbound(self) -> Splitter {
        match self {
            Era::V1_6 => wire::split_serverbound_1_6,
            _ => wire::split_serverbound,
        }
    }

    /// How to find the end of what the server sends such a client.
    pub fn split_clientbound(self) -> Splitter {
        match self {
            Era::V1_4 => wire::split_clientbound,
            Era::V1_5 => wire::split_clientbound_1_5,
            Era::V1_6 => wire::split_clientbound_1_6,
        }
    }

    fn blocks(self) -> fn(u16) -> u16 {
        match self {
            Era::V1_4 => ids::block_1_4,
            Era::V1_5 => ids::block_1_5,
            Era::V1_6 => ids::block_1_6,
        }
    }
}

/// A client's handshake (packet 0x02, from 1.3 on: older ones send one string instead).
#[derive(Debug)]
pub struct Handshake {
    pub protocol: u8,
    pub name: String,
    pub host: String,
    pub port: i32,
}

impl Handshake {
    pub fn read(packet: &[u8]) -> Result<Handshake> {
        let mut r = Reader::new(packet);
        if r.u8()? != 0x02 {
            bail!("expected a handshake, got packet {:#04x}", packet[0]);
        }
        let protocol = r.u8()?;
        let name = wire::read_string(&mut r, 16)?;
        let host = wire::read_string(&mut r, 255)?;
        let port = r.i32()?;
        Ok(Handshake { protocol, name, host, port })
    }
}

/// The server's half of the encryption exchange (0xFD): its id (`-` when it does not check
/// sessions), its public key and a token the client returns encrypted.
pub fn server_auth_data(server_id: &str, public_key: &[u8], token: &[u8]) -> Vec<u8> {
    let mut w = vec![0xFD];
    wire::put_string(&mut w, server_id, 20);
    wire::put_array(&mut w, public_key);
    wire::put_array(&mut w, token);
    w
}

/// The client's answer (0xFC): the shared secret and the token, both encrypted with the key.
pub fn read_shared_key(packet: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut r = Reader::new(packet);
    if r.u8()? != 0xFC {
        bail!("expected the encryption answer, got packet {:#04x}", packet[0]);
    }
    let secret = wire::read_array(&mut r)?.to_vec();
    let token = wire::read_array(&mut r)?.to_vec();
    Ok((secret, token))
}

/// The server's confirmation (an empty 0xFC); everything after it is encrypted both ways.
pub const SHARED_KEY_ACK: [u8; 5] = [0xFC, 0, 0, 0, 0];

/// A disconnect with `reason` (0xFF); also how a server refuses a login before it starts.
pub fn kick(reason: &str) -> Vec<u8> {
    let mut w = vec![0xFF];
    wire::put_string(&mut w, reason, 256);
    w
}

/// The answer to a server list ping from before 1.7, a disconnect: 1.4 to 1.6 (who send
/// `0xFE 0x01`) read the protocol (`protocol`: the one shown as compatible), the version, the
/// message of the day and the player counts from it; older clients the message and the counts.
pub fn ping_answer(modern: bool, protocol: u8, version: &str, motd: &str, online: usize, max: i32) -> Vec<u8> {
    if modern {
        kick(&format!("§1\0{protocol}\0{version}\0{motd}\0{online}\0{max}"))
    } else {
        // `§` separates the fields there: the message goes without its codes.
        let mut plain = String::new();
        let mut chars = motd.chars();
        while let Some(c) = chars.next() {
            if c == '§' {
                chars.next();
            } else {
                plain.push(c);
            }
        }
        kick(&format!("{plain}§{online}§{max}"))
    }
}

/// One connection of a 1.4.6 client: the 1.8 translator, and what 1.4.6 needs besides.
pub struct PreNetty {
    inner: Legacy18,
    version: &'static Version,
    /// The player's name, from the handshake (1.8's login start carries it).
    name: String,
    /// The tab list's names: 1.4.6 spawns and lists players by name.
    names: HashMap<uuid::Uuid, String>,
    /// The open windows' types, as the client numbers them.
    windows: HashMap<u8, u8>,
    era: Era,
}

impl PreNetty {
    pub fn new(version: &'static Version, name: &str) -> PreNetty {
        let era = Era::of(version.protocol as u8).unwrap_or(Era::V1_4);
        PreNetty { inner: Legacy18::new(version), version, name: name.to_owned(), names: HashMap::new(), windows: HashMap::new(), era }
    }

    /// What the 1.8 translator made for the client (`login`: in its login phase), as 1.4.6
    /// packets.
    fn to_client(&mut self, login: bool, payloads: Vec<Vec<u8>>, out: &mut Outcome) -> Result<()> {
        for payload in payloads {
            if login {
                self.login_clientbound(&payload, &mut out.to_client)?;
            } else {
                self.play_clientbound(&payload, &mut out.to_client)?;
            }
        }
        Ok(())
    }
}

impl Translator for PreNetty {
    fn version(&self) -> &'static Version {
        self.version
    }

    fn compression(&self) -> bool {
        false
    }

    fn clientbound(&mut self, payload: &[u8], out: &mut Outcome) -> Result<()> {
        let login = self.inner.phase == Phase::Login;
        let mut inner = Outcome::default();
        self.inner.clientbound(payload, &mut inner)?;
        out.to_server.extend(inner.to_server);
        self.to_client(login, inner.to_client, out)
    }

    fn serverbound(&mut self, packet: &[u8], out: &mut Outcome) -> Result<()> {
        let mut payloads = Vec::new();
        if self.inner.phase == Phase::Login {
            // The client's "initial spawn" (0xCD 0) ends its login; 1.8's starts with a name.
            if packet[..] == [0xCD, 0] {
                let mut start = Vec::new();
                start.put_varint(super::ids::login::LOGIN_START);
                start.put_string(&self.name);
                payloads.push(start);
            }
        } else {
            self.to_1_8(packet, &mut payloads)?;
        }
        for payload in payloads {
            let login = self.inner.phase == Phase::Login;
            let mut inner = Outcome::default();
            self.inner.serverbound(&payload, &mut inner)?;
            out.to_server.extend(inner.to_server);
            self.to_client(login, inner.to_client, out)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_packets_read_and_write() {
        let mut hs = vec![0x02, PROTOCOL];
        wire::put_string(&mut hs, "Notch", 16);
        wire::put_string(&mut hs, "localhost", 255);
        hs.put_i32(25565);
        assert_eq!(split_serverbound(&hs), Ok(Some(hs.len())));
        let handshake = Handshake::read(&hs).unwrap();
        assert_eq!((handshake.protocol, handshake.name.as_str(), handshake.host.as_str(), handshake.port), (51, "Notch", "localhost", 25565));

        let auth = server_auth_data("-", &[1, 2, 3], &[9, 9, 9, 9]);
        assert_eq!(split_clientbound(&auth), Ok(Some(auth.len())));
        let mut answer = vec![0xFC];
        wire::put_array(&mut answer, &[7; 128]);
        wire::put_array(&mut answer, &[8; 128]);
        assert_eq!(split_serverbound(&answer), Ok(Some(answer.len())));
        assert_eq!(read_shared_key(&answer).unwrap(), (vec![7; 128], vec![8; 128]));
        assert_eq!(split_clientbound(&SHARED_KEY_ACK), Ok(Some(5)));
    }

    #[test]
    fn pings_are_answered_in_both_old_forms() {
        let answer = ping_answer(true, PROTOCOL, "Paper-RS", "§aHi", 1, 20);
        let mut r = Reader::new(&answer[1..]);
        assert_eq!(wire::read_string(&mut r, 256).unwrap(), "§1\u{0}51\u{0}Paper-RS\u{0}§aHi\u{0}1\u{0}20");
        let answer = ping_answer(false, PROTOCOL, "Paper-RS", "§aHi", 1, 20);
        assert_eq!(wire::read_string(&mut Reader::new(&answer[1..]), 256).unwrap(), "Hi§1§20");
    }
}
