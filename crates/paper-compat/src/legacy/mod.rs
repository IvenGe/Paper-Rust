//! Clients of 1.8 (protocol 47), from before the flattening, the combat update and the
//! configuration phase: a different packet set, legacy block and item ids, fixed-point entity
//! positions and text as JSON. The translator does for the client what 26.3 expects of it
//! (the configuration phase, teleport confirmations, chunk batch acknowledgements) and keeps
//! what 1.8 needs that 26.3 does not send (where entities are, for relative moves).
//!
//! The formats follow minecraft-data's 1.8 protocol; what corresponds to what follows
//! ViaRewind (1.9 to 1.8) and ViaBackwards.

mod chunk;
mod entities;
mod inventory;
mod play;
pub mod prenetty;
mod serverbound;
mod tables;
pub mod text;
pub mod v1_7;

use crate::io::{self, Result};
use crate::{Outcome, Translator, Version};
use paper_data::packets::{configuration as cfg, login};
use paper_protocol::codec::{BlockPos, Reader, WriteExt};
use std::collections::{HashMap, VecDeque};
use tables::Tables;

/// 1.8's packet ids.
pub(crate) mod ids {
    pub mod login {
        pub const DISCONNECT: i32 = 0;
        pub const ENCRYPTION_REQUEST: i32 = 1;
        pub const SUCCESS: i32 = 2;
        pub const COMPRESS: i32 = 3;
        pub const LOGIN_START: i32 = 0;
        pub const ENCRYPTION_RESPONSE: i32 = 1;
    }
    pub mod cb {
        pub const KEEP_ALIVE: i32 = 0x00;
        pub const LOGIN: i32 = 0x01;
        pub const CHAT: i32 = 0x02;
        pub const UPDATE_TIME: i32 = 0x03;
        pub const ENTITY_EQUIPMENT: i32 = 0x04;
        pub const SPAWN_POSITION: i32 = 0x05;
        pub const UPDATE_HEALTH: i32 = 0x06;
        pub const RESPAWN: i32 = 0x07;
        pub const POSITION: i32 = 0x08;
        pub const HELD_ITEM_SLOT: i32 = 0x09;
        pub const ANIMATION: i32 = 0x0B;
        pub const NAMED_ENTITY_SPAWN: i32 = 0x0C;
        pub const COLLECT: i32 = 0x0D;
        pub const SPAWN_ENTITY: i32 = 0x0E;
        pub const SPAWN_ENTITY_LIVING: i32 = 0x0F;
        pub const SPAWN_ENTITY_PAINTING: i32 = 0x10;
        pub const SPAWN_ENTITY_EXPERIENCE_ORB: i32 = 0x11;
        pub const ENTITY_VELOCITY: i32 = 0x12;
        pub const ENTITY_DESTROY: i32 = 0x13;
        pub const REL_ENTITY_MOVE: i32 = 0x15;
        pub const ENTITY_LOOK: i32 = 0x16;
        pub const ENTITY_MOVE_LOOK: i32 = 0x17;
        pub const ENTITY_TELEPORT: i32 = 0x18;
        pub const ENTITY_HEAD_ROTATION: i32 = 0x19;
        pub const ENTITY_STATUS: i32 = 0x1A;
        pub const ATTACH_ENTITY: i32 = 0x1B;
        pub const ENTITY_METADATA: i32 = 0x1C;
        pub const ENTITY_EFFECT: i32 = 0x1D;
        pub const REMOVE_ENTITY_EFFECT: i32 = 0x1E;
        pub const EXPERIENCE: i32 = 0x1F;
        pub const MAP_CHUNK: i32 = 0x21;
        pub const MULTI_BLOCK_CHANGE: i32 = 0x22;
        pub const BLOCK_CHANGE: i32 = 0x23;
        pub const BLOCK_ACTION: i32 = 0x24;
        pub const BLOCK_BREAK_ANIMATION: i32 = 0x25;
        pub const EXPLOSION: i32 = 0x27;
        pub const WORLD_EVENT: i32 = 0x28;
        pub const NAMED_SOUND_EFFECT: i32 = 0x29;
        pub const WORLD_PARTICLES: i32 = 0x2A;
        pub const GAME_STATE_CHANGE: i32 = 0x2B;
        pub const SPAWN_ENTITY_WEATHER: i32 = 0x2C;
        pub const OPEN_WINDOW: i32 = 0x2D;
        pub const CLOSE_WINDOW: i32 = 0x2E;
        pub const SET_SLOT: i32 = 0x2F;
        pub const WINDOW_ITEMS: i32 = 0x30;
        pub const CRAFT_PROGRESS_BAR: i32 = 0x31;
        pub const TRANSACTION: i32 = 0x32;
        pub const UPDATE_SIGN: i32 = 0x33;
        pub const OPEN_SIGN_ENTITY: i32 = 0x36;
        pub const PLAYER_INFO: i32 = 0x38;
        pub const ABILITIES: i32 = 0x39;
        pub const TAB_COMPLETE: i32 = 0x3A;
        pub const SCOREBOARD_OBJECTIVE: i32 = 0x3B;
        pub const SCOREBOARD_SCORE: i32 = 0x3C;
        pub const SCOREBOARD_DISPLAY_OBJECTIVE: i32 = 0x3D;
        pub const SCOREBOARD_TEAM: i32 = 0x3E;
        pub const CUSTOM_PAYLOAD: i32 = 0x3F;
        pub const KICK_DISCONNECT: i32 = 0x40;
        pub const DIFFICULTY: i32 = 0x41;
        pub const CAMERA: i32 = 0x43;
        pub const WORLD_BORDER: i32 = 0x44;
        pub const TITLE: i32 = 0x45;
        pub const PLAYERLIST_HEADER: i32 = 0x47;
        pub const RESOURCE_PACK_SEND: i32 = 0x48;
    }
    pub mod sb {
        pub const KEEP_ALIVE: i32 = 0x00;
        pub const CHAT: i32 = 0x01;
        pub const USE_ENTITY: i32 = 0x02;
        pub const FLYING: i32 = 0x03;
        pub const POSITION: i32 = 0x04;
        pub const LOOK: i32 = 0x05;
        pub const POSITION_LOOK: i32 = 0x06;
        pub const BLOCK_DIG: i32 = 0x07;
        pub const BLOCK_PLACE: i32 = 0x08;
        pub const HELD_ITEM_SLOT: i32 = 0x09;
        pub const ARM_ANIMATION: i32 = 0x0A;
        pub const ENTITY_ACTION: i32 = 0x0B;
        pub const STEER_VEHICLE: i32 = 0x0C;
        pub const CLOSE_WINDOW: i32 = 0x0D;
        pub const WINDOW_CLICK: i32 = 0x0E;
        pub const TRANSACTION: i32 = 0x0F;
        pub const SET_CREATIVE_SLOT: i32 = 0x10;
        pub const ENCHANT_ITEM: i32 = 0x11;
        pub const UPDATE_SIGN: i32 = 0x12;
        pub const ABILITIES: i32 = 0x13;
        pub const TAB_COMPLETE: i32 = 0x14;
        pub const SETTINGS: i32 = 0x15;
        pub const CLIENT_COMMAND: i32 = 0x16;
        pub const CUSTOM_PAYLOAD: i32 = 0x17;
        pub const SPECTATE: i32 = 0x18;
        pub const RESOURCE_PACK_RECEIVE: i32 = 0x19;
    }
}

/// A 1.8 block position: x in the top 26 bits, then y (12 bits), then z (26 bits).
pub(crate) fn pos18(p: BlockPos) -> i64 {
    ((p.x as i64 & 0x3FF_FFFF) << 38) | ((p.y as i64 & 0xFFF) << 26) | (p.z as i64 & 0x3FF_FFFF)
}

pub(crate) fn from_pos18(v: i64) -> BlockPos {
    BlockPos::new((v >> 38) as i32, (v << 26 >> 52) as i32, (v << 38 >> 38) as i32)
}

/// A fixed-point position (32ths of a block).
pub(crate) fn fixed(v: f64) -> i32 {
    (v * 32.0).floor() as i32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Login,
    /// 26.3's configuration, which the translator goes through for the client.
    Configuration,
    Play,
}

/// One connection of a 1.8 client.
pub struct Legacy18 {
    version: &'static Version,
    tables: &'static Tables,
    phase: Phase,
    /// The player's own entity.
    player: i32,
    world: chunk::World,
    dimension: i8,
    game_mode: u8,
    hardcore: bool,
    difficulty: u8,
    /// Keep-alive ids sent (1.8 has VarInt ids, 26.3 longs).
    keep_alives: VecDeque<(i32, i64)>,
    pub(crate) entities: HashMap<i32, entities::Tracked>,
    /// The tab list: names and skins of players, which spawning one needs.
    players: HashMap<uuid::Uuid, play::ListEntry>,
    /// Where the player is (for teleports relative to it).
    position: (f64, f64, f64),
    rotation: (f32, f32),
    /// Teleports the client has not been told to confirm yet (it has no such step).
    sneaking: bool,
    sprinting: bool,
    /// The last window state id the server sent (26.3 clicks carry it).
    state_id: i32,
    /// The open window: its id and 26.3 menu type.
    window: Option<(i32, i32)>,
    /// Serverbound packets that arrived before play (the client is in play at once).
    early: Vec<Vec<u8>>,
    /// Tab completions asked for, by transaction id: the text typed.
    completions: VecDeque<(i32, String)>,
    next_completion: i32,
    /// Resource packs pushed, for the client's answer (which names none).
    resource_packs: VecDeque<uuid::Uuid>,
    /// Whether the server was told the client has loaded the world (1.8 does not say).
    loaded: bool,
}

/// 1.8's protocol.
pub const PROTOCOL: i32 = 47;

impl Legacy18 {
    pub fn new(version: &'static Version) -> Legacy18 {
        Legacy18 {
            version,
            tables: Tables::load("1.8"),
            phase: Phase::Login,
            player: 0,
            world: chunk::World { min_y: -64, has_sky: true },
            dimension: 0,
            game_mode: 0,
            hardcore: false,
            difficulty: 1,
            keep_alives: VecDeque::new(),
            entities: HashMap::new(),
            players: HashMap::new(),
            position: (0.0, 0.0, 0.0),
            rotation: (0.0, 0.0),
            sneaking: false,
            sprinting: false,
            state_id: 0,
            window: None,
            early: Vec::new(),
            completions: VecDeque::new(),
            next_completion: 1,
            resource_packs: VecDeque::new(),
            loaded: false,
        }
    }

    fn login_clientbound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        let mut r = Reader::new(body);
        let mut w = Vec::new();
        match id {
            login::clientbound::LOGIN_DISCONNECT => {
                let reason = r.string(1 << 18)?;
                w.put_varint(ids::login::DISCONNECT);
                w.put_string(&legacy_json(&reason));
            }
            login::clientbound::HELLO => {
                // Encryption request: 1.8 has no "should authenticate" flag at the end.
                w.put_varint(ids::login::ENCRYPTION_REQUEST);
                w.put_string(&r.string(20)?);
                w.put_byte_array(r.byte_array(1 << 16)?);
                w.put_byte_array(r.byte_array(1 << 16)?);
            }
            login::clientbound::LOGIN_FINISHED => {
                let uuid = r.uuid()?;
                let name = r.string(16)?;
                w.put_varint(ids::login::SUCCESS);
                w.put_string(&uuid.hyphenated().to_string());
                w.put_string(&name);
                // 1.8 goes straight to play; the translator does the configuration.
                let mut ack = Vec::new();
                ack.put_varint(login::serverbound::LOGIN_ACKNOWLEDGED);
                out.to_server.push(ack);
                self.phase = Phase::Configuration;
            }
            login::clientbound::LOGIN_COMPRESSION => {
                w.put_varint(ids::login::COMPRESS);
                w.put_varint(r.varint()?);
            }
            login::clientbound::CUSTOM_QUERY => {
                // A plugin query 1.8 cannot answer: "not understood".
                let mut answer = Vec::new();
                answer.put_varint(login::serverbound::CUSTOM_QUERY_ANSWER);
                answer.put_varint(r.varint()?);
                answer.put_bool(false);
                out.to_server.push(answer);
                return Ok(());
            }
            login::clientbound::COOKIE_REQUEST => {
                let mut answer = Vec::new();
                answer.put_varint(login::serverbound::COOKIE_RESPONSE);
                answer.put_string(&r.string(256)?);
                answer.put_bool(false);
                out.to_server.push(answer);
                return Ok(());
            }
            _ => return Ok(()),
        }
        out.to_client.push(w);
        Ok(())
    }

    /// 26.3's configuration, answered in the client's place.
    fn configuration_clientbound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        let mut r = Reader::new(body);
        let mut answer = Vec::new();
        match id {
            cfg::clientbound::SELECT_KNOWN_PACKS => {
                // The client's settings until it sends its own, then the vanilla pack.
                answer.put_varint(cfg::serverbound::CLIENT_INFORMATION);
                default_client_information(&mut answer);
                out.to_server.push(std::mem::take(&mut answer));
                answer.put_varint(cfg::serverbound::SELECT_KNOWN_PACKS);
                answer.put_varint(1);
                answer.put_string("minecraft");
                answer.put_string("core");
                answer.put_string(paper_data::VERSION_NAME);
            }
            cfg::clientbound::FINISH_CONFIGURATION => {
                answer.put_varint(cfg::serverbound::FINISH_CONFIGURATION);
                self.phase = Phase::Play;
                out.to_server.push(answer);
                // What the client sent meanwhile, now that the server plays.
                for payload in std::mem::take(&mut self.early) {
                    self.serverbound(&payload, out)?;
                }
                return Ok(());
            }
            cfg::clientbound::KEEP_ALIVE => {
                answer.put_varint(cfg::serverbound::KEEP_ALIVE);
                answer.put_i64(r.i64()?);
            }
            cfg::clientbound::PING => {
                answer.put_varint(cfg::serverbound::PONG);
                answer.put_i32(r.i32()?);
            }
            cfg::clientbound::CODE_OF_CONDUCT => {
                answer.put_varint(cfg::serverbound::ACCEPT_CODE_OF_CONDUCT);
            }
            cfg::clientbound::COOKIE_REQUEST => {
                answer.put_varint(cfg::serverbound::COOKIE_RESPONSE);
                answer.put_string(&r.string(256)?);
                answer.put_bool(false);
            }
            cfg::clientbound::DISCONNECT => {
                let reason = r.component()?;
                let mut w = Vec::new();
                w.put_varint(ids::cb::KICK_DISCONNECT);
                w.put_string(&text::json_1_8(&reason.0));
                out.to_client.push(w);
                return Ok(());
            }
            _ => return Ok(()),
        }
        out.to_server.push(answer);
        Ok(())
    }
}

/// A 26.3 JSON text (login disconnect reasons) as 1.8 JSON.
fn legacy_json(json: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(v) => text::json_1_8(&v),
        Err(_) => serde_json::json!({ "text": json }).to_string(),
    }
}

/// The settings of a client that has not said: English, 8 chunks, full chat, all skin parts.
fn default_client_information(w: &mut Vec<u8>) {
    w.put_string("en_us");
    w.put_i8(8);
    w.put_varint(0);
    w.put_bool(true);
    w.put_u8(0x7F);
    w.put_varint(1);
    w.put_bool(false);
    w.put_bool(true);
    w.put_varint(0);
}

impl Translator for Legacy18 {
    fn version(&self) -> &'static Version {
        self.version
    }

    fn clientbound(&mut self, payload: &[u8], out: &mut Outcome) -> Result<()> {
        let (id, body) = io::split(payload)?;
        match self.phase {
            Phase::Login => self.login_clientbound(id, body, out),
            Phase::Configuration => self.configuration_clientbound(id, body, out),
            Phase::Play => self.play_clientbound(id, body, out),
        }
    }

    fn serverbound(&mut self, payload: &[u8], out: &mut Outcome) -> Result<()> {
        let (id, body) = io::split(payload)?;
        match self.phase {
            Phase::Login => {
                let mut r = Reader::new(body);
                let mut w = Vec::new();
                match id {
                    ids::login::LOGIN_START => {
                        w.put_varint(login::serverbound::HELLO);
                        w.put_string(&r.string(16)?);
                        w.put_uuid(uuid::Uuid::nil());
                    }
                    ids::login::ENCRYPTION_RESPONSE => {
                        w.put_varint(login::serverbound::KEY);
                        w.put_byte_array(r.byte_array(1 << 12)?);
                        w.put_byte_array(r.byte_array(1 << 12)?);
                    }
                    _ => return Ok(()),
                }
                out.to_server.push(w);
                Ok(())
            }
            // Play packets before the server plays: kept until it does.
            Phase::Configuration => {
                self.early.push(payload.to_vec());
                Ok(())
            }
            Phase::Play => self.play_serverbound(id, body, out),
        }
    }
}
