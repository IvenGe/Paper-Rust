//! 1.4.6's wire format: packets with nothing in front (one ends where its layout does),
//! strings as UTF-16, items whose NBT is gzipped, and entity data made of those.
//!
//! The packet layouts are the ones ViaLegacy (GPL-3.0 or later) reads 1.4.6 with.

use super::{ids, Era};
use crate::io::Result;
use anyhow::bail;
use flate2::read::{GzDecoder, GzEncoder, ZlibEncoder};
use flate2::Compression;
use paper_nbt::Tag;
use paper_protocol::codec::{Reader, WriteExt};
use std::io::Read;

/// Why the end of a packet was not found.
enum Stop {
    /// Not all of it is there yet.
    More,
    Bad(String),
}

type Step<T = ()> = std::result::Result<T, Stop>;

/// Walks over a packet's fields to where it ends.
struct Walk<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Walk<'_> {
    fn skip(&mut self, n: usize) -> Step {
        if self.buf.len() - self.pos < n {
            return Err(Stop::More);
        }
        self.pos += n;
        Ok(())
    }

    fn read<const N: usize>(&mut self) -> Step<[u8; N]> {
        let start = self.pos;
        self.skip(N)?;
        Ok(self.buf[start..self.pos].try_into().expect("N bytes"))
    }

    fn short(&mut self) -> Step<i16> {
        self.read::<2>().map(i16::from_be_bytes)
    }

    fn int(&mut self) -> Step<i32> {
        self.read::<4>().map(i32::from_be_bytes)
    }

    /// `len` units of `unit` bytes.
    fn counted(&mut self, len: i32, unit: usize) -> Step {
        if len < 0 {
            return Err(Stop::Bad(format!("a negative length ({len})")));
        }
        self.skip(len as usize * unit)
    }

    fn string(&mut self) -> Step {
        let len = self.short()?;
        self.counted(len as i32, 2)
    }

    /// Bytes behind a short length.
    fn array(&mut self) -> Step {
        let len = self.short()?;
        self.counted(len as i32, 1)
    }

    /// Bytes behind an int length.
    fn int_array(&mut self) -> Step {
        let len = self.int()?;
        self.counted(len, 1)
    }

    fn item(&mut self) -> Step {
        if self.short()? >= 0 {
            self.skip(3)?;
            let len = self.short()?;
            self.counted(len.max(0) as i32, 1)?;
        }
        Ok(())
    }

    fn entity_data(&mut self) -> Step {
        loop {
            let [header] = self.read::<1>()?;
            if header == 0x7F {
                return Ok(());
            }
            match header >> 5 {
                0 => self.skip(1)?,
                1 => self.skip(2)?,
                2 | 3 => self.skip(4)?,
                4 => self.string()?,
                5 => self.item()?,
                6 => self.skip(12)?,
                other => return Err(Stop::Bad(format!("entity data of type {other}"))),
            }
        }
    }
}

fn measure(buf: &[u8], fields: impl FnOnce(&mut Walk<'_>, u8) -> Step) -> std::result::Result<Option<usize>, String> {
    let Some(&id) = buf.first() else { return Ok(None) };
    let mut walk = Walk { buf, pos: 1 };
    match fields(&mut walk, id) {
        Ok(()) => Ok(Some(walk.pos)),
        Err(Stop::More) => Ok(None),
        Err(Stop::Bad(why)) => Err(why),
    }
}

/// The length of the packet a 1.4.6 or 1.5 client sent at the front of `buf`; `None` while
/// not all of it has arrived.
pub fn split_serverbound(buf: &[u8]) -> std::result::Result<Option<usize>, String> {
    serverbound(buf, Era::V1_4)
}

/// The same for a 1.6 client.
pub fn split_serverbound_1_6(buf: &[u8]) -> std::result::Result<Option<usize>, String> {
    serverbound(buf, Era::V1_6)
}

fn serverbound(buf: &[u8], era: Era) -> std::result::Result<Option<usize>, String> {
    measure(buf, |w, id| match id {
        // 1.6: an action's parameter (a horse's jump), keys of riders, speeds as floats.
        0x13 if era == Era::V1_6 => w.skip(9),
        0x1B if era == Era::V1_6 => w.skip(10),
        0xCA if era == Era::V1_6 => w.skip(9),
        0x00 => w.skip(4),
        0x02 => {
            w.skip(1)?;
            w.string()?;
            w.string()?;
            w.skip(4)
        }
        0x03 | 0xCB | 0xFF => w.string(),
        0x07 | 0x0C => w.skip(9),
        0x0A | 0x65 | 0xCD | 0xFE => w.skip(1),
        0x0B => w.skip(33),
        0x0D => w.skip(41),
        0x0E => w.skip(11),
        0x0F => {
            w.skip(10)?;
            w.item()?;
            w.skip(3)
        }
        0x10 | 0x6C => w.skip(2),
        0x12 | 0x13 => w.skip(5),
        0x66 => {
            w.skip(7)?;
            w.item()
        }
        0x6A => w.skip(4),
        0x6B => {
            w.skip(2)?;
            w.item()
        }
        0x82 => {
            w.skip(10)?;
            (0..4).try_for_each(|_| w.string())
        }
        0xCA => w.skip(3),
        0xCC => {
            w.string()?;
            w.skip(4)
        }
        0xFA => {
            w.string()?;
            w.array()
        }
        0xFC => {
            w.array()?;
            w.array()
        }
        other => Err(Stop::Bad(format!("packet {other:#04x} is not one a 1.4.6 client sends"))),
    })
}

/// The length of the packet a server sent a 1.4.6 client at the front of `buf`; `None`
/// while not all of it has arrived.
pub fn split_clientbound(buf: &[u8]) -> std::result::Result<Option<usize>, String> {
    clientbound(buf, Era::V1_4)
}

/// The same for a 1.5 client.
pub fn split_clientbound_1_5(buf: &[u8]) -> std::result::Result<Option<usize>, String> {
    clientbound(buf, Era::V1_5)
}

/// The same for a 1.6 client.
pub fn split_clientbound_1_6(buf: &[u8]) -> std::result::Result<Option<usize>, String> {
    clientbound(buf, Era::V1_6)
}

fn clientbound(buf: &[u8], era: Era) -> std::result::Result<Option<usize>, String> {
    let v1_5 = era >= Era::V1_5;
    let v1_6 = era == Era::V1_6;
    measure(buf, |w, id| match id {
        // Health as a float, leads, speeds as floats (1.6).
        0x08 if v1_6 => w.skip(10),
        0x27 if v1_6 => w.skip(9),
        0xCA if v1_6 => w.skip(9),
        // Windows say whether to show their title (1.5), horses' their horse (1.6).
        0x64 if v1_5 => {
            w.skip(1)?;
            let [kind] = w.read::<1>()?;
            w.string()?;
            w.skip(2)?;
            if kind == 11 {
                w.skip(4)?;
            }
            Ok(())
        }
        // Particles and scoreboards (1.5).
        0x3F if v1_5 => {
            w.string()?;
            w.skip(32)
        }
        0xCE if v1_5 => {
            w.string()?;
            w.string()?;
            w.skip(1)
        }
        0xCF if v1_5 => {
            w.string()?;
            let [action] = w.read::<1>()?;
            if action != 1 {
                w.string()?;
                w.skip(4)?;
            }
            Ok(())
        }
        0xD0 if v1_5 => {
            w.skip(1)?;
            w.string()
        }
        0xD1 if v1_5 => {
            w.string()?;
            let [mode] = w.read::<1>()?;
            if mode == 0 || mode == 2 {
                w.string()?;
                w.string()?;
                w.string()?;
                w.skip(1)?;
            }
            if mode == 0 || mode == 3 || mode == 4 {
                let members = w.short()?;
                (0..members.max(0)).try_for_each(|_| w.string())?;
            }
            Ok(())
        }
        // The sign editor (1.6).
        0x85 if v1_6 => w.skip(13),
        0x00 | 0x1E => w.skip(4),
        0x01 => {
            w.skip(4)?;
            w.string()?;
            w.skip(5)
        }
        0x03 | 0xCB | 0xFF => w.string(),
        0x04 => w.skip(16),
        0x05 => {
            w.skip(6)?;
            w.item()
        }
        0x06 => w.skip(12),
        0x08 | 0x16 | 0x27 | 0x29 | 0x2B => w.skip(8),
        0x09 => {
            w.skip(8)?;
            w.string()
        }
        0x0A | 0x65 => w.skip(1),
        0x0B => w.skip(33),
        0x0C | 0x21 => w.skip(9),
        0x0D => w.skip(41),
        0x10 | 0x46 => w.skip(2),
        0x11 => w.skip(14),
        0x12 | 0x23 | 0x26 | 0x2A | 0x69 | 0xC8 => w.skip(5),
        0x14 => {
            w.skip(4)?;
            w.string()?;
            w.skip(16)?;
            w.entity_data()
        }
        0x17 => {
            w.skip(19)?;
            if w.int()? > 0 {
                w.skip(6)?;
            }
            Ok(())
        }
        0x18 => {
            w.skip(26)?;
            w.entity_data()
        }
        0x19 => {
            w.skip(4)?;
            w.string()?;
            w.skip(16)
        }
        0x1A | 0x22 | 0x3D => w.skip(18),
        0x1C => w.skip(10),
        0x1D => {
            let [n] = w.read::<1>()?;
            w.skip(n as usize * 4)
        }
        0x1F => w.skip(7),
        0x20 => w.skip(6),
        0x28 => {
            w.skip(4)?;
            w.entity_data()
        }
        0x33 => {
            w.skip(13)?;
            w.int_array()
        }
        0x34 => {
            w.skip(10)?;
            w.int_array()
        }
        0x35 => w.skip(12),
        0x36 => w.skip(14),
        0x37 | 0x47 => w.skip(17),
        0x38 => {
            let chunks = w.short()?;
            let size = w.int()?;
            w.skip(1)?;
            w.counted(size, 1)?;
            w.counted(chunks as i32, 12)
        }
        0x3C => {
            w.skip(28)?;
            let records = w.int()?;
            w.counted(records, 3)?;
            w.skip(12)
        }
        0x3E => {
            w.string()?;
            w.skip(17)
        }
        0x64 => {
            w.skip(2)?;
            w.string()?;
            w.skip(1)
        }
        0x67 => {
            w.skip(3)?;
            w.item()
        }
        0x68 => {
            w.skip(1)?;
            let slots = w.short()?;
            (0..slots.max(0)).try_for_each(|_| w.item())
        }
        0x6A => w.skip(4),
        0x6B => {
            w.skip(2)?;
            w.item()
        }
        0x82 => {
            w.skip(10)?;
            (0..4).try_for_each(|_| w.string())
        }
        0x83 => {
            w.skip(4)?;
            let len = u16::from_be_bytes(w.read::<2>()?);
            w.skip(len as usize)
        }
        0x84 => {
            w.skip(11)?;
            w.array()
        }
        0xC9 => {
            w.string()?;
            w.skip(3)
        }
        0xCA => w.skip(3),
        0xFA => {
            w.string()?;
            w.array()
        }
        0xFC => {
            w.array()?;
            w.array()
        }
        0xFD => {
            w.string()?;
            w.array()?;
            w.array()
        }
        other => Err(Stop::Bad(format!("packet {other:#04x} is not one a 1.4.6 server sends"))),
    })
}

/// A string as 1.4.6 reads one: its length in UTF-16 units, then the units. One longer than
/// the client takes at that place (`max`) is cut short.
pub fn put_string(w: &mut Vec<u8>, s: &str, max: usize) {
    let mut units: Vec<u16> = s.encode_utf16().take(max).collect();
    if units.len() == max && units.last().is_some_and(|u| (0xD800..0xDC00).contains(u)) {
        // Not half of a surrogate pair.
        units.pop();
    }
    w.put_u16(units.len() as u16);
    for unit in units {
        w.put_u16(unit);
    }
}

pub fn read_string(r: &mut Reader<'_>, max: usize) -> Result<String> {
    let len = r.i16()?;
    if len < 0 || len as usize > max {
        bail!("a string of {len} characters where at most {max} may be");
    }
    let units = (0..len).map(|_| r.u16()).collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(String::from_utf16_lossy(&units))
}

/// Bytes behind a short length.
pub fn read_array<'a>(r: &mut Reader<'a>) -> Result<&'a [u8]> {
    let len = r.i16()?;
    if len < 0 {
        bail!("an array of {len} bytes");
    }
    Ok(r.take(len as usize)?)
}

pub fn put_array(w: &mut Vec<u8>, bytes: &[u8]) {
    w.put_i16(bytes.len() as i16);
    w.put_slice(bytes);
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 32);
    GzEncoder::new(data, Compression::default()).read_to_end(&mut out).expect("compressing in memory");
    out
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 3 + 32);
    ZlibEncoder::new(data, Compression::new(4)).read_to_end(&mut out).expect("compressing in memory");
    out
}

/// A compound as 1.4.6 sends one: a named root, gzipped, behind its length (-1: none).
pub fn put_nbt(w: &mut Vec<u8>, tag: Option<&Tag>) {
    let Some(Tag::Compound(root)) = tag else {
        w.put_i16(-1);
        return;
    };
    let mut net = Vec::new();
    paper_nbt::write_network_compound(&mut net, root);
    let mut named = Vec::with_capacity(net.len() + 2);
    named.push(10);
    named.put_u16(0);
    named.extend_from_slice(&net[1..]);
    put_array(w, &gzip(&named));
}

/// A 1.8 slot (id, count, damage, named NBT or 0) as one from before Netty: ids `era` has,
/// NBT gzipped.
pub fn slot_to_old(era: Era, r: &mut Reader<'_>, w: &mut Vec<u8>) -> Result<()> {
    let id = r.i16()?;
    if id < 0 {
        w.put_i16(-1);
        return Ok(());
    }
    let count = r.i8()?;
    let damage = r.i16()?;
    let nbt = crate::legacy::inventory::read_named_nbt(r)?;
    let (id, damage) = ids::item(era, id, damage);
    w.put_i16(id);
    w.put_i8(count);
    w.put_i16(damage);
    put_nbt(w, nbt.as_ref());
    Ok(())
}

/// A 1.4.6 slot as 1.8's (1.4.6's ids are 1.8's too).
pub fn slot_to_1_8(r: &mut Reader<'_>, w: &mut Vec<u8>) -> Result<()> {
    let id = r.i16()?;
    if id < 0 {
        w.put_i16(-1);
        return Ok(());
    }
    w.put_i16(id);
    w.put_i8(r.i8()?);
    w.put_i16(r.i16()?);
    let len = r.i16()?;
    if len <= 0 {
        w.put_u8(0);
        return Ok(());
    }
    let mut named = Vec::new();
    GzDecoder::new(r.take(len as usize)?).take(1 << 21).read_to_end(&mut named)?;
    if named.first() != Some(&10) {
        bail!("item NBT that is not a compound");
    }
    w.put_slice(&named);
    Ok(())
}

/// What an entity is to 1.4.6, as far as its data goes (1.8's type numbers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Player,
    Mob(u8),
    Object(u8),
    Other,
}

enum Value {
    Byte(i8),
    Short(i16),
    Int(i32),
    Float(f32),
    Text(String),
    /// A slot, already in 1.4.6's form.
    Item(Vec<u8>),
}

impl Value {
    fn write(&self, w: &mut Vec<u8>, index: u8) {
        let ty = match self {
            Value::Byte(_) => 0,
            Value::Short(_) => 1,
            Value::Int(_) => 2,
            Value::Float(_) => 3,
            Value::Text(_) => 4,
            Value::Item(_) => 5,
        };
        w.put_u8(ty << 5 | index);
        match self {
            Value::Byte(v) => w.put_i8(*v),
            Value::Short(v) => w.put_i16(*v),
            Value::Int(v) => w.put_i32(*v),
            Value::Float(v) => w.put_f32(*v),
            Value::Text(s) => put_string(w, s, 64),
            Value::Item(slot) => w.put_slice(slot),
        }
    }
}

/// Animals and villagers: their age is an int at 12 (1.8: a byte).
fn ageable(mob: u8) -> bool {
    matches!(mob, 90..=93 | 95 | 96 | 98 | 100 | 120)
}

/// 1.8 entity data as `era`'s: only what it has, at its index and of its type (a value of
/// another type where the client expects one would break it). Living things' values moved in
/// 1.6 (health came in, names went from 5 and 6 to 10 and 11); 1.8 gave every entity a name,
/// at 2 and 3.
pub fn entity_data(era: Era, r: &mut Reader<'_>, w: &mut Vec<u8>, shape: Shape) -> Result<()> {
    let living = matches!(shape, Shape::Player | Shape::Mob(_));
    loop {
        let header = r.u8()?;
        if header == 0x7F {
            break;
        }
        let (ty, index) = (header >> 5, header & 0x1F);
        let value = match ty {
            0 => Value::Byte(r.i8()?),
            1 => Value::Short(r.i16()?),
            2 => Value::Int(r.i32()?),
            3 => Value::Float(r.f32()?),
            4 => Value::Text(r.string(32767)?),
            5 => {
                let mut slot = Vec::new();
                slot_to_old(era, r, &mut slot)?;
                Value::Item(slot)
            }
            6 | 7 => {
                r.take(12)?;
                continue;
            }
            other => bail!("entity data of type {other}"),
        };
        let v1_6 = era == Era::V1_6;
        let (index, value) = match (index, value, shape) {
            (0, v @ Value::Byte(_), _) | (1, v @ Value::Short(_), _) => (index, v),
            // Names of mobs (1.5 on).
            (2, v @ Value::Text(_), Shape::Mob(_)) if era >= Era::V1_5 => (if v1_6 { 10 } else { 5 }, v),
            (3, v @ Value::Byte(_), Shape::Mob(_)) if era >= Era::V1_5 => (if v1_6 { 11 } else { 6 }, v),
            // Health (1.6 on); before, bosses showed theirs at 16 (their bar).
            (6, v @ Value::Float(_), _) if living && v1_6 => (6, v),
            (6, Value::Float(health), Shape::Mob(63 | 64)) => (16, Value::Int(health.ceil() as i32)),
            (7, v @ Value::Int(_), _) if living => (if v1_6 { 7 } else { 8 }, v),
            (8 | 9, v @ Value::Byte(_), _) if living && v1_6 => (index, v),
            // Horses (1.6): their flags and kind.
            (16, v @ Value::Int(_), Shape::Mob(100)) | (19, v @ Value::Byte(_), Shape::Mob(100)) if v1_6 => (index, v),
            (12, Value::Byte(baby), Shape::Mob(54 | 57)) => (12, Value::Byte((baby != 0) as i8)),
            (12, Value::Byte(baby), Shape::Mob(mob)) if ageable(mob) => (12, Value::Int(if baby != 0 { -1 } else { 0 })),
            (13, v @ Value::Byte(_), Shape::Mob(51 | 54 | 57)) => (13, v),
            (16, v @ Value::Byte(_), Shape::Mob(50 | 52 | 55 | 56 | 61 | 62 | 65 | 90 | 91 | 99)) => (16, v),
            (16, Value::Short(block), Shape::Mob(58)) => (16, Value::Byte((ids::block(era, (block as u16) << 4) >> 4) as i8)),
            (17 | 18, v @ Value::Byte(_), Shape::Mob(58)) | (17, v @ Value::Byte(_), Shape::Mob(50)) => (index, v),
            (16, v @ Value::Int(_), Shape::Mob(120)) => (16, v),
            (10, v @ Value::Item(_), Shape::Object(2)) | (8, v @ Value::Item(_), Shape::Object(76)) => (index, v),
            // Item frames keep their item and rotation at 2 and 3.
            (8, v @ Value::Item(_), Shape::Object(71)) => (2, v),
            (9, v @ Value::Byte(_), Shape::Object(71)) => (3, v),
            _ => continue,
        };
        value.write(w, index);
    }
    w.put_u8(0x7F);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_utf16_and_cut_where_the_client_stops() {
        let mut w = Vec::new();
        put_string(&mut w, "hé", 16);
        assert_eq!(w, [0, 2, 0, b'h', 0, 0xE9]);
        let mut w = Vec::new();
        put_string(&mut w, "abcdef", 3);
        assert_eq!(read_string(&mut Reader::new(&w), 3).unwrap(), "abc");
        assert!(read_string(&mut Reader::new(&w), 2).is_err());
    }

    #[test]
    fn packets_are_found_by_their_layout() {
        let mut chat = vec![0x03];
        put_string(&mut chat, "hello", 100);
        assert_eq!(split_serverbound(&chat), Ok(Some(chat.len())));
        assert_eq!(split_serverbound(&chat[..chat.len() - 1]), Ok(None));
        let mut two = chat.clone();
        two.extend_from_slice(&[0x0A, 1]);
        assert_eq!(split_serverbound(&two), Ok(Some(chat.len())));
        assert!(split_serverbound(&[0x99]).is_err());
        // A click with an item that has NBT.
        let mut click = vec![0x66, 0, 0, 36, 0, 0, 1, 0];
        click.put_i16(276);
        click.put_i8(1);
        click.put_i16(0);
        put_array(&mut click, &[1, 2, 3]);
        assert_eq!(split_serverbound(&click), Ok(Some(click.len())));
    }

    #[test]
    fn items_round_trip_with_their_nbt() {
        let mut display = paper_nbt::Compound::default();
        display.insert("Name", Tag::String("Sword".into()));
        let mut root = paper_nbt::Compound::default();
        root.insert("display", Tag::Compound(display));
        let mut slot18 = Vec::new();
        slot18.put_i16(276);
        slot18.put_i8(1);
        slot18.put_i16(5);
        let mut net = Vec::new();
        paper_nbt::write_network_compound(&mut net, &root);
        slot18.push(10);
        slot18.put_u16(0);
        slot18.extend_from_slice(&net[1..]);

        let mut slot14 = Vec::new();
        slot_to_old(Era::V1_4, &mut Reader::new(&slot18), &mut slot14).unwrap();
        let mut back = Vec::new();
        slot_to_1_8(&mut Reader::new(&slot14), &mut back).unwrap();
        assert_eq!(back, slot18);
    }
}
