//! Entities for 1.8: spawned by kind (mobs, objects, players, paintings, orbs, lightning),
//! moved in 32ths of a block (the translator keeps where each one is, so rounding does not
//! drift), and the entity data 1.8 shows (flags, names, health, sizes, colours, items).

use super::ids::cb;
use super::tables::{self, EntityKind};
use super::{fixed, inventory, text, Legacy18};
use crate::io::Result;
use crate::Outcome;
use anyhow::bail;
use paper_data::packets::play::clientbound as p;
use paper_protocol::codec::{Decode, Reader, WriteExt};
use paper_protocol::packets::play as packets;

/// What the translator keeps of an entity the client knows.
#[derive(Clone, Debug)]
pub struct Tracked {
    /// The 26.3 type name.
    pub name: &'static str,
    pub kind: EntityKind,
    /// Where the server has it.
    pub pos: (f64, f64, f64),
    /// Where the client has it, in 32ths.
    pub sent: (i32, i32, i32),
    pub yaw: u8,
    pub pitch: u8,
    /// Riders (1.8 knows one per vehicle).
    pub passengers: Vec<i32>,
}

fn packet(id: i32) -> Vec<u8> {
    let mut w = Vec::new();
    w.put_varint(id);
    w
}

fn decode<T: Decode>(body: &[u8]) -> Result<T> {
    Ok(T::decode(&mut Reader::new(body))?)
}

/// 1.8 entity data: one header byte (`type << 5 | index`) per value, then the value; 0x7F
/// ends the list.
pub(super) struct Meta(pub Vec<u8>);

impl Meta {
    fn new() -> Meta {
        Meta(Vec::new())
    }
    fn header(&mut self, ty: u8, index: u8) {
        self.0.put_u8(ty << 5 | (index & 0x1F));
    }
    pub fn byte(&mut self, index: u8, v: i8) {
        self.header(0, index);
        self.0.put_i8(v);
    }
    pub fn short(&mut self, index: u8, v: i16) {
        self.header(1, index);
        self.0.put_i16(v);
    }
    pub fn int(&mut self, index: u8, v: i32) {
        self.header(2, index);
        self.0.put_i32(v);
    }
    pub fn float(&mut self, index: u8, v: f32) {
        self.header(3, index);
        self.0.put_f32(v);
    }
    pub fn string(&mut self, index: u8, v: &str) {
        self.header(4, index);
        self.0.put_string(v);
    }
    pub fn slot(&mut self, index: u8, item: &[u8]) {
        self.header(5, index);
        self.0.put_slice(item);
    }
    fn end(mut self) -> Vec<u8> {
        self.0.put_u8(0x7F);
        self.0
    }
}

/// Entity data 1.8 needs at spawn that 26.3 sends as the type: wither skeletons, zombie
/// villagers, elder guardians, horse kinds.
fn spawn_data(name: &str) -> Meta {
    let mut m = Meta::new();
    m.byte(0, 0);
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "wither_skeleton" => m.byte(13, 1),
        "zombie_villager" => m.byte(13, 1),
        "elder_guardian" => m.int(16, 4),
        "donkey" => m.byte(19, 1),
        "mule" => m.byte(19, 2),
        "zombie_horse" => m.byte(19, 3),
        "skeleton_horse" => m.byte(19, 4),
        "slime" | "magma_cube" => m.byte(16, 1),
        _ => {}
    }
    m
}

/// A 26.3 velocity (blocks per tick) as 1.8's shorts (8000ths), clamped as 1.8 does.
fn velocity(v: f64) -> i16 {
    (v.clamp(-3.9, 3.9) * 8000.0) as i16
}

impl Legacy18 {
    pub(super) fn entity_clientbound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        let mut w;
        match id {
            p::ADD_ENTITY => return self.add_entity(body, out),
            p::REMOVE_ENTITIES => {
                let remove: packets::RemoveEntities = decode(body)?;
                w = packet(cb::ENTITY_DESTROY);
                w.put_varint(remove.entity_ids.len() as i32);
                for e in &remove.entity_ids {
                    self.entities.remove(&e.0);
                    w.put_varint(e.0);
                }
            }
            p::MOVE_ENTITY_POS | p::MOVE_ENTITY_POS_ROT | p::MOVE_ENTITY_ROT => {
                let (entity, delta, rotation, on_ground) = match id {
                    p::MOVE_ENTITY_POS => {
                        let m: packets::MoveEntityPos = decode(body)?;
                        (m.entity_id, Some((m.dx, m.dy, m.dz)), None, m.on_ground)
                    }
                    p::MOVE_ENTITY_POS_ROT => {
                        let m: packets::MoveEntityPosRot = decode(body)?;
                        (m.entity_id, Some((m.dx, m.dy, m.dz)), Some((m.yaw, m.pitch)), m.on_ground)
                    }
                    _ => {
                        let m: packets::MoveEntityRot = decode(body)?;
                        (m.entity_id.0, None, Some((m.yaw.0, m.pitch.0)), m.on_ground)
                    }
                };
                let Some(e) = self.entities.get_mut(&entity) else { return Ok(()) };
                if let Some((dx, dy, dz)) = delta {
                    e.pos = (e.pos.0 + dx as f64 / 4096.0, e.pos.1 + dy as f64 / 4096.0, e.pos.2 + dz as f64 / 4096.0);
                }
                if let Some((yaw, pitch)) = rotation {
                    e.yaw = angle(yaw);
                    e.pitch = angle(pitch);
                }
                let target = (fixed(e.pos.0), fixed(e.pos.1), fixed(e.pos.2));
                let d = (target.0 - e.sent.0, target.1 - e.sent.1, target.2 - e.sent.2);
                let small = [d.0, d.1, d.2].iter().all(|v| (-128..128).contains(v));
                if !small {
                    w = teleport(entity, e, on_ground);
                } else if delta.is_some() && d != (0, 0, 0) {
                    w = packet(if rotation.is_some() { cb::ENTITY_MOVE_LOOK } else { cb::REL_ENTITY_MOVE });
                    w.put_varint(entity);
                    w.put_i8(d.0 as i8);
                    w.put_i8(d.1 as i8);
                    w.put_i8(d.2 as i8);
                    if rotation.is_some() {
                        w.put_u8(e.yaw);
                        w.put_u8(e.pitch);
                    }
                    w.put_bool(on_ground);
                } else if rotation.is_some() {
                    w = packet(cb::ENTITY_LOOK);
                    w.put_varint(entity);
                    w.put_u8(e.yaw);
                    w.put_u8(e.pitch);
                    w.put_bool(on_ground);
                } else {
                    return Ok(());
                }
                e.sent = target;
            }
            p::ENTITY_POSITION_SYNC | p::TELEPORT_ENTITY => {
                let (entity, pos, yaw, pitch, on_ground) = if id == p::ENTITY_POSITION_SYNC {
                    let s: packets::EntityPositionSync = decode(body)?;
                    (s.entity_id, (s.x, s.y, s.z), s.yaw, s.pitch, s.on_ground)
                } else {
                    // Entity, position, velocity, rotation, relative flags, on ground.
                    let mut r = Reader::new(body);
                    let entity = r.varint()?;
                    let pos = (r.f64()?, r.f64()?, r.f64()?);
                    r.take(24)?;
                    let (yaw, pitch) = (r.f32()?, r.f32()?);
                    r.i32()?;
                    (entity, pos, yaw, pitch, r.bool()?)
                };
                let Some(e) = self.entities.get_mut(&entity) else { return Ok(()) };
                e.pos = pos;
                e.yaw = angle(yaw);
                e.pitch = angle(pitch);
                w = teleport(entity, e, on_ground);
                e.sent = (fixed(pos.0), fixed(pos.1), fixed(pos.2));
            }
            p::ROTATE_HEAD => {
                let head: packets::RotateHead = decode(body)?;
                w = packet(cb::ENTITY_HEAD_ROTATION);
                w.put_varint(head.entity_id.0);
                w.put_u8(angle(head.head_yaw.0));
            }
            p::SET_ENTITY_MOTION => {
                let motion: packets::SetEntityMotion = decode(body)?;
                w = packet(cb::ENTITY_VELOCITY);
                w.put_varint(motion.entity_id.0);
                w.put_i16(velocity(motion.velocity.0));
                w.put_i16(velocity(motion.velocity.1));
                w.put_i16(velocity(motion.velocity.2));
            }
            p::SET_ENTITY_DATA => {
                let mut r = Reader::new(body);
                let entity = r.varint()?;
                let (name, kind) = if entity == self.player { ("minecraft:player", Some(EntityKind::Player)) } else { self.entities.get(&entity).map_or(("", None), |e| (e.name, Some(e.kind))) };
                let meta = self.entity_data(name, kind, &mut r)?;
                if meta.0.is_empty() {
                    return Ok(());
                }
                w = packet(cb::ENTITY_METADATA);
                w.put_varint(entity);
                w.put_slice(&meta.end());
            }
            p::SET_EQUIPMENT => {
                let mut r = Reader::new(body);
                let entity = r.varint()?;
                loop {
                    let slot = r.u8()?;
                    let item = crate::components::read_slot(&mut r)?;
                    // 26.3: 0 main hand, 1 off hand, 2 feet, 3 legs, 4 chest, 5 head;
                    // 1.8: 0 held, 1 feet, 2 legs, 3 chest, 4 head.
                    let legacy = match slot & 0x7F {
                        0 => Some(0),
                        n @ 2..=5 => Some(n as i16 - 1),
                        _ => None,
                    };
                    if let Some(legacy) = legacy {
                        let mut e = packet(cb::ENTITY_EQUIPMENT);
                        e.put_varint(entity);
                        e.put_i16(legacy);
                        e.put_slice(&inventory::item(self.tables, item.as_ref()));
                        out.to_client.push(e);
                    }
                    if slot & 0x80 == 0 {
                        break;
                    }
                }
                return Ok(());
            }
            p::UPDATE_MOB_EFFECT => {
                let effect: packets::UpdateMobEffect = decode(body)?;
                let Some(legacy) = tables::effect(effect.effect.0) else { return Ok(()) };
                w = packet(cb::ENTITY_EFFECT);
                w.put_varint(effect.entity_id.0);
                w.put_i8(legacy);
                w.put_i8(effect.amplifier.0.clamp(0, 127) as i8);
                w.put_varint(effect.duration.0.clamp(0, 32767));
                w.put_bool(effect.flags & 0x02 == 0);
            }
            p::REMOVE_MOB_EFFECT => {
                let effect: packets::RemoveMobEffect = decode(body)?;
                let Some(legacy) = tables::effect(effect.effect.0) else { return Ok(()) };
                w = packet(cb::REMOVE_ENTITY_EFFECT);
                w.put_varint(effect.entity_id.0);
                w.put_i8(legacy);
            }
            p::ANIMATE => {
                let animate: packets::Animate = decode(body)?;
                let action = match animate.animation {
                    0 => 2,
                    1 => 4,
                    2 => 5,
                    _ => return Ok(()),
                };
                w = packet(cb::ANIMATION);
                w.put_varint(animate.entity_id.0);
                w.put_u8(action);
            }
            p::SWING_ANIMATION => {
                let swing: packets::SwingAnimation = decode(body)?;
                if swing.hand.0 != 0 || swing.animation_type.0 == 0 {
                    return Ok(());
                }
                w = packet(cb::ANIMATION);
                w.put_varint(swing.entity_id.0);
                w.put_u8(0);
            }
            p::HURT_ANIMATION => {
                let hurt: packets::HurtAnimation = decode(body)?;
                w = packet(cb::ENTITY_STATUS);
                w.put_i32(hurt.entity_id.0);
                w.put_i8(2);
            }
            p::ENTITY_EVENT => {
                w = packet(cb::ENTITY_STATUS);
                w.extend_from_slice(body);
            }
            p::TAKE_ITEM_ENTITY => {
                let take: packets::TakeItemEntity = decode(body)?;
                w = packet(cb::COLLECT);
                w.put_varint(take.collected_id.0);
                w.put_varint(take.collector_id.0);
            }
            p::SET_PASSENGERS => {
                let set: packets::SetPassengers = decode(body)?;
                let before = self.entities.get(&set.vehicle).map(|e| e.passengers.clone()).unwrap_or_default();
                for gone in before.iter().filter(|p| !set.passengers.contains(p)) {
                    let mut a = packet(cb::ATTACH_ENTITY);
                    a.put_i32(*gone);
                    a.put_i32(-1);
                    a.put_bool(false);
                    out.to_client.push(a);
                }
                if let Some(rider) = set.passengers.first() {
                    let mut a = packet(cb::ATTACH_ENTITY);
                    a.put_i32(*rider);
                    a.put_i32(set.vehicle);
                    a.put_bool(false);
                    out.to_client.push(a);
                }
                if let Some(e) = self.entities.get_mut(&set.vehicle) {
                    e.passengers = set.passengers;
                }
                return Ok(());
            }
            p::SOUND | p::SOUND_ENTITY => return self.sound(id, body, out),
            p::LEVEL_PARTICLES => return self.particles(body, out),
            p::EXPLODE => {
                let explode: packets::Explode = decode(body)?;
                w = packet(cb::EXPLOSION);
                w.put_f32(explode.center.0 as f32);
                w.put_f32(explode.center.1 as f32);
                w.put_f32(explode.center.2 as f32);
                w.put_f32(explode.radius);
                w.put_i32(0);
                let knockback = explode.knockback.unwrap_or_default();
                w.put_f32(knockback.0 as f32);
                w.put_f32(knockback.1 as f32);
                w.put_f32(knockback.2 as f32);
            }
            _ => return self.inventory_clientbound(id, body, out),
        }
        out.to_client.push(w);
        Ok(())
    }

    fn add_entity(&mut self, body: &[u8], out: &mut Outcome) -> Result<()> {
        let add: packets::AddEntity = decode(body)?;
        let name = paper_data::registries::entries("minecraft:entity_type").and_then(|e| e.get(add.entity_type.0 as usize).copied()).unwrap_or("");
        let Some(kind) = tables::entity(name) else { return Ok(()) };
        let (x, y, z) = (fixed(add.x), fixed(add.y), fixed(add.z));
        let (pitch, yaw, head) = (angle(add.pitch.0), angle(add.yaw.0), angle(add.head_yaw.0));
        let id = add.entity_id.0;
        let mut w;
        match kind {
            EntityKind::Player => {
                w = packet(cb::NAMED_ENTITY_SPAWN);
                w.put_varint(id);
                w.put_uuid(add.uuid);
                w.put_i32(x);
                w.put_i32(y);
                w.put_i32(z);
                w.put_u8(yaw);
                w.put_u8(pitch);
                w.put_i16(0);
                let mut m = Meta::new();
                m.float(6, 20.0);
                w.put_slice(&m.end());
            }
            EntityKind::Mob(mob) => {
                w = packet(cb::SPAWN_ENTITY_LIVING);
                w.put_varint(id);
                w.put_u8(mob);
                w.put_i32(x);
                w.put_i32(y);
                w.put_i32(z);
                w.put_u8(yaw);
                w.put_u8(pitch);
                w.put_u8(head);
                w.put_i16(velocity(add.velocity.0));
                w.put_i16(velocity(add.velocity.1));
                w.put_i16(velocity(add.velocity.2));
                w.put_slice(&spawn_data(name).end());
            }
            EntityKind::Object(object, default_data) => {
                let data = match object {
                    70 => {
                        let block = self.tables.block(add.data.0 as u32) as i32;
                        (block >> 4) | ((block & 15) << 12)
                    }
                    71 => match add.data.0 {
                        2 => 2,
                        3 => 0,
                        4 => 1,
                        _ => 3,
                    },
                    73 => 16384,
                    90 => add.data.0.max(1),
                    _ => default_data,
                };
                w = packet(cb::SPAWN_ENTITY);
                w.put_varint(id);
                w.put_u8(object);
                w.put_i32(x);
                w.put_i32(y);
                w.put_i32(z);
                w.put_u8(pitch);
                w.put_u8(yaw);
                w.put_i32(data);
                if data != 0 {
                    w.put_i16(velocity(add.velocity.0));
                    w.put_i16(velocity(add.velocity.1));
                    w.put_i16(velocity(add.velocity.2));
                }
            }
            EntityKind::Painting => {
                // The motive comes later, with its data; 1.8 needs it now: the first one.
                let direction = match add.data.0 {
                    2 => 2,
                    3 => 0,
                    4 => 1,
                    _ => 3,
                };
                w = packet(cb::SPAWN_ENTITY_PAINTING);
                w.put_varint(id);
                w.put_string("Kebab");
                w.put_i64(super::pos18(paper_protocol::codec::BlockPos::new(add.x.floor() as i32, add.y.floor() as i32, add.z.floor() as i32)));
                w.put_u8(direction);
            }
            EntityKind::ExperienceOrb => {
                w = packet(cb::SPAWN_ENTITY_EXPERIENCE_ORB);
                w.put_varint(id);
                w.put_i32(x);
                w.put_i32(y);
                w.put_i32(z);
                w.put_i16(add.data.0.clamp(1, 32767) as i16);
            }
            EntityKind::Lightning => {
                w = packet(cb::SPAWN_ENTITY_WEATHER);
                w.put_varint(id);
                w.put_i8(1);
                w.put_i32(x);
                w.put_i32(y);
                w.put_i32(z);
            }
        }
        self.entities.insert(id, Tracked { name, kind, pos: (add.x, add.y, add.z), sent: (x, y, z), yaw, pitch, passengers: Vec::new() });
        out.to_client.push(w);
        Ok(())
    }

    /// 26.3 entity data as 1.8's, for what 1.8 shows.
    fn entity_data(&mut self, name: &str, kind: Option<EntityKind>, r: &mut Reader<'_>) -> Result<Meta> {
        let short = name.strip_prefix("minecraft:").unwrap_or(name);
        let living = matches!(kind, Some(EntityKind::Mob(_)) | Some(EntityKind::Player));
        let mut m = Meta::new();
        loop {
            let index = r.u8()?;
            if index == 0xFF {
                break;
            }
            let ty = r.varint()?;
            let value = read_value(r, ty)?;
            match (index, value) {
                (0, Value::Byte(flags)) => m.byte(0, flags & 0x2B),
                (1, Value::Int(air)) => m.short(1, air.clamp(-32768, 32767) as i16),
                (2, Value::Text(name)) => m.string(2, &name.map(|n| text::legacy_string(&n)).unwrap_or_default()),
                (3, Value::Bool(visible)) => m.byte(3, visible as i8),
                (4, Value::Bool(silent)) => m.byte(4, silent as i8),
                (9, Value::Float(health)) if living => m.float(6, health),
                (16, Value::Byte(parts)) if short == "player" => m.byte(10, parts),
                (16, Value::Bool(baby)) if living && short != "player" => m.byte(12, if baby { -1 } else { 0 }),
                (18, Value::Int(size)) if matches!(short, "slime" | "magma_cube" | "sulfur_cube") => m.byte(16, (size + 1).clamp(1, 127) as i8),
                (18, Value::Byte(wool)) if short == "sheep" => m.byte(16, wool),
                (17, Value::Bool(powered)) if short == "creeper" => m.byte(17, powered as i8),
                (16, Value::Int(swell)) if short == "creeper" => m.byte(16, swell.signum() as i8),
                (8, Value::Item(stack)) if short == "item" => m.slot(10, &inventory::item(self.tables, stack.as_ref())),
                (8, Value::Item(stack)) if short == "item_frame" || short == "glow_item_frame" => m.slot(8, &inventory::item(self.tables, stack.as_ref())),
                (9, Value::Int(rotation)) if short == "item_frame" || short == "glow_item_frame" => m.byte(9, (rotation / 2) as i8),
                (16, Value::BlockState(state)) if short == "enderman" => {
                    let block = state.map_or(0, |s| self.tables.block(s));
                    m.short(16, (block >> 4) as i16);
                    m.byte(17, (block & 15) as i8);
                }
                (17, Value::Bool(screaming)) if short == "enderman" => m.byte(18, screaming as i8),
                _ => {}
            }
        }
        Ok(m)
    }

    fn sound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        let mut r = Reader::new(body);
        let holder = r.varint()?;
        let name = if holder == 0 {
            let name = r.string(32767)?;
            r.option(|r| r.f32())?;
            name
        } else {
            match self.tables.sound(holder - 1) {
                Some(name) => name.to_owned(),
                None => return Ok(()),
            }
        };
        r.varint()?;
        let (x, y, z) = if id == p::SOUND {
            (r.i32()?, r.i32()?, r.i32()?)
        } else {
            let entity = r.varint()?;
            let Some(e) = self.entities.get(&entity) else { return Ok(()) };
            ((e.pos.0 * 8.0) as i32, (e.pos.1 * 8.0) as i32, (e.pos.2 * 8.0) as i32)
        };
        let (volume, pitch) = (r.f32()?, r.f32()?);
        let mut w = packet(cb::NAMED_SOUND_EFFECT);
        w.put_string(&name);
        w.put_i32(x);
        w.put_i32(y);
        w.put_i32(z);
        w.put_f32(volume);
        w.put_u8((pitch * 63.0).clamp(0.0, 255.0) as u8);
        out.to_client.push(w);
        Ok(())
    }

    fn particles(&mut self, body: &[u8], out: &mut Outcome) -> Result<()> {
        let mut r = Reader::new(body);
        let (kind, options) = crate::particles::read(&mut r)?;
        let name = paper_data::registries::entries("minecraft:particle_type").and_then(|e| e.get(kind as usize).copied()).unwrap_or("");
        let Some((legacy, data_count)) = tables::particle(name) else { return Ok(()) };
        let (override_limiter, _always) = (r.bool()?, r.bool()?);
        let (x, y, z) = (r.f64()?, r.f64()?, r.f64()?);
        let (ox, oy, oz) = (r.f32()?, r.f32()?, r.f32()?);
        let speed = r.f32()?.max(r.f32()?).max(r.f32()?);
        let count = r.varint()?;
        let mut w = packet(cb::WORLD_PARTICLES);
        w.put_i32(legacy);
        w.put_bool(override_limiter);
        w.put_f32(x as f32);
        w.put_f32(y as f32);
        w.put_f32(z as f32);
        w.put_f32(ox);
        w.put_f32(oy);
        w.put_f32(oz);
        w.put_f32(speed);
        w.put_i32(count);
        match (data_count, &options) {
            (1, crate::particles::Options::BlockState(state)) => {
                let block = self.tables.block(*state);
                w.put_varint((block >> 4) as i32 | ((block & 15) as i32) << 12);
            }
            (2, crate::particles::Options::Item(item)) => {
                let (id, damage) = self.tables.item(*item);
                w.put_varint(id as i32);
                w.put_varint(damage as i32);
            }
            (0, _) => {}
            _ => return Ok(()),
        }
        out.to_client.push(w);
        Ok(())
    }
}

fn teleport(entity: i32, e: &Tracked, on_ground: bool) -> Vec<u8> {
    let mut w = packet(cb::ENTITY_TELEPORT);
    w.put_varint(entity);
    w.put_i32(fixed(e.pos.0));
    w.put_i32(fixed(e.pos.1));
    w.put_i32(fixed(e.pos.2));
    w.put_u8(e.yaw);
    w.put_u8(e.pitch);
    w.put_bool(on_ground);
    w
}

/// An angle in degrees as a 256ths-of-a-turn byte.
fn angle(degrees: f32) -> u8 {
    ((degrees / 360.0 * 256.0).rem_euclid(256.0)) as u8
}

/// A 26.3 entity data value, as far as 1.8 cares.
enum Value {
    Byte(i8),
    Int(i32),
    Float(f32),
    Bool(bool),
    Text(Option<serde_json::Value>),
    Item(Option<paper_protocol::types::ItemStack>),
    BlockState(Option<u32>),
    Other,
}

fn read_value(r: &mut Reader<'_>, ty: i32) -> Result<Value> {
    use crate::io::Rw;
    Ok(match ty {
        0 => Value::Byte(r.i8()?),
        1 | 12 | 20 | 21..=32 | 35..=38 | 42 | 43 => Value::Int(r.varint()?),
        19 => Value::Int(r.varint()? - 1),
        2 => {
            r.varlong()?;
            Value::Other
        }
        3 => Value::Float(r.f32()?),
        4 => {
            r.string(1 << 16)?;
            Value::Other
        }
        5 => Value::Text(Some(r.component()?.0)),
        6 => Value::Text(r.option(|r| r.component())?.map(|c| c.0)),
        7 => Value::Item(crate::components::read_slot(r)?),
        8 => Value::Bool(r.bool()?),
        9 | 39 => {
            r.take(12)?;
            Value::Other
        }
        40 => {
            r.take(16)?;
            Value::Other
        }
        10 => {
            r.i64()?;
            Value::Other
        }
        11 => {
            r.option(|r| r.i64())?;
            Value::Other
        }
        13 => {
            r.option(|r| r.uuid())?;
            Value::Other
        }
        14 => Value::BlockState(Some(r.varint()? as u32)),
        15 => Value::BlockState(match r.varint()? {
            0 => None,
            s => Some(s as u32),
        }),
        16 => {
            crate::particles::read(r)?;
            Value::Other
        }
        17 => {
            for _ in 0..r.varint()? {
                crate::particles::read(r)?;
            }
            Value::Other
        }
        18 => {
            r.varint()?;
            r.varint()?;
            r.varint()?;
            Value::Other
        }
        33 => {
            r.option(|r| Ok((r.string(256)?, r.i64()?)))?;
            Value::Other
        }
        34 | 41 => {
            let mut rw = Rw { r: r.clone(), w: Vec::new() };
            if ty == 34 {
                crate::components::painting_variant(&crate::components::Same, &mut rw)?;
            } else {
                crate::components::resolvable_profile(&mut rw)?;
            }
            *r = rw.r;
            Value::Other
        }
        other => bail!("entity data of serializer {other}"),
    })
}
