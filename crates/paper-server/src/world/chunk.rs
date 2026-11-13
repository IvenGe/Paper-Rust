//! Chunk columns: block and biome storage, light arrays, heightmaps and the wire encoding.
//!
//! Section data sits behind `Arc`s so that a chunk can be snapshotted for a worker thread
//! (packet encoding, saving) by cloning pointers; the main thread copies a section only if
//! it modifies it while a snapshot is still alive.

use paper_data::blocks;
use paper_nbt::Compound;
use paper_protocol::codec::WriteExt;
use paper_protocol::packets::play::{ChunkBlockEntity, LevelChunkWithLight};
use paper_protocol::types::LightData;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Coordinates of a chunk column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// The chunk containing the block column `(x, z)`.
    pub fn of_block(x: i32, z: i32) -> Self {
        Self { x: x >> 4, z: z >> 4 }
    }

    /// Chebyshev distance in chunks.
    pub fn distance(self, other: ChunkPos) -> i32 {
        (self.x - other.x).abs().max((self.z - other.z).abs())
    }
}

#[inline]
fn index(x: usize, y: usize, z: usize) -> usize {
    (y << 8) | (z << 4) | x
}

/// Block states of one 16x16x16 section.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockStorage {
    /// Every block is the same state.
    Uniform(u16),
    /// One state id per block, indexed `y << 8 | z << 4 | x`.
    Dense(Box<[u16; 4096]>),
}

/// Biomes of one section at 4x4x4 resolution.
#[derive(Clone, Debug, PartialEq)]
pub enum BiomeStorage {
    Uniform(u8),
    /// Indexed `y << 4 | z << 2 | x` in biome cells.
    Dense(Box<[u8; 64]>),
}

/// A 16x16x16 slice of a chunk.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub blocks: BlockStorage,
    pub biomes: BiomeStorage,
    /// Blocks that are not air.
    pub non_air: u16,
    /// Blocks containing fluid.
    pub fluids: u16,
}

impl Section {
    pub fn empty(biome: u8) -> Self {
        Self { blocks: BlockStorage::Uniform(blocks::AIR as u16), biomes: BiomeStorage::Uniform(biome), non_air: 0, fluids: 0 }
    }

    /// A section filled with one block state.
    pub fn filled(state: u32, biome: u8) -> Self {
        let mut s = Self { blocks: BlockStorage::Uniform(state as u16), biomes: BiomeStorage::Uniform(biome), non_air: 0, fluids: 0 };
        s.recount();
        s
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> u32 {
        match &self.blocks {
            BlockStorage::Uniform(v) => *v as u32,
            BlockStorage::Dense(data) => data[index(x, y, z)] as u32,
        }
    }

    /// Whether any block of the section is one of `states`.
    pub fn contains_any(&self, states: &[u16]) -> bool {
        match &self.blocks {
            BlockStorage::Uniform(v) => states.contains(v),
            BlockStorage::Dense(data) => data.iter().any(|v| states.contains(v)),
        }
    }

    /// Sets a block and returns the previous state.
    pub fn set(&mut self, x: usize, y: usize, z: usize, state: u32) -> u32 {
        let state16 = state as u16;
        let old = match &mut self.blocks {
            BlockStorage::Uniform(v) => {
                if *v == state16 {
                    return state;
                }
                let old = *v;
                let mut data = Box::new([old; 4096]);
                data[index(x, y, z)] = state16;
                self.blocks = BlockStorage::Dense(data);
                old
            }
            BlockStorage::Dense(data) => std::mem::replace(&mut data[index(x, y, z)], state16),
        } as u32;
        if old != state {
            if !blocks::is_air(old) {
                self.non_air -= 1;
            }
            if !blocks::is_air(state) {
                self.non_air += 1;
            }
            if blocks::has_fluid(old) {
                self.fluids -= 1;
            }
            if blocks::has_fluid(state) {
                self.fluids += 1;
            }
        }
        old
    }

    /// Recomputes the block counters from the stored states.
    pub fn recount(&mut self) {
        match &self.blocks {
            BlockStorage::Uniform(v) => {
                let v = *v as u32;
                self.non_air = if blocks::is_air(v) { 0 } else { 4096 };
                self.fluids = if blocks::has_fluid(v) { 4096 } else { 0 };
            }
            BlockStorage::Dense(data) => {
                self.non_air = data.iter().filter(|s| !blocks::is_air(**s as u32)).count() as u16;
                self.fluids = data.iter().filter(|s| blocks::has_fluid(**s as u32)).count() as u16;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.non_air == 0
    }

    /// Collapses a dense section back to the uniform form when every block is equal.
    pub fn compact(&mut self) {
        if let BlockStorage::Dense(data) = &self.blocks {
            let first = data[0];
            if data.iter().all(|v| *v == first) {
                self.blocks = BlockStorage::Uniform(first);
            }
        }
    }

    pub fn biome(&self, x: usize, y: usize, z: usize) -> u8 {
        match &self.biomes {
            BiomeStorage::Uniform(b) => *b,
            BiomeStorage::Dense(data) => data[(y << 4) | (z << 2) | x],
        }
    }
}

/// Light levels (0-15) of one section, 4 bits per block.
#[derive(Clone, Debug, PartialEq)]
pub enum LightSection {
    /// Every block has this level.
    Uniform(u8),
    /// Nibble array indexed like [`BlockStorage::Dense`]; even indices use the low nibble.
    Dense(Box<[u8; 2048]>),
}

impl LightSection {
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> u8 {
        match self {
            LightSection::Uniform(v) => *v,
            LightSection::Dense(data) => {
                let i = index(x, y, z);
                (data[i >> 1] >> ((i & 1) << 2)) & 0x0F
            }
        }
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, level: u8) {
        let level = level & 0x0F;
        if let LightSection::Uniform(v) = self {
            if *v == level {
                return;
            }
            *self = LightSection::Dense(Box::new([*v | (*v << 4); 2048]));
        }
        if let LightSection::Dense(data) = self {
            let i = index(x, y, z);
            let shift = (i & 1) << 2;
            data[i >> 1] = (data[i >> 1] & !(0x0F << shift)) | (level << shift);
        }
    }

    /// Collapses to the uniform form when all nibbles are equal.
    pub fn compact(&mut self) {
        if let LightSection::Dense(data) = self {
            let first = data[0];
            if first & 0x0F == first >> 4 && data.iter().all(|b| *b == first) {
                *self = LightSection::Uniform(first & 0x0F);
            }
        }
    }

    /// The 2048-byte wire/disk form.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            LightSection::Uniform(v) => vec![*v | (*v << 4); 2048],
            LightSection::Dense(data) => data.to_vec(),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<LightSection> {
        let data: [u8; 2048] = bytes.try_into().ok()?;
        let mut s = LightSection::Dense(Box::new(data));
        s.compact();
        Some(s)
    }
}

/// A chunk column.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub pos: ChunkPos,
    /// Lowest block Y of the world.
    pub min_y: i32,
    /// Sections bottom to top.
    pub sections: Vec<Arc<Section>>,
    /// Sky light, one entry per section plus one below and one above the world.
    pub sky_light: Vec<Arc<LightSection>>,
    /// Block light, laid out like `sky_light`.
    pub block_light: Vec<Arc<LightSection>>,
    /// Block entity data keyed by `(y, z, x)` with chunk-local x/z.
    pub block_entities: BTreeMap<(i32, u8, u8), Compound>,
    /// Per column: height above `min_y` of the block above the highest motion-blocking
    /// block (0 when the column has none).
    pub motion_blocking: Box<[i16; 256]>,
    /// Per column: the same for the highest non-air block.
    pub world_surface: Box<[i16; 256]>,
    /// Whether the light arrays are valid.
    pub lit: bool,
    /// Modified since it was last saved.
    pub dirty: bool,
    /// Incremented on every change that affects what clients see.
    pub revision: u64,
    /// Freshly generated (not loaded from disk): creatures are placed once it is ready.
    pub fresh: bool,
    /// Saved entities read with the chunk, waiting for the server to bring them to life.
    pub entity_nbt: Vec<paper_nbt::Compound>,
    /// The chunk's entity file may hold entities (so an empty list must be written to
    /// clear it when they are gone).
    pub entities_saved: bool,
    /// Scheduled block and fluid ticks (block updates; see `world::ticks`).
    pub ticks: super::ticks::ChunkTicks,
}

fn motion_blocking(state: u32) -> bool {
    blocks::flags(state) & (blocks::FLAG_COLLISION | blocks::FLAG_FLUID) != 0
}

impl Chunk {
    /// An all-air chunk for a world spanning `[min_y, min_y + height)`.
    pub fn empty(pos: ChunkPos, min_y: i32, height: i32, biome: u8) -> Self {
        let count = (height / 16) as usize;
        Self {
            pos,
            min_y,
            sections: (0..count).map(|_| Arc::new(Section::empty(biome))).collect(),
            sky_light: (0..count + 2).map(|_| Arc::new(LightSection::Uniform(0))).collect(),
            block_light: (0..count + 2).map(|_| Arc::new(LightSection::Uniform(0))).collect(),
            block_entities: BTreeMap::new(),
            motion_blocking: Box::new([0; 256]),
            world_surface: Box::new([0; 256]),
            lit: false,
            dirty: false,
            revision: 0,
            fresh: false,
            entity_nbt: Vec::new(),
            entities_saved: false,
            ticks: Default::default(),
        }
    }

    pub fn height(&self) -> i32 {
        self.sections.len() as i32 * 16
    }

    pub fn max_y(&self) -> i32 {
        self.min_y + self.height()
    }

    pub fn in_bounds(&self, y: i32) -> bool {
        y >= self.min_y && y < self.max_y()
    }

    /// Block state at chunk-local `x`/`z` and world `y`; air outside the world.
    #[inline]
    pub fn get_block(&self, x: usize, y: i32, z: usize) -> u32 {
        if !self.in_bounds(y) {
            return blocks::AIR;
        }
        let rel = (y - self.min_y) as usize;
        self.sections[rel >> 4].get(x, rel & 15, z)
    }

    /// Sets a block and maintains heightmaps and counters. Returns the previous state, or
    /// `None` if `y` is outside the world. Lighting is the caller's concern.
    pub fn set_block(&mut self, x: usize, y: i32, z: usize, state: u32) -> Option<u32> {
        if !self.in_bounds(y) {
            return None;
        }
        let rel = (y - self.min_y) as usize;
        let section = &mut self.sections[rel >> 4];
        if section.get(x, rel & 15, z) == state {
            return Some(state);
        }
        let old = Arc::make_mut(section).set(x, rel & 15, z, state);
        self.update_heightmaps(x, rel, z, state);
        if blocks::flags(old) & blocks::FLAG_BLOCK_ENTITY != 0 && blocks::block_id_of_state(old) != blocks::block_id_of_state(state) {
            self.block_entities.remove(&(y, z as u8, x as u8));
        }
        // A new block with a block entity gets an empty one (whose type is named like the
        // block: chests, hoppers, furnaces, ...), so it ticks and is sent to clients.
        if blocks::flags(state) & blocks::FLAG_BLOCK_ENTITY != 0 && !self.block_entities.contains_key(&(y, z as u8, x as u8)) {
            let name = blocks::block_of_state(state).name;
            if paper_data::registries::id("minecraft:block_entity_type", name).is_some() {
                self.block_entities.insert((y, z as u8, x as u8), paper_nbt::Compound::new().with("id", paper_nbt::Tag::String(name.to_owned())));
            }
        }
        self.dirty = true;
        self.revision += 1;
        Some(old)
    }

    fn update_heightmaps(&mut self, x: usize, rel: usize, z: usize, state: u32) {
        let col = (z << 4) | x;
        let top = rel as i16 + 1;
        for (map, occupies) in
            [(&mut self.motion_blocking, motion_blocking as fn(u32) -> bool), (&mut self.world_surface, (|s| !blocks::is_air(s)) as fn(u32) -> bool)]
        {
            if occupies(state) {
                if top > map[col] {
                    map[col] = top;
                }
            } else if top == map[col] {
                // The top block of the column was removed: scan down for the next one.
                let mut h = rel;
                loop {
                    if h == 0 {
                        map[col] = 0;
                        break;
                    }
                    h -= 1;
                    if occupies(self.sections[h >> 4].get(x, h & 15, z)) {
                        map[col] = h as i16 + 1;
                        break;
                    }
                }
            }
        }
    }

    /// Recomputes both heightmaps and the section counters from the block data.
    pub fn recompute_derived(&mut self) {
        for section in &mut self.sections {
            let mut copy = (**section).clone();
            copy.recount();
            if copy.non_air != section.non_air || copy.fluids != section.fluids {
                *section = Arc::new(copy);
            }
        }
        for z in 0..16 {
            for x in 0..16 {
                let col = (z << 4) | x;
                self.motion_blocking[col] = 0;
                self.world_surface[col] = 0;
                let mut found_surface = false;
                for rel in (0..self.height() as usize).rev() {
                    let section = &self.sections[rel >> 4];
                    if section.is_empty() {
                        continue;
                    }
                    let state = section.get(x, rel & 15, z);
                    if !found_surface && !blocks::is_air(state) {
                        self.world_surface[col] = rel as i16 + 1;
                        found_surface = true;
                    }
                    if motion_blocking(state) {
                        self.motion_blocking[col] = rel as i16 + 1;
                        break;
                    }
                }
            }
        }
    }

    /// World Y of the first free block above the highest motion-blocking block of a column.
    pub fn top_y(&self, x: usize, z: usize) -> i32 {
        self.min_y + self.motion_blocking[(z << 4) | x] as i32
    }

    /// Sky light at a block; 15 above the world, 0 below it.
    pub fn sky_light_at(&self, x: usize, y: i32, z: usize) -> u8 {
        let rel = y - self.min_y + 16;
        if rel < 0 {
            return 0;
        }
        match self.sky_light.get((rel >> 4) as usize) {
            Some(s) => s.get(x, (rel & 15) as usize, z),
            None => 15,
        }
    }

    pub fn block_light_at(&self, x: usize, y: i32, z: usize) -> u8 {
        let rel = y - self.min_y + 16;
        if rel < 0 {
            return 0;
        }
        self.block_light.get((rel >> 4) as usize).map_or(0, |s| s.get(x, (rel & 15) as usize, z))
    }

    /// A cheap copy for use on another thread.
    pub fn snapshot(&self) -> ChunkSnapshot {
        ChunkSnapshot {
            pos: self.pos,
            min_y: self.min_y,
            revision: self.revision,
            sections: self.sections.clone(),
            sky_light: self.sky_light.clone(),
            block_light: self.block_light.clone(),
            block_entities: self.block_entities.clone(),
            motion_blocking: self.motion_blocking.clone(),
            world_surface: self.world_surface.clone(),
        }
    }
}

/// An immutable view of a chunk at one revision.
#[derive(Clone, Debug)]
pub struct ChunkSnapshot {
    pub pos: ChunkPos,
    pub min_y: i32,
    pub revision: u64,
    pub sections: Vec<Arc<Section>>,
    pub sky_light: Vec<Arc<LightSection>>,
    pub block_light: Vec<Arc<LightSection>>,
    pub block_entities: BTreeMap<(i32, u8, u8), Compound>,
    pub motion_blocking: Box<[i16; 256]>,
    pub world_surface: Box<[i16; 256]>,
}

/// `ceil(log2(n))`, the number of bits needed to index `n` values.
pub fn bits_for(n: u32) -> u8 {
    if n <= 1 {
        0
    } else {
        (32 - (n - 1).leading_zeros()) as u8
    }
}

/// Packs `values` at `bits` per entry; entries never span two longs.
pub fn pack_longs(values: impl ExactSizeIterator<Item = u64>, bits: u8, out: &mut Vec<u8>) {
    debug_assert!((1..=32).contains(&bits));
    let per_long = 64 / bits as usize;
    let mut current = 0u64;
    let mut filled = 0usize;
    let total = values.len();
    for (i, v) in values.enumerate() {
        current |= v << (filled * bits as usize);
        filled += 1;
        if filled == per_long || i + 1 == total {
            out.put_u64(current);
            current = 0;
            filled = 0;
        }
    }
}

/// Number of longs [`pack_longs`] writes for `entries` values.
pub fn packed_len(entries: usize, bits: u8) -> usize {
    let per_long = 64 / bits as usize;
    entries.div_ceil(per_long)
}

/// Reads entry `i` from a packed long array.
pub fn unpack(longs: &[u64], bits: u8, i: usize) -> u64 {
    let per_long = 64 / bits as usize;
    (longs[i / per_long] >> ((i % per_long) * bits as usize)) & ((1u64 << bits) - 1)
}

/// Writes a paletted container.
///
/// `min_indirect_bits`/`max_indirect_bits` are the bounds of the indirect format (4 and 8
/// for blocks, 1 and 3 for biomes); `direct_bits` is used when the palette would not fit.
fn write_container(values: &[u32], min_indirect_bits: u8, max_indirect_bits: u8, direct_bits: u8, out: &mut Vec<u8>) {
    // Build the palette in order of first appearance.
    let mut palette: Vec<u32> = Vec::new();
    let mut indices: Vec<u16> = Vec::with_capacity(values.len());
    let mut last = (u32::MAX, 0u16);
    let mut direct = false;
    for v in values {
        if *v == last.0 {
            indices.push(last.1);
            continue;
        }
        let i = match palette.iter().position(|p| p == v) {
            Some(i) => i,
            None => {
                palette.push(*v);
                if palette.len() > (1usize << max_indirect_bits) {
                    direct = true;
                    break;
                }
                palette.len() - 1
            }
        };
        last = (*v, i as u16);
        indices.push(i as u16);
    }
    if direct {
        out.put_u8(direct_bits);
        pack_longs(values.iter().map(|v| *v as u64), direct_bits, out);
        return;
    }
    if palette.len() == 1 {
        out.put_u8(0);
        out.put_varint(palette[0] as i32);
        return;
    }
    let bits = bits_for(palette.len() as u32).max(min_indirect_bits);
    out.put_u8(bits);
    out.put_varint(palette.len() as i32);
    for p in &palette {
        out.put_varint(*p as i32);
    }
    pack_longs(indices.iter().map(|i| *i as u64), bits, out);
}

/// Appends the wire form of one chunk section.
pub fn write_section(section: &Section, biome_count: u32, out: &mut Vec<u8>) {
    out.put_i16(section.non_air as i16);
    out.put_i16(section.fluids as i16);
    match &section.blocks {
        BlockStorage::Uniform(v) => {
            out.put_u8(0);
            out.put_varint(*v as i32);
        }
        BlockStorage::Dense(data) => {
            let values: Vec<u32> = data.iter().map(|v| *v as u32).collect();
            write_container(&values, 4, 8, bits_for(blocks::STATE_COUNT), out);
        }
    }
    match &section.biomes {
        BiomeStorage::Uniform(b) => {
            out.put_u8(0);
            out.put_varint(*b as i32);
        }
        BiomeStorage::Dense(data) => {
            let values: Vec<u32> = data.iter().map(|v| *v as u32).collect();
            write_container(&values, 1, 3, bits_for(biome_count), out);
        }
    }
}

fn mask_bit(mask: &mut Vec<u64>, i: usize) {
    if mask.len() <= i / 64 {
        mask.resize(i / 64 + 1, 0);
    }
    mask[i / 64] |= 1 << (i % 64);
}

fn light_data(sky: &[Arc<LightSection>], block: &[Arc<LightSection>]) -> LightData {
    let mut data = LightData::default();
    for (i, section) in sky.iter().enumerate() {
        match &**section {
            LightSection::Uniform(0) => mask_bit(&mut data.empty_sky_mask, i),
            other => {
                mask_bit(&mut data.sky_mask, i);
                data.sky_arrays.push(other.to_bytes());
            }
        }
    }
    for (i, section) in block.iter().enumerate() {
        match &**section {
            LightSection::Uniform(0) => mask_bit(&mut data.empty_block_mask, i),
            other => {
                mask_bit(&mut data.block_mask, i);
                data.block_arrays.push(other.to_bytes());
            }
        }
    }
    data
}

impl ChunkSnapshot {
    /// Light data for every section of the chunk.
    pub fn light_data(&self) -> LightData {
        light_data(&self.sky_light, &self.block_light)
    }

    /// Builds the chunk packet. `biome_count` is the size of the synchronised biome
    /// registry (it determines the direct biome palette width).
    pub fn to_packet(&self, biome_count: u32) -> LevelChunkWithLight {
        let mut data = Vec::with_capacity(self.sections.len() * 16);
        for section in &self.sections {
            write_section(section, biome_count, &mut data);
        }
        let height = self.sections.len() as u32 * 16;
        let bits = bits_for(height + 1);
        let pack = |map: &[i16; 256]| -> Vec<i64> {
            let mut raw = Vec::with_capacity(packed_len(256, bits) * 8);
            pack_longs(map.iter().map(|h| *h as u64), bits, &mut raw);
            raw.chunks_exact(8).map(|c| i64::from_be_bytes(c.try_into().unwrap())).collect()
        };
        let block_entities = self
            .block_entities
            .iter()
            .filter_map(|((y, z, x), nbt)| {
                let state = {
                    let rel = (*y - self.min_y) as usize;
                    self.sections.get(rel >> 4)?.get(*x as usize, rel & 15, *z as usize)
                };
                let name = blocks::block_of_state(state).name;
                let ty = nbt
                    .string("id")
                    .and_then(|id| paper_data::registries::id("minecraft:block_entity_type", id))
                    .or_else(|| paper_data::registries::id("minecraft:block_entity_type", name))?;
                // Clients are only sent what they render: container contents stay on the server.
                let mut data = nbt.clone();
                for hidden in ["Items", "LootTable", "LootTableSeed", "RecipesUsed", "lit_time_remaining", "lit_total_time", "cooking_time_spent", "cooking_total_time"] {
                    data.remove(hidden);
                }
                Some(ChunkBlockEntity { packed_xz: (*x << 4) | *z, y: *y as i16, block_entity_type: ty as i32, data: Some(paper_nbt::Tag::Compound(data)) })
            })
            .collect();
        LevelChunkWithLight {
            x: self.pos.x,
            z: self.pos.z,
            // 1 = WORLD_SURFACE, 4 = MOTION_BLOCKING
            heightmaps: vec![(1, pack(&self.world_surface)), (4, pack(&self.motion_blocking))],
            data,
            block_entities,
            light: self.light_data(),
        }
    }
}

/// Decodes the section array of a chunk packet (the inverse of [`write_section`]).
/// Used by the test client and by round-trip tests.
pub fn read_sections(mut data: &[u8], section_count: usize, biome_count: u32) -> Option<Vec<Section>> {
    fn varint(data: &mut &[u8]) -> Option<u32> {
        let mut v = 0u32;
        for i in 0..5 {
            let b = *data.first()?;
            *data = &data[1..];
            v |= ((b & 0x7F) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    fn longs(data: &mut &[u8], n: usize) -> Option<Vec<u64>> {
        if data.len() < n * 8 {
            return None;
        }
        let (head, tail) = data.split_at(n * 8);
        *data = tail;
        Some(head.chunks_exact(8).map(|c| u64::from_be_bytes(c.try_into().unwrap())).collect())
    }
    fn container(data: &mut &[u8], entries: usize, max_indirect: u8, direct_bits: u8) -> Option<Vec<u32>> {
        let bits = *data.first()?;
        *data = &data[1..];
        if bits == 0 {
            let v = varint(data)?;
            return Some(vec![v; entries]);
        }
        if bits <= max_indirect {
            let n = varint(data)? as usize;
            let palette: Vec<u32> = (0..n).map(|_| varint(data)).collect::<Option<_>>()?;
            let packed = longs(data, packed_len(entries, bits))?;
            return (0..entries).map(|i| palette.get(unpack(&packed, bits, i) as usize).copied()).collect();
        }
        let packed = longs(data, packed_len(entries, direct_bits))?;
        Some((0..entries).map(|i| unpack(&packed, direct_bits, i) as u32).collect())
    }

    let mut out = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        if data.len() < 4 {
            return None;
        }
        let non_air = u16::from_be_bytes([data[0], data[1]]);
        let fluids = u16::from_be_bytes([data[2], data[3]]);
        data = &data[4..];
        let states = container(&mut data, 4096, 8, bits_for(blocks::STATE_COUNT))?;
        let biomes = container(&mut data, 64, 3, bits_for(biome_count))?;
        let mut block_data = Box::new([0u16; 4096]);
        for (slot, s) in block_data.iter_mut().zip(&states) {
            *slot = *s as u16;
        }
        let mut biome_data = Box::new([0u8; 64]);
        for (slot, b) in biome_data.iter_mut().zip(&biomes) {
            *slot = *b as u8;
        }
        let mut section = Section { blocks: BlockStorage::Dense(block_data), biomes: BiomeStorage::Dense(biome_data), non_air, fluids };
        section.compact();
        if let BiomeStorage::Dense(d) = &section.biomes {
            if d.iter().all(|b| *b == d[0]) {
                section.biomes = BiomeStorage::Uniform(d[0]);
            }
        }
        out.push(section);
    }
    data.is_empty().then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_matches_documented_example() {
        // "Visual example" from the chunk format documentation: 5 bits per entry.
        let values: [u64; 24] = [1, 2, 2, 3, 4, 4, 5, 6, 6, 4, 8, 0, 7, 4, 3, 13, 15, 16, 9, 14, 10, 12, 0, 2];
        let mut out = Vec::new();
        pack_longs(values.iter().copied(), 5, &mut out);
        assert_eq!(out, [0x00, 0x20, 0x86, 0x31, 0x48, 0x41, 0x88, 0x41, 0x01, 0x01, 0x8A, 0x72, 0x60, 0xF6, 0x8C, 0x87]);
        let longs: Vec<u64> = out.chunks_exact(8).map(|c| u64::from_be_bytes(c.try_into().unwrap())).collect();
        for (i, v) in values.iter().enumerate() {
            assert_eq!(unpack(&longs, 5, i), *v);
        }
        assert_eq!(packed_len(24, 5), 2);
        assert_eq!(packed_len(4096, 4), 256);
        assert_eq!(packed_len(4096, 15), 1024);
        assert_eq!(packed_len(256, 9), 37);
    }

    #[test]
    fn bit_widths() {
        assert_eq!(bits_for(1), 0);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(16), 4);
        assert_eq!(bits_for(17), 5);
        assert_eq!(bits_for(385), 9);
    }

    #[test]
    fn documented_section_example() {
        // "00 00 | 00 00 | 00 00 | 01 02 27 03 CC FF CC FF CC FF CC FF": single-valued air
        // and an indirect biome palette [39, 3] with one long of data.
        let mut biomes = Box::new([0u8; 64]);
        // 0xCCFFCCFFCCFFCCFF at 1 bit per entry, LSB first.
        let long = 0xCCFF_CCFF_CCFF_CCFFu64;
        for (i, slot) in biomes.iter_mut().enumerate() {
            *slot = if (long >> i) & 1 == 0 { 39 } else { 3 };
        }
        // The first entry (bit 0) is 1 -> palette index 1 -> biome 3, so 3 appears first when
        // building the palette; the documented bytes list 39 first, which only swaps indices.
        let section = Section { blocks: BlockStorage::Uniform(0), biomes: BiomeStorage::Dense(biomes), non_air: 0, fluids: 0 };
        let mut out = Vec::new();
        write_section(&section, 65, &mut out);
        assert_eq!(out[..6], [0, 0, 0, 0, 0, 0]);
        assert_eq!(out[6], 1, "one bit per biome entry");
        assert_eq!(out[7], 2, "palette of two");
        assert_eq!(out.len(), 6 + 4 + 8);
        let back = read_sections(&out, 1, 65).unwrap();
        assert_eq!(back[0], section);
    }

    #[test]
    fn light_nibbles() {
        let mut l = LightSection::Uniform(0);
        l.set(1, 2, 3, 15);
        l.set(0, 0, 0, 7);
        assert_eq!(l.get(1, 2, 3), 15);
        assert_eq!(l.get(0, 0, 0), 7);
        assert_eq!(l.get(1, 0, 0), 0);
        let bytes = l.to_bytes();
        assert_eq!(bytes[0], 0x07, "index 0 is the low nibble of byte 0");
        assert_eq!(LightSection::from_bytes(&bytes).unwrap(), l);
        let mut full = LightSection::Dense(Box::new([0xFF; 2048]));
        full.compact();
        assert_eq!(full, LightSection::Uniform(15));
        assert_eq!(LightSection::Uniform(15).to_bytes(), vec![0xFF; 2048]);
    }
}
