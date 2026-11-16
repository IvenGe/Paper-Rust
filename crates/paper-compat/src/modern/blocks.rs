//! Chunks and blocks: block states, biomes and block entity types by their old ids, light
//! masks as long arrays (26.3 sends them as byte arrays), and the block entities 26.1 draws
//! beds with.

use super::{Modern, V26_2};
use crate::io::{Result, Rw};
use crate::Outcome;
use paper_nbt::{Compound, Tag};
use crate::chunk::{ceil_log2, read_palette, write_palette};
use paper_protocol::codec::{BlockPos, Reader, WriteExt};
use std::sync::OnceLock;

/// The bed block entity's id in 26.1 (`minecraft:bed`).
const BED_BLOCK_ENTITY: i32 = 25;

/// Whether a 26.3 block state is a bed.
fn is_bed(state: u32) -> bool {
    static BEDS: OnceLock<Vec<bool>> = OnceLock::new();
    let beds = BEDS.get_or_init(|| (0..paper_data::blocks::STATE_COUNT).map(|s| paper_data::blocks::block_of_state(s).name.ends_with("_bed")).collect());
    beds.get(state as usize).copied().unwrap_or(false)
}

impl Modern {
    fn state(&self, state: u32) -> u32 {
        self.data.mapping("blockstates").old(state as i32).max(0) as u32
    }

    fn biome(&self, biome: u32) -> u32 {
        self.data.registry("minecraft:worldgen/biome").old(biome as i32).max(0) as u32
    }

    fn biome_count(&self) -> usize {
        match self.data.registry("minecraft:worldgen/biome") {
            crate::data::RegistryMap::Filtered { keep, .. } => keep.len(),
            _ => paper_data::synced::entries("minecraft:worldgen/biome").map_or(64, |e| e.len()),
        }
    }

    /// Takes the dimension the player is now in (from login and respawn).
    pub(super) fn enter_dimension(&mut self, dimension_type: i32) {
        let name = paper_data::synced::entries("minecraft:dimension_type").and_then(|e| e.get(dimension_type as usize).copied());
        if let Some(info) = name.and_then(paper_data::synced::dimension_type) {
            self.min_y = info.min_y;
            self.sections = (info.height / 16) as usize;
        }
    }

    /// 26.1 draws beds from a block entity, which 26.2 dropped.
    fn needs_bed_entities(&self) -> bool {
        self.protocol() < V26_2
    }
}

fn bed_entity(old_id: i32, pos: BlockPos) -> Vec<u8> {
    let mut w = Vec::new();
    w.put_varint(old_id);
    w.put_position(pos);
    w.put_varint(BED_BLOCK_ENTITY);
    paper_nbt::write_network_compound(&mut w, &Compound::default());
    w
}

/// Converts a light mask from 26.3's bit set (a byte array) to a long array.
fn light_mask(rw: &mut Rw<'_>) -> Result<()> {
    let longs = rw.r.bitset()?;
    rw.w.put_varint(longs.len() as i32);
    for long in longs {
        rw.w.put_u64(long);
    }
    Ok(())
}

fn light_masks(rw: &mut Rw<'_>) -> Result<()> {
    for _ in 0..4 {
        light_mask(rw)?;
    }
    Ok(())
}

pub(super) fn light_update(old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    rw.varint()?;
    light_masks(&mut rw)?;
    Ok(rw.finish())
}

pub(super) fn chunk(m: &mut Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.i32()?;
    rw.i32()?;
    rw.copy(|r| r.list(16, |r| Ok((r.varint()?, r.list(1 << 12, |r| r.i64())?))))?;
    let data = rw.r.byte_array(1 << 22)?;
    let mut r = Reader::new(data);
    let mut sections = Vec::with_capacity(data.len());
    let (block_bits, old_block_bits) = (ceil_log2(paper_data::blocks::STATE_COUNT as usize), ceil_log2(m.data.size("blockstates")));
    let biome_total = paper_data::synced::entries("minecraft:worldgen/biome").map_or(64, |e| e.len());
    let (biome_bits, old_biome_bits) = (ceil_log2(biome_total), ceil_log2(m.biome_count()));
    let mut beds = Vec::new();
    let mut index = 0usize;
    while !r.is_empty() {
        let non_air = r.i16()?;
        let fluids = r.i16()?;
        let mut blocks = read_palette(&mut r, 4096, 4, 8, block_bits)?;
        let mut biomes = read_palette(&mut r, 64, 1, 3, biome_bits)?;
        if m.needs_bed_entities() && non_air > 0 {
            for i in blocks.find(is_bed) {
                beds.push((i, index));
            }
        }
        blocks.map(|s| m.state(s));
        biomes.map(|b| m.biome(b));
        sections.put_i16(non_air);
        sections.put_i16(fluids);
        write_palette(&mut sections, &blocks, old_block_bits);
        write_palette(&mut sections, &biomes, old_biome_bits);
        index += 1;
    }
    rw.w.put_byte_array(&sections);
    let count = rw.r.len(1 << 20)?;
    let mut entities = Vec::new();
    for _ in 0..count {
        let packed_xz = rw.r.u8()?;
        let y = rw.r.i16()?;
        let kind = rw.r.varint()?;
        let tag = rw.r.nbt()?;
        let Some((kind, tag)) = block_entity(m, kind, tag) else { continue };
        entities.push((packed_xz, y, kind, tag));
    }
    for (i, section) in beds {
        let (bx, by, bz) = (i & 15, i >> 8, (i >> 4) & 15);
        let y = (m.min_y + section as i32 * 16 + by as i32) as i16;
        let packed = ((bx as u8) << 4) | bz as u8;
        if !entities.iter().any(|(p, ey, _, _)| *p == packed && *ey == y) {
            entities.push((packed, y, BED_BLOCK_ENTITY, Some(Tag::Compound(Compound::default()))));
        }
    }
    rw.w.put_varint(entities.len() as i32);
    for (packed, y, kind, tag) in entities {
        rw.w.put_u8(packed);
        rw.w.put_i16(y);
        rw.w.put_varint(kind);
        match tag {
            Some(tag) => paper_nbt::write_network(&mut rw.w, &tag),
            None => paper_nbt::write_network_end(&mut rw.w),
        }
    }
    light_masks(&mut rw)?;
    Ok(rw.finish())
}

/// A block entity's old type id and data, or `None` if the old version does not have the
/// type.
fn block_entity(m: &Modern, kind: i32, tag: Option<Tag>) -> Option<(i32, Option<Tag>)> {
    let name = paper_data::registries::entries("minecraft:block_entity_type").and_then(|e| e.get(kind as usize).copied()).unwrap_or("");
    let old = m.registry_id("minecraft:block_entity_type", kind);
    if old < 0 {
        return None;
    }
    let tag = match (name, tag) {
        ("minecraft:decorated_pot", Some(Tag::Compound(mut c))) => {
            // 26.3: `sherds: {back: {id: ...}, left, right, front}`; before: a list of four
            // item names.
            if let Some(Tag::Compound(sherds)) = c.remove("sherds") {
                let list = ["back", "left", "right", "front"]
                    .iter()
                    .map(|side| Tag::String(sherds.get(side).and_then(Tag::as_compound).and_then(|s| s.get("id")).and_then(Tag::as_str).unwrap_or("minecraft:brick").to_owned()))
                    .collect();
                c.insert("sherds", Tag::List(list));
            }
            Some(Tag::Compound(c))
        }
        (_, tag) => tag,
    };
    Some((old, tag))
}

pub(super) fn block_update(m: &mut Modern, old: i32, body: &[u8], out: &mut Outcome) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    let pos = rw.copy(|r| r.position())?;
    let state = rw.r.varint()? as u32;
    rw.w.put_varint(m.state(state) as i32);
    if m.needs_bed_entities() && is_bed(state) {
        // The block, then its block entity (which the caller sends last).
        let id = m.old_id(paper_data::packets::play::clientbound::BLOCK_ENTITY_DATA);
        out.to_client.push(rw.finish());
        return Ok(bed_entity(id, pos));
    }
    Ok(rw.finish())
}

pub(super) fn section_update(m: &mut Modern, old: i32, body: &[u8], out: &mut Outcome) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    let packed = rw.i64()?;
    let (sx, sy, sz) = ((packed >> 42) as i32, (packed << 44 >> 44) as i32, (packed << 22 >> 42) as i32);
    let count = rw.varint()?;
    let mut beds = Vec::new();
    for _ in 0..count {
        let v = rw.r.varlong()?;
        let state = (v >> 12) as u32;
        if m.needs_bed_entities() && is_bed(state) {
            beds.push(BlockPos::new(sx * 16 + ((v >> 8) & 15) as i32, sy * 16 + (v & 15) as i32, sz * 16 + ((v >> 4) & 15) as i32));
        }
        rw.w.put_varlong(((m.state(state) as i64) << 12) | (v & 0xFFF));
    }
    if !beds.is_empty() {
        let id = m.old_id(paper_data::packets::play::clientbound::BLOCK_ENTITY_DATA);
        // The blocks first, then their block entities.
        let blocks = rw.finish();
        let mut deferred: Vec<Vec<u8>> = beds.into_iter().map(|pos| bed_entity(id, pos)).collect();
        out.to_client.push(blocks);
        let last = deferred.pop();
        out.to_client.extend(deferred);
        return Ok(last.unwrap_or_default());
    }
    Ok(rw.finish())
}

pub(super) fn block_event(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.copy(|r| r.position())?;
    rw.u8()?;
    rw.u8()?;
    rw.map_varint(|block| m.registry_id("minecraft:block", block).max(0))?;
    Ok(rw.finish())
}

pub(super) fn block_entity_data(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.copy(|r| r.position())?;
    let kind = rw.r.varint()?;
    let tag = rw.r.nbt()?;
    let Some((kind, tag)) = block_entity(m, kind, tag) else {
        // An empty update of a type the client has: harmless.
        return Ok(rw.w);
    };
    rw.w.put_varint(kind);
    match tag {
        Some(tag) => paper_nbt::write_network(&mut rw.w, &tag),
        None => paper_nbt::write_network_end(&mut rw.w),
    }
    Ok(rw.w)
}

/// Level events that carry a block state (2001 a block broken, 3008 a block brushed) or a
/// jukebox song (1010).
pub(super) fn level_event(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    let event = rw.i32()?;
    rw.copy(|r| r.position())?;
    let data = rw.r.i32()?;
    let data = match event {
        2001 | 3008 => m.state(data as u32) as i32,
        1010 => m.registry_id("minecraft:jukebox_song", data).max(0),
        _ => data,
    };
    rw.w.put_i32(data);
    Ok(rw.finish())
}

/// 26.3 says which side of the sign with a VarInt (1 front), before with a boolean.
pub(super) fn open_sign_editor(old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.copy(|r| r.position())?;
    let side = rw.r.varint()?;
    rw.w.put_bool(side == 1);
    Ok(rw.finish())
}
