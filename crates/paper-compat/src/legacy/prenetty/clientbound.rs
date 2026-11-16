//! 1.8 packets (what the 1.8 translator makes of the server's) as 1.4.6 packets. What 1.4.6
//! has no packet for (scoreboards, titles, particles, the world border, ...) is left out.

use super::wire::{self, put_string, Shape};
use super::{ids, PreNetty};
use crate::io::Result;
use crate::legacy::ids::{cb, login};
use crate::legacy::{from_pos18, text};
use anyhow::bail;
use paper_protocol::codec::{Reader, WriteExt};

fn packet(id: u8) -> Vec<u8> {
    vec![id]
}

/// 1.8 JSON text as 1.4.6's: `§` codes, translations in English where the server knows them.
pub(super) fn legacy_text(json: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(value) => text::legacy_string(&value),
        Err(_) => json.to_owned(),
    }
}

/// 1.4.6 has survival, creative and adventure; spectators are adventurers.
fn game_mode(mode: u8) -> u8 {
    let hardcore = mode & 8;
    hardcore
        | match mode & 7 {
            3 => 2,
            other => other,
        }
}

/// The colour and format codes in effect at the end of `text`.
fn style_at_end(text: &str) -> String {
    let mut style = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '§' {
            continue;
        }
        let Some(code) = chars.next() else { break };
        match code.to_ascii_lowercase() {
            '0'..='9' | 'a'..='f' | 'r' => {
                style.clear();
                if code != 'r' {
                    style.push('§');
                    style.push(code);
                }
            }
            'k'..='o' => {
                style.push('§');
                style.push(code);
            }
            _ => {}
        }
    }
    style
}

/// 1.4.6 takes chat lines of at most 119 characters: longer text goes over several, each
/// starting in the colour the one before ended in.
pub(super) fn chat_lines(text: &str) -> Vec<String> {
    const MAX: usize = 119;
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut units = 0;
        let mut chars = paragraph.chars().peekable();
        while let Some(c) = chars.next() {
            // A code and its letter stay together.
            let token: String = if c == '§' { chars.next().map_or_else(|| "§".to_owned(), |code| format!("§{code}")) } else { c.to_string() };
            let len = token.encode_utf16().count();
            if units + len > MAX {
                let style = style_at_end(&line);
                lines.push(std::mem::replace(&mut line, style));
                units = line.encode_utf16().count();
            }
            line.push_str(&token);
            units += len;
        }
        lines.push(line);
    }
    lines
}

/// Where a hanging entity's block is from the block it hangs on, by its facing (south, west,
/// north, east): 1.4.6 places frames and paintings by the latter, 1.8 by the former.
fn facing_offset(facing: i32) -> (i32, i32) {
    match facing & 3 {
        0 => (0, 1),
        1 => (-1, 0),
        2 => (0, -1),
        _ => (1, 0),
    }
}

/// A 1.8 chunk as 1.4.6's: block ids and their metadata apart, the whole zlib-compressed.
fn chunk(x: i32, z: i32, full: bool, mask: u16, data: &[u8], sky: bool) -> Result<Vec<u8>> {
    let sections = mask.count_ones() as usize;
    let blocks = sections * 8192;
    let light = sections * 2048;
    let lights = light * (1 + sky as usize);
    if data.len() < blocks + lights {
        bail!("a chunk of {} bytes for {sections} sections", data.len());
    }
    let mut raw = vec![0u8; sections * 6144];
    let (block_ids, metadata) = raw.split_at_mut(sections * 4096);
    for (i, pair) in data[..blocks].chunks_exact(2).enumerate() {
        let block = ids::block(u16::from_le_bytes([pair[0], pair[1]]));
        block_ids[i] = (block >> 4) as u8;
        metadata[i / 2] |= ((block & 15) as u8) << (4 * (i % 2));
    }
    raw.extend_from_slice(&data[blocks..blocks + lights]);
    if full {
        // The biomes (absent when the chunk is being unloaded).
        let biomes = data.get(blocks + lights..blocks + lights + 256).unwrap_or(&[0u8; 256][..]);
        raw.extend_from_slice(biomes);
    }
    let packed = wire::zlib(&raw);
    let mut w = packet(0x33);
    w.put_i32(x);
    w.put_i32(z);
    w.put_bool(full);
    w.put_u16(mask);
    w.put_u16(0);
    w.put_i32(packed.len() as i32);
    w.put_slice(&packed);
    Ok(w)
}

impl PreNetty {
    /// What the entity is, for its data.
    fn shape(&self, entity: i32) -> Shape {
        use crate::legacy::tables::EntityKind;
        match self.inner.entities.get(&entity).map(|e| e.kind) {
            Some(EntityKind::Player) => Shape::Player,
            Some(EntityKind::Mob(mob)) => Shape::Mob(mob),
            Some(EntityKind::Object(object, _)) => Shape::Object(object),
            _ if entity == self.inner.player => Shape::Player,
            _ => Shape::Other,
        }
    }

    /// A 1.8 login packet: only a refusal has a 1.4.6 counterpart (it is in play once logged in).
    pub(super) fn login_clientbound(&mut self, payload: &[u8], out: &mut Vec<Vec<u8>>) -> Result<()> {
        let mut r = Reader::new(payload);
        if r.varint()? == login::DISCONNECT {
            let mut w = packet(0xFF);
            put_string(&mut w, &legacy_text(&r.string(1 << 18)?), 256);
            out.push(w);
        }
        Ok(())
    }

    /// A 1.8 play packet as 1.4.6's.
    pub(super) fn play_clientbound(&mut self, payload: &[u8], out: &mut Vec<Vec<u8>>) -> Result<()> {
        let mut r = Reader::new(payload);
        let id = r.varint()?;
        let mut w;
        match id {
            cb::KEEP_ALIVE => {
                w = packet(0x00);
                w.put_i32(r.varint()?);
            }
            cb::LOGIN => {
                let entity = r.i32()?;
                let mode = r.u8()?;
                let dimension = r.i8()?;
                let difficulty = r.u8()?;
                let max_players = r.u8()?;
                let level = r.string(16)?;
                w = packet(0x01);
                w.put_i32(entity);
                put_string(&mut w, &level, 16);
                w.put_u8(game_mode(mode));
                w.put_i8(dimension);
                w.put_u8(difficulty);
                // The world's height: unused.
                w.put_u8(0);
                w.put_u8(max_players);
            }
            cb::CHAT => {
                let json = r.string(1 << 18)?;
                // Above the hotbar: 1.4.6 has no such place.
                if r.i8()? == 2 {
                    return Ok(());
                }
                for line in chat_lines(&legacy_text(&json)) {
                    let mut w = packet(0x03);
                    put_string(&mut w, &line, 119);
                    out.push(w);
                }
                return Ok(());
            }
            cb::UPDATE_TIME => {
                w = packet(0x04);
                w.put_i64(r.i64()?);
                w.put_i64(r.i64()?);
            }
            cb::ENTITY_EQUIPMENT => {
                w = packet(0x05);
                w.put_i32(r.varint()?);
                w.put_i16(r.i16()?);
                wire::slot_to_1_4(&mut r, &mut w)?;
            }
            cb::SPAWN_POSITION => {
                let pos = from_pos18(r.i64()?);
                w = packet(0x06);
                w.put_i32(pos.x);
                w.put_i32(pos.y);
                w.put_i32(pos.z);
            }
            cb::UPDATE_HEALTH => {
                let health = r.f32()?;
                let food = r.varint()?;
                let saturation = r.f32()?;
                w = packet(0x08);
                // Whole half hearts: rounded up, so a living player is not shown dead.
                w.put_i16(health.ceil().clamp(-32768.0, 32767.0) as i16);
                w.put_i16(food.clamp(0, 32767) as i16);
                w.put_f32(saturation);
            }
            cb::RESPAWN => {
                let dimension = r.i32()?;
                let difficulty = r.u8()?;
                let mode = r.u8()?;
                let level = r.string(16)?;
                w = packet(0x09);
                w.put_i32(dimension);
                w.put_u8(difficulty);
                w.put_u8(game_mode(mode));
                w.put_i16(256);
                put_string(&mut w, &level, 16);
            }
            cb::POSITION => {
                // 1.4.6 has no relative teleports: where the 1.8 translator has the player.
                let (x, y, z) = self.inner.position;
                let (yaw, pitch) = self.inner.rotation;
                w = packet(0x0D);
                // To the client: x, its eyes, its feet, z.
                w.put_f64(x);
                w.put_f64(y + 1.62);
                w.put_f64(y);
                w.put_f64(z);
                w.put_f32(yaw);
                w.put_f32(pitch);
                w.put_bool(false);
            }
            cb::HELD_ITEM_SLOT => {
                w = packet(0x10);
                w.put_i16(r.i8()? as i16);
            }
            cb::ANIMATION => {
                let entity = r.varint()?;
                // 1.8: swing, hurt, wake up, eat, critical, magic critical; 1.4.6 counts from 1
                // and has nothing at 4.
                let animation = match r.u8()? {
                    0 => 1,
                    1 => 2,
                    2 => 3,
                    3 => 5,
                    4 => 6,
                    5 => 7,
                    _ => return Ok(()),
                };
                w = packet(0x12);
                w.put_i32(entity);
                w.put_u8(animation);
            }
            cb::NAMED_ENTITY_SPAWN => {
                let entity = r.varint()?;
                let uuid = r.uuid()?;
                let (x, y, z) = (r.i32()?, r.i32()?, r.i32()?);
                let (yaw, pitch) = (r.u8()?, r.u8()?);
                let held = r.i16()?;
                w = packet(0x14);
                w.put_i32(entity);
                put_string(&mut w, self.names.get(&uuid).map_or("Player", String::as_str), 16);
                w.put_i32(x);
                w.put_i32(y);
                w.put_i32(z);
                w.put_u8(yaw);
                w.put_u8(pitch);
                w.put_i16(if held > 0 { ids::item(held, 0).0 } else { 0 });
                wire::entity_data(&mut r, &mut w, Shape::Player)?;
            }
            cb::COLLECT => {
                w = packet(0x16);
                w.put_i32(r.varint()?);
                w.put_i32(r.varint()?);
            }
            cb::SPAWN_ENTITY => {
                let entity = r.varint()?;
                let kind = r.u8()?;
                let (mut x, mut y, mut z) = (r.i32()?, r.i32()?, r.i32()?);
                let (pitch, yaw) = (r.u8()?, r.u8()?);
                let data = r.i32()?;
                let velocity = if data != 0 { (r.i16()?, r.i16()?, r.i16()?) } else { (0, 0, 0) };
                let Some((kind, data)) = ids::object(kind, data) else { return Ok(()) };
                if kind == 71 {
                    let (dx, dz) = facing_offset(data);
                    x = ((x >> 5) - dx) << 5;
                    y = (y >> 5) << 5;
                    z = ((z >> 5) - dz) << 5;
                }
                w = packet(0x17);
                w.put_i32(entity);
                w.put_u8(kind);
                w.put_i32(x);
                w.put_i32(y);
                w.put_i32(z);
                w.put_u8(pitch);
                w.put_u8(yaw);
                w.put_i32(data);
                if data > 0 {
                    w.put_i16(velocity.0);
                    w.put_i16(velocity.1);
                    w.put_i16(velocity.2);
                }
            }
            cb::SPAWN_ENTITY_LIVING => {
                let entity = r.varint()?;
                let kind = r.u8()?;
                let Some(mob) = ids::mob(kind) else { return Ok(()) };
                w = packet(0x18);
                w.put_i32(entity);
                w.put_u8(mob);
                // Position, rotations, head and velocity are the same.
                w.put_slice(r.take(12 + 3 + 6)?);
                wire::entity_data(&mut r, &mut w, Shape::Mob(kind))?;
            }
            cb::SPAWN_ENTITY_PAINTING => {
                let entity = r.varint()?;
                let title = r.string(13)?;
                let pos = from_pos18(r.i64()?);
                let facing = r.u8()? as i32;
                let (dx, dz) = facing_offset(facing);
                w = packet(0x19);
                w.put_i32(entity);
                put_string(&mut w, &title, 13);
                w.put_i32(pos.x - dx);
                w.put_i32(pos.y);
                w.put_i32(pos.z - dz);
                w.put_i32(facing);
            }
            cb::SPAWN_ENTITY_EXPERIENCE_ORB => {
                w = packet(0x1A);
                w.put_i32(r.varint()?);
                w.put_slice(r.take(14)?);
            }
            cb::ENTITY_VELOCITY => {
                w = packet(0x1C);
                w.put_i32(r.varint()?);
                w.put_slice(r.take(6)?);
            }
            cb::ENTITY_DESTROY => {
                let entities = r.list(1 << 16, |r| r.varint())?;
                // At most 127 a packet: 1.4.6 reads the count as a signed byte.
                for some in entities.chunks(127) {
                    let mut w = packet(0x1D);
                    w.put_u8(some.len() as u8);
                    for entity in some {
                        w.put_i32(*entity);
                    }
                    out.push(w);
                }
                return Ok(());
            }
            cb::REL_ENTITY_MOVE => {
                w = packet(0x1F);
                w.put_i32(r.varint()?);
                w.put_slice(r.take(3)?);
            }
            cb::ENTITY_LOOK => {
                w = packet(0x20);
                w.put_i32(r.varint()?);
                w.put_slice(r.take(2)?);
            }
            cb::ENTITY_MOVE_LOOK => {
                w = packet(0x21);
                w.put_i32(r.varint()?);
                w.put_slice(r.take(5)?);
            }
            cb::ENTITY_TELEPORT => {
                w = packet(0x22);
                w.put_i32(r.varint()?);
                w.put_slice(r.take(14)?);
            }
            cb::ENTITY_HEAD_ROTATION => {
                w = packet(0x23);
                w.put_i32(r.varint()?);
                w.put_u8(r.u8()?);
            }
            cb::ENTITY_STATUS => {
                let entity = r.i32()?;
                let status = r.i8()?;
                // Hearts, spawner flames and the rest of 1.6 to 1.8's.
                if !(0..=17).contains(&status) {
                    return Ok(());
                }
                w = packet(0x26);
                w.put_i32(entity);
                w.put_i8(status);
            }
            cb::ATTACH_ENTITY => {
                let rider = r.i32()?;
                let vehicle = r.i32()?;
                // Leads came in 1.6.
                if r.bool()? {
                    return Ok(());
                }
                w = packet(0x27);
                w.put_i32(rider);
                w.put_i32(vehicle);
            }
            cb::ENTITY_METADATA => {
                let entity = r.varint()?;
                let mut data = Vec::new();
                wire::entity_data(&mut r, &mut data, self.shape(entity))?;
                if data.len() == 1 {
                    return Ok(());
                }
                w = packet(0x28);
                w.put_i32(entity);
                w.put_slice(&data);
            }
            cb::ENTITY_EFFECT => {
                let entity = r.varint()?;
                let effect = r.i8()?;
                let amplifier = r.i8()?;
                let duration = r.varint()?;
                // 1.4.6 has the twenty effects up to wither.
                if !(1..=20).contains(&effect) {
                    return Ok(());
                }
                w = packet(0x29);
                w.put_i32(entity);
                w.put_i8(effect);
                w.put_i8(amplifier);
                w.put_i16(duration.clamp(0, 32767) as i16);
            }
            cb::REMOVE_ENTITY_EFFECT => {
                let entity = r.varint()?;
                let effect = r.i8()?;
                if !(1..=20).contains(&effect) {
                    return Ok(());
                }
                w = packet(0x2A);
                w.put_i32(entity);
                w.put_i8(effect);
            }
            cb::EXPERIENCE => {
                w = packet(0x2B);
                w.put_f32(r.f32()?);
                w.put_i16(r.varint()?.clamp(0, 32767) as i16);
                w.put_i16(r.varint()?.clamp(0, 32767) as i16);
            }
            cb::MAP_CHUNK => {
                let (x, z) = (r.i32()?, r.i32()?);
                let full = r.bool()?;
                let mask = r.u16()?;
                let data = r.byte_array(1 << 22)?;
                w = chunk(x, z, full, mask, data, self.inner.world.has_sky)?;
            }
            cb::MULTI_BLOCK_CHANGE => {
                let (x, z) = (r.i32()?, r.i32()?);
                let count = r.varint()?;
                let mut records = Vec::with_capacity(count.clamp(0, 4096) as usize * 4);
                for _ in 0..count {
                    let (xz, y) = (r.u8()?, r.u8()?);
                    let block = ids::block(r.varint()? as u16);
                    records.put_u16((xz as u16) << 8 | y as u16);
                    records.put_u16(block);
                }
                w = packet(0x34);
                w.put_i32(x);
                w.put_i32(z);
                w.put_i16(count as i16);
                w.put_i32(records.len() as i32);
                w.put_slice(&records);
            }
            cb::BLOCK_CHANGE => {
                let pos = from_pos18(r.i64()?);
                let block = ids::block(r.varint()? as u16);
                w = packet(0x35);
                w.put_i32(pos.x);
                w.put_u8(pos.y as u8);
                w.put_i32(pos.z);
                w.put_i16((block >> 4) as i16);
                w.put_u8((block & 15) as u8);
            }
            cb::BLOCK_ACTION => {
                let pos = from_pos18(r.i64()?);
                let (a, b) = (r.u8()?, r.u8()?);
                let block = ids::block((r.varint()? as u16) << 4) >> 4;
                w = packet(0x36);
                w.put_i32(pos.x);
                w.put_i16(pos.y as i16);
                w.put_i32(pos.z);
                w.put_u8(a);
                w.put_u8(b);
                w.put_i16(block as i16);
            }
            cb::BLOCK_BREAK_ANIMATION => {
                let entity = r.varint()?;
                let pos = from_pos18(r.i64()?);
                w = packet(0x37);
                w.put_i32(entity);
                w.put_i32(pos.x);
                w.put_i32(pos.y);
                w.put_i32(pos.z);
                w.put_i8(r.i8()?);
            }
            cb::EXPLOSION => {
                w = packet(0x3C);
                for _ in 0..3 {
                    w.put_f64(r.f32()? as f64);
                }
                w.put_f32(r.f32()?);
                let records = r.i32()?;
                w.put_i32(records);
                w.put_slice(r.take(records.max(0) as usize * 3)?);
                w.put_slice(r.take(12)?);
            }
            cb::WORLD_EVENT => {
                let event = r.i32()?;
                let pos = from_pos18(r.i64()?);
                let mut data = r.i32()?;
                let global = r.bool()?;
                // Sounds and effects up to the anvil's and the spawner's flames.
                if !(1000..=1022).contains(&event) && !(2000..=2004).contains(&event) {
                    return Ok(());
                }
                if event == 2001 {
                    let block = ids::block(((data & 0xFFF) as u16) << 4 | ((data >> 12) & 15) as u16);
                    data = (block >> 4) as i32 | ((block & 15) as i32) << 12;
                }
                w = packet(0x3D);
                w.put_i32(event);
                w.put_i32(pos.x);
                w.put_u8(pos.y as u8);
                w.put_i32(pos.z);
                w.put_i32(data);
                w.put_bool(global);
            }
            cb::NAMED_SOUND_EFFECT => {
                let name = r.string(256)?;
                if name.len() > 32 {
                    return Ok(());
                }
                w = packet(0x3E);
                put_string(&mut w, &name, 32);
                w.put_slice(r.take(12 + 4 + 1)?);
            }
            cb::GAME_STATE_CHANGE => {
                let reason = r.u8()?;
                let value = r.f32()?;
                // Beds, rain, game modes, credits, the demo and arrows; not 1.8's fades.
                if reason > 6 {
                    return Ok(());
                }
                w = packet(0x46);
                w.put_u8(reason);
                w.put_u8(if reason == 3 { game_mode(value as u8) } else { value as u8 });
            }
            cb::SPAWN_ENTITY_WEATHER => {
                w = packet(0x47);
                w.put_i32(r.varint()?);
                w.put_slice(r.take(13)?);
            }
            cb::OPEN_WINDOW => {
                let window = r.u8()?;
                let kind = r.string(32)?;
                let title = r.string(1 << 16)?;
                let slots = r.u8()?;
                let kind = match kind.as_str() {
                    "minecraft:chest" | "minecraft:container" => 0,
                    "minecraft:crafting_table" => 1,
                    "minecraft:furnace" => 2,
                    "minecraft:dispenser" | "minecraft:dropper" => 3,
                    "minecraft:enchanting_table" => 4,
                    "minecraft:brewing_stand" => 5,
                    "minecraft:villager" => 6,
                    "minecraft:beacon" => 7,
                    "minecraft:anvil" => 8,
                    // Hoppers and horses: no window 1.4.6 could number the same way.
                    _ => return Ok(()),
                };
                self.windows.insert(window, kind);
                w = packet(0x64);
                w.put_u8(window);
                w.put_u8(kind);
                put_string(&mut w, &legacy_text(&title), 32);
                w.put_u8(slots);
            }
            cb::CLOSE_WINDOW => {
                let window = r.u8()?;
                self.windows.remove(&window);
                w = packet(0x65);
                w.put_u8(window);
            }
            cb::SET_SLOT => {
                w = packet(0x67);
                w.put_i8(r.i8()?);
                w.put_i16(r.i16()?);
                wire::slot_to_1_4(&mut r, &mut w)?;
            }
            cb::WINDOW_ITEMS => {
                w = packet(0x68);
                w.put_u8(r.u8()?);
                let slots = r.i16()?;
                w.put_i16(slots);
                for _ in 0..slots {
                    wire::slot_to_1_4(&mut r, &mut w)?;
                }
            }
            cb::CRAFT_PROGRESS_BAR => {
                let window = r.u8()?;
                let property = r.i16()?;
                let value = r.i16()?;
                let property = match self.windows.get(&window) {
                    // Furnaces: 1.4.6 has the progress first, then the fuel and its total, and
                    // no total cooking time.
                    Some(2) => match property {
                        0 => 1,
                        1 => 2,
                        2 => 0,
                        _ => return Ok(()),
                    },
                    // Enchanting tables: only the three levels.
                    Some(4) if property > 2 => return Ok(()),
                    _ => property,
                };
                w = packet(0x69);
                w.put_u8(window);
                w.put_i16(property);
                w.put_i16(value);
            }
            cb::TRANSACTION => {
                w = packet(0x6A);
                w.put_slice(r.take(4)?);
            }
            cb::UPDATE_SIGN => {
                let pos = from_pos18(r.i64()?);
                w = packet(0x82);
                w.put_i32(pos.x);
                w.put_i16(pos.y as i16);
                w.put_i32(pos.z);
                for _ in 0..4 {
                    put_string(&mut w, &legacy_text(&r.string(1 << 16)?), 15);
                }
            }
            cb::PLAYER_INFO => return self.player_list(&mut r, out),
            cb::ABILITIES => {
                w = packet(0xCA);
                w.put_i8(r.i8()?);
                w.put_u8((r.f32()? * 255.0).clamp(0.0, 255.0) as u8);
                w.put_u8((r.f32()? * 255.0).clamp(0.0, 255.0) as u8);
            }
            cb::TAB_COMPLETE => {
                let matches = r.list(1 << 12, |r| r.string(32767))?;
                w = packet(0xCB);
                put_string(&mut w, &matches.join("\0"), 32767);
            }
            cb::KICK_DISCONNECT => {
                w = packet(0xFF);
                put_string(&mut w, &legacy_text(&r.string(1 << 18)?), 256);
            }
            _ => return Ok(()),
        }
        out.push(w);
        Ok(())
    }

    /// 1.8's tab list as 1.4.6's: a name, online or not, and a ping, one packet each.
    fn player_list(&mut self, r: &mut Reader<'_>, out: &mut Vec<Vec<u8>>) -> Result<()> {
        let action = r.varint()?;
        let count = r.varint()?;
        for _ in 0..count {
            let uuid = r.uuid()?;
            let (name, online, ping) = match action {
                0 => {
                    let name = r.string(16)?;
                    let properties = r.varint()?;
                    for _ in 0..properties {
                        r.string(32767)?;
                        r.string(32767)?;
                        if r.bool()? {
                            r.string(32767)?;
                        }
                    }
                    r.varint()?;
                    let ping = r.varint()?;
                    if r.bool()? {
                        r.string(1 << 18)?;
                    }
                    self.names.insert(uuid, name.clone());
                    (name, true, ping)
                }
                1 => {
                    r.varint()?;
                    continue;
                }
                2 => {
                    let ping = r.varint()?;
                    let Some(name) = self.names.get(&uuid) else { continue };
                    (name.clone(), true, ping)
                }
                3 => {
                    if r.bool()? {
                        r.string(1 << 18)?;
                    }
                    continue;
                }
                4 => {
                    let Some(name) = self.names.remove(&uuid) else { continue };
                    (name, false, 0)
                }
                other => bail!("tab list action {other}"),
            };
            let mut w = packet(0xC9);
            put_string(&mut w, &name, 16);
            w.put_bool(online);
            w.put_i16(ping.clamp(0, 32767) as i16);
            out.push(w);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_chat_goes_over_lines_in_its_colour() {
        let text = format!("§c{}", "x".repeat(200));
        let lines = chat_lines(&text);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].encode_utf16().count(), 119);
        assert!(lines[1].starts_with("§cx"), "{}", lines[1]);
        assert_eq!(chat_lines("a\nb"), ["a", "b"]);
    }

    #[test]
    fn chunks_split_ids_from_metadata() {
        // One section: a white wool (35:0) at index 0, orange wool (35:1) at 1, granite at 2.
        let mut data = vec![0u8; 8192 + 2048 + 2048 + 256];
        data[0..2].copy_from_slice(&(35u16 << 4).to_le_bytes());
        data[2..4].copy_from_slice(&(35u16 << 4 | 1).to_le_bytes());
        data[4..6].copy_from_slice(&(1u16 << 4 | 1).to_le_bytes());
        let packet = chunk(0, 0, true, 1, &data, true).unwrap();
        let mut r = Reader::new(&packet[1..]);
        assert_eq!((r.i32().unwrap(), r.i32().unwrap(), r.bool().unwrap(), r.u16().unwrap(), r.u16().unwrap()), (0, 0, true, 1, 0));
        let len = r.i32().unwrap() as usize;
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(r.take(len).unwrap()), &mut raw).unwrap();
        assert_eq!(raw.len(), 4096 + 2048 * 3 + 256);
        assert_eq!(&raw[0..3], &[35, 35, 1]);
        assert_eq!(raw[4096], 0x10, "metadata nibbles, low first: 0 then 1");
        assert_eq!(raw[4097] & 0x0F, 0, "granite became plain stone");
    }
}
