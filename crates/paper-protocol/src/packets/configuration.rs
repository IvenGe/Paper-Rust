//! Configuration state: registries, tags, feature flags and client settings are exchanged
//! here before (and whenever the server re-enters configuration during) play.

use super::common::define_common_packets;
use super::{packet, packet_id};
use crate::codec::{Decode, Encode, Reader, Result, WriteExt};
use crate::types::KnownPack;
use paper_nbt::Tag;

define_common_packets!(configuration);

// --- clientbound ---------------------------------------------------------------------------

packet! {
    /// Ends configuration; the client validates what it received and acknowledges.
    FinishConfiguration = configuration::clientbound::FINISH_CONFIGURATION {}
}

packet! {
    /// Clears the client's chat state (signed message chain).
    ResetChat = configuration::clientbound::RESET_CHAT {}
}

/// One synchronised registry. Entry order defines the network ids. `data` may be omitted
/// for entries the client can take from a known pack both sides agreed on.
#[derive(Clone, Debug, PartialEq)]
pub struct RegistryData {
    pub registry: String,
    pub entries: Vec<(String, Option<Tag>)>,
}

packet_id!(RegistryData = configuration::clientbound::REGISTRY_DATA);

impl Encode for RegistryData {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.registry);
        w.put_list(&self.entries, |w, (id, data)| {
            w.put_string(id);
            w.put_option(data.as_ref(), |w, tag| w.put_nbt(tag));
        });
    }
}

impl Decode for RegistryData {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let registry = r.identifier()?;
        let entries = r.list(usize::MAX, |r| {
            let id = r.identifier()?;
            let data = if r.bool()? { r.nbt()? } else { None };
            Ok((id, data))
        })?;
        Ok(Self { registry, entries })
    }
}

packet! {
    /// The feature flags enabled on the server (normally just `minecraft:vanilla`).
    UpdateEnabledFeatures = configuration::clientbound::UPDATE_ENABLED_FEATURES {
        features: Vec<String>,
    }
}

packet! {
    /// Offers data packs the client may already have; it answers with the subset it knows.
    SelectKnownPacks = configuration::clientbound::SELECT_KNOWN_PACKS {
        packs: Vec<KnownPack>,
    }
}

packet! {
    /// Shows the server's code of conduct; configuration waits for the acceptance.
    CodeOfConduct = configuration::clientbound::CODE_OF_CONDUCT {
        text: String,
    }
}

// --- serverbound ---------------------------------------------------------------------------

packet! {
    /// Acknowledges [`FinishConfiguration`]; switches the connection to play.
    FinishConfigurationAck = configuration::serverbound::FINISH_CONFIGURATION {}
}

packet! {
    /// The known packs the client shares with the server, in the server's order.
    ServerboundSelectKnownPacks = configuration::serverbound::SELECT_KNOWN_PACKS {
        packs: Vec<KnownPack>,
    }
}

packet! {
    AcceptCodeOfConduct = configuration::serverbound::ACCEPT_CODE_OF_CONDUCT {}
}

/// Every serverbound configuration packet the server understands.
#[derive(Clone, Debug, PartialEq)]
pub enum Serverbound {
    ClientInformation(ClientInformation),
    CookieResponse(CookieResponse),
    CustomPayload(ServerboundCustomPayload),
    FinishConfiguration,
    KeepAlive(ServerboundKeepAlive),
    Pong(Pong),
    ResourcePack(ResourcePackResponse),
    SelectKnownPacks(ServerboundSelectKnownPacks),
    AcceptCodeOfConduct,
    /// A packet this implementation does not act on (e.g. custom click actions).
    Other { id: i32 },
}

impl Serverbound {
    /// Decodes a payload (packet id + body).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        use paper_data::packets::configuration::serverbound as id;
        let mut r = Reader::new(payload);
        let packet_id = r.varint()?;
        let packet = match packet_id {
            id::CLIENT_INFORMATION => Serverbound::ClientInformation(Decode::decode(&mut r)?),
            id::COOKIE_RESPONSE => Serverbound::CookieResponse(Decode::decode(&mut r)?),
            id::CUSTOM_PAYLOAD => Serverbound::CustomPayload(Decode::decode(&mut r)?),
            id::FINISH_CONFIGURATION => Serverbound::FinishConfiguration,
            id::KEEP_ALIVE => Serverbound::KeepAlive(Decode::decode(&mut r)?),
            id::PONG => Serverbound::Pong(Decode::decode(&mut r)?),
            id::RESOURCE_PACK => Serverbound::ResourcePack(Decode::decode(&mut r)?),
            id::SELECT_KNOWN_PACKS => Serverbound::SelectKnownPacks(Decode::decode(&mut r)?),
            id::ACCEPT_CODE_OF_CONDUCT => Serverbound::AcceptCodeOfConduct,
            other if (0..id::NAMES.len() as i32).contains(&other) => return Ok(Serverbound::Other { id: other }),
            other => return Err(crate::codec::DecodeError::UnknownPacket { state: "configuration", id: other }),
        };
        r.finish()?;
        Ok(packet)
    }
}
