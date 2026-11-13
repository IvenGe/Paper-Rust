//! Packets that exist with the same layout in both the configuration and play states.
//!
//! [`define_common_packets!`] stamps the definitions out once per state so that each state
//! module carries its own types with the right packet ids.

use crate::codec::{Decode, DecodeError, Encode, Reader, Result, WriteExt};

/// The client's settings, sent on join and whenever they change.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientSettings {
    pub locale: String,
    pub view_distance: i8,
    /// 0 enabled, 1 commands only, 2 hidden.
    pub chat_mode: i32,
    pub chat_colors: bool,
    /// Bit mask of displayed skin parts (cape, jacket, sleeves, legs, hat).
    pub skin_parts: u8,
    /// 0 left, 1 right.
    pub main_hand: i32,
    pub text_filtering: bool,
    pub allow_server_listings: bool,
    /// 0 all, 1 decreased, 2 minimal.
    pub particle_status: i32,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            locale: "en_us".into(),
            view_distance: 10,
            chat_mode: 0,
            chat_colors: true,
            skin_parts: 0x7F,
            main_hand: 1,
            text_filtering: false,
            allow_server_listings: true,
            particle_status: 0,
        }
    }
}

impl Encode for ClientSettings {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.locale);
        w.put_i8(self.view_distance);
        w.put_varint(self.chat_mode);
        w.put_bool(self.chat_colors);
        w.put_u8(self.skin_parts);
        w.put_varint(self.main_hand);
        w.put_bool(self.text_filtering);
        w.put_bool(self.allow_server_listings);
        w.put_varint(self.particle_status);
    }
}

impl Decode for ClientSettings {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let locale = r.string(16)?;
        let view_distance = r.i8()?;
        let chat_mode = r.varint()?;
        if !(0..=2).contains(&chat_mode) {
            return Err(DecodeError::InvalidEnum { what: "chat mode", value: chat_mode as i64 });
        }
        let chat_colors = r.bool()?;
        let skin_parts = r.u8()?;
        let main_hand = r.varint()?;
        if !(0..=1).contains(&main_hand) {
            return Err(DecodeError::InvalidEnum { what: "main hand", value: main_hand as i64 });
        }
        let text_filtering = r.bool()?;
        let allow_server_listings = r.bool()?;
        let particle_status = r.varint()?;
        if !(0..=2).contains(&particle_status) {
            return Err(DecodeError::InvalidEnum { what: "particle status", value: particle_status as i64 });
        }
        Ok(Self { locale, view_distance, chat_mode, chat_colors, skin_parts, main_hand, text_filtering, allow_server_listings, particle_status })
    }
}

/// The tags of one registry: `(tag name, numeric entry ids)`.
#[derive(Clone, Debug, PartialEq)]
pub struct RegistryTags {
    pub registry: String,
    pub tags: Vec<(String, Vec<i32>)>,
}

impl Encode for RegistryTags {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.registry);
        w.put_list(&self.tags, |w, (name, ids)| {
            w.put_string(name);
            w.put_list(ids, |w, id| w.put_varint(*id));
        });
    }
}

impl Decode for RegistryTags {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let registry = r.identifier()?;
        let tags = r.list(usize::MAX, |r| Ok((r.identifier()?, r.list(usize::MAX, |r| r.varint())?)))?;
        Ok(Self { registry, tags })
    }
}

/// A label for an entry of the server links list: built-in (by id) or custom text.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerLinkLabel {
    /// 0 bug report, 1 community guidelines, 2 support, 3 status, 4 feedback, 5 community,
    /// 6 website, 7 forums, 8 news, 9 announcements.
    BuiltIn(i32),
    Custom(crate::text::Component),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServerLink {
    pub label: ServerLinkLabel,
    pub url: String,
}

impl Encode for ServerLink {
    fn encode(&self, w: &mut Vec<u8>) {
        match &self.label {
            ServerLinkLabel::BuiltIn(id) => {
                w.put_bool(true);
                w.put_varint(*id);
            }
            ServerLinkLabel::Custom(text) => {
                w.put_bool(false);
                w.put_component(text);
            }
        }
        w.put_string(&self.url);
    }
}

impl Decode for ServerLink {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let label = if r.bool()? { ServerLinkLabel::BuiltIn(r.varint()?) } else { ServerLinkLabel::Custom(r.component()?) };
        Ok(Self { label, url: r.string(32767)? })
    }
}

/// Defines the packets shared by the configuration and play states for one state module.
macro_rules! define_common_packets {
    ($state:ident) => {
        // --- clientbound ---

        $crate::packets::packet! {
            /// Plugin message to the client.
            CustomPayload = $state::clientbound::CUSTOM_PAYLOAD {
                channel: String,
                data: $crate::codec::RemainingBytes,
            }
        }

        $crate::packets::packet! {
            /// Closes the connection with a reason shown to the player.
            Disconnect = $state::clientbound::DISCONNECT {
                reason: $crate::text::Component,
            }
        }

        $crate::packets::packet! {
            /// Liveness probe; the client must echo the id within 15 seconds.
            KeepAlive = $state::clientbound::KEEP_ALIVE {
                id: i64,
            }
        }

        $crate::packets::packet! {
            /// Answered by the client with a `Pong` carrying the same id.
            Ping = $state::clientbound::PING {
                id: i32,
            }
        }

        $crate::packets::packet! {
            /// Replaces the tags of the listed registries.
            UpdateTags = $state::clientbound::UPDATE_TAGS {
                registries: Vec<$crate::packets::common::RegistryTags>,
            }
        }

        $crate::packets::packet! {
            /// Asks the client to send back a previously stored cookie.
            CookieRequest = $state::clientbound::COOKIE_REQUEST {
                key: String,
            }
        }

        $crate::packets::packet! {
            /// Stores a cookie on the client (survives transfers between servers).
            StoreCookie = $state::clientbound::STORE_COOKIE {
                key: String,
                payload: $crate::codec::ByteArray,
            }
        }

        $crate::packets::packet! {
            /// Tells the client to reconnect to another server.
            Transfer = $state::clientbound::TRANSFER {
                host: String,
                port: $crate::codec::VarInt,
            }
        }

        $crate::packets::packet! {
            /// Links shown in the client's pause menu.
            ServerLinks = $state::clientbound::SERVER_LINKS {
                links: Vec<$crate::packets::common::ServerLink>,
            }
        }

        $crate::packets::packet! {
            /// Removes one resource pack, or all of them when `uuid` is absent.
            ResourcePackPop = $state::clientbound::RESOURCE_PACK_POP {
                uuid: Option<::uuid::Uuid>,
            }
        }

        $crate::packets::packet! {
            /// Offers a resource pack to the client.
            ResourcePackPush = $state::clientbound::RESOURCE_PACK_PUSH {
                uuid: ::uuid::Uuid,
                url: String,
                /// 40 character hexadecimal SHA-1, or empty.
                hash: String,
                forced: bool,
                prompt: Option<$crate::text::Component>,
            }
        }

        // --- serverbound ---

        $crate::packets::packet! {
            ClientInformation = $state::serverbound::CLIENT_INFORMATION {
                settings: $crate::packets::common::ClientSettings,
            }
        }

        $crate::packets::packet! {
            /// Plugin message from the client.
            ServerboundCustomPayload = $state::serverbound::CUSTOM_PAYLOAD {
                channel: String,
                data: $crate::codec::RemainingBytes,
            }
        }

        $crate::packets::packet! {
            ServerboundKeepAlive = $state::serverbound::KEEP_ALIVE {
                id: i64,
            }
        }

        $crate::packets::packet! {
            Pong = $state::serverbound::PONG {
                id: i32,
            }
        }

        $crate::packets::packet! {
            /// The client's reaction to a resource pack offer.
            ResourcePackResponse = $state::serverbound::RESOURCE_PACK {
                uuid: ::uuid::Uuid,
                /// 0 loaded, 1 declined, 2 download failed, 3 accepted, 4 downloaded,
                /// 5 invalid URL, 6 reload failed, 7 discarded.
                result: $crate::codec::VarInt,
            }
        }

        $crate::packets::packet! {
            CookieResponse = $state::serverbound::COOKIE_RESPONSE {
                key: String,
                payload: Option<$crate::codec::ByteArray>,
            }
        }
    };
}

pub(crate) use define_common_packets;
