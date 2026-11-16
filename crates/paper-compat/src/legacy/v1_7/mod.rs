//! Clients of 1.7 (protocol 4: 1.7.2 to 1.7.5, protocol 5: 1.7.6 to 1.7.10): Netty's framing
//! without compression, entity ids as ints in most packets, block positions as three numbers,
//! chunks as separate id and metadata arrays, items with gzipped NBT, text as JSON, and none of
//! 1.8's blocks, items and mobs.
//!
//! The translator works through the 1.8 one: the server's 26.3 packets become 1.8's there and
//! 1.7's here, and the client's go the other way, as ViaRewind (GPL-3.0 or later) goes from 1.8
//! down to 1.7.

mod clientbound;
pub mod ids;
mod serverbound;

use super::{Legacy18, Phase};
use crate::io::Result;
use crate::{Outcome, Translator, Version};
use paper_protocol::codec::{Reader, WriteExt};
use std::collections::HashMap;

/// A player in the tab list, as 1.7 spawns them: name and skin properties.
struct Listed {
    name: String,
    properties: Vec<(String, String, Option<String>)>,
}

/// One connection of a 1.7 client.
pub struct Legacy17 {
    inner: Legacy18,
    version: &'static Version,
    /// Protocol 5 (1.7.6 on): UUIDs with dashes, skins in player spawns.
    newer: bool,
    players: HashMap<uuid::Uuid, Listed>,
    /// The open windows' 1.7 types.
    windows: HashMap<u8, u8>,
}

impl Legacy17 {
    pub fn new(version: &'static Version) -> Legacy17 {
        Legacy17 { inner: Legacy18::new(version), version, newer: version.protocol >= 5, players: HashMap::new(), windows: HashMap::new() }
    }

    /// What the 1.8 translator made for the client (`login`: in its login phase), as 1.7's.
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

    /// A 1.7 login packet as 1.8's: the encryption answer has short array lengths.
    fn login_serverbound(packet: &[u8], out: &mut Vec<Vec<u8>>) -> Result<()> {
        let mut r = Reader::new(packet);
        let id = r.varint()?;
        if id != super::ids::login::ENCRYPTION_RESPONSE {
            out.push(packet.to_vec());
            return Ok(());
        }
        let mut w = Vec::new();
        w.put_varint(id);
        for _ in 0..2 {
            w.put_byte_array(super::prenetty::wire::read_array(&mut r)?);
        }
        out.push(w);
        Ok(())
    }
}

impl Translator for Legacy17 {
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
            Legacy17::login_serverbound(packet, &mut payloads)?;
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
