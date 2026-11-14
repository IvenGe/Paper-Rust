//! Serverbound play packets.

use super::{ClientInformation, CookieResponse, Pong, ResourcePackResponse, ServerboundCustomPayload, ServerboundKeepAlive};
use crate::codec::{BlockPos, ByteArray, Decode, DecodeError, Encode, Reader, Result, VarInt, WriteExt};
use crate::packets::{packet, packet_id};
use crate::types::{HashedSlot, LpVec3, Slot};
use uuid::Uuid;

packet! {
    /// Confirms a teleport and reports where the client ended up.
    AcceptTeleportation = play::serverbound::ACCEPT_TELEPORTATION {
        teleport_id: VarInt,
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
    }
}

packet! {
    /// The player attacks an entity.
    Attack = play::serverbound::ATTACK {
        entity_id: VarInt,
    }
}

packet! {
    /// Requests a game mode change (the F3+F4 switcher); needs operator permission.
    ChangeGameMode = play::serverbound::CHANGE_GAME_MODE {
        game_mode: VarInt,
    }
}

packet! {
    /// Acknowledges signed chat messages seen so far.
    ChatAck = play::serverbound::CHAT_ACK {
        message_count: VarInt,
    }
}

packet! {
    /// A command without signed arguments; `command` excludes the leading slash.
    ChatCommand = play::serverbound::CHAT_COMMAND {
        command: String,
    }
}

/// A command with signed message arguments. The signatures are parsed and dropped: this
/// server does not relay signed chat.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatCommandSigned {
    pub command: String,
    pub timestamp: i64,
    pub salt: i64,
    pub signed_arguments: Vec<String>,
}

packet_id!(ChatCommandSigned = play::serverbound::CHAT_COMMAND_SIGNED);

impl Encode for ChatCommandSigned {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.command);
        w.put_i64(self.timestamp);
        w.put_i64(self.salt);
        w.put_list(&self.signed_arguments, |w, name| {
            w.put_string(name);
            w.put_slice(&[0u8; 256]);
        });
        w.put_varint(0);
        w.put_slice(&[0u8; 3]);
        w.put_u8(0);
    }
}

impl Decode for ChatCommandSigned {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let command = r.string(32767)?;
        let timestamp = r.i64()?;
        let salt = r.i64()?;
        let signed_arguments = r.list(8, |r| {
            let name = r.string(16)?;
            r.take(256)?;
            Ok(name)
        })?;
        let _message_count = r.varint()?;
        r.take(3)?; // acknowledged: fixed bit set of 20 bits
        let _checksum = r.u8()?;
        Ok(Self { command, timestamp, salt, signed_arguments })
    }
}

/// A chat message. Signature data is parsed for framing but not retained.
#[derive(Clone, Debug, PartialEq)]
pub struct Chat {
    pub message: String,
    pub timestamp: i64,
    pub salt: i64,
    pub signed: bool,
}

packet_id!(Chat = play::serverbound::CHAT);

impl Encode for Chat {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.message);
        w.put_i64(self.timestamp);
        w.put_i64(self.salt);
        w.put_bool(false);
        w.put_varint(0);
        w.put_slice(&[0u8; 3]);
        w.put_u8(0);
    }
}

impl Decode for Chat {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let message = r.string(256)?;
        let timestamp = r.i64()?;
        let salt = r.i64()?;
        let signed = r.bool()?;
        if signed {
            r.take(256)?;
        }
        let _message_count = r.varint()?;
        r.take(3)?;
        let _checksum = r.u8()?;
        Ok(Self { message, timestamp, salt, signed })
    }
}

packet! {
    /// The client's chat signing session (public key). Accepted and ignored.
    ChatSessionUpdate = play::serverbound::CHAT_SESSION_UPDATE {
        session_id: Uuid,
        expires_at: i64,
        public_key: ByteArray,
        key_signature: ByteArray,
    }
}

packet! {
    /// Flow control for chunk batches: how many chunks per tick the client can take.
    ChunkBatchReceived = play::serverbound::CHUNK_BATCH_RECEIVED {
        chunks_per_tick: f32,
    }
}

packet! {
    /// 0 perform respawn, 1 request statistics, 2 request game rule values.
    ClientCommand = play::serverbound::CLIENT_COMMAND {
        action: VarInt,
    }
}

packet! {
    /// Sent at the end of every client tick.
    ClientTickEnd = play::serverbound::CLIENT_TICK_END {}
}

packet! {
    /// Asks for tab completions of `text` (everything left of the cursor, with the slash).
    CommandSuggestion = play::serverbound::COMMAND_SUGGESTION {
        transaction_id: VarInt,
        text: String,
    }
}

packet! {
    /// Acknowledges `StartConfiguration`; switches the connection to configuration.
    ConfigurationAcknowledged = play::serverbound::CONFIGURATION_ACKNOWLEDGED {}
}

/// A click in a container window.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerClick {
    pub window_id: i32,
    pub state_id: i32,
    /// Clicked slot; -999 for outside the window.
    pub slot: i16,
    pub button: i8,
    /// 0 pickup, 1 quick move, 2 swap, 3 clone, 4 throw, 5 quick craft (drag), 6 pickup all.
    pub mode: i32,
    /// The slots the client believes changed, with hashed contents.
    pub changed_slots: Vec<(i16, HashedSlot)>,
    pub carried: HashedSlot,
}

packet_id!(ContainerClick = play::serverbound::CONTAINER_CLICK);

impl Encode for ContainerClick {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.window_id);
        w.put_varint(self.state_id);
        w.put_i16(self.slot);
        w.put_i8(self.button);
        w.put_varint(self.mode);
        w.put_list(&self.changed_slots, |w, (slot, item)| {
            w.put_i16(*slot);
            item.encode(w);
        });
        self.carried.encode(w);
    }
}

impl Decode for ContainerClick {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            window_id: r.varint()?,
            state_id: r.varint()?,
            slot: r.i16()?,
            button: r.i8()?,
            mode: r.varint()?,
            changed_slots: r.list(128, |r| Ok((r.i16()?, HashedSlot::decode(r)?)))?,
            carried: HashedSlot::decode(r)?,
        })
    }
}

packet! {
    ServerboundContainerClose = play::serverbound::CONTAINER_CLOSE {
        window_id: VarInt,
    }
}

packet! {
    /// A button in a window: the three enchantments of an enchanting table, ...
    ContainerButtonClick = play::serverbound::CONTAINER_BUTTON_CLICK {
        window_id: VarInt,
        button_id: VarInt,
    }
}

packet! {
    /// The name typed into an anvil.
    RenameItem = play::serverbound::RENAME_ITEM {
        name: String,
    }
}

packet! {
    /// A trade picked in a villager's trading window: the server moves matching payment
    /// from the inventory into the payment slots.
    SelectTrade = play::serverbound::SELECT_TRADE {
        item: VarInt,
    }
}

packet! {
    /// Right-click on an entity.
    Interact = play::serverbound::INTERACT {
        entity_id: VarInt,
        /// 0 main hand, 1 off hand.
        hand: VarInt,
        /// Where on the entity the cursor was, relative to the entity's position.
        target: LpVec3,
        sneaking: bool,
    }
}

/// Bit flags carried by the movement packets.
pub mod move_flags {
    pub const ON_GROUND: u8 = 0x01;
    pub const HORIZONTAL_COLLISION: u8 = 0x02;
}

packet! {
    MovePlayerPos = play::serverbound::MOVE_PLAYER_POS {
        x: f64,
        y: f64,
        z: f64,
        flags: u8,
    }
}

packet! {
    MovePlayerPosRot = play::serverbound::MOVE_PLAYER_POS_ROT {
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        flags: u8,
    }
}

packet! {
    MovePlayerRot = play::serverbound::MOVE_PLAYER_ROT {
        yaw: f32,
        pitch: f32,
        flags: u8,
    }
}

packet! {
    MovePlayerStatusOnly = play::serverbound::MOVE_PLAYER_STATUS_ONLY {
        flags: u8,
    }
}

packet! {
    /// Middle-click on a block.
    PickItemFromBlock = play::serverbound::PICK_ITEM_FROM_BLOCK {
        pos: BlockPos,
        include_data: bool,
    }
}

packet! {
    /// Latency probe from the debug screen; answered with `PongResponse`.
    PingRequest = play::serverbound::PING_REQUEST {
        payload: i64,
    }
}

packet! {
    /// 0x02: the player is flying.
    ServerboundPlayerAbilities = play::serverbound::PLAYER_ABILITIES {
        flags: i8,
    }
}

/// Status values of [`PlayerAction`].
pub mod player_action {
    pub const START_DESTROY_BLOCK: i32 = 0;
    pub const ABORT_DESTROY_BLOCK: i32 = 1;
    pub const STOP_DESTROY_BLOCK: i32 = 2;
    pub const DROP_ALL_ITEMS: i32 = 3;
    pub const DROP_ITEM: i32 = 4;
    pub const RELEASE_USE_ITEM: i32 = 5;
    pub const SWAP_ITEM_WITH_OFFHAND: i32 = 6;
    pub const STAB: i32 = 7;
}

packet! {
    /// Digging, dropping and hand-swap actions.
    PlayerAction = play::serverbound::PLAYER_ACTION {
        status: VarInt,
        pos: BlockPos,
        /// 0 bottom, 1 top, 2 north, 3 south, 4 west, 5 east.
        face: i8,
        sequence: VarInt,
    }
}

/// Action ids of [`PlayerCommand`].
pub mod player_command {
    pub const LEAVE_BED: i32 = 0;
    pub const START_SPRINTING: i32 = 1;
    pub const STOP_SPRINTING: i32 = 2;
    pub const START_HORSE_JUMP: i32 = 3;
    pub const STOP_HORSE_JUMP: i32 = 4;
    pub const OPEN_VEHICLE_INVENTORY: i32 = 5;
    pub const START_ELYTRA_FLYING: i32 = 6;
}

packet! {
    PlayerCommand = play::serverbound::PLAYER_COMMAND {
        entity_id: VarInt,
        action: VarInt,
        jump_boost: VarInt,
    }
}

/// Key flags of [`PlayerInput`].
pub mod input_flags {
    pub const FORWARD: u8 = 0x01;
    pub const BACKWARD: u8 = 0x02;
    pub const LEFT: u8 = 0x04;
    pub const RIGHT: u8 = 0x08;
    pub const JUMP: u8 = 0x10;
    pub const SNEAK: u8 = 0x20;
    pub const SPRINT: u8 = 0x40;
}

packet! {
    /// Which movement keys are held; this is how sneaking is reported.
    PlayerInput = play::serverbound::PLAYER_INPUT {
        flags: u8,
    }
}

packet! {
    /// The client left the loading screen and starts simulating its player.
    PlayerLoaded = play::serverbound::PLAYER_LOADED {}
}

packet! {
    /// The player swings at nothing in particular.
    Punch = play::serverbound::PUNCH {}
}

packet! {
    /// Selected hotbar slot, 0-8.
    SetCarriedItem = play::serverbound::SET_CARRIED_ITEM {
        slot: i16,
    }
}

/// Creative mode: set an inventory slot to an arbitrary item.
#[derive(Clone, Debug, PartialEq)]
pub struct SetCreativeModeSlot {
    /// Window slot of the player inventory; -1 drops the item.
    pub slot: i16,
    pub item: Slot,
}

packet_id!(SetCreativeModeSlot = play::serverbound::SET_CREATIVE_MODE_SLOT);

impl Encode for SetCreativeModeSlot {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_i16(self.slot);
        self.item.encode_untrusted(w);
    }
}

impl Decode for SetCreativeModeSlot {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { slot: r.i16()?, item: Slot::decode_untrusted(r)? })
    }
}

packet! {
    /// Right-click with an item.
    UseItem = play::serverbound::USE_ITEM {
        hand: VarInt,
        sequence: VarInt,
        yaw: f32,
        pitch: f32,
    }
}

packet! {
    /// Right-click on a block (placing, opening, using).
    UseItemOn = play::serverbound::USE_ITEM_ON {
        hand: VarInt,
        pos: BlockPos,
        /// The clicked face: 0 bottom, 1 top, 2 north, 3 south, 4 west, 5 east.
        face: VarInt,
        cursor_x: f32,
        cursor_y: f32,
        cursor_z: f32,
        inside_block: bool,
        world_border_hit: bool,
        sequence: VarInt,
    }
}

/// Every serverbound play packet the server acts on.
#[derive(Clone, Debug, PartialEq)]
pub enum Serverbound {
    AcceptTeleportation(AcceptTeleportation),
    Attack(Attack),
    ChangeGameMode(ChangeGameMode),
    ChatAck(ChatAck),
    ChatCommand(ChatCommand),
    ChatCommandSigned(ChatCommandSigned),
    Chat(Chat),
    ChatSessionUpdate(ChatSessionUpdate),
    ChunkBatchReceived(ChunkBatchReceived),
    ClientCommand(ClientCommand),
    ClientTickEnd,
    ClientInformation(ClientInformation),
    CommandSuggestion(CommandSuggestion),
    ConfigurationAcknowledged,
    ContainerButtonClick(ContainerButtonClick),
    ContainerClick(ContainerClick),
    ContainerClose(ServerboundContainerClose),
    RenameItem(RenameItem),
    SelectTrade(SelectTrade),
    CookieResponse(CookieResponse),
    CustomPayload(ServerboundCustomPayload),
    Interact(Interact),
    KeepAlive(ServerboundKeepAlive),
    MovePlayerPos(MovePlayerPos),
    MovePlayerPosRot(MovePlayerPosRot),
    MovePlayerRot(MovePlayerRot),
    MovePlayerStatusOnly(MovePlayerStatusOnly),
    PickItemFromBlock(PickItemFromBlock),
    PingRequest(PingRequest),
    PlayerAbilities(ServerboundPlayerAbilities),
    PlayerAction(PlayerAction),
    PlayerCommand(PlayerCommand),
    PlayerInput(PlayerInput),
    PlayerLoaded,
    Pong(Pong),
    Punch,
    ResourcePack(ResourcePackResponse),
    SetCarriedItem(SetCarriedItem),
    SetCreativeModeSlot(SetCreativeModeSlot),
    UseItem(UseItem),
    UseItemOn(UseItemOn),
    /// A valid packet this implementation does not act on (recipe book, sign editing, ...).
    Other { id: i32 },
}

impl Serverbound {
    /// Decodes a payload (packet id + body).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        use paper_data::packets::play::serverbound as id;
        let mut r = Reader::new(payload);
        let packet_id = r.varint()?;
        macro_rules! d {
            ($variant:ident) => {
                Serverbound::$variant(Decode::decode(&mut r)?)
            };
        }
        let packet = match packet_id {
            id::ACCEPT_TELEPORTATION => d!(AcceptTeleportation),
            id::ATTACK => d!(Attack),
            id::CHANGE_GAME_MODE => d!(ChangeGameMode),
            id::CHAT_ACK => d!(ChatAck),
            id::CHAT_COMMAND => d!(ChatCommand),
            id::CHAT_COMMAND_SIGNED => d!(ChatCommandSigned),
            id::CHAT => d!(Chat),
            id::CHAT_SESSION_UPDATE => d!(ChatSessionUpdate),
            id::CHUNK_BATCH_RECEIVED => d!(ChunkBatchReceived),
            id::CLIENT_COMMAND => d!(ClientCommand),
            id::CLIENT_TICK_END => Serverbound::ClientTickEnd,
            id::CLIENT_INFORMATION => d!(ClientInformation),
            id::COMMAND_SUGGESTION => d!(CommandSuggestion),
            id::CONFIGURATION_ACKNOWLEDGED => Serverbound::ConfigurationAcknowledged,
            id::CONTAINER_BUTTON_CLICK => d!(ContainerButtonClick),
            id::CONTAINER_CLICK => d!(ContainerClick),
            id::RENAME_ITEM => d!(RenameItem),
            id::SELECT_TRADE => d!(SelectTrade),
            id::CONTAINER_CLOSE => d!(ContainerClose),
            id::COOKIE_RESPONSE => d!(CookieResponse),
            id::CUSTOM_PAYLOAD => d!(CustomPayload),
            id::INTERACT => d!(Interact),
            id::KEEP_ALIVE => d!(KeepAlive),
            id::MOVE_PLAYER_POS => d!(MovePlayerPos),
            id::MOVE_PLAYER_POS_ROT => d!(MovePlayerPosRot),
            id::MOVE_PLAYER_ROT => d!(MovePlayerRot),
            id::MOVE_PLAYER_STATUS_ONLY => d!(MovePlayerStatusOnly),
            id::PICK_ITEM_FROM_BLOCK => d!(PickItemFromBlock),
            id::PING_REQUEST => d!(PingRequest),
            id::PLAYER_ABILITIES => d!(PlayerAbilities),
            id::PLAYER_ACTION => d!(PlayerAction),
            id::PLAYER_COMMAND => d!(PlayerCommand),
            id::PLAYER_INPUT => d!(PlayerInput),
            id::PLAYER_LOADED => Serverbound::PlayerLoaded,
            id::PONG => d!(Pong),
            id::PUNCH => Serverbound::Punch,
            id::RESOURCE_PACK => d!(ResourcePack),
            id::SET_CARRIED_ITEM => d!(SetCarriedItem),
            id::SET_CREATIVE_MODE_SLOT => d!(SetCreativeModeSlot),
            id::USE_ITEM => d!(UseItem),
            id::USE_ITEM_ON => d!(UseItemOn),
            other if (0..id::NAMES.len() as i32).contains(&other) => return Ok(Serverbound::Other { id: other }),
            other => return Err(DecodeError::UnknownPacket { state: "play", id: other }),
        };
        r.finish()?;
        Ok(packet)
    }
}
