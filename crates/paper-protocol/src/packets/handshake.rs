//! Handshake state: a single serverbound packet that selects the next state.

use super::packet_id;
use crate::codec::{Decode, DecodeError, Encode, Reader, Result, WriteExt};

/// What the client wants to do after the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Status,
    Login,
    /// Login caused by a Transfer packet from another server.
    Transfer,
}

/// First packet of every connection.
#[derive(Clone, Debug, PartialEq)]
pub struct Intention {
    pub protocol_version: i32,
    pub server_address: String,
    pub server_port: u16,
    pub intent: Intent,
}

packet_id!(Intention = handshake::serverbound::INTENTION);

impl Encode for Intention {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.protocol_version);
        w.put_string(&self.server_address);
        w.put_u16(self.server_port);
        w.put_varint(match self.intent {
            Intent::Status => 1,
            Intent::Login => 2,
            Intent::Transfer => 3,
        });
    }
}

impl Decode for Intention {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let protocol_version = r.varint()?;
        // Forwarding proxies (BungeeCord) append data to the address, so allow far more
        // than the 255 characters vanilla permits; the frame size still bounds it.
        let server_address = r.string(32767)?;
        let server_port = r.u16()?;
        let intent = match r.varint()? {
            1 => Intent::Status,
            2 => Intent::Login,
            3 => Intent::Transfer,
            other => return Err(DecodeError::InvalidEnum { what: "handshake intent", value: other as i64 }),
        };
        Ok(Self { protocol_version, server_address, server_port, intent })
    }
}
