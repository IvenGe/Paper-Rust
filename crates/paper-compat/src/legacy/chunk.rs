//! Chunks for clients from before 1.9: sections of two-byte blocks (`id << 4 | meta`, little
//! endian) between y 0 and 255, their block and sky light, and one biome byte per column.

use super::tables::Tables;
use crate::chunk::read_palette;
use crate::io::Result;
use paper_protocol::codec::{Reader, WriteExt};

/// What the translator knows of the world the player is in.
pub struct World {
    pub min_y: i32,
    pub has_sky: bool,
}

/// The light of one section in a 26.3 light data block.
struct Light {
    sky: Vec<Option<Vec<u8>>>,
    block: Vec<Option<Vec<u8>>>,
}

fn read_light(r: &mut Reader<'_>) -> Result<Light> {
    let masks: Vec<Vec<u64>> = (0..4).map(|_| r.bitset()).collect::<std::result::Result<_, _>>()?;
    let has = |mask: &[u64], i: usize| mask.get(i / 64).is_some_and(|l| l & (1 << (i % 64)) != 0);
    let sky_arrays = r.list(64, |r| Ok(r.byte_array(2048)?.to_vec()))?;
    let block_arrays = r.list(64, |r| Ok(r.byte_array(2048)?.to_vec()))?;
    let spread = |mask: &[u64], arrays: Vec<Vec<u8>>| {
        let mut arrays = arrays.into_iter();
        (0..64).map(|i| if has(mask, i) { arrays.next() } else { None }).collect::<Vec<_>>()
    };
    Ok(Light { sky: spread(&masks[0], sky_arrays), block: spread(&masks[1], block_arrays) })
}

/// A 26.3 `level_chunk_with_light` body as a 1.8 `map_chunk` payload (packet `id`).
pub fn chunk(tables: &Tables, world: &World, id: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(body);
    let (x, z) = (r.i32()?, r.i32()?);
    r.list(16, |r| Ok((r.varint()?, r.list(1 << 12, |r| r.i64())?)))?;
    let data = r.byte_array(1 << 22)?;
    r.list(1 << 20, |r| {
        r.u8()?;
        r.i16()?;
        r.varint()?;
        r.nbt()?;
        Ok(())
    })?;
    let light = read_light(&mut r)?;

    let block_bits = crate::chunk::ceil_log2(paper_data::blocks::STATE_COUNT as usize);
    let biome_total = paper_data::synced::entries("minecraft:worldgen/biome").map_or(64, |e| e.len());
    let biome_bits = crate::chunk::ceil_log2(biome_total);
    let mut sections = Vec::new();
    let mut biomes = [1u8; 256];
    let biome_section = ((63 - world.min_y) / 16) as usize;
    let mut s = Reader::new(data);
    let mut index = 0usize;
    while !s.is_empty() {
        let non_air = s.i16()?;
        s.i16()?;
        let blocks = read_palette(&mut s, 4096, 4, 8, block_bits)?;
        let biome_palette = read_palette(&mut s, 64, 1, 3, biome_bits)?;
        if index == biome_section {
            for (column, biome) in biomes.iter_mut().enumerate() {
                let (bx, bz) = (column & 15, column >> 4);
                *biome = tables.biome(biome_palette.get((bz >> 2) * 4 + (bx >> 2)));
            }
        }
        let bottom = world.min_y + index as i32 * 16;
        if (0..256).contains(&bottom) && non_air > 0 {
            sections.push(((bottom / 16) as usize, index, blocks));
        }
        index += 1;
    }

    let mut mask = 0u16;
    let mut out = Vec::with_capacity(sections.len() * 12_288 + 256);
    for (legacy, _, blocks) in &sections {
        mask |= 1 << legacy;
        for i in 0..4096 {
            out.put_slice(&tables.block(blocks.get(i)).to_le_bytes());
        }
    }
    for (_, world_index, _) in &sections {
        match light.block.get(world_index + 1).and_then(Option::as_ref) {
            Some(nibbles) => out.put_slice(nibbles),
            None => out.put_slice(&[0u8; 2048]),
        }
    }
    if world.has_sky {
        for (_, world_index, _) in &sections {
            match light.sky.get(world_index + 1).and_then(Option::as_ref) {
                Some(nibbles) => out.put_slice(nibbles),
                None => out.put_slice(&[0xFFu8; 2048]),
            }
        }
    }
    out.put_slice(&biomes);

    let mut w = Vec::with_capacity(out.len() + 16);
    w.put_varint(id);
    w.put_i32(x);
    w.put_i32(z);
    w.put_bool(true);
    w.put_u16(mask);
    w.put_byte_array(&out);
    Ok(w)
}

/// Unloads a chunk on a 1.8 client: a full chunk with no sections and no data.
pub fn unload(id: i32, x: i32, z: i32) -> Vec<u8> {
    let mut w = Vec::new();
    w.put_varint(id);
    w.put_i32(x);
    w.put_i32(z);
    w.put_bool(true);
    w.put_u16(0);
    w.put_varint(0);
    w
}


/// The four lines of a sign's front, as 1.8 JSON, from its block entity data.
fn sign_lines(tag: &paper_nbt::Tag) -> Option<[String; 4]> {
    let messages = tag.as_compound()?.get("front_text")?.as_compound()?.get("messages")?.as_list()?;
    let line = |i: usize| messages.get(i).map(|m| super::text::json_1_8(&paper_protocol::text::Component::from_nbt(m).0)).unwrap_or_else(|| "{\"text\":\"\"}".to_owned());
    Some([line(0), line(1), line(2), line(3)])
}

fn is_sign(kind: i32) -> bool {
    let name = paper_data::registries::entries("minecraft:block_entity_type").and_then(|e| e.get(kind as usize).copied()).unwrap_or("");
    name == "minecraft:sign" || name == "minecraft:hanging_sign"
}

fn update_sign(id: i32, pos: paper_protocol::codec::BlockPos, lines: [String; 4]) -> Vec<u8> {
    let mut w = Vec::new();
    w.put_varint(id);
    w.put_i64(super::pos18(pos));
    for line in lines {
        w.put_string(&line);
    }
    w
}

/// 1.8 sends sign text apart from the chunk: an `update_sign` for each sign of a 26.3 chunk.
pub fn signs(_tables: &Tables, id: i32, body: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut r = Reader::new(body);
    let (x, z) = (r.i32()?, r.i32()?);
    r.list(16, |r| Ok((r.varint()?, r.list(1 << 12, |r| r.i64())?)))?;
    r.byte_array(1 << 22)?;
    let entities = r.list(1 << 20, |r| Ok((r.u8()?, r.i16()?, r.varint()?, r.nbt()?)))?;
    Ok(entities
        .into_iter()
        .filter(|(_, y, kind, _)| is_sign(*kind) && (0..256).contains(&(*y as i32)))
        .filter_map(|(xz, y, _, tag)| {
            let lines = sign_lines(tag.as_ref()?)?;
            Some(update_sign(id, paper_protocol::codec::BlockPos::new(x * 16 + (xz >> 4) as i32, y as i32, z * 16 + (xz & 15) as i32), lines))
        })
        .collect())
}

/// A 26.3 `block_entity_data` body as a 1.8 `update_sign`, if it is a sign.
pub fn sign_update(_tables: &Tables, id: i32, body: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut r = Reader::new(body);
    let pos = r.position()?;
    let kind = r.varint()?;
    let tag = r.nbt()?;
    if !is_sign(kind) || !(0..256).contains(&pos.y) {
        return Ok(None);
    }
    Ok(tag.as_ref().and_then(sign_lines).map(|lines| update_sign(id, pos, lines)))
}
