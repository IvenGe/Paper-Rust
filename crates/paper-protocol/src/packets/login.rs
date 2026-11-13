//! Login state: authentication, encryption and compression setup.

use super::{packet, packet_id};
use crate::codec::{ByteArray, Decode, Encode, Reader, RemainingBytes, Result, VarInt, WriteExt};
use crate::types::{GameProfile, JsonText};
use uuid::Uuid;

// --- serverbound ---------------------------------------------------------------------------

/// Starts the login. `uuid` is what the client believes its UUID to be; servers ignore it.
#[derive(Clone, Debug, PartialEq)]
pub struct Hello {
    pub name: String,
    pub uuid: Uuid,
}

packet_id!(Hello = login::serverbound::HELLO);

impl Encode for Hello {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.name);
        w.put_uuid(self.uuid);
    }
}

impl Decode for Hello {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { name: r.string(16)?, uuid: r.uuid()? })
    }
}

packet! {
    /// Encryption response: both fields are RSA-encrypted with the server's public key.
    Key = login::serverbound::KEY {
        shared_secret: ByteArray,
        verify_token: ByteArray,
    }
}

/// Answer to a [`CustomQuery`]; `data` is `None` when the client did not understand it.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomQueryAnswer {
    pub message_id: i32,
    pub data: Option<Vec<u8>>,
}

packet_id!(CustomQueryAnswer = login::serverbound::CUSTOM_QUERY_ANSWER);

impl Encode for CustomQueryAnswer {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.message_id);
        match &self.data {
            Some(data) => {
                w.put_bool(true);
                w.put_slice(data);
            }
            None => w.put_bool(false),
        }
    }
}

impl Decode for CustomQueryAnswer {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let message_id = r.varint()?;
        let data = if r.bool()? { Some(r.rest().to_vec()) } else { None };
        Ok(Self { message_id, data })
    }
}

packet! {
    /// Acknowledges [`LoginFinished`]; switches the connection to the configuration state.
    LoginAcknowledged = login::serverbound::LOGIN_ACKNOWLEDGED {}
}

// --- clientbound ---------------------------------------------------------------------------

packet! {
    /// Disconnects during login. The reason is a JSON text component, not NBT.
    LoginDisconnect = login::clientbound::LOGIN_DISCONNECT {
        reason: JsonText,
    }
}

packet! {
    /// Encryption request.
    EncryptionRequest = login::clientbound::HELLO {
        server_id: String,
        public_key: ByteArray,
        verify_token: ByteArray,
        /// Whether the client should authenticate with the session server.
        should_authenticate: bool,
    }
}

packet! {
    /// Login success: the final profile and a per-login session id.
    LoginFinished = login::clientbound::LOGIN_FINISHED {
        profile: GameProfile,
        session_id: Uuid,
    }
}

packet! {
    /// Enables the compressed packet format for everything that follows.
    LoginCompression = login::clientbound::LOGIN_COMPRESSION {
        threshold: VarInt,
    }
}

packet! {
    /// Login plugin request (used e.g. for Velocity modern forwarding).
    CustomQuery = login::clientbound::CUSTOM_QUERY {
        message_id: VarInt,
        channel: String,
        data: RemainingBytes,
    }
}
