//! 1.8 packets (what the 1.8 translator makes of the server's) as 1.7 packets. The ids are the
//! same; entity ids are ints where 1.8 has VarInts, block positions three numbers, and what 1.7
//! has no packet for (titles, the world border, ...) is left out.

use super::{ids, Legacy17, Listed};
use crate::io::Result;
use crate::legacy::ids::{cb, login};
use crate::legacy::prenetty::clientbound::{chunk_with, facing_offset, legacy_text};
use crate::legacy::prenetty::wire::{self, Shape};
use crate::legacy::{from_pos18, inventory};
use anyhow::bail;
use paper_protocol::codec::{Reader, WriteExt};

fn packet(id: i32) -> Vec<u8> {
    let mut w = Vec::new();
    w.put_varint(id);
    w
}

/// 1.7 has survival, creative and adventure; spectators are adventurers.
fn game_mode(mode: u8) -> u8 {
    (mode & 8)
        | match mode & 7 {
            3 => 2,
            other => other,
        }
}

/// A 1.8 slot as 1.7's: ids it has, NBT gzipped behind a short length.
pub(super) fn slot(r: &mut Reader<'_>, w: &mut Vec<u8>) -> Result<()> {
    let id = r.i16()?;
    if id < 0 {
        w.put_i16(-1);
        return Ok(());
    }
    let count = r.i8()?;
    let damage = r.i16()?;
    let nbt = inventory::read_named_nbt(r)?;
    let (id, damage) = ids::item(id, damage);
    w.put_i16(id);
    w.put_i8(count);
    w.put_i16(damage);
    wire::put_nbt(w, nbt.as_ref());
    Ok(())
}

/// Animals, villagers and horses: their age is an int at 12 (1.8: a byte).
fn ageable(mob: u8) -> bool {
    matches!(mob, 90..=93 | 95 | 96 | 98 | 100 | 120)
}

/// 1.8 entity data as 1.7's: names of mobs at 10 and 11 (1.8 moved them to every entity, at 2
/// and 3), ages as ints, items in 1.7's form, and only what 1.7 has.
fn entity_data(r: &mut Reader<'_>, w: &mut Vec<u8>, shape: Shape) -> Result<()> {
    let living = matches!(shape, Shape::Player | Shape::Mob(_));
    loop {
        let header = r.u8()?;
        if header == 0x7F {
            break;
        }
        let (ty, index) = (header >> 5, header & 0x1F);
        // (type, value bytes) as 1.7 writes them.
        let mut value = Vec::new();
        let mut ty_out = ty;
        match ty {
            0 => value.put_i8(r.i8()?),
            1 => value.put_i16(r.i16()?),
            2 => value.put_i32(r.i32()?),
            3 => value.put_f32(r.f32()?),
            4 => value.put_string(&r.string(32767)?),
            5 => slot(r, &mut value)?,
            6 => value.put_slice(r.take(12)?),
            7 => {
                r.take(12)?;
                continue;
            }
            other => bail!("entity data of type {other}"),
        }
        let index = match (index, ty, shape) {
            (0, 0, _) | (1, 1, _) => index,
            (2, 4, Shape::Mob(_)) => 10,
            (3, 0, Shape::Mob(_)) => 11,
            (6, 3, _) | (7, 2, _) | (8 | 9, 0, _) if living => index,
            (12, 0, Shape::Mob(54 | 57)) => {
                value = vec![(value[0] != 0) as u8];
                12
            }
            (12, 0, Shape::Mob(mob)) if ageable(mob) => {
                let baby = value[0] != 0;
                value.clear();
                value.put_i32(if baby { -1 } else { 0 });
                ty_out = 2;
                12
            }
            (13, 0, Shape::Mob(51 | 54 | 57)) => 13,
            (16, 0, Shape::Mob(50 | 52 | 55 | 56 | 61 | 62 | 65 | 90 | 91 | 99)) => 16,
            (16, 1, Shape::Mob(58)) => {
                let block = i16::from_be_bytes([value[0], value[1]]) as u16;
                value = vec![(ids::block(block << 4) >> 4) as u8];
                ty_out = 0;
                16
            }
            (17 | 18, 0, Shape::Mob(58)) | (17, 0, Shape::Mob(50)) => index,
            (16, 2, Shape::Mob(100 | 120)) | (19, 0, Shape::Mob(100)) => index,
            (10, 5, Shape::Object(2)) | (8, 5, Shape::Object(76)) => index,
            // Item frames keep their item and rotation at 2 and 3.
            (8, 5, Shape::Object(71)) => 2,
            (9, 0, Shape::Object(71)) => 3,
            _ => continue,
        };
        w.put_u8(ty_out << 5 | index);
        w.put_slice(&value);
    }
    w.put_u8(0x7F);
    Ok(())
}

impl Legacy17 {
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

    /// A 1.8 login packet as 1.7's: byte arrays behind short lengths, UUIDs without dashes
    /// before 1.7.6, no compression.
    pub(super) fn login_clientbound(&mut self, payload: &[u8], out: &mut Vec<Vec<u8>>) -> Result<()> {
        let mut r = Reader::new(payload);
        let id = r.varint()?;
        let mut w = packet(id);
        match id {
            login::ENCRYPTION_REQUEST => {
                w.put_string(&r.string(20)?);
                wire::put_array(&mut w, r.byte_array(1 << 16)?);
                wire::put_array(&mut w, r.byte_array(1 << 16)?);
            }
            login::SUCCESS => {
                let uuid = r.string(36)?;
                w.put_string(&if self.newer { uuid } else { uuid.replace('-', "") });
                w.put_string(&r.string(16)?);
            }
            login::COMPRESS => return Ok(()),
            _ => w.put_slice(r.rest()),
        }
        out.push(w);
        Ok(())
    }

    /// A 1.8 play packet as 1.7's.
    pub(super) fn play_clientbound(&mut self, payload: &[u8], out: &mut Vec<Vec<u8>>) -> Result<()> {
        let mut r = Reader::new(payload);
        let id = r.varint()?;
        let mut w = packet(id);
        match id {
            cb::KEEP_ALIVE => w.put_i32(r.varint()?),
            cb::LOGIN => {
                w.put_i32(r.i32()?);
                w.put_u8(game_mode(r.u8()?));
                // Dimension, difficulty, player count, level type; not reduced debug info.
                w.put_slice(r.take(3)?);
                w.put_string(&r.string(16)?);
            }
            cb::CHAT => {
                let json = r.string(1 << 18)?;
                // Above the hotbar: 1.7 has no such place.
                if r.i8()? == 2 {
                    return Ok(());
                }
                w.put_string(&json);
            }
            cb::ENTITY_EQUIPMENT => {
                w.put_i32(r.varint()?);
                w.put_i16(r.i16()?);
                slot(&mut r, &mut w)?;
            }
            cb::SPAWN_POSITION => {
                let pos = from_pos18(r.i64()?);
                w.put_i32(pos.x);
                w.put_i32(pos.y);
                w.put_i32(pos.z);
            }
            cb::UPDATE_HEALTH => {
                w.put_f32(r.f32()?);
                w.put_i16(r.varint()?.clamp(0, 32767) as i16);
                w.put_f32(r.f32()?);
            }
            cb::RESPAWN => {
                w.put_i32(r.i32()?);
                w.put_u8(r.u8()?);
                w.put_u8(game_mode(r.u8()?));
                w.put_string(&r.string(16)?);
            }
            cb::POSITION => {
                // 1.7 has no relative teleports: where the 1.8 translator has the player, with
                // its eyes for the height.
                let (x, y, z) = self.inner.position;
                let (yaw, pitch) = self.inner.rotation;
                w.put_f64(x);
                w.put_f64(y + 1.62);
                w.put_f64(z);
                w.put_f32(yaw);
                w.put_f32(pitch);
                w.put_bool(false);
            }
            cb::NAMED_ENTITY_SPAWN => {
                w.put_varint(r.varint()?);
                let uuid = r.uuid()?;
                let listed = self.players.get(&uuid);
                let text = uuid.hyphenated().to_string();
                w.put_string(&if self.newer { text } else { text.replace('-', "") });
                w.put_string(listed.map_or("Player", |l| l.name.as_str()));
                if self.newer {
                    let properties = listed.map_or(&[][..], |l| l.properties.as_slice());
                    w.put_varint(properties.len() as i32);
                    for (name, value, signature) in properties {
                        w.put_string(name);
                        w.put_string(value);
                        w.put_string(signature.as_deref().unwrap_or(""));
                    }
                }
                // Position and rotation.
                w.put_slice(r.take(14)?);
                let held = r.i16()?;
                w.put_i16(if held > 0 { ids::item(held, 0).0 } else { 0 });
                entity_data(&mut r, &mut w, Shape::Player)?;
            }
            cb::COLLECT => {
                w.put_i32(r.varint()?);
                w.put_i32(r.varint()?);
            }
            cb::SPAWN_ENTITY => {
                w.put_varint(r.varint()?);
                let kind = r.u8()?;
                let (mut x, mut y, mut z) = (r.i32()?, r.i32()?, r.i32()?);
                let (pitch, yaw) = (r.u8()?, r.u8()?);
                let data = r.i32()?;
                let velocity = if data != 0 { (r.i16()?, r.i16()?, r.i16()?) } else { (0, 0, 0) };
                let Some((kind, data)) = ids::object(kind, data) else { return Ok(()) };
                if kind == 71 {
                    // 1.7 places a frame by the block it hangs on.
                    let (dx, dz) = facing_offset(data);
                    x = ((x >> 5) - dx) << 5;
                    y = (y >> 5) << 5;
                    z = ((z >> 5) - dz) << 5;
                }
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
                w.put_varint(r.varint()?);
                let kind = r.u8()?;
                let Some(mob) = ids::mob(kind) else { return Ok(()) };
                w.put_u8(mob);
                w.put_slice(r.take(12 + 3 + 6)?);
                entity_data(&mut r, &mut w, Shape::Mob(kind))?;
            }
            cb::SPAWN_ENTITY_PAINTING => {
                w.put_varint(r.varint()?);
                w.put_string(&r.string(13)?);
                let pos = from_pos18(r.i64()?);
                let facing = r.u8()? as i32;
                let (dx, dz) = facing_offset(facing);
                w.put_i32(pos.x - dx);
                w.put_i32(pos.y);
                w.put_i32(pos.z - dz);
                w.put_i32(facing);
            }
            cb::ENTITY_VELOCITY => {
                w.put_i32(r.varint()?);
                w.put_slice(r.take(6)?);
            }
            cb::ENTITY_DESTROY => {
                let entities = r.list(1 << 16, |r| r.varint())?;
                // At most 127 a packet: 1.7 reads the count as a signed byte.
                for some in entities.chunks(127) {
                    let mut w = packet(id);
                    w.put_u8(some.len() as u8);
                    for entity in some {
                        w.put_i32(*entity);
                    }
                    out.push(w);
                }
                return Ok(());
            }
            cb::REL_ENTITY_MOVE => {
                w.put_i32(r.varint()?);
                w.put_slice(r.take(3)?);
            }
            cb::ENTITY_LOOK => {
                w.put_i32(r.varint()?);
                w.put_slice(r.take(2)?);
            }
            cb::ENTITY_MOVE_LOOK => {
                w.put_i32(r.varint()?);
                w.put_slice(r.take(5)?);
            }
            cb::ENTITY_TELEPORT => {
                w.put_i32(r.varint()?);
                w.put_slice(r.take(14)?);
            }
            cb::ENTITY_HEAD_ROTATION => {
                w.put_i32(r.varint()?);
                w.put_u8(r.u8()?);
            }
            cb::ENTITY_METADATA => {
                let entity = r.varint()?;
                let mut data = Vec::new();
                entity_data(&mut r, &mut data, self.shape(entity))?;
                if data.len() == 1 {
                    return Ok(());
                }
                w.put_i32(entity);
                w.put_slice(&data);
            }
            cb::ENTITY_EFFECT => {
                let entity = r.varint()?;
                let effect = r.i8()?;
                let amplifier = r.i8()?;
                let duration = r.varint()?;
                // The 23 effects up to saturation.
                if !(1..=23).contains(&effect) {
                    return Ok(());
                }
                w.put_i32(entity);
                w.put_i8(effect);
                w.put_i8(amplifier);
                w.put_i16(duration.clamp(0, 32767) as i16);
            }
            cb::REMOVE_ENTITY_EFFECT => {
                w.put_i32(r.varint()?);
                w.put_i8(r.i8()?);
            }
            cb::EXPERIENCE => {
                w.put_f32(r.f32()?);
                w.put_i16(r.varint()?.clamp(0, 32767) as i16);
                w.put_i16(r.varint()?.clamp(0, 32767) as i16);
            }
            cb::MAP_CHUNK => {
                let (x, z) = (r.i32()?, r.i32()?);
                let full = r.bool()?;
                let mask = r.u16()?;
                let data = r.byte_array(1 << 22)?;
                w = chunk_with(cb::MAP_CHUNK as u8, x, z, full, mask, data, self.inner.world.has_sky, ids::block)?;
            }
            cb::MULTI_BLOCK_CHANGE => {
                let (x, z) = (r.i32()?, r.i32()?);
                let count = r.varint()?;
                let mut records = Vec::with_capacity(count.clamp(0, 4096) as usize * 4);
                for _ in 0..count {
                    let (xz, y) = (r.u8()?, r.u8()?);
                    records.put_u16((xz as u16) << 8 | y as u16);
                    records.put_u16(ids::block(r.varint()? as u16));
                }
                w.put_i32(x);
                w.put_i32(z);
                w.put_i16(count as i16);
                w.put_i32(records.len() as i32);
                w.put_slice(&records);
            }
            cb::BLOCK_CHANGE => {
                let pos = from_pos18(r.i64()?);
                let block = ids::block(r.varint()? as u16);
                w.put_i32(pos.x);
                w.put_u8(pos.y as u8);
                w.put_i32(pos.z);
                w.put_varint((block >> 4) as i32);
                w.put_u8((block & 15) as u8);
            }
            cb::BLOCK_ACTION => {
                let pos = from_pos18(r.i64()?);
                w.put_i32(pos.x);
                w.put_i16(pos.y as i16);
                w.put_i32(pos.z);
                w.put_slice(r.take(2)?);
                w.put_varint((ids::block((r.varint()? as u16) << 4) >> 4) as i32);
            }
            cb::BLOCK_BREAK_ANIMATION => {
                w.put_varint(r.varint()?);
                let pos = from_pos18(r.i64()?);
                w.put_i32(pos.x);
                w.put_i32(pos.y);
                w.put_i32(pos.z);
                w.put_i8(r.i8()?);
            }
            cb::WORLD_EVENT => {
                let event = r.i32()?;
                let pos = from_pos18(r.i64()?);
                let mut data = r.i32()?;
                if event == 2001 {
                    let block = ids::block(((data & 0xFFF) as u16) << 4 | ((data >> 12) & 15) as u16);
                    data = (block >> 4) as i32 | ((block & 15) as i32) << 12;
                }
                w.put_i32(event);
                w.put_i32(pos.x);
                w.put_u8(pos.y as u8);
                w.put_i32(pos.z);
                w.put_i32(data);
                w.put_bool(r.bool()?);
            }
            cb::WORLD_PARTICLES => {
                let particle = r.i32()?;
                let _far = r.bool()?;
                let floats = r.take(7 * 4)?;
                let count = r.i32()?;
                let mut data = Vec::new();
                while !r.is_empty() {
                    data.push(r.varint()?);
                }
                let Some(name) = ids::particle(particle, &data) else { return Ok(()) };
                w.put_string(&name);
                w.put_slice(floats);
                w.put_i32(count);
            }
            cb::GAME_STATE_CHANGE => {
                let reason = r.u8()?;
                let value = r.f32()?;
                // The elder guardian's face came in 1.8.
                if reason > 8 {
                    return Ok(());
                }
                w.put_u8(reason);
                w.put_f32(if reason == 3 { game_mode(value as u8) as f32 } else { value });
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
                    "minecraft:dispenser" => 3,
                    "minecraft:enchanting_table" => 4,
                    "minecraft:brewing_stand" => 5,
                    "minecraft:villager" => 6,
                    "minecraft:beacon" => 7,
                    "minecraft:anvil" => 8,
                    "minecraft:hopper" => 9,
                    "minecraft:dropper" => 10,
                    "EntityHorse" => 11,
                    _ => return Ok(()),
                };
                self.windows.insert(window, kind);
                w.put_u8(window);
                w.put_u8(kind);
                w.put_string(&legacy_text(&title).chars().take(32).collect::<String>());
                w.put_u8(slots);
                w.put_bool(true);
                if kind == 11 {
                    w.put_i32(r.i32()?);
                }
            }
            cb::CLOSE_WINDOW => {
                let window = r.u8()?;
                self.windows.remove(&window);
                w.put_u8(window);
            }
            cb::SET_SLOT => {
                w.put_i8(r.i8()?);
                w.put_i16(r.i16()?);
                slot(&mut r, &mut w)?;
            }
            cb::WINDOW_ITEMS => {
                w.put_u8(r.u8()?);
                let slots = r.i16()?;
                w.put_i16(slots);
                for _ in 0..slots {
                    slot(&mut r, &mut w)?;
                }
            }
            cb::CRAFT_PROGRESS_BAR => {
                let window = r.u8()?;
                let property = r.i16()?;
                let value = r.i16()?;
                let property = match self.windows.get(&window) {
                    // Furnaces: 1.7 has the progress first, then the fuel and its total, and no
                    // total cooking time.
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
                w.put_u8(window);
                w.put_i16(property);
                w.put_i16(value);
            }
            cb::UPDATE_SIGN => {
                let pos = from_pos18(r.i64()?);
                w.put_i32(pos.x);
                w.put_i16(pos.y as i16);
                w.put_i32(pos.z);
                for _ in 0..4 {
                    w.put_string(&legacy_text(&r.string(1 << 16)?).chars().take(15).collect::<String>());
                }
            }
            cb::OPEN_SIGN_ENTITY => {
                let pos = from_pos18(r.i64()?);
                w.put_i32(pos.x);
                w.put_i32(pos.y);
                w.put_i32(pos.z);
            }
            cb::PLAYER_INFO => return self.player_list(&mut r, out),
            cb::SCOREBOARD_OBJECTIVE => {
                let name = r.string(16)?;
                let mode = r.i8()?;
                let value = if mode == 1 { String::new() } else { r.string(32)? };
                w.put_string(&name);
                w.put_string(&value);
                w.put_i8(mode);
            }
            cb::SCOREBOARD_SCORE => {
                let name = r.string(40)?;
                let action = r.i8()?;
                let objective = r.string(16)?;
                w.put_string(&name);
                w.put_i8(action);
                if action != 1 {
                    w.put_string(&objective);
                    w.put_i32(r.varint()?);
                }
            }
            cb::SCOREBOARD_TEAM => {
                w.put_string(&r.string(16)?);
                let mode = r.i8()?;
                w.put_i8(mode);
                if mode == 0 || mode == 2 {
                    for _ in 0..3 {
                        w.put_string(&r.string(32)?);
                    }
                    w.put_i8(r.i8()?);
                    // Name tag visibility and colour came in 1.8.
                    r.string(32)?;
                    r.i8()?;
                }
                if mode == 0 || mode == 3 || mode == 4 {
                    let members = r.list(1 << 16, |r| r.string(40))?;
                    w.put_i16(members.len() as i16);
                    for member in members {
                        w.put_string(&member);
                    }
                }
            }
            cb::CUSTOM_PAYLOAD => {
                w.put_string(&r.string(20)?);
                let data = r.rest();
                w.put_i16(data.len() as i16);
                w.put_slice(data);
            }
            // The same in 1.7.
            cb::UPDATE_TIME
            | cb::HELD_ITEM_SLOT
            | cb::ANIMATION
            | cb::SPAWN_ENTITY_EXPERIENCE_ORB
            | cb::ENTITY_STATUS
            | cb::ATTACH_ENTITY
            | cb::EXPLOSION
            | cb::NAMED_SOUND_EFFECT
            | cb::SPAWN_ENTITY_WEATHER
            | cb::TRANSACTION
            | cb::ABILITIES
            | cb::TAB_COMPLETE
            | cb::SCOREBOARD_DISPLAY_OBJECTIVE
            | cb::KICK_DISCONNECT => w.put_slice(r.rest()),
            _ => return Ok(()),
        }
        out.push(w);
        Ok(())
    }

    /// 1.8's tab list as 1.7's: a name, online or not, and a ping, one packet each.
    fn player_list(&mut self, r: &mut Reader<'_>, out: &mut Vec<Vec<u8>>) -> Result<()> {
        let action = r.varint()?;
        let count = r.varint()?;
        for _ in 0..count {
            let uuid = r.uuid()?;
            let (name, online, ping) = match action {
                0 => {
                    let name = r.string(16)?;
                    let properties = r.list(64, |r| {
                        let (name, value) = (r.string(32767)?, r.string(32767)?);
                        let signature = if r.bool()? { Some(r.string(32767)?) } else { None };
                        Ok((name, value, signature))
                    })?;
                    r.varint()?;
                    let ping = r.varint()?;
                    if r.bool()? {
                        r.string(1 << 18)?;
                    }
                    self.players.insert(uuid, Listed { name: name.clone(), properties });
                    (name, true, ping)
                }
                1 => {
                    r.varint()?;
                    continue;
                }
                2 => {
                    let ping = r.varint()?;
                    let Some(listed) = self.players.get(&uuid) else { continue };
                    (listed.name.clone(), true, ping)
                }
                3 => {
                    if r.bool()? {
                        r.string(1 << 18)?;
                    }
                    continue;
                }
                4 => {
                    let Some(listed) = self.players.remove(&uuid) else { continue };
                    (listed.name, false, 0)
                }
                other => bail!("tab list action {other}"),
            };
            let mut w = packet(cb::PLAYER_INFO);
            w.put_string(&name);
            w.put_bool(online);
            w.put_i16(ping.clamp(0, 32767) as i16);
            out.push(w);
        }
        Ok(())
    }
}
