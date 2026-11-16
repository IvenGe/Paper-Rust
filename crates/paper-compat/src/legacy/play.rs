//! The play state from the server: the world, chat, the player's state, the tab list,
//! scoreboards and the world border.

use super::ids::cb;
use super::{chunk, pos18, text, Legacy18};
use crate::io::Result;
use crate::Outcome;
use paper_data::packets::play::clientbound as p;
use paper_protocol::codec::{Decode, Reader, WriteExt};
use paper_protocol::packets::play as packets;
use paper_protocol::text::Component;
use serde_json::{json, Value};

/// What 1.8 needs to know of a player on the tab list to spawn them.
#[derive(Clone, Debug, Default)]
pub struct ListEntry {
    pub name: String,
    pub properties: Vec<(String, String, Option<String>)>,
    pub game_mode: i32,
    pub latency: i32,
    pub display_name: Option<Value>,
}

fn packet(id: i32) -> Vec<u8> {
    let mut w = Vec::new();
    w.put_varint(id);
    w
}

fn decode<T: Decode>(body: &[u8]) -> Result<T> {
    Ok(T::decode(&mut Reader::new(body))?)
}

/// The legacy (`§`) text of a component, cut to `max` characters (scoreboard names).
fn short_text(c: &Component, max: usize) -> String {
    text::legacy_string(&c.0).chars().take(max).collect()
}

impl Legacy18 {
    pub(super) fn play_clientbound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        let mut w;
        match id {
            p::LOGIN => {
                let login: packets::Login = decode(body)?;
                self.player = login.entity_id;
                self.hardcore = login.hardcore;
                self.enter(login.spawn.dimension_type, login.spawn.game_mode);
                w = packet(cb::LOGIN);
                w.put_i32(login.entity_id);
                w.put_u8(self.game_mode | if login.hardcore { 8 } else { 0 });
                w.put_i8(self.dimension);
                w.put_u8(self.difficulty);
                w.put_u8(login.max_players.0.clamp(0, 255) as u8);
                w.put_string(if login.spawn.is_flat { "flat" } else { "default" });
                w.put_bool(login.reduced_debug_info);
            }
            p::RESPAWN => {
                let respawn: packets::Respawn = decode(body)?;
                self.enter(respawn.spawn.dimension_type, respawn.spawn.game_mode);
                self.entities.clear();
                // The server waits for the client to load the world again: it is told once
                // the client has its new position, as after joining.
                self.loaded = false;
                w = packet(cb::RESPAWN);
                w.put_i32(self.dimension as i32);
                w.put_u8(self.difficulty);
                w.put_u8(self.game_mode);
                w.put_string(if respawn.spawn.is_flat { "flat" } else { "default" });
            }
            p::PLAYER_POSITION => {
                let tp: packets::PlayerPosition = decode(body)?;
                let flags = tp.flags & 0x1F;
                let rel = |bit: i32, base: f64, v: f64| if flags & bit != 0 { base + v } else { v };
                self.position = (rel(1, self.position.0, tp.x), rel(2, self.position.1, tp.y), rel(4, self.position.2, tp.z));
                self.rotation = (rel(8, self.rotation.0 as f64, tp.yaw as f64) as f32, rel(16, self.rotation.1 as f64, tp.pitch as f64) as f32);
                w = packet(cb::POSITION);
                w.put_f64(tp.x);
                w.put_f64(tp.y);
                w.put_f64(tp.z);
                w.put_f32(tp.yaw);
                w.put_f32(tp.pitch);
                w.put_u8(flags as u8);
                // 1.8 does not confirm teleports: confirm for it, where it ends up.
                let mut confirm = packet(paper_data::packets::play::serverbound::ACCEPT_TELEPORTATION);
                confirm.put_varint(tp.teleport_id.0);
                confirm.put_f64(self.position.0);
                confirm.put_f64(self.position.1);
                confirm.put_f64(self.position.2);
                confirm.put_f32(self.rotation.0);
                confirm.put_f32(self.rotation.1);
                out.to_server.push(confirm);
                if !std::mem::replace(&mut self.loaded, true) {
                    out.to_server.push(packet(paper_data::packets::play::serverbound::PLAYER_LOADED));
                }
            }
            p::KEEP_ALIVE => {
                let id = Reader::new(body).i64()?;
                let short = id as i32 & 0x7FFF_FFFF;
                self.keep_alives.push_back((short, id));
                if self.keep_alives.len() > 16 {
                    self.keep_alives.pop_front();
                }
                w = packet(cb::KEEP_ALIVE);
                w.put_varint(short);
            }
            p::LEVEL_CHUNK_WITH_LIGHT => {
                out.to_client.push(chunk::chunk(self.tables, &self.world, cb::MAP_CHUNK, body)?);
                for sign in chunk::signs(self.tables, cb::UPDATE_SIGN, body)? {
                    out.to_client.push(sign);
                }
                return Ok(());
            }
            p::FORGET_LEVEL_CHUNK => {
                let forget: packets::ForgetLevelChunk = decode(body)?;
                w = chunk::unload(cb::MAP_CHUNK, forget.x, forget.z);
            }
            p::CHUNK_BATCH_FINISHED => {
                // 1.8 does not pace chunks: acknowledge for it at a steady rate.
                let mut ack = packet(paper_data::packets::play::serverbound::CHUNK_BATCH_RECEIVED);
                ack.put_f32(25.0);
                out.to_server.push(ack);
                return Ok(());
            }
            p::BLOCK_UPDATE => {
                let update: packets::BlockUpdate = decode(body)?;
                if !(0..256).contains(&update.pos.y) {
                    return Ok(());
                }
                w = packet(cb::BLOCK_CHANGE);
                w.put_i64(pos18(update.pos));
                w.put_varint(self.tables.block(update.state.0 as u32) as i32);
            }
            p::SECTION_BLOCKS_UPDATE => {
                let update: packets::SectionBlocksUpdate = decode(body)?;
                let base = update.section_y * 16;
                let blocks: Vec<_> = update.blocks.iter().filter(|(_, y, _, _)| (0..256).contains(&(base + *y as i32))).collect();
                if blocks.is_empty() {
                    return Ok(());
                }
                w = packet(cb::MULTI_BLOCK_CHANGE);
                w.put_i32(update.section_x);
                w.put_i32(update.section_z);
                w.put_varint(blocks.len() as i32);
                for (x, y, z, state) in blocks {
                    w.put_u8(x << 4 | z);
                    w.put_u8((base + *y as i32) as u8);
                    w.put_varint(self.tables.block(*state) as i32);
                }
            }
            p::BLOCK_EVENT => {
                let event: packets::BlockEvent = decode(body)?;
                let block = paper_data::registries::entries("minecraft:block").and_then(|e| e.get(event.block.0 as usize).copied()).and_then(paper_data::blocks::default_state).unwrap_or(0);
                w = packet(cb::BLOCK_ACTION);
                w.put_i64(pos18(event.pos));
                w.put_u8(event.action);
                w.put_u8(event.param);
                w.put_varint((self.tables.block(block) >> 4) as i32);
            }
            p::BLOCK_DESTRUCTION => {
                let d: packets::BlockDestruction = decode(body)?;
                w = packet(cb::BLOCK_BREAK_ANIMATION);
                w.put_varint(d.entity_id.0);
                w.put_i64(pos18(d.pos));
                w.put_i8(d.stage as i8);
            }
            p::BLOCK_ENTITY_DATA => {
                if let Some(sign) = chunk::sign_update(self.tables, cb::UPDATE_SIGN, body)? {
                    out.to_client.push(sign);
                }
                return Ok(());
            }
            p::OPEN_SIGN_EDITOR => {
                let open: packets::OpenSignEditor = decode(body)?;
                w = packet(cb::OPEN_SIGN_ENTITY);
                w.put_i64(pos18(open.pos));
            }
            p::LEVEL_EVENT => {
                let event: packets::LevelEvent = decode(body)?;
                let data = match event.event {
                    2001 => {
                        let block = self.tables.block(event.data as u32) as i32;
                        (block >> 4) | ((block & 15) << 12)
                    }
                    _ => event.data,
                };
                w = packet(cb::WORLD_EVENT);
                w.put_i32(event.event);
                w.put_i64(pos18(event.pos));
                w.put_i32(data);
                w.put_bool(event.global);
            }
            p::SYSTEM_CHAT => {
                let chat: packets::SystemChat = decode(body)?;
                w = packet(cb::CHAT);
                w.put_string(&text::json_1_8(&chat.content.0));
                w.put_i8(if chat.overlay { 2 } else { 1 });
            }
            p::DISGUISED_CHAT => {
                let chat: packets::DisguisedChat = decode(body)?;
                let kind = paper_data::synced::entries("minecraft:chat_type").and_then(|e| e.get(chat.chat_type as usize).copied()).unwrap_or("minecraft:chat");
                let key = match kind {
                    "minecraft:say_command" => "chat.type.announcement",
                    "minecraft:emote_command" => "chat.type.emote",
                    "minecraft:msg_command_incoming" => "commands.message.display.incoming",
                    "minecraft:msg_command_outgoing" => "commands.message.display.outgoing",
                    "minecraft:team_msg_command_incoming" | "minecraft:team_msg_command_outgoing" => "chat.type.team.text",
                    _ => "chat.type.text",
                };
                let mut with = vec![chat.sender_name.0, chat.message.0];
                if key == "commands.message.display.outgoing" {
                    with[0] = chat.target_name.map_or(with[0].clone(), |t| t.0);
                }
                w = packet(cb::CHAT);
                w.put_string(&text::json_1_8(&json!({ "translate": key, "with": with })));
                w.put_i8(0);
            }
            p::SET_ACTION_BAR_TEXT => {
                let bar: packets::SetActionBarText = decode(body)?;
                w = packet(cb::CHAT);
                // 1.8 shows action bar text from its legacy form only.
                w.put_string(&json!({ "text": text::legacy_string(&bar.text.0) }).to_string());
                w.put_i8(2);
            }
            p::SET_TITLE_TEXT | p::SET_SUBTITLE_TEXT => {
                let text_value = Reader::new(body).component()?;
                w = packet(cb::TITLE);
                w.put_varint(if id == p::SET_TITLE_TEXT { 0 } else { 1 });
                w.put_string(&text::json_1_8(&text_value.0));
            }
            p::SET_TITLES_ANIMATION => {
                let times: packets::SetTitlesAnimation = decode(body)?;
                w = packet(cb::TITLE);
                w.put_varint(2);
                w.put_i32(times.fade_in);
                w.put_i32(times.stay);
                w.put_i32(times.fade_out);
            }
            p::CLEAR_TITLES => {
                let clear: packets::ClearTitles = decode(body)?;
                w = packet(cb::TITLE);
                w.put_varint(if clear.reset { 4 } else { 3 });
            }
            p::TAB_LIST => {
                let list: packets::TabList = decode(body)?;
                w = packet(cb::PLAYERLIST_HEADER);
                w.put_string(&text::json_1_8(&list.header.0));
                w.put_string(&text::json_1_8(&list.footer.0));
            }
            p::PLAYER_INFO_UPDATE => {
                self.player_info(body, out)?;
                return Ok(());
            }
            p::PLAYER_INFO_REMOVE => {
                let remove: packets::PlayerInfoRemove = decode(body)?;
                w = packet(cb::PLAYER_INFO);
                w.put_varint(4);
                w.put_varint(remove.uuids.len() as i32);
                for uuid in remove.uuids {
                    self.players.remove(&uuid);
                    w.put_uuid(uuid);
                }
            }
            p::SET_TIME => {
                let time: packets::SetTime = decode(body)?;
                let day = time.clocks.first().map_or(6000, |c| if c.rate == 0.0 { -c.time.max(1) } else { c.time });
                w = packet(cb::UPDATE_TIME);
                w.put_i64(time.world_age);
                w.put_i64(day);
            }
            p::SET_HEALTH => {
                w = packet(cb::UPDATE_HEALTH);
                w.extend_from_slice(body);
            }
            p::SET_EXPERIENCE => {
                w = packet(cb::EXPERIENCE);
                w.extend_from_slice(body);
            }
            p::PLAYER_ABILITIES => {
                w = packet(cb::ABILITIES);
                w.extend_from_slice(body);
            }
            p::SET_HELD_SLOT => {
                let slot: packets::SetHeldSlot = decode(body)?;
                w = packet(cb::HELD_ITEM_SLOT);
                w.put_i8(slot.slot.0 as i8);
            }
            p::GAME_EVENT => {
                let event: packets::GameEvent = decode(body)?;
                let reason = match event.event {
                    1 => 2,
                    2 => 1,
                    0 | 3..=8 | 10 => event.event,
                    _ => return Ok(()),
                };
                if event.event == 3 {
                    self.game_mode = event.value as u8;
                }
                w = packet(cb::GAME_STATE_CHANGE);
                w.put_u8(reason);
                w.put_f32(event.value);
            }
            p::CHANGE_DIFFICULTY => {
                let d: packets::ChangeDifficulty = decode(body)?;
                self.difficulty = d.difficulty;
                w = packet(cb::DIFFICULTY);
                w.put_u8(d.difficulty);
            }
            p::SET_DEFAULT_SPAWN_POSITION => {
                let spawn: packets::SetDefaultSpawnPosition = decode(body)?;
                w = packet(cb::SPAWN_POSITION);
                w.put_i64(pos18(spawn.pos));
            }
            p::SET_OBJECTIVE => {
                let o: packets::SetObjective = decode(body)?;
                w = packet(cb::SCOREBOARD_OBJECTIVE);
                w.put_string(&o.name.chars().take(16).collect::<String>());
                w.put_i8(o.mode as i8);
                if o.mode == 0 || o.mode == 2 {
                    w.put_string(&short_text(&o.display_name, 32));
                    w.put_string(if o.render_type == 1 { "hearts" } else { "integer" });
                }
            }
            p::SET_SCORE => {
                let s: packets::SetScore = decode(body)?;
                w = packet(cb::SCOREBOARD_SCORE);
                w.put_string(&s.owner.chars().take(40).collect::<String>());
                w.put_i8(0);
                w.put_string(&s.objective);
                w.put_varint(s.value);
            }
            p::RESET_SCORE => {
                let s: packets::ResetScore = decode(body)?;
                w = packet(cb::SCOREBOARD_SCORE);
                w.put_string(&s.owner.chars().take(40).collect::<String>());
                w.put_i8(1);
                w.put_string(s.objective.as_deref().unwrap_or(""));
            }
            p::SET_DISPLAY_OBJECTIVE => {
                let d: packets::SetDisplayObjective = decode(body)?;
                // 26.3: 0 list, 1 sidebar, 2 below name, 3-18 team sidebars; as in 1.8.
                w = packet(cb::SCOREBOARD_DISPLAY_OBJECTIVE);
                w.put_i8(d.slot.0 as i8);
                w.put_string(&d.objective);
            }
            p::SET_PLAYER_TEAM => {
                let team: packets::SetPlayerTeam = decode(body)?;
                w = packet(cb::SCOREBOARD_TEAM);
                w.put_string(&team.name.chars().take(16).collect::<String>());
                w.put_i8(team.method);
                if let (0 | 2, Some(params)) = (team.method, &team.parameters) {
                    w.put_string(&short_text(&params.display_name, 32));
                    w.put_string(&short_text(&params.prefix, 16));
                    w.put_string(&short_text(&params.suffix, 16));
                    w.put_i8(params.options);
                    w.put_string(match params.nametag_visibility {
                        1 => "never",
                        2 => "hideForOtherTeams",
                        3 => "hideForOwnTeam",
                        _ => "always",
                    });
                    w.put_i8(params.color.map_or(-1, |c| c as i8));
                }
                if matches!(team.method, 0 | 3 | 4) {
                    w.put_varint(team.members.len() as i32);
                    for member in &team.members {
                        w.put_string(&member.chars().take(40).collect::<String>());
                    }
                }
            }
            p::INITIALIZE_BORDER => {
                let b: packets::InitializeBorder = decode(body)?;
                w = packet(cb::WORLD_BORDER);
                w.put_varint(3);
                w.put_f64(b.center_x);
                w.put_f64(b.center_z);
                w.put_f64(b.old_size);
                w.put_f64(b.new_size);
                w.put_varlong(b.lerp_time.0);
                w.put_varint(b.absolute_max_size.0);
                w.put_varint(b.warning_time.0);
                w.put_varint(b.warning_blocks.0);
            }
            p::SET_BORDER_SIZE => {
                let b: packets::SetBorderSize = decode(body)?;
                w = packet(cb::WORLD_BORDER);
                w.put_varint(0);
                w.put_f64(b.size);
            }
            p::SET_BORDER_LERP_SIZE => {
                let b: packets::SetBorderLerpSize = decode(body)?;
                w = packet(cb::WORLD_BORDER);
                w.put_varint(1);
                w.put_f64(b.old_size);
                w.put_f64(b.new_size);
                w.put_varlong(b.lerp_time.0);
            }
            p::SET_BORDER_CENTER => {
                let b: packets::SetBorderCenter = decode(body)?;
                w = packet(cb::WORLD_BORDER);
                w.put_varint(2);
                w.put_f64(b.center_x);
                w.put_f64(b.center_z);
            }
            p::SET_BORDER_WARNING_DELAY | p::SET_BORDER_WARNING_DISTANCE => {
                w = packet(cb::WORLD_BORDER);
                w.put_varint(if id == p::SET_BORDER_WARNING_DELAY { 4 } else { 5 });
                w.put_varint(Reader::new(body).varint()?);
            }
            p::SET_CAMERA => {
                w = packet(cb::CAMERA);
                w.extend_from_slice(body);
            }
            p::DISCONNECT => {
                let reason = Reader::new(body).component()?;
                w = packet(cb::KICK_DISCONNECT);
                w.put_string(&text::json_1_8(&reason.0));
            }
            p::PLAYER_COMBAT_KILL => {
                // 1.8's combat event "entity dead": the player, the killer (none) and the
                // message.
                let kill: packets::PlayerCombatKill = decode(body)?;
                w = packet(0x42);
                w.put_varint(2);
                w.put_varint(kill.player_id.0);
                w.put_i32(-1);
                w.put_string(&text::json_1_8(&kill.message.0));
            }
            p::COMMAND_SUGGESTIONS => {
                let s: packets::CommandSuggestions = decode(body)?;
                // 1.8 replaces the last word typed with each match.
                let typed = self.completions.iter().position(|(t, _)| *t == s.transaction_id).and_then(|i| self.completions.remove(i)).map(|(_, text)| text).unwrap_or_default();
                let prefix = typed.chars().take(s.start.max(0) as usize).collect::<String>();
                let last_word_start = typed.rfind(' ').map_or(0, |i| i + 1);
                w = packet(cb::TAB_COMPLETE);
                w.put_varint(s.matches.len() as i32);
                for (m, _) in &s.matches {
                    let full = format!("{prefix}{m}");
                    w.put_string(&full[last_word_start.min(full.len())..]);
                }
            }
            p::CUSTOM_PAYLOAD => {
                let mut r = Reader::new(body);
                let channel = r.string(256)?;
                let channel = match channel.as_str() {
                    "minecraft:brand" => "MC|Brand".to_owned(),
                    other => other.to_owned(),
                };
                w = packet(cb::CUSTOM_PAYLOAD);
                w.put_string(&channel);
                w.extend_from_slice(r.rest());
            }
            p::RESOURCE_PACK_PUSH => {
                let mut r = Reader::new(body);
                let pack = r.uuid()?;
                let url = r.string(32767)?;
                let hash = r.string(40)?;
                self.resource_packs.push_back(pack);
                w = packet(cb::RESOURCE_PACK_SEND);
                w.put_string(&url);
                w.put_string(&hash);
            }
            _ => return self.entity_clientbound(id, body, out),
        }
        out.to_client.push(w);
        Ok(())
    }

    /// Takes the world the player is in now (from login and respawn).
    fn enter(&mut self, dimension_type: i32, game_mode: i32) {
        let name = paper_data::synced::entries("minecraft:dimension_type").and_then(|e| e.get(dimension_type as usize).copied()).unwrap_or("minecraft:overworld");
        self.dimension = match name {
            "minecraft:the_nether" => -1,
            "minecraft:the_end" => 1,
            _ => 0,
        };
        if let Some(info) = paper_data::synced::dimension_type(name) {
            self.world = chunk::World { min_y: info.min_y, has_sky: info.has_skylight };
        }
        self.game_mode = game_mode.clamp(0, 3) as u8;
    }

    fn player_info(&mut self, body: &[u8], out: &mut Outcome) -> Result<()> {
        use packets::player_info::*;
        let update: packets::PlayerInfoUpdate = decode(body)?;
        let entry_of = |e: &packets::PlayerInfoEntry| ListEntry {
            name: e.name.clone(),
            properties: e.properties.iter().map(|p| (p.name.clone(), p.value.clone(), p.signature.clone())).collect(),
            game_mode: e.game_mode,
            latency: e.latency,
            display_name: e.display_name.as_ref().map(|c| text::to_1_8(&c.0)),
        };
        if update.actions & ADD_PLAYER != 0 {
            let mut w = packet(cb::PLAYER_INFO);
            w.put_varint(0);
            w.put_varint(update.entries.len() as i32);
            for e in &update.entries {
                let entry = entry_of(e);
                w.put_uuid(e.uuid);
                w.put_string(&entry.name);
                w.put_varint(entry.properties.len() as i32);
                for (name, value, signature) in &entry.properties {
                    w.put_string(name);
                    w.put_string(value);
                    w.put_bool(signature.is_some());
                    if let Some(s) = signature {
                        w.put_string(s);
                    }
                }
                w.put_varint(entry.game_mode);
                w.put_varint(entry.latency);
                w.put_bool(entry.display_name.is_some());
                if let Some(name) = &entry.display_name {
                    w.put_string(&name.to_string());
                }
                self.players.insert(e.uuid, entry);
            }
            out.to_client.push(w);
            return Ok(());
        }
        for (flag, action) in [(UPDATE_GAME_MODE, 1), (UPDATE_LATENCY, 2), (UPDATE_DISPLAY_NAME, 3)] {
            if update.actions & flag == 0 {
                continue;
            }
            let mut w = packet(cb::PLAYER_INFO);
            w.put_varint(action);
            w.put_varint(update.entries.len() as i32);
            for e in &update.entries {
                w.put_uuid(e.uuid);
                match action {
                    1 => w.put_varint(e.game_mode),
                    2 => w.put_varint(e.latency),
                    _ => {
                        w.put_bool(e.display_name.is_some());
                        if let Some(name) = &e.display_name {
                            w.put_string(&text::json_1_8(&name.0));
                        }
                    }
                }
                if let Some(known) = self.players.get_mut(&e.uuid) {
                    match action {
                        1 => known.game_mode = e.game_mode,
                        2 => known.latency = e.latency,
                        _ => known.display_name = e.display_name.as_ref().map(|c| text::to_1_8(&c.0)),
                    }
                }
            }
            out.to_client.push(w);
        }
        Ok(())
    }
}
