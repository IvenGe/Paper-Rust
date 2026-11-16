//! What a 1.8 client sends, as 26.3 packets.

use super::ids::{cb, sb};
use super::{from_pos18, inventory, text, Legacy18};
use crate::io::Result;
use crate::Outcome;
use paper_protocol::codec::{Reader, VarInt, WriteExt};
use paper_protocol::types::LpVec3;
use paper_protocol::packets::encode_packet;
use paper_protocol::packets::play as packets;
use paper_protocol::types::HashedSlot;

/// 26.3 movement flags: on the ground (horizontal collision unknown).
fn ground(on_ground: bool) -> u8 {
    on_ground as u8
}

impl Legacy18 {
    pub(super) fn play_serverbound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        let mut r = Reader::new(body);
        let payload = match id {
            sb::KEEP_ALIVE => {
                let short = r.varint()?;
                let Some(index) = self.keep_alives.iter().position(|(s, _)| *s == short) else { return Ok(()) };
                let (_, long) = self.keep_alives.remove(index).unwrap_or_default();
                encode_packet(&packets::ServerboundKeepAlive { id: long })
            }
            sb::CHAT => {
                let message = r.string(100)?;
                match message.strip_prefix('/') {
                    Some(command) => encode_packet(&packets::ChatCommand { command: command.to_owned() }),
                    None => {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
                        encode_packet(&packets::Chat { message, timestamp: now, salt: 0, signed: false })
                    }
                }
            }
            sb::USE_ENTITY => {
                let target = r.varint()?;
                match r.varint()? {
                    1 => encode_packet(&packets::Attack { entity_id: VarInt(target) }),
                    kind => {
                        let at = if kind == 2 { LpVec3(r.f32()? as f64, r.f32()? as f64, r.f32()? as f64) } else { LpVec3(0.0, 0.0, 0.0) };
                        encode_packet(&packets::Interact { entity_id: VarInt(target), hand: VarInt(0), target: at, sneaking: self.sneaking })
                    }
                }
            }
            sb::FLYING => encode_packet(&packets::MovePlayerStatusOnly { flags: ground(r.bool()?) }),
            sb::POSITION => {
                let (x, y, z) = (r.f64()?, r.f64()?, r.f64()?);
                self.position = (x, y, z);
                encode_packet(&packets::MovePlayerPos { x, y, z, flags: ground(r.bool()?) })
            }
            sb::LOOK => {
                let (yaw, pitch) = (r.f32()?, r.f32()?);
                self.rotation = (yaw, pitch);
                encode_packet(&packets::MovePlayerRot { yaw, pitch, flags: ground(r.bool()?) })
            }
            sb::POSITION_LOOK => {
                let (x, y, z, yaw, pitch) = (r.f64()?, r.f64()?, r.f64()?, r.f32()?, r.f32()?);
                self.position = (x, y, z);
                self.rotation = (yaw, pitch);
                encode_packet(&packets::MovePlayerPosRot { x, y, z, yaw, pitch, flags: ground(r.bool()?) })
            }
            sb::BLOCK_DIG => {
                let status = r.i8()?;
                let pos = from_pos18(r.i64()?);
                let face = r.i8()?;
                // 1.8: 0 start, 1 cancel, 2 finish, 3 drop stack, 4 drop item, 5 release;
                // 26.3 has "change direction" at 1, after start.
                let status = match status {
                    0 => 0,
                    1 => 2,
                    2 => 3,
                    3 => 4,
                    4 => 5,
                    _ => 6,
                };
                encode_packet(&packets::PlayerAction { status: VarInt(status), pos, face, sequence: VarInt(0) })
            }
            sb::BLOCK_PLACE => {
                let pos = from_pos18(r.i64()?);
                let face = r.i8()?;
                let _held = r.i16()?;
                if _held >= 0 {
                    r.i8()?;
                    r.i16()?;
                    let nbt = r.u8()?;
                    if nbt != 0 {
                        // The item's NBT: not needed, the server knows what is held.
                        return self.place_with_item_nbt(pos, face, out);
                    }
                }
                let cursor = (r.u8()? as f32 / 16.0, r.u8()? as f32 / 16.0, r.u8()? as f32 / 16.0);
                if face == -1 {
                    encode_packet(&packets::UseItem { hand: VarInt(0), sequence: VarInt(0), yaw: self.rotation.0, pitch: self.rotation.1 })
                } else {
                    encode_packet(&packets::UseItemOn {
                        hand: VarInt(0),
                        pos,
                        face: VarInt(face as i32),
                        cursor_x: cursor.0,
                        cursor_y: cursor.1,
                        cursor_z: cursor.2,
                        inside_block: false,
                        world_border_hit: false,
                        sequence: VarInt(0),
                    })
                }
            }
            sb::HELD_ITEM_SLOT => encode_packet(&packets::SetCarriedItem { slot: r.i16()? }),
            sb::ARM_ANIMATION => encode_packet(&packets::Punch {}),
            sb::ENTITY_ACTION => {
                let entity = r.varint()?;
                let action = r.varint()?;
                let jump = r.varint()?;
                match action {
                    0 | 1 => {
                        self.sneaking = action == 0;
                        self.input()
                    }
                    2 => encode_packet(&packets::PlayerCommand { entity_id: VarInt(entity), action: VarInt(0), jump_boost: VarInt(0) }),
                    3 | 4 => {
                        self.sprinting = action == 3;
                        out.to_server.push(self.input());
                        encode_packet(&packets::PlayerCommand { entity_id: VarInt(entity), action: VarInt(if action == 3 { 1 } else { 2 }), jump_boost: VarInt(0) })
                    }
                    5 => encode_packet(&packets::PlayerCommand { entity_id: VarInt(entity), action: VarInt(3), jump_boost: VarInt(jump) }),
                    _ => encode_packet(&packets::PlayerCommand { entity_id: VarInt(entity), action: VarInt(5), jump_boost: VarInt(0) }),
                }
            }
            sb::STEER_VEHICLE => {
                let (sideways, forward, flags) = (r.f32()?, r.f32()?, r.u8()?);
                let mut input = 0u8;
                if forward > 0.0 {
                    input |= 0x01;
                }
                if forward < 0.0 {
                    input |= 0x02;
                }
                if sideways > 0.0 {
                    input |= 0x04;
                }
                if sideways < 0.0 {
                    input |= 0x08;
                }
                if flags & 1 != 0 {
                    input |= 0x10;
                }
                if flags & 2 != 0 || self.sneaking {
                    input |= 0x20;
                }
                encode_packet(&packets::PlayerInput { flags: input })
            }
            sb::CLOSE_WINDOW => {
                let window = r.i8()? as i32;
                self.window = None;
                encode_packet(&packets::ServerboundContainerClose { window_id: VarInt(window) })
            }
            sb::WINDOW_CLICK => {
                let window = r.i8()? as i32;
                let slot = r.i16()?;
                let button = r.i8()?;
                let action = r.i16()?;
                let mode = r.i8()?;
                // 1.8 waits for the server to accept the click; the server corrects the
                // window afterwards if it saw it differently.
                let mut accepted = Vec::new();
                accepted.put_varint(cb::TRANSACTION);
                accepted.put_i8(window as i8);
                accepted.put_i16(action);
                accepted.put_bool(true);
                out.to_client.push(accepted);
                let slot = if slot < 0 { slot } else { self.slot_from_1_8(window, slot) };
                encode_packet(&packets::ContainerClick {
                    window_id: window,
                    state_id: self.state_id,
                    slot,
                    button,
                    mode: mode as i32,
                    changed_slots: Vec::new(),
                    carried: HashedSlot(None),
                })
            }
            sb::SET_CREATIVE_SLOT => {
                let slot = r.i16()?;
                let mut w = Vec::new();
                w.put_varint(paper_data::packets::play::serverbound::SET_CREATIVE_MODE_SLOT);
                w.put_i16(slot);
                inventory::item_from_1_8(self.tables, &mut r, &mut w)?;
                w
            }
            sb::ENCHANT_ITEM => {
                let (window, button) = (r.i8()?, r.i8()?);
                encode_packet(&packets::ContainerButtonClick { window_id: VarInt(window as i32), button_id: VarInt(button as i32) })
            }
            sb::UPDATE_SIGN => {
                let pos = from_pos18(r.i64()?);
                let mut lines: [String; 4] = Default::default();
                for line in &mut lines {
                    let json = r.string(32767)?;
                    *line = serde_json::from_str::<serde_json::Value>(&json).map(|v| text::plain(&v)).unwrap_or(json);
                }
                encode_packet(&packets::SignUpdate { pos, lines, front: true })
            }
            sb::ABILITIES => {
                let flags = r.i8()?;
                encode_packet(&packets::ServerboundPlayerAbilities { flags: flags & 0x02 })
            }
            sb::TAB_COMPLETE => {
                let typed = r.string(32767)?;
                let transaction = self.next_completion;
                self.next_completion = self.next_completion.wrapping_add(1).max(1);
                self.completions.push_back((transaction, typed.clone()));
                if self.completions.len() > 8 {
                    self.completions.pop_front();
                }
                encode_packet(&packets::CommandSuggestion { transaction_id: VarInt(transaction), text: typed })
            }
            sb::SETTINGS => {
                let locale = r.string(16)?;
                let view = r.i8()?;
                let chat = r.i8()?;
                let colors = r.bool()?;
                let skin = r.u8()?;
                let mut w = Vec::new();
                w.put_varint(paper_data::packets::play::serverbound::CLIENT_INFORMATION);
                w.put_string(&locale.to_ascii_lowercase());
                w.put_i8(view);
                w.put_varint(chat as i32);
                w.put_bool(colors);
                w.put_u8(skin);
                w.put_varint(1);
                w.put_bool(false);
                w.put_bool(true);
                w.put_varint(0);
                w
            }
            sb::CLIENT_COMMAND => match r.varint()? {
                0 => encode_packet(&packets::ClientCommand { action: VarInt(0) }),
                1 => encode_packet(&packets::ClientCommand { action: VarInt(1) }),
                _ => return Ok(()),
            },
            sb::CUSTOM_PAYLOAD => {
                let channel = r.string(20)?;
                let channel = match channel.as_str() {
                    "MC|Brand" => "minecraft:brand".to_owned(),
                    other if other.contains(':') => other.to_owned(),
                    _ => return Ok(()),
                };
                let mut w = Vec::new();
                w.put_varint(paper_data::packets::play::serverbound::CUSTOM_PAYLOAD);
                w.put_string(&channel);
                w.put_slice(r.rest());
                w
            }
            sb::SPECTATE => {
                let mut w = Vec::new();
                w.put_varint(paper_data::packets::play::serverbound::TELEPORT_TO_ENTITY);
                w.put_uuid(r.uuid()?);
                w
            }
            sb::RESOURCE_PACK_RECEIVE => {
                let _hash = r.string(40)?;
                let result = r.varint()?;
                let Some(pack) = self.resource_packs.front().copied() else { return Ok(()) };
                if result != 3 {
                    self.resource_packs.pop_front();
                }
                // 1.8: 0 loaded, 1 declined, 2 failed, 3 accepted; 26.3: 0 loaded,
                // 1 declined, 2 failed to download, 3 accepted.
                let mut w = Vec::new();
                w.put_varint(paper_data::packets::play::serverbound::RESOURCE_PACK);
                w.put_uuid(pack);
                w.put_varint(result);
                w
            }
            sb::TRANSACTION => return Ok(()),
            _ => return Ok(()),
        };
        out.to_server.push(payload);
        Ok(())
    }

    /// 26.3's input of the movement keys 1.8 tells as sneaking and sprinting.
    fn input(&self) -> Vec<u8> {
        encode_packet(&packets::PlayerInput { flags: if self.sneaking { 0x20 } else { 0 } | if self.sprinting { 0x40 } else { 0 } })
    }

    fn place_with_item_nbt(&mut self, pos: paper_protocol::codec::BlockPos, face: i8, out: &mut Outcome) -> Result<()> {
        let payload = if face == -1 {
            encode_packet(&packets::UseItem { hand: VarInt(0), sequence: VarInt(0), yaw: self.rotation.0, pitch: self.rotation.1 })
        } else {
            encode_packet(&packets::UseItemOn { hand: VarInt(0), pos, face: VarInt(face as i32), cursor_x: 0.5, cursor_y: 0.5, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(0) })
        };
        out.to_server.push(payload);
        Ok(())
    }
}
