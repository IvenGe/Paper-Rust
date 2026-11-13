//! World persistence in Minecraft's Anvil format.
//!
//! * `<world>/region/r.<x>.<z>.mca` holds 32x32 chunks: a 4 KiB table of sector offsets, a
//!   4 KiB table of timestamps, then every chunk as a length, a compression type and its
//!   compressed NBT, padded to 4 KiB sectors.
//! * `<world>/level.dat` is gzip-compressed NBT with the seed, spawn point and time.
//! * `<world>/playerdata/<uuid>.dat` is gzip-compressed NBT per player.
//!
//! Chunk NBT uses vanilla's layout (`DataVersion`, `xPos`/`yPos`/`zPos`, `Status`, `sections`
//! with paletted `block_states` and `biomes`, light arrays, `Heightmaps`, `block_entities`), so
//! the data this server models can move between it and vanilla or Paper. Data is not upgraded
//! between Minecraft versions.
//!
//! Reads run on worker threads. Writes go through one I/O thread ([`ChunkIo`]); until a chunk
//! is on disk it stays in its world's pending table, which reads consult first, so a chunk
//! that is unloaded and requested again straight away never comes back stale.

use super::chunk::{BiomeStorage, BlockStorage, Chunk, ChunkPos, ChunkSnapshot, GenStatus, LightSection, ProtoData, Section};
use super::ticks::{ticks_from_nbt, ticks_to_nbt, ChunkTicks, SavedTicks, TickKind};
use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{Receiver, Sender};
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};
use flate2::Compression;
use paper_data::blocks;
use paper_nbt::{Compound, Tag};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SECTOR: usize = 4096;
/// Decompressed NBT larger than this is rejected as corrupt.
const MAX_NBT: usize = 64 * 1024 * 1024;
/// Region files kept open per world.
const MAX_OPEN_REGIONS: usize = 64;

pub fn data_version() -> i32 {
    paper_data::DATA_VERSION as i32
}

// --- region files ------------------------------------------------------------------------

/// One `.mca` file.
pub struct RegionFile {
    path: PathBuf,
    file: File,
    offsets: Box<[u32; 1024]>,
    timestamps: Box<[u32; 1024]>,
    /// Which 4 KiB sectors are in use (the first two hold the header).
    used: Vec<bool>,
}

fn region_index(pos: ChunkPos) -> usize {
    ((pos.x & 31) + (pos.z & 31) * 32) as usize
}

impl RegionFile {
    /// Opens a region file; returns `None` if it does not exist and `create` is false.
    pub fn open(path: &Path, create: bool) -> Result<Option<RegionFile>> {
        if !create && !path.exists() {
            return Ok(None);
        }
        let mut file = OpenOptions::new().read(true).write(true).create(create).truncate(false).open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut offsets = Box::new([0u32; 1024]);
        let mut timestamps = Box::new([0u32; 1024]);
        if file.metadata()?.len() < 2 * SECTOR as u64 {
            // New (or cut short before its header was complete): start empty.
            file.set_len(0)?;
            file.write_all(&[0u8; 2 * SECTOR])?;
        } else {
            let mut header = vec![0u8; 2 * SECTOR];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut header)?;
            for i in 0..1024 {
                offsets[i] = u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());
                timestamps[i] = u32::from_be_bytes(header[SECTOR + i * 4..SECTOR + i * 4 + 4].try_into().unwrap());
            }
        }
        let sectors = (file.metadata()?.len() as usize).div_ceil(SECTOR).max(2);
        let mut used = vec![false; sectors];
        used[0] = true;
        used[1] = true;
        for (i, entry) in offsets.iter_mut().enumerate() {
            if *entry == 0 {
                continue;
            }
            let (start, count) = ((*entry >> 8) as usize, (*entry & 0xFF) as usize);
            if start < 2 || count == 0 || start + count > sectors {
                log::warn!("Region file {} has an invalid entry for chunk {}; ignoring it", path.display(), i);
                *entry = 0;
                continue;
            }
            used[start..start + count].iter_mut().for_each(|u| *u = true);
        }
        Ok(Some(RegionFile { path: path.to_owned(), file, offsets, timestamps, used }))
    }

    fn external_path(&self, pos: ChunkPos) -> PathBuf {
        self.path.with_file_name(format!("c.{}.{}.mcc", pos.x, pos.z))
    }

    /// The decompressed NBT of a chunk, or `None` if the region has no data for it.
    pub fn read(&mut self, pos: ChunkPos) -> Result<Option<Vec<u8>>> {
        let entry = self.offsets[region_index(pos)];
        if entry == 0 {
            return Ok(None);
        }
        let (start, count) = ((entry >> 8) as usize, (entry & 0xFF) as usize);
        let mut head = [0u8; 5];
        self.file.seek(SeekFrom::Start((start * SECTOR) as u64))?;
        self.file.read_exact(&mut head)?;
        let length = u32::from_be_bytes(head[..4].try_into().unwrap()) as usize;
        let kind = head[4];
        if length == 0 {
            return Ok(None);
        }
        let compressed = if kind & 0x80 != 0 {
            std::fs::read(self.external_path(pos)).with_context(|| format!("reading the external data of chunk {},{}", pos.x, pos.z))?
        } else {
            if length - 1 > count * SECTOR - 5 {
                bail!("chunk {},{} claims {} bytes but has {} sectors", pos.x, pos.z, length, count);
            }
            let mut data = vec![0u8; length - 1];
            self.file.read_exact(&mut data)?;
            data
        };
        decompress(kind & 0x7F, &compressed).map(Some)
    }

    /// Writes (or replaces) a chunk's NBT, zlib-compressed.
    pub fn write(&mut self, pos: ChunkPos, nbt: &[u8]) -> Result<()> {
        let mut encoder = ZlibEncoder::new(Vec::with_capacity(nbt.len() / 4), Compression::default());
        encoder.write_all(nbt)?;
        let compressed = encoder.finish()?;
        let external = self.external_path(pos);
        let mut payload = Vec::with_capacity(compressed.len() + 5);
        if compressed.len() + 5 > 255 * SECTOR {
            // Too big for the region: the data goes to its own file, as vanilla does.
            write_atomically(&external, &compressed)?;
            payload.extend_from_slice(&1u32.to_be_bytes());
            payload.push(2 | 0x80);
        } else {
            payload.extend_from_slice(&(compressed.len() as u32 + 1).to_be_bytes());
            payload.push(2);
            payload.extend_from_slice(&compressed);
            if external.exists() {
                let _ = std::fs::remove_file(&external);
            }
        }
        let needed = payload.len().div_ceil(SECTOR);
        let index = region_index(pos);

        // Release the old sectors, then take the first run that fits.
        let old = self.offsets[index];
        if old != 0 {
            let (start, count) = ((old >> 8) as usize, (old & 0xFF) as usize);
            self.used[start..start + count].iter_mut().for_each(|u| *u = false);
        }
        let mut start = 2;
        let mut run = 0;
        let mut found = None;
        for (i, used) in self.used.iter().enumerate().skip(2) {
            if *used {
                run = 0;
                start = i + 1;
            } else {
                run += 1;
                if run == needed {
                    found = Some(start);
                    break;
                }
            }
        }
        let start = found.unwrap_or(if run > 0 { start } else { self.used.len() });
        if start + needed > self.used.len() {
            self.used.resize(start + needed, false);
        }
        self.used[start..start + needed].iter_mut().for_each(|u| *u = true);

        payload.resize(needed * SECTOR, 0);
        self.file.seek(SeekFrom::Start((start * SECTOR) as u64))?;
        self.file.write_all(&payload)?;

        self.offsets[index] = ((start as u32) << 8) | needed as u32;
        self.timestamps[index] = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
        self.file.seek(SeekFrom::Start((index * 4) as u64))?;
        self.file.write_all(&self.offsets[index].to_be_bytes())?;
        self.file.seek(SeekFrom::Start((SECTOR + index * 4) as u64))?;
        self.file.write_all(&self.timestamps[index].to_be_bytes())?;
        Ok(())
    }

    pub fn sync(&mut self) -> Result<()> {
        self.file.sync_data()?;
        Ok(())
    }
}

fn decompress(kind: u8, data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() * 4);
    match kind {
        1 => GzDecoder::new(data).take(MAX_NBT as u64 + 1).read_to_end(&mut out)?,
        2 => ZlibDecoder::new(data).take(MAX_NBT as u64 + 1).read_to_end(&mut out)?,
        3 => {
            out.extend_from_slice(data);
            out.len()
        }
        4 => bail!("LZ4-compressed chunks are not supported; set region-file-compression=deflate where the world was saved"),
        other => bail!("unknown chunk compression type {other}"),
    };
    if out.len() > MAX_NBT {
        bail!("chunk data is larger than {MAX_NBT} bytes");
    }
    Ok(out)
}

/// Writes a file through a temporary file and a rename, so a crash leaves the old or the new
/// version, never half of one.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
    {
        let mut file = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

pub fn read_gzip_nbt(path: &Path) -> Result<Option<Compound>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let data = decompress(1, &bytes).with_context(|| format!("decompressing {}", path.display()))?;
    let (_, root) = paper_nbt::read_named(&mut data.as_slice(), MAX_NBT).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(root))
}

pub fn gzip_nbt(root: &Compound) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    paper_nbt::write_named(&mut raw, "", root);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw)?;
    Ok(encoder.finish()?)
}

// --- paletted data -----------------------------------------------------------------------

/// Bits needed to store values `0..n` (0 for `n <= 1`).
fn ceil_log2(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

/// Packs values into longs without letting a value span two longs (the format since 1.16).
fn pack(values: impl ExactSizeIterator<Item = u64>, bits: u32) -> Vec<i64> {
    let per_long = (64 / bits) as usize;
    let mut out = vec![0i64; values.len().div_ceil(per_long)];
    for (i, v) in values.enumerate() {
        out[i / per_long] |= (v << ((i % per_long) as u32 * bits)) as i64;
    }
    out
}

fn unpack(longs: &[i64], bits: u32, count: usize) -> Result<Vec<u32>> {
    let per_long = (64 / bits) as usize;
    if longs.len() != count.div_ceil(per_long) {
        bail!("packed array has {} longs, expected {} for {} values of {} bits", longs.len(), count.div_ceil(per_long), count, bits);
    }
    let mask = (1u64 << bits) - 1;
    Ok((0..count).map(|i| ((longs[i / per_long] as u64 >> ((i % per_long) as u32 * bits)) & mask) as u32).collect())
}

pub fn state_nbt(state: u32) -> Tag {
    let info = blocks::block_of_state(state);
    let mut c = Compound::new().with("Name", Tag::String(info.name.to_owned()));
    let props = blocks::properties_of(state);
    if !props.is_empty() {
        let mut p = Compound::new();
        for (k, v) in props {
            p.insert(k, Tag::String(v.to_owned()));
        }
        c.insert("Properties", Tag::Compound(p));
    }
    Tag::Compound(c)
}

pub fn state_from_nbt(tag: &Tag) -> Option<u32> {
    let c = tag.as_compound()?;
    let name = c.string("Name")?;
    let mut state = blocks::default_state(name)?;
    if let Some(props) = c.compound("Properties") {
        for (k, v) in props.iter() {
            if let Some(s) = v.as_str().and_then(|v| blocks::with_property(state, k, v)) {
                state = s;
            }
        }
    }
    Some(state)
}

fn biome_names() -> &'static [&'static str] {
    paper_data::synced::entries("minecraft:worldgen/biome").unwrap_or(&[])
}

fn block_states_nbt(section: &Section) -> Compound {
    match &section.blocks {
        BlockStorage::Uniform(v) => Compound::new().with("palette", Tag::List(vec![state_nbt(*v as u32)])),
        BlockStorage::Dense(data) => {
            let mut palette: Vec<u16> = Vec::new();
            let mut lookup: HashMap<u16, u64> = HashMap::new();
            let indices: Vec<u64> = data
                .iter()
                .map(|s| {
                    *lookup.entry(*s).or_insert_with(|| {
                        palette.push(*s);
                        (palette.len() - 1) as u64
                    })
                })
                .collect();
            let mut c = Compound::new().with("palette", Tag::List(palette.iter().map(|s| state_nbt(*s as u32)).collect()));
            if palette.len() > 1 {
                c.insert("data", Tag::LongArray(pack(indices.into_iter(), ceil_log2(palette.len()).max(4))));
            }
            c
        }
    }
}

fn biomes_nbt(section: &Section) -> Compound {
    let names = biome_names();
    let name = |id: u8| Tag::String(names.get(id as usize).copied().unwrap_or("minecraft:plains").to_owned());
    match &section.biomes {
        BiomeStorage::Uniform(b) => Compound::new().with("palette", Tag::List(vec![name(*b)])),
        BiomeStorage::Dense(data) => {
            let mut palette: Vec<u8> = Vec::new();
            let indices: Vec<u64> = data
                .iter()
                .map(|b| match palette.iter().position(|p| p == b) {
                    Some(i) => i as u64,
                    None => {
                        palette.push(*b);
                        (palette.len() - 1) as u64
                    }
                })
                .collect();
            let mut c = Compound::new().with("palette", Tag::List(palette.iter().map(|b| name(*b)).collect()));
            if palette.len() > 1 {
                c.insert("data", Tag::LongArray(pack(indices.into_iter(), ceil_log2(palette.len()))));
            }
            c
        }
    }
}

fn read_block_states(c: &Compound, unknown: &mut usize) -> Result<BlockStorage> {
    let palette: Vec<u32> = c
        .list("palette")
        .ok_or_else(|| anyhow!("block_states without a palette"))?
        .iter()
        .map(|t| {
            state_from_nbt(t).unwrap_or_else(|| {
                *unknown += 1;
                blocks::AIR
            })
        })
        .collect();
    match (palette.len(), c.long_array("data")) {
        (0, _) => bail!("empty block palette"),
        (1, _) | (_, None) => Ok(BlockStorage::Uniform(palette[0] as u16)),
        (n, Some(data)) => {
            let indices = unpack(data, ceil_log2(n).max(4), 4096)?;
            let mut out = Box::new([0u16; 4096]);
            for (slot, i) in out.iter_mut().zip(indices) {
                *slot = *palette.get(i as usize).ok_or_else(|| anyhow!("block palette index {i} out of range"))? as u16;
            }
            Ok(BlockStorage::Dense(out))
        }
    }
}

fn read_biomes(c: &Compound, default: u8) -> Result<BiomeStorage> {
    let names = biome_names();
    let palette: Vec<u8> = c
        .list("palette")
        .unwrap_or(&[])
        .iter()
        .map(|t| t.as_str().and_then(|n| names.iter().position(|e| *e == n)).map_or(default, |i| i as u8))
        .collect();
    match (palette.len(), c.long_array("data")) {
        (0, _) => Ok(BiomeStorage::Uniform(default)),
        (1, _) | (_, None) => Ok(BiomeStorage::Uniform(palette[0])),
        (n, Some(data)) => {
            let indices = unpack(data, ceil_log2(n), 64)?;
            let mut out = Box::new([0u8; 64]);
            for (slot, i) in out.iter_mut().zip(indices) {
                *slot = *palette.get(i as usize).ok_or_else(|| anyhow!("biome palette index {i} out of range"))?;
            }
            Ok(BiomeStorage::Dense(out))
        }
    }
}

// --- chunks ------------------------------------------------------------------------------

/// What the I/O thread needs to write a chunk.
pub struct SaveChunk {
    pub snapshot: ChunkSnapshot,
    pub lit: bool,
    pub has_skylight: bool,
    /// Scheduled block and fluid ticks, relative to the time of saving.
    pub ticks: SavedTicks,
    /// `Full`, or how far an unfinished chunk got.
    pub status: GenStatus,
    /// Heightmaps and entities of an unfinished chunk.
    pub proto: Option<Box<ProtoData>>,
}

impl SaveChunk {
    /// A finished chunk.
    pub fn full(chunk: &Chunk, has_skylight: bool, age: i64) -> Self {
        SaveChunk { snapshot: chunk.snapshot(), lit: chunk.lit, has_skylight, ticks: chunk.ticks.save(age), status: GenStatus::Full, proto: None }
    }
}

impl SaveChunk {
    /// The chunk again, for a read that finds it still waiting to be written.
    fn to_chunk(&self) -> Chunk {
        let s = &self.snapshot;
        Chunk {
            pos: s.pos,
            min_y: s.min_y,
            sections: s.sections.clone(),
            sky_light: s.sky_light.clone(),
            block_light: s.block_light.clone(),
            block_entities: s.block_entities.clone(),
            motion_blocking: s.motion_blocking.clone(),
            world_surface: s.world_surface.clone(),
            lit: self.lit,
            dirty: false,
            revision: 0,
            fresh: false,
            entity_nbt: Vec::new(),
            entities_saved: false,
            ticks: ChunkTicks::from_saved(self.ticks.clone()),
            status: self.status,
            proto: self.proto.clone(),
        }
    }
}

/// Vanilla's `Status` of a chunk.
fn status_name(status: GenStatus) -> &'static str {
    match status {
        GenStatus::Terrain => "minecraft:carvers",
        GenStatus::Decorated => "minecraft:features",
        GenStatus::Full => "minecraft:full",
    }
}

/// A saved `Status` as far as this server continues from it; `None` for chunks saved
/// before their terrain was complete (they are generated again).
fn parse_status(name: &str) -> Option<GenStatus> {
    Some(match name.trim_start_matches("minecraft:") {
        "full" | "light" | "spawn" => GenStatus::Full,
        "features" | "initialize_light" => GenStatus::Decorated,
        "carvers" => GenStatus::Terrain,
        _ => return None,
    })
}

/// Worldgen heightmaps (`ProtoChunk` layout: highest y per `x * 16 + z`) in vanilla's
/// packing (first free y above `min_y` per `z * 16 + x`).
fn wg_heightmap_tag(map: &[i16; 256], min_y: i32, bits: u32) -> Tag {
    let values = (0..256).map(|i| {
        let (x, z) = (i & 15, i >> 4);
        (map[x * 16 + z] as i32 + 1 - min_y).max(0) as u64
    });
    Tag::LongArray(pack(values.collect::<Vec<_>>().into_iter(), bits))
}

fn wg_heightmap_from(longs: &[i64], min_y: i32, bits: u32) -> Option<[i16; 256]> {
    let values = unpack(longs, bits, 256).ok()?;
    let mut map = [0i16; 256];
    for (i, v) in values.into_iter().enumerate() {
        let (x, z) = (i & 15, i >> 4);
        map[x * 16 + z] = (v as i32 - 1 + min_y) as i16;
    }
    Some(map)
}

/// A chunk as vanilla stores it.
pub fn chunk_to_nbt(save: &SaveChunk) -> Compound {
    let s = &save.snapshot;
    let min_section = s.min_y >> 4;
    let count = s.sections.len();
    let mut sections = Vec::with_capacity(count + 2);
    for i in 0..count + 2 {
        let mut c = Compound::new().with("Y", Tag::Byte((min_section - 1 + i as i32) as i8));
        if (1..=count).contains(&i) {
            let section = &s.sections[i - 1];
            c.insert("block_states", Tag::Compound(block_states_nbt(section)));
            c.insert("biomes", Tag::Compound(biomes_nbt(section)));
        }
        if save.lit {
            if !matches!(*s.block_light[i], LightSection::Uniform(0)) {
                c.insert("BlockLight", Tag::ByteArray(s.block_light[i].to_bytes()));
            }
            if save.has_skylight {
                c.insert("SkyLight", Tag::ByteArray(s.sky_light[i].to_bytes()));
            }
        }
        if c.len() > 1 {
            sections.push(Tag::Compound(c));
        }
    }
    let height = (count * 16) as u32;
    let heightmap_bits = ceil_log2(height as usize + 1);
    let heightmap = |map: &[i16; 256]| Tag::LongArray(pack(map.iter().map(|h| *h as u64), heightmap_bits));
    let block_entities = s
        .block_entities
        .iter()
        .map(|((y, z, x), data)| {
            let mut c = data.clone();
            c.insert("x", Tag::Int(s.pos.x * 16 + *x as i32));
            c.insert("y", Tag::Int(*y));
            c.insert("z", Tag::Int(s.pos.z * 16 + *z as i32));
            Tag::Compound(c)
        })
        .collect();
    let mut heightmaps = Compound::new().with("MOTION_BLOCKING", heightmap(&s.motion_blocking)).with("WORLD_SURFACE", heightmap(&s.world_surface));
    // Unfinished chunks keep the worldgen heightmaps and their entities, as in vanilla.
    let proto_entities = match &save.proto {
        Some(proto) => {
            if let Some(maps) = &proto.wg_heightmaps {
                heightmaps.insert("WORLD_SURFACE_WG", wg_heightmap_tag(&maps[0], s.min_y, heightmap_bits));
                heightmaps.insert("OCEAN_FLOOR_WG", wg_heightmap_tag(&maps[1], s.min_y, heightmap_bits));
            }
            Some(proto.entities.iter().cloned().map(Tag::Compound).collect::<Vec<_>>())
        }
        None => None,
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let mut out = Compound::new()
        .with("DataVersion", Tag::Int(data_version()))
        .with("xPos", Tag::Int(s.pos.x))
        .with("yPos", Tag::Int(min_section))
        .with("zPos", Tag::Int(s.pos.z))
        .with("Status", Tag::String(status_name(save.status).into()))
        .with("LastUpdate", Tag::Long(now / 50))
        .with("InhabitedTime", Tag::Long(0))
        .with("isLightOn", Tag::Byte(save.lit as i8))
        .with("sections", Tag::List(sections))
        .with("Heightmaps", Tag::Compound(heightmaps))
        .with("block_entities", Tag::List(block_entities))
        .with("block_ticks", ticks_to_nbt(&save.ticks.blocks, TickKind::Block))
        .with("fluid_ticks", ticks_to_nbt(&save.ticks.fluids, TickKind::Fluid))
        .with("structures", Tag::Compound(Compound::new().with("References", Tag::Compound(Compound::new())).with("starts", Tag::Compound(Compound::new()))));
    if let Some(entities) = proto_entities {
        out.insert("entities", Tag::List(entities));
    }
    out
}

/// Reads a chunk saved by this server, vanilla or Paper. Unfinished chunks come back with
/// their `status`; `None` for chunks saved before their terrain was complete (they are
/// generated again).
pub fn chunk_from_nbt(nbt: &Compound, pos: ChunkPos, min_y: i32, height: i32, has_skylight: bool) -> Result<Option<Chunk>> {
    let Some(status) = parse_status(nbt.string("Status").unwrap_or("minecraft:full")) else { return Ok(None) };
    if let (Some(x), Some(z)) = (nbt.i32("xPos"), nbt.i32("zPos")) {
        if (x, z) != (pos.x, pos.z) {
            bail!("chunk {},{} is stored where {},{} belongs", x, z, pos.x, pos.z);
        }
    }
    let plains = biome_names().iter().position(|b| *b == "minecraft:plains").unwrap_or(0) as u8;
    let mut chunk = Chunk::empty(pos, min_y, height, plains);
    let min_section = min_y >> 4;
    let count = chunk.sections.len() as i32;
    let mut unknown = 0usize;
    let mut sky_present = vec![false; chunk.sky_light.len()];
    for tag in nbt.list("sections").unwrap_or(&[]) {
        let Some(c) = tag.as_compound() else { continue };
        let y = c.i32("Y").ok_or_else(|| anyhow!("section without Y"))?;
        let light_index = y - (min_section - 1);
        if !(0..count + 2).contains(&light_index) {
            continue;
        }
        let section_index = y - min_section;
        if (0..count).contains(&section_index) {
            if let Some(states) = c.compound("block_states") {
                let blocks = read_block_states(states, &mut unknown).with_context(|| format!("section {y} of chunk {},{}", pos.x, pos.z))?;
                let biomes = c.compound("biomes").map(|b| read_biomes(b, plains)).transpose()?.unwrap_or(BiomeStorage::Uniform(plains));
                let mut section = Section { blocks, biomes, non_air: 0, fluids: 0 };
                section.recount();
                section.compact();
                chunk.sections[section_index as usize] = Arc::new(section);
            }
        }
        if let Some(light) = c.byte_array("BlockLight").and_then(LightSection::from_bytes) {
            chunk.block_light[light_index as usize] = Arc::new(light);
        }
        if let Some(light) = c.byte_array("SkyLight").and_then(LightSection::from_bytes) {
            chunk.sky_light[light_index as usize] = Arc::new(light);
            sky_present[light_index as usize] = true;
        }
    }
    if unknown > 0 {
        log::warn!("Chunk {},{} has {} block states this version does not know; they were loaded as air", pos.x, pos.z, unknown);
    }
    for tag in nbt.list("block_entities").unwrap_or(&[]) {
        let Some(c) = tag.as_compound() else { continue };
        let (Some(x), Some(y), Some(z)) = (c.i32("x"), c.i32("y"), c.i32("z")) else { continue };
        let mut data = c.clone();
        for key in ["x", "y", "z"] {
            data.remove(key);
        }
        chunk.block_entities.insert((y, (z & 15) as u8, (x & 15) as u8), data);
    }
    let saved_ticks = SavedTicks {
        blocks: ticks_from_nbt(nbt.list("block_ticks").unwrap_or(&[]), TickKind::Block, pos),
        fluids: ticks_from_nbt(nbt.list("fluid_ticks").unwrap_or(&[]), TickKind::Fluid, pos),
    };
    chunk.ticks = ChunkTicks::from_saved(saved_ticks);
    chunk.recompute_derived();
    // Stored light is used only if it is complete; otherwise it is computed again.
    chunk.lit = nbt.bool("isLightOn").unwrap_or(false) && (!has_skylight || sky_present.iter().all(|p| *p));
    if !chunk.lit {
        for light in chunk.sky_light.iter_mut().chain(chunk.block_light.iter_mut()) {
            *light = Arc::new(LightSection::Uniform(0));
        }
    }
    chunk.dirty = false;
    chunk.status = status;
    if status != GenStatus::Full {
        chunk.lit = false;
        let bits = ceil_log2(height as usize + 1);
        let maps = nbt.compound("Heightmaps");
        let wg = |key: &str| maps.and_then(|m| m.long_array(key)).and_then(|l| wg_heightmap_from(l, min_y, bits));
        let wg_heightmaps = match (wg("WORLD_SURFACE_WG"), wg("OCEAN_FLOOR_WG")) {
            (Some(surface), Some(ocean)) => Some(Box::new([surface, ocean])),
            _ => None,
        };
        let entities = nbt.list("entities").unwrap_or(&[]).iter().filter_map(|t| t.as_compound().cloned()).collect();
        chunk.proto = Some(Box::new(ProtoData { wg_heightmaps, entities }));
    }
    Ok(Some(chunk))
}

// --- level.dat -----------------------------------------------------------------------------

/// What `level.dat` records about a world.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelData {
    pub name: String,
    pub seed: i64,
    /// The generator the world was created with: `vanilla`, `flat`, or the built-in
    /// `noise`, `nether` and `end` (see `server::Server::make_generator`).
    pub generator: String,
    pub spawn: (i32, i32, i32),
    pub spawn_yaw: f32,
    pub age: i64,
    pub day_time: i64,
    pub daylight_cycle: bool,
    pub game_type: i32,
    pub hardcore: bool,
}

impl LevelData {
    pub fn to_nbt(&self) -> Compound {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let generator = if self.generator == "flat" {
            Compound::new().with("type", Tag::String("minecraft:flat".into()))
        } else {
            Compound::new().with("type", Tag::String("minecraft:noise".into())).with("settings", Tag::String("minecraft:overworld".into()))
        };
        let dimensions = Compound::new().with(
            "minecraft:overworld",
            Tag::Compound(Compound::new().with("type", Tag::String("minecraft:overworld".into())).with("generator", Tag::Compound(generator))),
        );
        let data = Compound::new()
            .with("DataVersion", Tag::Int(data_version()))
            .with("version", Tag::Int(19133))
            .with(
                "Version",
                Tag::Compound(
                    Compound::new()
                        .with("Id", Tag::Int(data_version()))
                        .with("Name", Tag::String(paper_data::VERSION_NAME.into()))
                        .with("Series", Tag::String("main".into()))
                        .with("Snapshot", Tag::Byte(0)),
                ),
            )
            .with("LevelName", Tag::String(self.name.clone()))
            .with("GameType", Tag::Int(self.game_type))
            .with("hardcore", Tag::Byte(self.hardcore as i8))
            .with("allowCommands", Tag::Byte(1))
            .with("initialized", Tag::Byte(1))
            .with("Time", Tag::Long(self.age))
            .with("DayTime", Tag::Long(self.day_time))
            .with("LastPlayed", Tag::Long(now))
            .with(
                "spawn",
                Tag::Compound(
                    Compound::new()
                        .with("dimension", Tag::String("minecraft:overworld".into()))
                        .with("pos", Tag::IntArray(vec![self.spawn.0, self.spawn.1, self.spawn.2]))
                        .with("yaw", Tag::Float(self.spawn_yaw))
                        .with("pitch", Tag::Float(0.0)),
                ),
            )
            .with("SpawnX", Tag::Int(self.spawn.0))
            .with("SpawnY", Tag::Int(self.spawn.1))
            .with("SpawnZ", Tag::Int(self.spawn.2))
            .with("SpawnAngle", Tag::Float(self.spawn_yaw))
            .with(
                "WorldGenSettings",
                Tag::Compound(
                    Compound::new()
                        .with("seed", Tag::Long(self.seed))
                        .with("generate_features", Tag::Byte(1))
                        .with("bonus_chest", Tag::Byte(0))
                        .with("dimensions", Tag::Compound(dimensions)),
                ),
            )
            .with("GameRules", Tag::Compound(Compound::new().with("doDaylightCycle", Tag::String(self.daylight_cycle.to_string()))))
            .with("DataPacks", Tag::Compound(Compound::new().with("Enabled", Tag::List(vec![Tag::String("vanilla".into())])).with("Disabled", Tag::List(Vec::new()))))
            .with("ServerBrands", Tag::List(vec![Tag::String(crate::net::BRAND.into())]))
            .with("WasModded", Tag::Byte(0))
            .with("raining", Tag::Byte(0))
            .with("thundering", Tag::Byte(0))
            .with("rainTime", Tag::Int(0))
            .with("thunderTime", Tag::Int(0))
            .with("clearWeatherTime", Tag::Int(0))
            .with("paper-rs", Tag::Compound(Compound::new().with("generator", Tag::String(self.generator.clone())).with("daylight_cycle", Tag::Byte(self.daylight_cycle as i8))));
        Compound::new().with("Data", Tag::Compound(data))
    }

    pub fn from_nbt(root: &Compound) -> Result<LevelData> {
        let data = root.compound("Data").ok_or_else(|| anyhow!("level.dat has no Data compound"))?;
        let seed = data.compound("WorldGenSettings").and_then(|w| w.i64("seed")).or_else(|| data.i64("RandomSeed")).unwrap_or(0);
        let ours = data.compound("paper-rs");
        let generator = match ours.and_then(|p| p.string("generator")) {
            Some(g) => g.to_owned(),
            None => {
                let kind = data
                    .compound("WorldGenSettings")
                    .and_then(|w| w.compound("dimensions"))
                    .and_then(|d| d.compound("minecraft:overworld"))
                    .and_then(|o| o.compound("generator"))
                    .and_then(|g| g.string("type"))
                    .unwrap_or("minecraft:noise");
                // A world from vanilla or Paper.
                if kind.ends_with("flat") { "flat" } else { "vanilla" }.to_owned()
            }
        };
        let spawn = match data.compound("spawn").and_then(|s| s.get("pos")).and_then(Tag::as_int_array) {
            Some([x, y, z]) => (*x, *y, *z),
            _ => (data.i32("SpawnX").unwrap_or(0), data.i32("SpawnY").unwrap_or(64), data.i32("SpawnZ").unwrap_or(0)),
        };
        let spawn_yaw = data.compound("spawn").and_then(|s| s.f64("yaw")).or_else(|| data.f64("SpawnAngle")).unwrap_or(0.0) as f32;
        let daylight_cycle = match ours.and_then(|p| p.bool("daylight_cycle")) {
            Some(v) => v,
            None => data.compound("GameRules").and_then(|g| g.string("doDaylightCycle")).is_none_or(|v| v != "false"),
        };
        Ok(LevelData {
            name: data.string("LevelName").unwrap_or("world").to_owned(),
            seed,
            generator,
            spawn,
            spawn_yaw,
            age: data.i64("Time").unwrap_or(0),
            day_time: data.i64("DayTime").unwrap_or(0),
            daylight_cycle,
            game_type: data.i32("GameType").unwrap_or(0),
            hardcore: data.bool("hardcore").unwrap_or(false),
        })
    }
}

// --- the I/O thread ------------------------------------------------------------------------

enum IoJob {
    Chunk(Arc<WorldStorage>, Arc<SaveChunk>),
    /// The entities of a chunk (`entities/r.x.z.mca`).
    Entities(Arc<WorldStorage>, ChunkPos, Arc<Compound>),
    Player(Arc<WorldStorage>, uuid::Uuid, Arc<Compound>),
    File { path: PathBuf, bytes: Vec<u8> },
    Flush(std::sync::mpsc::Sender<()>),
}

/// The thread that writes chunks and other world files, in the order they were queued.
#[derive(Clone)]
pub struct ChunkIo {
    tx: Sender<IoJob>,
}

impl ChunkIo {
    pub fn start() -> ChunkIo {
        let (tx, rx): (Sender<IoJob>, Receiver<IoJob>) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("Chunk I/O".into())
            .spawn(move || {
                for job in rx {
                    match job {
                        IoJob::Chunk(storage, chunk) => {
                            let pos = chunk.snapshot.pos;
                            if let Err(e) = storage.write_chunk(&chunk) {
                                log::error!("Failed to save chunk {},{} of {}: {e:#}", pos.x, pos.z, storage.dir.display());
                            }
                            let mut pending = storage.pending.lock();
                            if pending.get(&pos).is_some_and(|p| Arc::ptr_eq(p, &chunk)) {
                                pending.remove(&pos);
                            }
                        }
                        IoJob::Entities(storage, pos, data) => {
                            if let Err(e) = storage.write_entities(pos, &data) {
                                log::error!("Failed to save the entities of chunk {},{} of {}: {e:#}", pos.x, pos.z, storage.dir.display());
                            }
                            let mut pending = storage.pending_entities.lock();
                            if pending.get(&pos).is_some_and(|p| Arc::ptr_eq(p, &data)) {
                                pending.remove(&pos);
                            }
                        }
                        IoJob::Player(storage, uuid, data) => {
                            let result = gzip_nbt(&data).and_then(|bytes| {
                                let path = storage.player_path(uuid);
                                if let Some(parent) = path.parent() {
                                    std::fs::create_dir_all(parent)?;
                                }
                                write_atomically(&path, &bytes)
                            });
                            if let Err(e) = result {
                                log::error!("Failed to save the player data of {uuid}: {e:#}");
                            }
                            let mut pending = storage.pending_players.lock();
                            if pending.get(&uuid).is_some_and(|p| Arc::ptr_eq(p, &data)) {
                                pending.remove(&uuid);
                            }
                        }
                        IoJob::File { path, bytes } => {
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if let Err(e) = write_atomically(&path, &bytes) {
                                log::error!("Failed to write {}: {e:#}", path.display());
                            }
                        }
                        IoJob::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .expect("starting the chunk I/O thread");
        ChunkIo { tx }
    }

    /// Writes a file in the background.
    pub fn write_file(&self, path: PathBuf, bytes: Vec<u8>) {
        let _ = self.tx.send(IoJob::File { path, bytes });
    }

    /// Waits until everything queued so far is written.
    pub fn flush(&self) {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.tx.send(IoJob::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }
}

// --- one world's files ---------------------------------------------------------------------

/// The files of one world: regions, `level.dat`, player data.
///
/// Like CraftBukkit, every world has its own folder with a `level.dat`; the Nether and the
/// End keep their regions in a dimension subfolder (`world_nether/DIM-1/region`,
/// `world_the_end/DIM1/region`).
pub struct WorldStorage {
    pub dir: PathBuf,
    /// Where regions (and other per-dimension data) live: `dir` or `dir/DIM-1`, `dir/DIM1`.
    pub dimension_dir: PathBuf,
    io: ChunkIo,
    regions: Mutex<HashMap<(i32, i32), Arc<Mutex<RegionFile>>>>,
    entity_regions: Mutex<HashMap<(i32, i32), Arc<Mutex<RegionFile>>>>,
    /// Entity chunks queued for writing.
    pending_entities: Mutex<HashMap<ChunkPos, Arc<Compound>>>,
    /// Chunks queued for writing, by position; the newest version of each.
    pending: Mutex<HashMap<ChunkPos, Arc<SaveChunk>>>,
    /// Player data queued for writing.
    pending_players: Mutex<HashMap<uuid::Uuid, Arc<Compound>>>,
    /// Held open (and locked) while the server uses the world.
    _session_lock: File,
}

impl WorldStorage {
    /// Opens (creating if needed) a world directory and locks it against other servers.
    pub fn open(dir: &Path, io: ChunkIo) -> Result<Arc<WorldStorage>> {
        Self::open_dimension(dir, "", io)
    }

    /// Opens a world whose regions are in the subfolder `dimension` (`DIM-1`, `DIM1`, or
    /// empty for the overworld).
    pub fn open_dimension(dir: &Path, dimension: &str, io: ChunkIo) -> Result<Arc<WorldStorage>> {
        let dimension_dir = if dimension.is_empty() { dir.to_owned() } else { dir.join(dimension) };
        std::fs::create_dir_all(dimension_dir.join("region")).with_context(|| format!("creating {}", dimension_dir.display()))?;
        let lock_path = dir.join("session.lock");
        let mut lock = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lock_path).with_context(|| format!("opening {}", lock_path.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => bail!("{} is in use by another server", dir.display()),
            Err(std::fs::TryLockError::Error(e)) => return Err(e).with_context(|| format!("locking {}", lock_path.display())),
        }
        lock.set_len(0)?;
        lock.write_all("☃".as_bytes())?;
        Ok(Arc::new(WorldStorage {
            dir: dir.to_owned(),
            dimension_dir,
            io,
            regions: Mutex::new(HashMap::new()),
            entity_regions: Mutex::new(HashMap::new()),
            pending_entities: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            pending_players: Mutex::new(HashMap::new()),
            _session_lock: lock,
        }))
    }

    pub fn io(&self) -> &ChunkIo {
        &self.io
    }

    fn region(&self, pos: ChunkPos, create: bool) -> Result<Option<Arc<Mutex<RegionFile>>>> {
        Self::open_region(&self.regions, &self.dimension_dir.join("region"), pos, create)
    }

    fn entity_region(&self, pos: ChunkPos, create: bool) -> Result<Option<Arc<Mutex<RegionFile>>>> {
        Self::open_region(&self.entity_regions, &self.dimension_dir.join("entities"), pos, create)
    }

    fn open_region(regions: &Mutex<HashMap<(i32, i32), Arc<Mutex<RegionFile>>>>, dir: &Path, pos: ChunkPos, create: bool) -> Result<Option<Arc<Mutex<RegionFile>>>> {
        let key = (pos.x >> 5, pos.z >> 5);
        let mut regions = regions.lock();
        if let Some(region) = regions.get(&key) {
            return Ok(Some(region.clone()));
        }
        if create {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let path = dir.join(format!("r.{}.{}.mca", key.0, key.1));
        let Some(file) = RegionFile::open(&path, create)? else { return Ok(None) };
        if regions.len() >= MAX_OPEN_REGIONS {
            // Close an arbitrary region; any one in use stays alive through its Arc.
            if let Some(k) = regions.keys().next().copied() {
                regions.remove(&k);
            }
        }
        let region = Arc::new(Mutex::new(file));
        regions.insert(key, region.clone());
        Ok(Some(region))
    }

    /// Loads a chunk: from the queue of unwritten chunks, else from its region file.
    pub fn load_chunk(&self, pos: ChunkPos, min_y: i32, height: i32, has_skylight: bool) -> Result<Option<Chunk>> {
        if let Some(pending) = self.pending.lock().get(&pos).cloned() {
            return Ok(Some(pending.to_chunk()));
        }
        let Some(region) = self.region(pos, false)? else { return Ok(None) };
        let Some(bytes) = region.lock().read(pos)? else { return Ok(None) };
        let (_, nbt) = paper_nbt::read_named(&mut bytes.as_slice(), MAX_NBT).with_context(|| format!("parsing chunk {},{}", pos.x, pos.z))?;
        chunk_from_nbt(&nbt, pos, min_y, height, has_skylight)
    }

    /// Queues a chunk for writing.
    pub fn save_chunk(self: &Arc<Self>, chunk: SaveChunk) {
        let chunk = Arc::new(chunk);
        self.pending.lock().insert(chunk.snapshot.pos, chunk.clone());
        let _ = self.io.tx.send(IoJob::Chunk(self.clone(), chunk));
    }

    fn write_chunk(&self, chunk: &SaveChunk) -> Result<()> {
        let mut nbt = Vec::with_capacity(64 * 1024);
        paper_nbt::write_named(&mut nbt, "", &chunk_to_nbt(chunk));
        let region = self.region(chunk.snapshot.pos, true)?.expect("created");
        let mut region = region.lock();
        region.write(chunk.snapshot.pos, &nbt)
    }

    /// The saved entities of a chunk (the `Entities` list), from the write queue or the
    /// entity region file.
    pub fn load_entities(&self, pos: ChunkPos) -> Result<Vec<Compound>> {
        let root = match self.pending_entities.lock().get(&pos).cloned() {
            Some(pending) => (*pending).clone(),
            None => {
                let Some(region) = self.entity_region(pos, false)? else { return Ok(Vec::new()) };
                let Some(bytes) = region.lock().read(pos)? else { return Ok(Vec::new()) };
                paper_nbt::read_named(&mut bytes.as_slice(), MAX_NBT).with_context(|| format!("parsing the entities of chunk {},{}", pos.x, pos.z))?.1
            }
        };
        Ok(root.list("Entities").unwrap_or(&[]).iter().filter_map(|t| t.as_compound().cloned()).collect())
    }

    /// Queues the entities of a chunk for writing (vanilla's entity chunk layout).
    pub fn save_entities(self: &Arc<Self>, pos: ChunkPos, entities: Vec<Compound>) {
        let root = Compound::new()
            .with("DataVersion", Tag::Int(data_version()))
            .with("Position", Tag::IntArray(vec![pos.x, pos.z]))
            .with("Entities", Tag::List(entities.into_iter().map(Tag::Compound).collect()));
        let root = Arc::new(root);
        self.pending_entities.lock().insert(pos, root.clone());
        let _ = self.io.tx.send(IoJob::Entities(self.clone(), pos, root));
    }

    fn write_entities(&self, pos: ChunkPos, root: &Compound) -> Result<()> {
        let mut nbt = Vec::with_capacity(4 * 1024);
        paper_nbt::write_named(&mut nbt, "", root);
        let region = self.entity_region(pos, true)?.expect("created");
        let mut region = region.lock();
        region.write(pos, &nbt)
    }

    /// Flushes region files to the disk (after [`ChunkIo::flush`]).
    pub fn sync(&self) {
        for region in self.entity_regions.lock().values() {
            if let Err(e) = region.lock().sync() {
                log::warn!("Could not sync an entity region file of {}: {e}", self.dir.display());
            }
        }
        for region in self.regions.lock().values() {
            if let Err(e) = region.lock().sync() {
                log::warn!("Could not sync a region file of {}: {e}", self.dir.display());
            }
        }
    }

    pub fn read_level(&self) -> Result<Option<LevelData>> {
        match read_gzip_nbt(&self.dir.join("level.dat"))? {
            Some(root) => LevelData::from_nbt(&root).map(Some),
            None => Ok(None),
        }
    }

    /// Queues `level.dat` for writing.
    pub fn save_level(&self, level: &LevelData) -> Result<()> {
        self.io.write_file(self.dir.join("level.dat"), gzip_nbt(&level.to_nbt())?);
        Ok(())
    }

    /// Game rules from `data/minecraft/game_rules.dat`, or from the `GameRules` of an older
    /// `level.dat`. `None` for a new world.
    pub fn read_game_rules(&self) -> Result<Option<super::rules::GameRules>> {
        if let Some(root) = read_gzip_nbt(&self.dir.join("data").join("minecraft").join("game_rules.dat"))? {
            return Ok(Some(super::rules::GameRules::from_nbt(&root)));
        }
        let Some(level) = read_gzip_nbt(&self.dir.join("level.dat"))? else { return Ok(None) };
        Ok(level.compound("Data").and_then(|d| d.compound("GameRules")).map(super::rules::GameRules::from_legacy))
    }

    pub fn save_game_rules(&self, rules: &super::rules::GameRules) -> Result<()> {
        self.io.write_file(self.dir.join("data").join("minecraft").join("game_rules.dat"), gzip_nbt(&rules.to_nbt())?);
        Ok(())
    }

    fn player_path(&self, uuid: uuid::Uuid) -> PathBuf {
        self.dir.join("playerdata").join(format!("{}.dat", uuid.hyphenated()))
    }

    pub fn read_player(&self, uuid: uuid::Uuid) -> Result<Option<Compound>> {
        if let Some(pending) = self.pending_players.lock().get(&uuid) {
            return Ok(Some((**pending).clone()));
        }
        read_gzip_nbt(&self.player_path(uuid))
    }

    /// Queues a player's data for writing.
    pub fn save_player(self: &Arc<Self>, uuid: uuid::Uuid, data: Compound) {
        let data = Arc::new(data);
        self.pending_players.lock().insert(uuid, data.clone());
        let _ = self.io.tx.send(IoJob::Player(self.clone(), uuid, data));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::generator::{FlatGenerator, Generator, NoiseGenerator};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("paper-rs-storage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn packing_round_trips() {
        for bits in [1, 4, 5, 9, 15] {
            let values: Vec<u64> = (0..4096u64).map(|i| (i * 7919) % (1 << bits)).collect();
            let packed = pack(values.iter().copied(), bits);
            assert_eq!(packed.len(), 4096usize.div_ceil((64 / bits) as usize));
            let back = unpack(&packed, bits, 4096).unwrap();
            assert!(back.iter().zip(&values).all(|(a, b)| *a as u64 == *b), "{bits} bits");
        }
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(17), 5);
        assert_eq!(ceil_log2(385), 9);
    }

    #[test]
    fn region_file_reads_back_what_it_wrote() {
        let dir = temp_dir("region");
        let path = dir.join("r.0.0.mca");
        let mut region = RegionFile::open(&path, true).unwrap().unwrap();
        let small = vec![7u8; 100];
        let big: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        region.write(ChunkPos::new(0, 0), &small).unwrap();
        region.write(ChunkPos::new(31, 31), &big).unwrap();
        region.write(ChunkPos::new(5, 3), &small).unwrap();
        // Grow a chunk so it has to move.
        region.write(ChunkPos::new(0, 0), &big).unwrap();
        drop(region);
        let mut region = RegionFile::open(&path, false).unwrap().unwrap();
        assert_eq!(region.read(ChunkPos::new(0, 0)).unwrap().unwrap(), big);
        assert_eq!(region.read(ChunkPos::new(31, 31)).unwrap().unwrap(), big);
        assert_eq!(region.read(ChunkPos::new(5, 3)).unwrap().unwrap(), small);
        assert!(region.read(ChunkPos::new(1, 1)).unwrap().is_none());
        // Incompressible data beyond 255 sectors goes to an external .mcc file.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let huge: Vec<u8> = (0..1_200_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 32) as u8
            })
            .collect();
        region.write(ChunkPos::new(2, 2), &huge).unwrap();
        assert!(dir.join("c.2.2.mcc").exists());
        assert_eq!(region.read(ChunkPos::new(2, 2)).unwrap().unwrap(), huge);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chunks_survive_a_round_trip_through_nbt() {
        let generator = NoiseGenerator::new(42);
        let pos = ChunkPos::new(3, -2);
        let mut chunk = generator.generate(pos, -64, 384);
        let stairs = blocks::parse_state("minecraft:oak_stairs[facing=east,half=top]").unwrap();
        chunk.set_block(1, 100, 2, stairs).unwrap();
        // Fake light so the light arrays are written.
        for (i, light) in chunk.sky_light.iter_mut().enumerate() {
            *light = Arc::new(if i % 2 == 0 { LightSection::Uniform(15) } else { LightSection::Uniform(3) });
        }
        let mut lamp = LightSection::Uniform(0);
        lamp.set(4, 5, 6, 14);
        chunk.block_light[7] = Arc::new(lamp);
        chunk.lit = true;

        let save = SaveChunk { snapshot: chunk.snapshot(), lit: true, has_skylight: true, ticks: SavedTicks::default(), status: GenStatus::Full, proto: None };
        let nbt = chunk_to_nbt(&save);
        let mut bytes = Vec::new();
        paper_nbt::write_named(&mut bytes, "", &nbt);
        let (_, parsed) = paper_nbt::read_named(&mut bytes.as_slice(), MAX_NBT).unwrap();
        let back = chunk_from_nbt(&parsed, pos, -64, 384, true).unwrap().unwrap();

        for y in [-64, -1, 0, 63, 100, 200] {
            for (x, z) in [(0, 0), (1, 2), (15, 15), (7, 9)] {
                assert_eq!(back.get_block(x, y, z), chunk.get_block(x, y, z), "block at {x},{y},{z}");
            }
        }
        assert_eq!(back.get_block(1, 100, 2), stairs);
        for (a, b) in back.sections.iter().zip(&chunk.sections) {
            assert_eq!(a.biomes, b.biomes);
        }
        assert!(back.lit);
        assert_eq!(back.sky_light, chunk.sky_light);
        assert_eq!(back.block_light[7].get(4, 5, 6), 14);
        assert_eq!(back.motion_blocking, chunk.motion_blocking);
        assert_eq!(back.world_surface, chunk.world_surface);
    }

    #[test]
    fn storage_loads_saved_and_pending_chunks() {
        let dir = temp_dir("world");
        let io = ChunkIo::start();
        let storage = WorldStorage::open(&dir, io.clone()).unwrap();
        assert!(WorldStorage::open(&dir, io.clone()).is_err(), "a second server cannot open the world");
        let generator = FlatGenerator::new(-64);
        let pos = ChunkPos::new(-40, 70);
        let mut chunk = generator.generate(pos, -64, 384);
        chunk.set_block(3, -60, 3, blocks::default_state("minecraft:gold_block").unwrap()).unwrap();
        storage.save_chunk(SaveChunk { snapshot: chunk.snapshot(), lit: false, has_skylight: true, ticks: SavedTicks::default(), status: GenStatus::Full, proto: None });
        // Readable at once, whether or not the write has happened yet.
        let loaded = storage.load_chunk(pos, -64, 384, true).unwrap().unwrap();
        assert_eq!(loaded.get_block(3, -60, 3), blocks::default_state("minecraft:gold_block").unwrap());
        io.flush();
        assert!(storage.pending.lock().is_empty());
        assert!(dir.join("region/r.-2.2.mca").exists());
        let loaded = storage.load_chunk(pos, -64, 384, true).unwrap().unwrap();
        assert_eq!(loaded.get_block(3, -60, 3), blocks::default_state("minecraft:gold_block").unwrap());
        assert!(!loaded.lit, "unlit chunks are lit again after loading");
        assert!(storage.load_chunk(ChunkPos::new(0, 0), -64, 384, true).unwrap().is_none());

        let level = LevelData { name: "world".into(), seed: -99, generator: "flat".into(), spawn: (1, 2, 3), spawn_yaw: 90.0, age: 1234, day_time: 6000, daylight_cycle: false, game_type: 1, hardcore: false };
        storage.save_level(&level).unwrap();
        io.flush();
        assert_eq!(storage.read_level().unwrap().unwrap(), level);
        drop(storage);
        assert!(WorldStorage::open(&dir, io).is_ok(), "the lock is released with the storage");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
