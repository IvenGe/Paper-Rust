//! Clientbound play packets.

use crate::codec::{BlockPos, ByteArray, Decode, DecodeError, Encode, Nbt, Reader, RemainingBytes, Result, VarInt, WriteExt};
use crate::packets::{packet, packet_id};
use crate::text::Component;
use crate::types::{Angle, LightData, LpVec3, Property, Slot};
use paper_nbt::Tag;
use uuid::Uuid;

packet! {
    /// Marks the start/end of a bundle: packets between two delimiters are applied in one
    /// client tick.
    BundleDelimiter = play::clientbound::BUNDLE_DELIMITER {}
}

packet! {
    /// Spawns an entity. Never sent for the receiving player's own entity.
    AddEntity = play::clientbound::ADD_ENTITY {
        entity_id: VarInt,
        uuid: Uuid,
        /// Protocol id in `minecraft:entity_type`.
        entity_type: VarInt,
        x: f64,
        y: f64,
        z: f64,
        velocity: LpVec3,
        pitch: Angle,
        yaw: Angle,
        head_yaw: Angle,
        /// Type-specific object data (0 for players).
        data: VarInt,
    }
}

packet! {
    /// 0 leave bed, 1 critical hit, 2 magic critical hit.
    Animate = play::clientbound::ANIMATE {
        entity_id: VarInt,
        animation: u8,
    }
}

packet! {
    /// Confirms a block interaction so the client drops its prediction for `sequence`.
    BlockChangedAck = play::clientbound::BLOCK_CHANGED_ACK {
        sequence: VarInt,
    }
}

packet! {
    /// Block breaking progress overlay (stage 0-9; anything else removes it).
    BlockDestruction = play::clientbound::BLOCK_DESTRUCTION {
        /// Id of the entity breaking the block (one overlay per id).
        entity_id: VarInt,
        pos: BlockPos,
        stage: u8,
    }
}

packet! {
    BlockEntityData = play::clientbound::BLOCK_ENTITY_DATA {
        pos: BlockPos,
        /// Protocol id in `minecraft:block_entity_type`.
        block_entity_type: VarInt,
        data: Nbt,
    }
}

packet! {
    BlockUpdate = play::clientbound::BLOCK_UPDATE {
        pos: BlockPos,
        state: VarInt,
    }
}

/// Several block changes within one chunk section.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionBlocksUpdate {
    pub section_x: i32,
    pub section_y: i32,
    pub section_z: i32,
    /// `(x, y, z, state)` with section-local coordinates 0-15.
    pub blocks: Vec<(u8, u8, u8, u32)>,
}

packet_id!(SectionBlocksUpdate = play::clientbound::SECTION_BLOCKS_UPDATE);

impl Encode for SectionBlocksUpdate {
    fn encode(&self, w: &mut Vec<u8>) {
        let packed = ((self.section_x as i64 & 0x3F_FFFF) << 42) | (self.section_y as i64 & 0xF_FFFF) | ((self.section_z as i64 & 0x3F_FFFF) << 20);
        w.put_i64(packed);
        w.put_list(&self.blocks, |w, (x, y, z, state)| {
            w.put_varlong(((*state as i64) << 12) | ((*x as i64) << 8) | ((*z as i64) << 4) | *y as i64);
        });
    }
}

impl Decode for SectionBlocksUpdate {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let packed = r.i64()?;
        let blocks = r.list(4096, |r| {
            let v = r.varlong()?;
            Ok((((v >> 8) & 15) as u8, (v & 15) as u8, ((v >> 4) & 15) as u8, (v >> 12) as u32))
        })?;
        Ok(Self { section_x: (packed >> 42) as i32, section_y: (packed << 44 >> 44) as i32, section_z: (packed << 22 >> 42) as i32, blocks })
    }
}

/// Boss bar operations.
#[derive(Clone, Debug, PartialEq)]
pub enum BossEventAction {
    Add { title: Component, health: f32, color: i32, division: i32, flags: u8 },
    Remove,
    UpdateHealth(f32),
    UpdateTitle(Component),
    UpdateStyle { color: i32, division: i32 },
    UpdateFlags(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BossEvent {
    pub uuid: Uuid,
    pub action: BossEventAction,
}

packet_id!(BossEvent = play::clientbound::BOSS_EVENT);

impl Encode for BossEvent {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_uuid(self.uuid);
        match &self.action {
            BossEventAction::Add { title, health, color, division, flags } => {
                w.put_varint(0);
                w.put_component(title);
                w.put_f32(*health);
                w.put_varint(*color);
                w.put_varint(*division);
                w.put_u8(*flags);
            }
            BossEventAction::Remove => w.put_varint(1),
            BossEventAction::UpdateHealth(health) => {
                w.put_varint(2);
                w.put_f32(*health);
            }
            BossEventAction::UpdateTitle(title) => {
                w.put_varint(3);
                w.put_component(title);
            }
            BossEventAction::UpdateStyle { color, division } => {
                w.put_varint(4);
                w.put_varint(*color);
                w.put_varint(*division);
            }
            BossEventAction::UpdateFlags(flags) => {
                w.put_varint(5);
                w.put_u8(*flags);
            }
        }
    }
}

packet! {
    ChangeDifficulty = play::clientbound::CHANGE_DIFFICULTY {
        /// 0 peaceful, 1 easy, 2 normal, 3 hard.
        difficulty: u8,
        locked: bool,
    }
}

packet! {
    /// End of a chunk batch; the client answers with its preferred chunks-per-tick rate.
    ChunkBatchFinished = play::clientbound::CHUNK_BATCH_FINISHED {
        batch_size: VarInt,
    }
}

packet! {
    ChunkBatchStart = play::clientbound::CHUNK_BATCH_START {}
}

packet! {
    ClearTitles = play::clientbound::CLEAR_TITLES {
        reset: bool,
    }
}

/// Tab-completion results for a `Command Suggestions Request`.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandSuggestions {
    pub transaction_id: i32,
    /// Start of the text range to replace.
    pub start: i32,
    pub length: i32,
    pub matches: Vec<(String, Option<Component>)>,
}

packet_id!(CommandSuggestions = play::clientbound::COMMAND_SUGGESTIONS);

impl Encode for CommandSuggestions {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.transaction_id);
        w.put_varint(self.start);
        w.put_varint(self.length);
        w.put_list(&self.matches, |w, (text, tooltip)| {
            w.put_string(text);
            w.put_option(tooltip.as_ref(), |w, t| w.put_component(t));
        });
    }
}

impl Decode for CommandSuggestions {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            transaction_id: r.varint()?,
            start: r.varint()?,
            length: r.varint()?,
            matches: r.list(usize::MAX, |r| Ok((r.string(32767)?, r.option(|r| r.component())?)))?,
        })
    }
}

/// How an argument node parses its input: a `minecraft:command_argument_type` id plus the
/// parser-specific properties, already encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parser {
    pub id: i32,
    pub properties: Vec<u8>,
}

impl Parser {
    fn named(name: &str, properties: Vec<u8>) -> Parser {
        let id = paper_data::registries::id("minecraft:command_argument_type", name)
            .unwrap_or_else(|| panic!("unknown command argument type {name}")) as i32;
        Parser { id, properties }
    }

    /// A parser without properties, by registry name (e.g. `minecraft:game_profile`).
    pub fn simple(name: &str) -> Parser {
        Parser::named(name, Vec::new())
    }

    pub fn bool() -> Parser {
        Parser::simple("brigadier:bool")
    }

    /// `brigadier:string`: 0 single word, 1 quotable phrase, 2 greedy phrase.
    pub fn string(kind: i32) -> Parser {
        let mut p = Vec::new();
        p.put_varint(kind);
        Parser::named("brigadier:string", p)
    }

    pub fn word() -> Parser {
        Parser::string(0)
    }

    pub fn greedy_string() -> Parser {
        Parser::string(2)
    }

    pub fn integer(min: Option<i32>, max: Option<i32>) -> Parser {
        let mut p = vec![min.is_some() as u8 | (max.is_some() as u8) << 1];
        if let Some(v) = min {
            p.put_i32(v);
        }
        if let Some(v) = max {
            p.put_i32(v);
        }
        Parser::named("brigadier:integer", p)
    }

    pub fn long(min: Option<i64>, max: Option<i64>) -> Parser {
        let mut p = vec![min.is_some() as u8 | (max.is_some() as u8) << 1];
        if let Some(v) = min {
            p.put_i64(v);
        }
        if let Some(v) = max {
            p.put_i64(v);
        }
        Parser::named("brigadier:long", p)
    }

    pub fn float(min: Option<f32>, max: Option<f32>) -> Parser {
        let mut p = vec![min.is_some() as u8 | (max.is_some() as u8) << 1];
        if let Some(v) = min {
            p.put_f32(v);
        }
        if let Some(v) = max {
            p.put_f32(v);
        }
        Parser::named("brigadier:float", p)
    }

    pub fn double(min: Option<f64>, max: Option<f64>) -> Parser {
        let mut p = vec![min.is_some() as u8 | (max.is_some() as u8) << 1];
        if let Some(v) = min {
            p.put_f64(v);
        }
        if let Some(v) = max {
            p.put_f64(v);
        }
        Parser::named("brigadier:double", p)
    }

    /// `minecraft:entity`: a selector, player name or UUID.
    pub fn entity(single: bool, players_only: bool) -> Parser {
        Parser::named("minecraft:entity", vec![single as u8 | (players_only as u8) << 1])
    }

    pub fn score_holder(multiple: bool) -> Parser {
        Parser::named("minecraft:score_holder", vec![multiple as u8])
    }

    /// `minecraft:time` with a minimum duration in ticks.
    pub fn time(min: i32) -> Parser {
        let mut p = Vec::new();
        p.put_i32(min);
        Parser::named("minecraft:time", p)
    }

    /// One of the registry-keyed parsers (`minecraft:resource`, `minecraft:resource_key`,
    /// `minecraft:resource_or_tag`, `minecraft:resource_or_tag_key`, `minecraft:resource_selector`).
    pub fn registry(parser: &str, registry: &str) -> Parser {
        let mut p = Vec::new();
        p.put_string(registry);
        Parser::named(parser, p)
    }
}

/// The kind of a command graph node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Root,
    Literal(String),
    Argument {
        name: String,
        parser: Parser,
        /// Suggestion provider, e.g. `minecraft:ask_server`.
        suggestions: Option<String>,
    },
}

/// One node of the command graph sent in [`Commands`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandNode {
    pub kind: NodeKind,
    pub executable: bool,
    /// The node needs a permission level above 0 (client-side hint only).
    pub restricted: bool,
    pub children: Vec<i32>,
    pub redirect: Option<i32>,
}

impl CommandNode {
    pub fn root() -> Self {
        Self { kind: NodeKind::Root, executable: false, restricted: false, children: Vec::new(), redirect: None }
    }

    pub fn literal(name: impl Into<String>) -> Self {
        Self { kind: NodeKind::Literal(name.into()), executable: false, restricted: false, children: Vec::new(), redirect: None }
    }

    pub fn argument(name: impl Into<String>, parser: Parser) -> Self {
        Self {
            kind: NodeKind::Argument { name: name.into(), parser, suggestions: None },
            executable: false,
            restricted: false,
            children: Vec::new(),
            redirect: None,
        }
    }

    pub fn executable(mut self) -> Self {
        self.executable = true;
        self
    }

    /// Ask the server for suggestions while the player types this argument.
    pub fn ask_server(mut self) -> Self {
        if let NodeKind::Argument { suggestions, .. } = &mut self.kind {
            *suggestions = Some("minecraft:ask_server".into());
        }
        self
    }
}

/// The command graph the client uses for syntax highlighting and tab completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commands {
    pub nodes: Vec<CommandNode>,
    pub root_index: i32,
}

packet_id!(Commands = play::clientbound::COMMANDS);

impl Encode for Commands {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_list(&self.nodes, |w, node| {
            let mut flags = match node.kind {
                NodeKind::Root => 0u8,
                NodeKind::Literal(_) => 1,
                NodeKind::Argument { .. } => 2,
            };
            if node.executable {
                flags |= 0x04;
            }
            if node.redirect.is_some() {
                flags |= 0x08;
            }
            if matches!(&node.kind, NodeKind::Argument { suggestions: Some(_), .. }) {
                flags |= 0x10;
            }
            if node.restricted {
                flags |= 0x20;
            }
            w.put_u8(flags);
            w.put_list(&node.children, |w, c| w.put_varint(*c));
            if let Some(redirect) = node.redirect {
                w.put_varint(redirect);
            }
            match &node.kind {
                NodeKind::Root => {}
                NodeKind::Literal(name) => w.put_string(name),
                NodeKind::Argument { name, parser, suggestions } => {
                    w.put_string(name);
                    w.put_varint(parser.id);
                    w.put_slice(&parser.properties);
                    if let Some(s) = suggestions {
                        w.put_string(s);
                    }
                }
            }
        });
        w.put_varint(self.root_index);
    }
}

impl Parser {
    /// Reads a parser id and its properties; the property layout depends on the parser.
    fn decode(r: &mut Reader<'_>) -> Result<Parser> {
        let id = r.varint()?;
        let before = r.peek();
        let name = paper_data::registries::name("minecraft:command_argument_type", id as u32).unwrap_or("");
        match name {
            "brigadier:string" => {
                r.varint()?;
            }
            "brigadier:integer" | "brigadier:float" | "brigadier:long" | "brigadier:double" => {
                let flags = r.u8()?;
                let width = if matches!(name, "brigadier:integer" | "brigadier:float") { 4 } else { 8 };
                for bit in [1, 2] {
                    if flags & bit != 0 {
                        r.take(width)?;
                    }
                }
            }
            "minecraft:entity" | "minecraft:score_holder" => {
                r.u8()?;
            }
            "minecraft:time" => {
                r.i32()?;
            }
            "minecraft:resource" | "minecraft:resource_key" | "minecraft:resource_or_tag" | "minecraft:resource_or_tag_key" | "minecraft:resource_selector" => {
                r.string(32767)?;
            }
            _ => {}
        }
        let used = before.len() - r.remaining();
        Ok(Parser { id, properties: before[..used].to_vec() })
    }
}

impl Decode for Commands {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let nodes = r.list(1 << 20, |r| {
            let flags = r.u8()?;
            let children = r.list(1 << 20, |r| r.varint())?;
            let redirect = if flags & 0x08 != 0 { Some(r.varint()?) } else { None };
            let kind = match flags & 0x03 {
                0 => NodeKind::Root,
                1 => NodeKind::Literal(r.string(32767)?),
                _ => {
                    let name = r.string(32767)?;
                    let parser = Parser::decode(r)?;
                    let suggestions = if flags & 0x10 != 0 { Some(r.string(32767)?) } else { None };
                    NodeKind::Argument { name, parser, suggestions }
                }
            };
            Ok(CommandNode { kind, executable: flags & 0x04 != 0, restricted: flags & 0x20 != 0, children, redirect })
        })?;
        Ok(Self { nodes, root_index: r.varint()? })
    }
}

packet! {
    ContainerClose = play::clientbound::CONTAINER_CLOSE {
        window_id: VarInt,
    }
}

/// Replaces the whole contents of a window (0 = the player inventory).
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerSetContent {
    pub window_id: i32,
    pub state_id: i32,
    pub slots: Vec<Slot>,
    pub carried: Slot,
}

packet_id!(ContainerSetContent = play::clientbound::CONTAINER_SET_CONTENT);

impl Encode for ContainerSetContent {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.window_id);
        w.put_varint(self.state_id);
        w.put_list(&self.slots, |w, s| s.encode_trusted(w));
        self.carried.encode_trusted(w);
    }
}

impl Decode for ContainerSetContent {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { window_id: r.varint()?, state_id: r.varint()?, slots: r.list(4096, Slot::decode_trusted)?, carried: Slot::decode_trusted(r)? })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContainerSetSlot {
    pub window_id: i32,
    pub state_id: i32,
    pub slot: i16,
    pub item: Slot,
}

packet_id!(ContainerSetSlot = play::clientbound::CONTAINER_SET_SLOT);

impl Encode for ContainerSetSlot {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.window_id);
        w.put_varint(self.state_id);
        w.put_i16(self.slot);
        self.item.encode_trusted(w);
    }
}

impl Decode for ContainerSetSlot {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { window_id: r.varint()?, state_id: r.varint()?, slot: r.i16()?, item: Slot::decode_trusted(r)? })
    }
}

/// A chat message without signature data, formatted on the client by a chat type.
#[derive(Clone, Debug, PartialEq)]
pub struct DisguisedChat {
    pub message: Component,
    /// Network id in the synchronised `minecraft:chat_type` registry.
    pub chat_type: i32,
    pub sender_name: Component,
    pub target_name: Option<Component>,
}

packet_id!(DisguisedChat = play::clientbound::DISGUISED_CHAT);

impl Encode for DisguisedChat {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_component(&self.message);
        // "ID or X": registry id + 1; 0 would introduce an inline chat type definition.
        w.put_varint(self.chat_type + 1);
        w.put_component(&self.sender_name);
        w.put_option(self.target_name.as_ref(), |w, t| w.put_component(t));
    }
}

impl Decode for DisguisedChat {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let message = r.component()?;
        let chat_type = r.varint()? - 1;
        if chat_type < 0 {
            return Err(DecodeError::Other("inline chat types are not supported".into()));
        }
        Ok(Self { message, chat_type, sender_name: r.component()?, target_name: r.option(|r| r.component())? })
    }
}

packet! {
    /// Entity status (animations and effects keyed by a per-entity-type byte).
    EntityEvent = play::clientbound::ENTITY_EVENT {
        entity_id: i32,
        status: i8,
    }
}

/// Absolute entity position and rotation ("teleport entity" in older documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct EntityPositionSync {
    pub entity_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

packet_id!(EntityPositionSync = play::clientbound::ENTITY_POSITION_SYNC);

impl Encode for EntityPositionSync {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.entity_id);
        // Position path type 0: a single linear end position.
        w.put_varint(0);
        w.put_f64(self.x);
        w.put_f64(self.y);
        w.put_f64(self.z);
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
        w.put_bool(self.on_ground);
    }
}

impl Decode for EntityPositionSync {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let entity_id = r.varint()?;
        let (x, y, z) = match r.varint()? {
            0 => (r.f64()?, r.f64()?, r.f64()?),
            1 => {
                // Stepped path: the last step is where the entity ends up.
                let steps = r.list(1024, |r| Ok((r.f64()?, r.f64()?, r.f64()?, r.varint()?)))?;
                steps.last().map(|s| (s.0, s.1, s.2)).ok_or_else(|| DecodeError::Other("empty position path".into()))?
            }
            other => return Err(DecodeError::InvalidEnum { what: "position path type", value: other as i64 }),
        };
        Ok(Self { entity_id, x, y, z, yaw: r.f32()?, pitch: r.f32()?, on_ground: r.bool()? })
    }
}

/// Unloads a chunk column. Z comes first on the wire (the client reads one long).
#[derive(Clone, Debug, PartialEq)]
pub struct ForgetLevelChunk {
    pub x: i32,
    pub z: i32,
}

packet_id!(ForgetLevelChunk = play::clientbound::FORGET_LEVEL_CHUNK);

impl Encode for ForgetLevelChunk {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_i32(self.z);
        w.put_i32(self.x);
    }
}

impl Decode for ForgetLevelChunk {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let z = r.i32()?;
        Ok(Self { x: r.i32()?, z })
    }
}

/// Event ids for [`GameEvent`].
pub mod game_event {
    pub const NO_RESPAWN_BLOCK: u8 = 0;
    pub const START_RAINING: u8 = 1;
    pub const STOP_RAINING: u8 = 2;
    pub const CHANGE_GAME_MODE: u8 = 3;
    pub const WIN_GAME: u8 = 4;
    pub const RAIN_LEVEL_CHANGE: u8 = 7;
    pub const THUNDER_LEVEL_CHANGE: u8 = 8;
    pub const IMMEDIATE_RESPAWN: u8 = 11;
    /// Required after every Login/Respawn before the client leaves the loading screen.
    pub const START_WAITING_FOR_CHUNKS: u8 = 13;
}

packet! {
    GameEvent = play::clientbound::GAME_EVENT {
        event: u8,
        value: f32,
    }
}

packet! {
    /// Plays the damage tilt animation.
    HurtAnimation = play::clientbound::HURT_ANIMATION {
        entity_id: VarInt,
        yaw: f32,
    }
}

/// A block entity inside a chunk packet.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkBlockEntity {
    /// `(x & 15) << 4 | (z & 15)`.
    pub packed_xz: u8,
    pub y: i16,
    pub block_entity_type: i32,
    pub data: Option<Tag>,
}

/// A full chunk column with its lighting.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelChunkWithLight {
    pub x: i32,
    pub z: i32,
    /// `(heightmap type id, packed heights)`; 1 world surface, 4 motion blocking,
    /// 5 motion blocking without leaves.
    pub heightmaps: Vec<(i32, Vec<i64>)>,
    /// The chunk sections, bottom to top, in the chunk section wire format.
    pub data: Vec<u8>,
    pub block_entities: Vec<ChunkBlockEntity>,
    pub light: LightData,
}

packet_id!(LevelChunkWithLight = play::clientbound::LEVEL_CHUNK_WITH_LIGHT);

impl Encode for LevelChunkWithLight {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_i32(self.x);
        w.put_i32(self.z);
        w.put_list(&self.heightmaps, |w, (ty, data)| {
            w.put_varint(*ty);
            w.put_list(data, |w, l| w.put_i64(*l));
        });
        w.put_byte_array(&self.data);
        w.put_list(&self.block_entities, |w, be| {
            w.put_u8(be.packed_xz);
            w.put_i16(be.y);
            w.put_varint(be.block_entity_type);
            match &be.data {
                Some(tag) => w.put_nbt(tag),
                None => paper_nbt::write_network_end(w),
            }
        });
        self.light.encode(w);
    }
}

impl Decode for LevelChunkWithLight {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            x: r.i32()?,
            z: r.i32()?,
            heightmaps: r.list(16, |r| Ok((r.varint()?, r.list(1024, |r| r.i64())?)))?,
            data: r.byte_array(usize::MAX)?.to_vec(),
            block_entities: r.list(usize::MAX, |r| Ok(ChunkBlockEntity { packed_xz: r.u8()?, y: r.i16()?, block_entity_type: r.varint()?, data: r.nbt()? }))?,
            light: LightData::decode(r)?,
        })
    }
}

packet! {
    /// A world event: sounds and particle effects tied to a block position.
    /// Event 2001 is "block broken" with the block state id as data.
    LevelEvent = play::clientbound::LEVEL_EVENT {
        event: i32,
        pos: BlockPos,
        data: i32,
        /// Play at full volume regardless of distance.
        global: bool,
    }
}

packet! {
    /// Replaces the light of the listed sections of a loaded chunk.
    LightUpdate = play::clientbound::LIGHT_UPDATE {
        x: VarInt,
        z: VarInt,
        light: LightData,
    }
}

/// The fields describing the world a player (re)spawns into, shared by Login and Respawn.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnInfo {
    /// Network id in the synchronised `minecraft:dimension_type` registry.
    pub dimension_type: i32,
    pub dimension_name: String,
    pub hashed_seed: i64,
    /// 0 survival, 1 creative, 2 adventure, 3 spectator.
    pub game_mode: i32,
    pub previous_game_mode: Option<i32>,
    pub is_debug: bool,
    pub is_flat: bool,
    pub death_location: Option<(String, BlockPos)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
}

impl Encode for SpawnInfo {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.dimension_type);
        w.put_string(&self.dimension_name);
        w.put_i64(self.hashed_seed);
        w.put_varint(self.game_mode);
        w.put_varint(self.previous_game_mode.map_or(0, |m| m + 1));
        w.put_bool(self.is_debug);
        w.put_bool(self.is_flat);
        w.put_option(self.death_location.as_ref(), |w, (dimension, pos)| {
            w.put_string(dimension);
            w.put_position(*pos);
        });
        w.put_varint(self.portal_cooldown);
        w.put_varint(self.sea_level);
    }
}

impl Decode for SpawnInfo {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            dimension_type: r.varint()?,
            dimension_name: r.identifier()?,
            hashed_seed: r.i64()?,
            game_mode: r.varint()?,
            previous_game_mode: match r.varint()? {
                0 => None,
                n => Some(n - 1),
            },
            is_debug: r.bool()?,
            is_flat: r.bool()?,
            death_location: r.option(|r| Ok((r.identifier()?, r.position()?)))?,
            portal_cooldown: r.varint()?,
            sea_level: r.varint()?,
        })
    }
}

packet! {
    /// First play packet: the player's entity id and the world they join.
    Login = play::clientbound::LOGIN {
        entity_id: i32,
        hardcore: bool,
        dimension_names: Vec<String>,
        max_players: VarInt,
        view_distance: VarInt,
        simulation_distance: VarInt,
        reduced_debug_info: bool,
        enable_respawn_screen: bool,
        limited_crafting: bool,
        spawn: SpawnInfo,
        online_mode: bool,
        enforces_secure_chat: bool,
    }
}

/// Relative entity movement. Deltas are fixed-point with 12 fractional bits
/// (`(current - previous) * 4096`), so a single packet covers at most 8 blocks per axis.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveEntityPos {
    pub entity_id: i32,
    pub dx: i16,
    pub dy: i16,
    pub dz: i16,
    pub on_ground: bool,
}

packet_id!(MoveEntityPos = play::clientbound::MOVE_ENTITY_POS);

fn put_single_delta(w: &mut Vec<u8>, on_ground: bool, dx: i16, dy: i16, dz: i16) {
    // Properties: bit 0 on-ground, remaining bits the step count (0 = one linear delta).
    w.put_varint(on_ground as i32);
    w.put_i16(dx);
    w.put_i16(dy);
    w.put_i16(dz);
}

/// Reads the delta block of a movement packet, summing stepped deltas into one.
fn get_delta(r: &mut Reader<'_>) -> Result<(bool, i16, i16, i16)> {
    let properties = r.varint()?;
    let on_ground = properties & 1 != 0;
    let steps = (properties as u32) >> 1;
    if steps == 0 {
        return Ok((on_ground, r.i16()?, r.i16()?, r.i16()?));
    }
    let (mut dx, mut dy, mut dz) = (0i16, 0i16, 0i16);
    for _ in 0..steps.min(1024) {
        let _tick_offset = r.varint()?;
        dx = dx.wrapping_add(r.i16()?);
        dy = dy.wrapping_add(r.i16()?);
        dz = dz.wrapping_add(r.i16()?);
    }
    Ok((on_ground, dx, dy, dz))
}

impl Encode for MoveEntityPos {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.entity_id);
        put_single_delta(w, self.on_ground, self.dx, self.dy, self.dz);
    }
}

impl Decode for MoveEntityPos {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let entity_id = r.varint()?;
        let (on_ground, dx, dy, dz) = get_delta(r)?;
        Ok(Self { entity_id, dx, dy, dz, on_ground })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MoveEntityPosRot {
    pub entity_id: i32,
    pub dx: i16,
    pub dy: i16,
    pub dz: i16,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

packet_id!(MoveEntityPosRot = play::clientbound::MOVE_ENTITY_POS_ROT);

impl Encode for MoveEntityPosRot {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.entity_id);
        put_single_delta(w, self.on_ground, self.dx, self.dy, self.dz);
        w.put_angle(self.yaw);
        w.put_angle(self.pitch);
    }
}

impl Decode for MoveEntityPosRot {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let entity_id = r.varint()?;
        let (on_ground, dx, dy, dz) = get_delta(r)?;
        Ok(Self { entity_id, dx, dy, dz, yaw: r.angle()?, pitch: r.angle()?, on_ground })
    }
}

packet! {
    MoveEntityRot = play::clientbound::MOVE_ENTITY_ROT {
        entity_id: VarInt,
        on_ground: bool,
        yaw: Angle,
        pitch: Angle,
    }
}

packet! {
    /// Opens a container window of a `minecraft:menu` type.
    OpenScreen = play::clientbound::OPEN_SCREEN {
        window_id: VarInt,
        menu_type: VarInt,
        title: Component,
    }
}

/// Bit flags for [`PlayerAbilities`].
pub mod abilities {
    pub const INVULNERABLE: i8 = 0x01;
    pub const FLYING: i8 = 0x02;
    pub const ALLOW_FLYING: i8 = 0x04;
    pub const INSTANT_BREAK: i8 = 0x08;
}

packet! {
    PlayerAbilities = play::clientbound::PLAYER_ABILITIES {
        flags: i8,
        flying_speed: f32,
        fov_modifier: f32,
    }
}

packet! {
    PlayerInfoRemove = play::clientbound::PLAYER_INFO_REMOVE {
        uuids: Vec<Uuid>,
    }
}

/// Action bits for [`PlayerInfoUpdate`].
pub mod player_info {
    pub const ADD_PLAYER: u8 = 0x01;
    pub const INITIALIZE_CHAT: u8 = 0x02;
    pub const UPDATE_GAME_MODE: u8 = 0x04;
    pub const UPDATE_LISTED: u8 = 0x08;
    pub const UPDATE_LATENCY: u8 = 0x10;
    pub const UPDATE_DISPLAY_NAME: u8 = 0x20;
    pub const UPDATE_LIST_PRIORITY: u8 = 0x40;
    pub const UPDATE_HAT: u8 = 0x80;
}

/// One player in a [`PlayerInfoUpdate`]. Only the fields selected by the packet's
/// `actions` are transmitted.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerInfoEntry {
    pub uuid: Uuid,
    pub name: String,
    pub properties: Vec<Property>,
    pub game_mode: i32,
    pub listed: bool,
    pub latency: i32,
    pub display_name: Option<Component>,
    pub list_priority: i32,
    pub show_hat: bool,
}

/// Adds or updates entries of the player list (tab list, skins, name completion).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerInfoUpdate {
    pub actions: u8,
    pub entries: Vec<PlayerInfoEntry>,
}

packet_id!(PlayerInfoUpdate = play::clientbound::PLAYER_INFO_UPDATE);

impl Encode for PlayerInfoUpdate {
    fn encode(&self, w: &mut Vec<u8>) {
        use player_info::*;
        w.put_u8(self.actions);
        w.put_list(&self.entries, |w, e| {
            w.put_uuid(e.uuid);
            if self.actions & ADD_PLAYER != 0 {
                w.put_string(&e.name);
                e.properties.encode(w);
            }
            if self.actions & INITIALIZE_CHAT != 0 {
                // No chat session: this server does not relay signed chat.
                w.put_bool(false);
            }
            if self.actions & UPDATE_GAME_MODE != 0 {
                w.put_varint(e.game_mode);
            }
            if self.actions & UPDATE_LISTED != 0 {
                w.put_bool(e.listed);
            }
            if self.actions & UPDATE_LATENCY != 0 {
                w.put_varint(e.latency);
            }
            if self.actions & UPDATE_DISPLAY_NAME != 0 {
                w.put_option(e.display_name.as_ref(), |w, c| w.put_component(c));
            }
            if self.actions & UPDATE_LIST_PRIORITY != 0 {
                w.put_varint(e.list_priority);
            }
            if self.actions & UPDATE_HAT != 0 {
                w.put_bool(e.show_hat);
            }
        });
    }
}

impl Decode for PlayerInfoUpdate {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        use player_info::*;
        let actions = r.u8()?;
        let entries = r.list(usize::MAX, |r| {
            let mut e = PlayerInfoEntry {
                uuid: r.uuid()?,
                name: String::new(),
                properties: Vec::new(),
                game_mode: 0,
                listed: false,
                latency: 0,
                display_name: None,
                list_priority: 0,
                show_hat: false,
            };
            if actions & ADD_PLAYER != 0 {
                e.name = r.string(16)?;
                e.properties = r.list(16, Property::decode)?;
            }
            if actions & INITIALIZE_CHAT != 0 && r.bool()? {
                let _session = r.uuid()?;
                let _expires = r.i64()?;
                let _key = r.byte_array(512)?;
                let _signature = r.byte_array(4096)?;
            }
            if actions & UPDATE_GAME_MODE != 0 {
                e.game_mode = r.varint()?;
            }
            if actions & UPDATE_LISTED != 0 {
                e.listed = r.bool()?;
            }
            if actions & UPDATE_LATENCY != 0 {
                e.latency = r.varint()?;
            }
            if actions & UPDATE_DISPLAY_NAME != 0 {
                e.display_name = r.option(|r| r.component())?;
            }
            if actions & UPDATE_LIST_PRIORITY != 0 {
                e.list_priority = r.varint()?;
            }
            if actions & UPDATE_HAT != 0 {
                e.show_hat = r.bool()?;
            }
            Ok(e)
        })?;
        Ok(Self { actions, entries })
    }
}

packet! {
    /// Teleports the receiving player; must be confirmed with the same teleport id.
    PlayerPosition = play::clientbound::PLAYER_POSITION {
        teleport_id: VarInt,
        x: f64,
        y: f64,
        z: f64,
        velocity_x: f64,
        velocity_y: f64,
        velocity_z: f64,
        yaw: f32,
        pitch: f32,
        /// See [`crate::types::teleport_flags`].
        flags: i32,
    }
}

packet! {
    /// Answer to a serverbound play `Ping Request`.
    PongResponse = play::clientbound::PONG_RESPONSE {
        payload: i64,
    }
}

packet! {
    RemoveEntities = play::clientbound::REMOVE_ENTITIES {
        entity_ids: Vec<VarInt>,
    }
}

packet! {
    /// Moves the player to another world (or respawns them).
    Respawn = play::clientbound::RESPAWN {
        spawn: SpawnInfo,
        /// 0x01 keep attributes, 0x02 keep entity metadata.
        data_kept: i8,
    }
}

packet! {
    RotateHead = play::clientbound::ROTATE_HEAD {
        entity_id: VarInt,
        head_yaw: Angle,
    }
}

packet! {
    /// MOTD and icon shown for this server in the client's server list entry.
    ServerData = play::clientbound::SERVER_DATA {
        motd: Component,
        icon: Option<ByteArray>,
    }
}

packet! {
    SetActionBarText = play::clientbound::SET_ACTION_BAR_TEXT {
        text: Component,
    }
}

packet! {
    /// Centre of the client's chunk loading area.
    SetChunkCacheCenter = play::clientbound::SET_CHUNK_CACHE_CENTER {
        x: VarInt,
        z: VarInt,
    }
}

packet! {
    SetChunkCacheRadius = play::clientbound::SET_CHUNK_CACHE_RADIUS {
        view_distance: VarInt,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetCursorItem {
    pub item: Slot,
}

packet_id!(SetCursorItem = play::clientbound::SET_CURSOR_ITEM);

impl Encode for SetCursorItem {
    fn encode(&self, w: &mut Vec<u8>) {
        self.item.encode_trusted(w);
    }
}

packet! {
    /// World spawn point (also where compasses point).
    SetDefaultSpawnPosition = play::clientbound::SET_DEFAULT_SPAWN_POSITION {
        dimension: String,
        pos: BlockPos,
        yaw: f32,
        pitch: f32,
    }
}

packet! {
    /// Entity metadata: the body is a pre-encoded metadata list including its terminator.
    SetEntityData = play::clientbound::SET_ENTITY_DATA {
        entity_id: VarInt,
        metadata: RemainingBytes,
    }
}

packet! {
    SetEntityMotion = play::clientbound::SET_ENTITY_MOTION {
        entity_id: VarInt,
        velocity: LpVec3,
    }
}

/// Equipment slot ids for [`SetEquipment`].
pub mod equipment_slot {
    pub const MAIN_HAND: u8 = 0;
    pub const OFF_HAND: u8 = 1;
    pub const BOOTS: u8 = 2;
    pub const LEGGINGS: u8 = 3;
    pub const CHESTPLATE: u8 = 4;
    pub const HELMET: u8 = 5;
    pub const BODY: u8 = 6;
    pub const SADDLE: u8 = 7;
}

/// What an entity holds and wears, as seen by other players.
#[derive(Clone, Debug, PartialEq)]
pub struct SetEquipment {
    pub entity_id: i32,
    pub equipment: Vec<(u8, Slot)>,
}

packet_id!(SetEquipment = play::clientbound::SET_EQUIPMENT);

impl Encode for SetEquipment {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.entity_id);
        for (i, (slot, item)) in self.equipment.iter().enumerate() {
            // The top bit says another entry follows.
            let more = i + 1 < self.equipment.len();
            w.put_u8(*slot | if more { 0x80 } else { 0 });
            item.encode_trusted(w);
        }
    }
}

impl Decode for SetEquipment {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let entity_id = r.varint()?;
        let mut equipment = Vec::new();
        loop {
            let slot = r.u8()?;
            equipment.push((slot & 0x7F, Slot::decode_trusted(r)?));
            if slot & 0x80 == 0 {
                break;
            }
        }
        Ok(Self { entity_id, equipment })
    }
}

packet! {
    SetExperience = play::clientbound::SET_EXPERIENCE {
        /// Progress to the next level, 0-1.
        bar: f32,
        level: VarInt,
        total: VarInt,
    }
}

packet! {
    SetHealth = play::clientbound::SET_HEALTH {
        health: f32,
        food: VarInt,
        saturation: f32,
    }
}

packet! {
    SetHeldSlot = play::clientbound::SET_HELD_SLOT {
        slot: VarInt,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetPlayerInventory {
    /// Index in the player inventory (not a window slot index).
    pub slot: i32,
    pub item: Slot,
}

packet_id!(SetPlayerInventory = play::clientbound::SET_PLAYER_INVENTORY);

impl Encode for SetPlayerInventory {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.slot);
        self.item.encode_trusted(w);
    }
}

packet! {
    SetSimulationDistance = play::clientbound::SET_SIMULATION_DISTANCE {
        simulation_distance: VarInt,
    }
}

packet! {
    SetSubtitleText = play::clientbound::SET_SUBTITLE_TEXT {
        text: Component,
    }
}

/// The state of one `minecraft:world_clock` in a [`SetTime`] packet.
#[derive(Clone, Debug, PartialEq)]
pub struct ClockState {
    /// Network id in the synchronised `minecraft:world_clock` registry.
    pub clock: i32,
    pub time: i64,
    pub fraction: f32,
    /// Clock ticks per client tick (0 freezes the clock on the client).
    pub rate: f32,
}

impl Encode for ClockState {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.clock);
        w.put_varlong(self.time);
        w.put_f32(self.fraction);
        w.put_f32(self.rate);
    }
}

impl Decode for ClockState {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { clock: r.varint()?, time: r.varlong()?, fraction: r.f32()?, rate: r.f32()? })
    }
}

packet! {
    /// World age and the clocks that drive the day/night cycle.
    SetTime = play::clientbound::SET_TIME {
        world_age: i64,
        clocks: Vec<ClockState>,
    }
}

packet! {
    SetTitleText = play::clientbound::SET_TITLE_TEXT {
        text: Component,
    }
}

packet! {
    /// Title timings in ticks.
    SetTitlesAnimation = play::clientbound::SET_TITLES_ANIMATION {
        fade_in: i32,
        stay: i32,
        fade_out: i32,
    }
}

/// A sound: a `minecraft:sound_event` registry id or an inline definition by name.
#[derive(Clone, Debug, PartialEq)]
pub enum SoundRef {
    Id(i32),
    Named { name: String, fixed_range: Option<f32> },
}

impl Encode for SoundRef {
    fn encode(&self, w: &mut Vec<u8>) {
        match self {
            SoundRef::Id(id) => w.put_varint(id + 1),
            SoundRef::Named { name, fixed_range } => {
                w.put_varint(0);
                w.put_string(name);
                w.put_option(fixed_range.as_ref(), |w, range| w.put_f32(*range));
            }
        }
    }
}

impl Decode for SoundRef {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        match r.varint()? {
            0 => Ok(SoundRef::Named { name: r.identifier()?, fixed_range: r.option(|r| r.f32())? }),
            n => Ok(SoundRef::Id(n - 1)),
        }
    }
}

/// Plays a sound at a position.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub sound: SoundRef,
    /// 0 master, 1 music, 2 records, 3 weather, 4 blocks, 5 hostile, 6 neutral, 7 players,
    /// 8 ambient, 9 voice, 10 ui.
    pub category: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

packet_id!(Sound = play::clientbound::SOUND);

impl Encode for Sound {
    fn encode(&self, w: &mut Vec<u8>) {
        self.sound.encode(w);
        w.put_varint(self.category);
        // Fixed point with 3 fractional bits.
        w.put_i32((self.x * 8.0) as i32);
        w.put_i32((self.y * 8.0) as i32);
        w.put_i32((self.z * 8.0) as i32);
        w.put_f32(self.volume);
        w.put_f32(self.pitch);
        w.put_i64(self.seed);
    }
}

impl Decode for Sound {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            sound: SoundRef::decode(r)?,
            category: r.varint()?,
            x: r.i32()? as f64 / 8.0,
            y: r.i32()? as f64 / 8.0,
            z: r.i32()? as f64 / 8.0,
            volume: r.f32()?,
            pitch: r.f32()?,
            seed: r.i64()?,
        })
    }
}

packet! {
    /// Returns the client to the configuration state; it acknowledges first.
    StartConfiguration = play::clientbound::START_CONFIGURATION {}
}

packet! {
    /// An entity swings a hand.
    SwingAnimation = play::clientbound::SWING_ANIMATION {
        entity_id: VarInt,
        /// 0 main hand, 1 off hand.
        hand: VarInt,
        /// 0 none, 1 whack, 2 stab.
        animation_type: VarInt,
        /// Duration in ticks.
        duration: VarInt,
    }
}

packet! {
    /// A chat-window (or, with `overlay`, action bar) message from the server.
    SystemChat = play::clientbound::SYSTEM_CHAT {
        content: Component,
        overlay: bool,
    }
}

packet! {
    /// Header and footer of the player list.
    TabList = play::clientbound::TAB_LIST {
        header: Component,
        footer: Component,
    }
}

packet! {
    TickingState = play::clientbound::TICKING_STATE {
        tick_rate: f32,
        frozen: bool,
    }
}

packet! {
    TickingStep = play::clientbound::TICKING_STEP {
        steps: VarInt,
    }
}

// --- entities, combat, containers and scoreboards -----------------------------------------

packet! {
    /// An entity picks up an item entity (plays the pickup animation and sound).
    TakeItemEntity = play::clientbound::TAKE_ITEM_ENTITY {
        collected_id: VarInt,
        collector_id: VarInt,
        count: VarInt,
    }
}

/// An entity was damaged: tilts the camera and plays the hurt effect.
#[derive(Clone, Debug, PartialEq)]
pub struct DamageEvent {
    pub entity_id: i32,
    /// Id in the `minecraft:damage_type` registry.
    pub source_type: i32,
    /// Entity that caused the damage, if any.
    pub cause_id: Option<i32>,
    /// Entity that dealt it directly (an arrow), if any.
    pub direct_id: Option<i32>,
    pub source_position: Option<(f64, f64, f64)>,
}

packet_id!(DamageEvent = play::clientbound::DAMAGE_EVENT);

impl Encode for DamageEvent {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.entity_id);
        w.put_varint(self.source_type);
        w.put_varint(self.cause_id.map_or(0, |id| id + 1));
        w.put_varint(self.direct_id.map_or(0, |id| id + 1));
        w.put_option(self.source_position.as_ref(), |w, (x, y, z)| {
            w.put_f64(*x);
            w.put_f64(*y);
            w.put_f64(*z);
        });
    }
}

impl Decode for DamageEvent {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let entity_id = r.varint()?;
        let source_type = r.varint()?;
        let id = |v: i32| (v > 0).then(|| v - 1);
        let cause_id = id(r.varint()?);
        let direct_id = id(r.varint()?);
        let source_position = r.option(|r| Ok((r.f64()?, r.f64()?, r.f64()?)))?;
        Ok(Self { entity_id, source_type, cause_id, direct_id, source_position })
    }
}

packet! {
    /// Shows the death screen with a message.
    PlayerCombatKill = play::clientbound::PLAYER_COMBAT_KILL {
        player_id: VarInt,
        message: Component,
    }
}

/// A block action: chest lid, note block, piston, bell...
#[derive(Clone, Debug, PartialEq)]
pub struct BlockEvent {
    pub pos: BlockPos,
    pub action: u8,
    pub param: u8,
    /// Block registry id.
    pub block: i32,
}

packet_id!(BlockEvent = play::clientbound::BLOCK_EVENT);

impl Encode for BlockEvent {
    fn encode(&self, w: &mut Vec<u8>) {
        self.pos.encode(w);
        w.put_u8(self.action);
        w.put_u8(self.param);
        w.put_varint(self.block);
    }
}

impl Decode for BlockEvent {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { pos: BlockPos::decode(r)?, action: r.u8()?, param: r.u8()?, block: r.varint()? })
    }
}

/// A container property, such as a furnace's progress.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerSetData {
    pub window_id: i32,
    pub property: i16,
    pub value: i16,
}

packet_id!(ContainerSetData = play::clientbound::CONTAINER_SET_DATA);

impl Encode for ContainerSetData {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.window_id);
        w.put_i16(self.property);
        w.put_i16(self.value);
    }
}

impl Decode for ContainerSetData {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { window_id: r.varint()?, property: r.i16()?, value: r.i16()? })
    }
}

/// Attribute values of an entity (`minecraft:attribute` registry ids).
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateAttributes {
    pub entity_id: i32,
    /// (attribute id, base value, modifiers (id, amount, operation))
    pub attributes: Vec<(i32, f64, Vec<(String, f64, i8)>)>,
}

packet_id!(UpdateAttributes = play::clientbound::UPDATE_ATTRIBUTES);

impl Encode for UpdateAttributes {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.entity_id);
        w.put_list(&self.attributes, |w, (id, base, modifiers)| {
            w.put_varint(*id);
            w.put_f64(*base);
            w.put_list(modifiers, |w, (mid, amount, op)| {
                w.put_string(mid);
                w.put_f64(*amount);
                w.put_i8(*op);
            });
        });
    }
}

packet! {
    UpdateMobEffect = play::clientbound::UPDATE_MOB_EFFECT {
        entity_id: VarInt,
        effect: VarInt,
        amplifier: VarInt,
        duration: VarInt,
        /// 1 ambient, 2 particles visible, 4 icon shown, 8 blend
        flags: i8,
    }
}

packet! {
    RemoveMobEffect = play::clientbound::REMOVE_MOB_EFFECT {
        entity_id: VarInt,
        effect: VarInt,
    }
}

/// Teleports a (non-player-controlled) entity.
#[derive(Clone, Debug, PartialEq)]
pub struct TeleportEntity {
    pub entity_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity: (f64, f64, f64),
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

packet_id!(TeleportEntity = play::clientbound::TELEPORT_ENTITY);

impl Encode for TeleportEntity {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.entity_id);
        for v in [self.x, self.y, self.z, self.velocity.0, self.velocity.1, self.velocity.2] {
            w.put_f64(v);
        }
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
        w.put_i32(0);
        w.put_bool(self.on_ground);
    }
}

/// A sound attached to an entity.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundEntity {
    pub sound: SoundRef,
    pub category: i32,
    pub entity_id: i32,
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

packet_id!(SoundEntity = play::clientbound::SOUND_ENTITY);

impl Encode for SoundEntity {
    fn encode(&self, w: &mut Vec<u8>) {
        self.sound.encode(w);
        w.put_varint(self.category);
        w.put_varint(self.entity_id);
        w.put_f32(self.volume);
        w.put_f32(self.pitch);
        w.put_i64(self.seed);
    }
}

/// An explosion: the client plays its effects and applies `knockback` to its player.
#[derive(Clone, Debug, PartialEq)]
pub struct Explode {
    pub center: (f64, f64, f64),
    pub radius: f32,
    pub block_count: i32,
    pub knockback: Option<(f64, f64, f64)>,
    /// Particle type id (one without options, e.g. `minecraft:explosion_emitter`).
    pub particle: i32,
    pub sound: SoundRef,
}

packet_id!(Explode = play::clientbound::EXPLODE);

impl Encode for Explode {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_f64(self.center.0);
        w.put_f64(self.center.1);
        w.put_f64(self.center.2);
        w.put_f32(self.radius);
        w.put_i32(self.block_count);
        w.put_option(self.knockback.as_ref(), |w, (x, y, z)| {
            w.put_f64(*x);
            w.put_f64(*y);
            w.put_f64(*z);
        });
        w.put_varint(self.particle);
        self.sound.encode(w);
        // Block particles: none.
        w.put_varint(0);
        // Play the explosion sound.
        w.put_bool(true);
    }
}

impl Decode for Explode {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let center = (r.f64()?, r.f64()?, r.f64()?);
        let radius = r.f32()?;
        let block_count = r.i32()?;
        let knockback = r.option(|r| Ok((r.f64()?, r.f64()?, r.f64()?)))?;
        let particle = r.varint()?;
        let sound = SoundRef::decode(r)?;
        let _block_particles = r.varint()?;
        let _play_sound = r.bool()?;
        Ok(Self { center, radius, block_count, knockback, particle, sound })
    }
}

/// Creates (0), removes (1) or updates (2) a scoreboard objective.
#[derive(Clone, Debug, PartialEq)]
pub struct SetObjective {
    pub name: String,
    pub mode: u8,
    pub display_name: Component,
    /// 0 integer, 1 hearts.
    pub render_type: i32,
}

packet_id!(SetObjective = play::clientbound::SET_OBJECTIVE);

impl Encode for SetObjective {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.name);
        w.put_u8(self.mode);
        if self.mode != 1 {
            w.put_component(&self.display_name);
            w.put_varint(self.render_type);
            // No number format.
            w.put_bool(false);
        }
    }
}

packet! {
    /// Shows an objective in a slot (0 list, 1 sidebar, 2 below name, 3-18 team sidebars).
    SetDisplayObjective = play::clientbound::SET_DISPLAY_OBJECTIVE {
        slot: VarInt,
        objective: String,
    }
}

/// Sets a score.
#[derive(Clone, Debug, PartialEq)]
pub struct SetScore {
    pub owner: String,
    pub objective: String,
    pub value: i32,
    pub display_name: Option<Component>,
}

packet_id!(SetScore = play::clientbound::SET_SCORE);

impl Encode for SetScore {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.owner);
        w.put_string(&self.objective);
        w.put_varint(self.value);
        w.put_option(self.display_name.as_ref(), |w, c| w.put_component(c));
        w.put_bool(false);
    }
}

/// Removes a score (from one objective, or all when `objective` is `None`).
#[derive(Clone, Debug, PartialEq)]
pub struct ResetScore {
    pub owner: String,
    pub objective: Option<String>,
}

packet_id!(ResetScore = play::clientbound::RESET_SCORE);

impl Encode for ResetScore {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.owner);
        w.put_option(self.objective.as_ref(), |w, o| w.put_string(o));
    }
}

/// Team display settings.
#[derive(Clone, Debug, PartialEq)]
pub struct TeamParameters {
    pub display_name: Component,
    pub prefix: Component,
    pub suffix: Component,
    /// 0 always, 1 never, 2 hide for other teams, 3 hide for own team.
    pub nametag_visibility: i32,
    /// 0 always, 1 never, 2 push other teams, 3 push own team.
    pub collision_rule: i32,
    /// Chat formatting colour id (0-15), or `None`.
    pub color: Option<i32>,
    /// 1 friendly fire, 2 see friendly invisibles.
    pub options: i8,
}

/// Creates (0), removes (1), updates (2) a team, or adds (3) / removes (4) members.
#[derive(Clone, Debug, PartialEq)]
pub struct SetPlayerTeam {
    pub name: String,
    pub method: i8,
    pub parameters: Option<TeamParameters>,
    pub members: Vec<String>,
}

packet_id!(SetPlayerTeam = play::clientbound::SET_PLAYER_TEAM);

impl Encode for SetPlayerTeam {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.name);
        w.put_i8(self.method);
        if self.method == 0 || self.method == 2 {
            let p = self.parameters.clone().unwrap_or(TeamParameters {
                display_name: Component::text(self.name.clone()),
                prefix: Component::text(""),
                suffix: Component::text(""),
                nametag_visibility: 0,
                collision_rule: 0,
                color: None,
                options: 0,
            });
            w.put_component(&p.display_name);
            w.put_component(&p.prefix);
            w.put_component(&p.suffix);
            w.put_varint(p.nametag_visibility);
            w.put_varint(p.collision_rule);
            w.put_option(p.color.as_ref(), |w, c| w.put_varint(*c));
            w.put_i8(p.options);
        }
        if self.method == 0 || self.method == 3 || self.method == 4 {
            w.put_list(&self.members, |w, m| w.put_string(m));
        }
    }
}
