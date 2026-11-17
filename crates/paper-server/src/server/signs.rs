//! Signs (`SignBlock`, `SignBlockEntity`): the text editor opens when a player places a sign
//! and when they click one (on the side they face). The text is kept in the sign's block entity
//! and shown to everyone. Dyes colour a side's text, glow ink sacs make it glow (ink sacs undo
//! that), and honeycomb waxes the sign so it cannot be edited any more.

use super::player::{HOTBAR_START, OFFHAND_SLOT};
use super::survival::category;
use super::Server;
use crate::config::GameMode;
use crate::world::chunk::ChunkPos;
use crate::world::entity::Vec3;
use paper_data::{blocks, items};
use paper_nbt::{Compound, Tag};
use paper_protocol::codec::{BlockPos, Nbt, VarInt};
use paper_protocol::packets::play;
use paper_protocol::text::Component;

/// Whether a block is a sign (standing, wall or hanging).
pub fn is_sign(name: &str) -> bool {
    name.ends_with("_sign")
}

fn is_hanging(name: &str) -> bool {
    name.ends_with("_hanging_sign")
}

/// The block entity type of a sign.
fn entity_type(name: &str) -> &'static str {
    if is_hanging(name) {
        "minecraft:hanging_sign"
    } else {
        "minecraft:sign"
    }
}

/// `SignText` with nothing written: four empty lines in black.
fn empty_side() -> Compound {
    Compound::new()
        .with("messages", Tag::List(vec![Tag::String(String::new()); 4]))
        .with("color", Tag::String("black".into()))
        .with("has_glowing_text", Tag::Byte(0))
}

fn side_key(front: bool) -> &'static str {
    if front {
        "front_text"
    } else {
        "back_text"
    }
}

/// `SignBlock.getYRotationDegrees`.
fn sign_yaw(state: u32) -> f32 {
    if let Some(rotation) = blocks::property(state, "rotation").and_then(|r| r.parse::<f32>().ok()) {
        return rotation * 22.5;
    }
    match blocks::property(state, "facing") {
        Some("west") => 90.0,
        Some("north") => 180.0,
        Some("east") => 270.0,
        _ => 0.0,
    }
}

fn wrap_degrees(d: f32) -> f32 {
    let d = d % 360.0;
    if d >= 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    }
}

/// `SignBlockEntity.isFacingFrontText`: whether a player at `at` sees the sign's front.
fn facing_front(state: u32, pos: BlockPos, at: Vec3) -> bool {
    let name = blocks::block_of_state(state).name;
    // The middle of the sign: against the wall for wall signs, else the middle of the block.
    let (mut cx, mut cz) = (0.5, 0.5);
    if name.contains("_wall_") && !is_hanging(name) {
        let (sx, sz) = match blocks::property(state, "facing") {
            Some("north") => (0.0, -1.0),
            Some("south") => (0.0, 1.0),
            Some("west") => (-1.0, 0.0),
            _ => (1.0, 0.0),
        };
        cx -= sx * 0.4375;
        cz -= sz * 0.4375;
    }
    let (xd, zd) = (at.x - (pos.x as f64 + cx), at.z - (pos.z as f64 + cz));
    let to_player = zd.atan2(xd).to_degrees() as f32 - 90.0;
    wrap_degrees(to_player - sign_yaw(state)).abs() <= 90.0
}

/// `ChatFormatting.stripFormatting`: section-sign codes are taken out of what players write.
fn strip_formatting(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{a7}' && chars.peek().is_some_and(|n| n.is_ascii_hexdigit() || matches!(n.to_ascii_lowercase(), 'k'..='o' | 'r')) {
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

impl Server {
    fn sign_key(pos: BlockPos) -> (ChunkPos, (i32, u8, u8)) {
        (ChunkPos::of_block(pos.x, pos.z), (pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8))
    }

    /// The sign's block entity, made with empty sides if it has none yet.
    fn sign_data(&mut self, world: usize, pos: BlockPos) -> Option<&mut Compound> {
        let state = self.worlds[world].block_at(pos.x, pos.y, pos.z)?;
        let name = blocks::block_of_state(state).name;
        if !is_sign(name) {
            return None;
        }
        let (chunk, key) = Self::sign_key(pos);
        let c = self.worlds[world].chunks.get_mut(&chunk)?;
        c.dirty = true;
        let nbt = c.block_entities.entry(key).or_insert_with(|| Compound::new().with("id", Tag::String(entity_type(name).into())));
        for front in [true, false] {
            if nbt.compound(side_key(front)).is_none() {
                nbt.insert(side_key(front), Tag::Compound(empty_side()));
            }
        }
        if nbt.get("is_waxed").is_none() {
            nbt.insert("is_waxed", Tag::Byte(0));
        }
        Some(nbt)
    }

    /// Shows everyone near the sign what it says (`sendBlockUpdated`); players who get the
    /// chunk later get a packet made anew.
    fn send_sign(&mut self, world: usize, pos: BlockPos) {
        if let Some(c) = self.worlds[world].chunks.get_mut(&ChunkPos::of_block(pos.x, pos.z)) {
            c.revision += 1;
        }
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let Some(data) = self.sign_data(world, pos).map(|d| d.clone()) else { return };
        let Some(ty) = paper_data::registries::id("minecraft:block_entity_type", entity_type(blocks::block_of_state(state).name)) else { return };
        let packet = play::BlockEntityData { pos, block_entity_type: VarInt(ty as i32), data: Nbt(Some(Tag::Compound(data))) };
        self.broadcast_to_chunk_viewers(world, ChunkPos::of_block(pos.x, pos.z), &packet);
    }

    /// Opens the sign editor for a player on one side (`SignBlock.openTextEdit`), unless
    /// plugins refuse (`PlayerOpenSignEvent`; `cause` is its `Cause`): only they may write on
    /// it until they are done.
    fn open_sign_editor(&mut self, player: i32, world: usize, pos: BlockPos, front: bool, cause: &str) {
        if self.sign_data(world, pos).is_none() || !crate::plugins::events::on_sign_open(self, player, world, pos, front, cause) {
            return;
        }
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let Some(p) = self.players.get_mut(&player) else { return };
        p.editing_sign = Some((world, pos, front));
        p.conn.send(&play::BlockUpdate { pos, state: VarInt(state as i32) });
        p.conn.send(&play::OpenSignEditor { pos, side: VarInt(i32::from(front)) });
    }

    fn someone_else_editing(&self, player: i32, world: usize, pos: BlockPos) -> bool {
        self.players.values().any(|p| p.entity_id != player && p.editing_sign.is_some_and(|(w, at, _)| w == world && at == pos))
    }

    /// After a player placed a sign (`SignItem.updateCustomBlockEntityTag`): its front opens
    /// for writing.
    pub(super) fn sign_placed(&mut self, player: i32, world: usize, pos: BlockPos) {
        self.open_sign_editor(player, world, pos, true, "PLACE");
    }

    /// A click on a sign (`SignBlock.useItemOn` and `useWithoutItem`): dyes, glow ink sacs,
    /// ink sacs and honeycomb work on the side the player faces; otherwise its editor opens
    /// there, unless the sign is waxed or someone else is writing on it. Returns whether the
    /// click was used.
    pub(super) fn use_sign(&mut self, player: i32, world: usize, pos: BlockPos, hand: i32) -> bool {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return false };
        let name = blocks::block_of_state(state).name;
        if !is_sign(name) {
            return false;
        }
        let Some(p) = self.players.get(&player) else { return false };
        let (at, creative, may_build) = (Vec3::new(p.loc.x, p.loc.y, p.loc.z), p.game_mode == GameMode::Creative, matches!(p.game_mode, GameMode::Survival | GameMode::Creative));
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let held = p.inventory[slot].0.clone();
        let front = facing_front(state, pos, at);
        let other = self.someone_else_editing(player, world, pos);
        let center = Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5);
        let waxed = self.sign_data(world, pos).is_some_and(|d| d.bool("is_waxed") == Some(true));
        // `SignApplicator`s.
        let item_name = held.as_ref().and_then(|s| items::get(s.item as u32)).map_or("", |i| i.name);
        let applied = if !waxed && !other && may_build {
            let side = side_key(front);
            let data = self.sign_data(world, pos).expect("a sign");
            let text = data.compound(side).cloned().unwrap_or_else(empty_side);
            let mut text_now = text.clone();
            let sound = match item_name {
                dye if dye.ends_with("_dye") => {
                    let color = dye.trim_start_matches("minecraft:").trim_end_matches("_dye");
                    text_now.insert("color", Tag::String(color.into()));
                    Some("minecraft:item.dye.use")
                }
                "minecraft:glow_ink_sac" => {
                    text_now.insert("has_glowing_text", Tag::Byte(1));
                    Some("minecraft:item.glow_ink_sac.use")
                }
                "minecraft:ink_sac" => {
                    text_now.insert("has_glowing_text", Tag::Byte(0));
                    Some("minecraft:item.ink_sac.use")
                }
                "minecraft:honeycomb" => {
                    data.insert("is_waxed", Tag::Byte(1));
                    Some("")
                }
                _ => None,
            };
            match sound {
                // Nothing changes (the colour it has already, glowing already...): no use.
                Some(s) if !s.is_empty() && text_now == text => None,
                Some(s) => {
                    data.insert(side, Tag::Compound(text_now));
                    Some(s)
                }
                None => None,
            }
        } else {
            None
        };
        if let Some(sound) = applied {
            if sound.is_empty() {
                self.broadcast_level_event(world, 3003, pos, 0, false);
            } else {
                self.play_sound(world, center, sound, category::BLOCKS, 1.0, 1.0, None);
            }
            if !creative {
                self.use_held(player, hand);
            }
            self.award_stat(player, "minecraft:used", item_name, 1);
            self.send_sign(world, pos);
            return true;
        }
        if waxed {
            let sound = if is_hanging(name) { "minecraft:block.hanging_sign.waxed_interact_fail" } else { "minecraft:block.sign.waxed_interact_fail" };
            self.play_sound(world, center, sound, category::BLOCKS, 1.0, 1.0, None);
            return true;
        }
        if other || !may_build {
            return false;
        }
        self.open_sign_editor(player, world, pos, front, "INTERACT");
        true
    }

    /// `ServerboundSignUpdatePacket`: what a player wrote on the side they were given
    /// (`SignBlockEntity.updateSignText`). Formatting codes are taken out, and only the
    /// player the editor was opened for may write, on an unwaxed sign.
    pub(super) fn handle_sign_update(&mut self, player: i32, update: play::SignUpdate) {
        let Some(p) = self.players.get_mut(&player) else { return };
        let Some((world, pos, front)) = p.editing_sign.take() else { return };
        if pos != update.pos || front != update.front {
            return;
        }
        if self.sign_data(world, pos).is_none_or(|d| d.bool("is_waxed") == Some(true)) {
            return;
        }
        let lines: Vec<Component> = update.lines.iter().map(|line| Component::text(strip_formatting(line))).collect();
        // `SignChangeEvent`: plugins may change the lines, or cancel (the sign stays as it was).
        let Some(lines) = crate::plugins::events::on_sign_change(self, player, world, pos, front, &lines) else {
            self.send_sign(world, pos);
            return;
        };
        let Some(data) = self.sign_data(world, pos) else { return };
        let side = side_key(front);
        let mut text = data.compound(side).cloned().unwrap_or_else(empty_side);
        text.insert("messages", Tag::List(lines.iter().take(4).map(Component::to_nbt).collect()));
        text.remove("filtered_messages");
        data.insert(side, Tag::Compound(text));
        self.send_sign(world, pos);
    }

    /// A sign's text for plugins (`Sign`, `SignSide`): each side's lines, colour and glow, and
    /// whether it is waxed. `None` if the block is not a sign.
    pub fn sign_value(&mut self, world: usize, pos: BlockPos) -> Option<paper_bridge::Value> {
        use paper_bridge::{bmap, Value};
        let data = self.sign_data(world, pos)?.clone();
        let side = |front: bool| {
            let text = data.compound(side_key(front)).cloned().unwrap_or_else(empty_side);
            let lines: Vec<Value> = (0..4)
                .map(|i| text.list("messages").and_then(|m| m.get(i)).map_or_else(|| Component::text(""), Component::from_nbt))
                .map(|c| crate::plugins::component_value(&c))
                .collect();
            bmap! {
                "lines" => Value::List(lines),
                "color" => text.string("color").unwrap_or("black").to_owned(),
                "glowing" => text.bool("has_glowing_text").unwrap_or(false),
            }
        };
        Some(bmap! { "front" => side(true), "back" => side(false), "waxed" => data.bool("is_waxed").unwrap_or(false) })
    }

    /// Writes what a plugin set on a sign (`BlockState.update`) and shows it to players.
    pub fn set_sign_value(&mut self, world: usize, pos: BlockPos, value: &paper_bridge::Value) -> bool {
        let Some(data) = self.sign_data(world, pos) else { return false };
        for front in [true, false] {
            let given = value.get(side_key(front).trim_end_matches("_text"));
            if given.is_null() {
                continue;
            }
            let mut text = data.compound(side_key(front)).cloned().unwrap_or_else(empty_side);
            if let Some(lines) = given.list("lines") {
                let messages = (0..4).map(|i| lines.get(i).and_then(crate::plugins::component_from).unwrap_or_else(|| Component::text("")).to_nbt()).collect();
                text.insert("messages", Tag::List(messages));
                text.remove("filtered_messages");
            }
            if let Some(color) = given.str("color") {
                text.insert("color", Tag::String(color.to_owned()));
            }
            if let Some(glowing) = given.bool("glowing") {
                text.insert("has_glowing_text", Tag::Byte(glowing as i8));
            }
            data.insert(side_key(front), Tag::Compound(text));
        }
        if let Some(waxed) = value.bool("waxed") {
            data.insert("is_waxed", Tag::Byte(waxed as i8));
        }
        self.send_sign(world, pos);
        true
    }
}
