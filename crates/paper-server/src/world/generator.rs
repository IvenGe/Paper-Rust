//! World generators.
//!
//! Two generators exist:
//!
//! * [`FlatGenerator`] reproduces vanilla's default superflat preset (bedrock, two layers
//!   of dirt, grass) and is selected by `level-type=minecraft:flat`.
//! * [`NoiseGenerator`] produces rolling terrain with water, beaches and trees. It is this
//!   project's own generator, not a port of vanilla's world generation: worlds it creates
//!   do not match what vanilla or Paper would generate from the same seed.
//!
//! Generators only place blocks and biomes; lighting is a separate stage.

use super::chunk::{BiomeStorage, BlockStorage, Chunk, ChunkPos, Section};
use paper_data::blocks;
use std::sync::Arc;

/// Produces the terrain of a chunk. Implementations must be deterministic for a given
/// position so that neighbouring chunks agree on features that cross chunk borders.
pub trait Generator: Send + Sync {
    /// Generates blocks and biomes for `pos` in a world spanning `[min_y, min_y + height)`.
    fn generate(&self, pos: ChunkPos, min_y: i32, height: i32) -> Chunk;
    /// Whether this is a superflat world (changes the client's horizon and void fog).
    fn is_flat(&self) -> bool;
    fn sea_level(&self) -> i32;
    /// Y of the highest solid block at a block column, used to place the world spawn.
    fn surface_y(&self, x: i32, z: i32) -> i32;
    fn name(&self) -> &'static str;
}

fn state(name: &str) -> u32 {
    blocks::default_state(name).unwrap_or_else(|| panic!("block {name} is missing from the data tables"))
}

fn biome(name: &str) -> u8 {
    paper_data::synced::id("minecraft:worldgen/biome", name).unwrap_or(0) as u8
}

/// Vanilla's default superflat preset.
pub struct FlatGenerator {
    /// Block states from the bottom of the world upwards.
    layers: Vec<u32>,
    biome: u8,
    min_y: i32,
}

impl FlatGenerator {
    pub fn new(min_y: i32) -> Self {
        Self {
            layers: vec![state("minecraft:bedrock"), state("minecraft:dirt"), state("minecraft:dirt"), state("minecraft:grass_block")],
            biome: biome("minecraft:plains"),
            min_y,
        }
    }
}

impl Generator for FlatGenerator {
    fn generate(&self, pos: ChunkPos, min_y: i32, height: i32) -> Chunk {
        let mut chunk = Chunk::empty(pos, min_y, height, self.biome);
        // All layers fit the lowest section; build it once and fill by layer.
        let mut data = Box::new([blocks::AIR as u16; 4096]);
        for (y, layer) in self.layers.iter().enumerate().take(16) {
            data[y << 8..(y + 1) << 8].fill(*layer as u16);
        }
        let mut section = Section { blocks: BlockStorage::Dense(data), biomes: BiomeStorage::Uniform(self.biome), non_air: 0, fluids: 0 };
        section.recount();
        chunk.sections[0] = Arc::new(section);
        let top = self.layers.len() as i16;
        chunk.motion_blocking.fill(top);
        chunk.world_surface.fill(top);
        chunk
    }

    fn is_flat(&self) -> bool {
        true
    }

    fn sea_level(&self) -> i32 {
        // Vanilla reports -63 for the default superflat preset.
        self.min_y + 1
    }

    fn surface_y(&self, _x: i32, _z: i32) -> i32 {
        self.min_y + self.layers.len() as i32 - 1
    }

    fn name(&self) -> &'static str {
        "flat"
    }
}

/// 64-bit mix (SplitMix64 finaliser) used to derive per-position randomness from the seed.
fn mix(mut v: u64) -> u64 {
    v = (v ^ (v >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    v ^ (v >> 31)
}

fn hash2(seed: u64, x: i32, z: i32) -> u64 {
    mix(seed ^ mix((x as u32 as u64) << 32 | z as u32 as u64))
}

/// Smooth value noise in `[-1, 1]` with bicubic-style interpolation.
fn value_noise(seed: u64, x: f64, z: f64) -> f64 {
    let (x0, z0) = (x.floor(), z.floor());
    let (fx, fz) = (x - x0, z - z0);
    let (ix, iz) = (x0 as i32, z0 as i32);
    let corner = |dx: i32, dz: i32| (hash2(seed, ix.wrapping_add(dx), iz.wrapping_add(dz)) >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
    let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v) = (fade(fx), fade(fz));
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * u;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * u;
    top + (bottom - top) * v
}

/// Fractal sum of value noise octaves, normalised to roughly `[-1, 1]`.
fn fbm(seed: u64, x: f64, z: f64, octaves: u32) -> f64 {
    let (mut sum, mut amplitude, mut frequency, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for octave in 0..octaves {
        sum += value_noise(seed.wrapping_add(octave as u64 * 0x9E37_79B9), x * frequency, z * frequency) * amplitude;
        norm += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum / norm
}

/// Rolling hills, lakes and oceans, beaches and scattered trees.
pub struct NoiseGenerator {
    seed: u64,
    sea_level: i32,
    stone: u32,
    dirt: u32,
    grass: u32,
    sand: u32,
    gravel: u32,
    water: u32,
    bedrock: u32,
    deepslate: u32,
    snow_block: u32,
    oak_log: u32,
    oak_leaves: u32,
    short_grass: u32,
    plains: u8,
    forest: u8,
    ocean: u8,
    beach: u8,
    mountains: u8,
}

impl NoiseGenerator {
    pub fn new(seed: i64) -> Self {
        let leaves = blocks::with_property(state("minecraft:oak_leaves"), "persistent", "false").unwrap_or_else(|| state("minecraft:oak_leaves"));
        // Distance 1 keeps generated leaves from being treated as detached from a log.
        let leaves = blocks::with_property(leaves, "distance", "1").unwrap_or(leaves);
        Self {
            seed: seed as u64,
            sea_level: 63,
            stone: state("minecraft:stone"),
            dirt: state("minecraft:dirt"),
            grass: state("minecraft:grass_block"),
            sand: state("minecraft:sand"),
            gravel: state("minecraft:gravel"),
            water: state("minecraft:water"),
            bedrock: state("minecraft:bedrock"),
            deepslate: state("minecraft:deepslate"),
            snow_block: state("minecraft:snow_block"),
            oak_log: state("minecraft:oak_log"),
            oak_leaves: leaves,
            short_grass: state("minecraft:short_grass"),
            plains: biome("minecraft:plains"),
            forest: biome("minecraft:forest"),
            ocean: biome("minecraft:ocean"),
            beach: biome("minecraft:beach"),
            mountains: biome("minecraft:windswept_hills"),
        }
    }

    /// Terrain height (Y of the highest solid block) at a block column.
    fn height_at(&self, x: i32, z: i32) -> i32 {
        let (fx, fz) = (x as f64, z as f64);
        let continent = fbm(self.seed, fx / 420.0, fz / 420.0, 4);
        let hills = fbm(self.seed ^ 0xA5A5, fx / 96.0, fz / 96.0, 4);
        let detail = fbm(self.seed ^ 0x5A5A, fx / 24.0, fz / 24.0, 2);
        // Mountains only where the continent value is high.
        let mountain = ((continent - 0.25).max(0.0) * 2.5).powi(2);
        let h = 66.0 + continent * 22.0 + hills * (9.0 + mountain * 45.0) + detail * 2.0 + mountain * 30.0;
        h.round() as i32
    }

    fn forestness(&self, x: i32, z: i32) -> f64 {
        fbm(self.seed ^ 0xF0F0, x as f64 / 180.0, z as f64 / 180.0, 3)
    }

    /// If a tree is rooted at this column, its trunk height.
    fn tree_at(&self, x: i32, z: i32) -> Option<i32> {
        let ground = self.height_at(x, z);
        if ground <= self.sea_level + 1 || ground > 110 {
            return None;
        }
        let density = match self.forestness(x, z) {
            f if f > 0.15 => 28,
            f if f > -0.2 => 260,
            _ => return None,
        };
        let h = hash2(self.seed ^ 0x7EE5, x, z);
        // A local-maximum rule keeps trunks from touching: a column grows a tree only if its
        // hash beats its eight neighbours as well as the density roll.
        if h % density != 0 {
            return None;
        }
        for dz in -1..=1 {
            for dx in -1..=1 {
                if (dx != 0 || dz != 0) && hash2(self.seed ^ 0x7EE5, x + dx, z + dz) % density == 0 && hash2(self.seed ^ 0x7EE6, x + dx, z + dz) > hash2(self.seed ^ 0x7EE6, x, z) {
                    return None;
                }
            }
        }
        Some(4 + (h >> 20) as i32 % 3)
    }

    fn biome_at(&self, x: i32, z: i32, height: i32) -> u8 {
        if height < self.sea_level - 2 {
            self.ocean
        } else if height <= self.sea_level + 1 {
            self.beach
        } else if height > 105 {
            self.mountains
        } else if self.forestness(x, z) > 0.15 {
            self.forest
        } else {
            self.plains
        }
    }
}

impl Generator for NoiseGenerator {
    fn generate(&self, pos: ChunkPos, min_y: i32, height: i32) -> Chunk {
        let mut chunk = Chunk::empty(pos, min_y, height, self.plains);
        let max_y = min_y + height;
        let (bx, bz) = (pos.x * 16, pos.z * 16);
        let mut heights = [[0i32; 16]; 16];
        for z in 0..16 {
            for x in 0..16 {
                heights[z][x] = self.height_at(bx + x as i32, bz + z as i32).clamp(min_y + 1, max_y - 24);
            }
        }

        // Terrain columns, written straight into dense section arrays.
        let section_count = (height / 16) as usize;
        let mut data: Vec<Option<Box<[u16; 4096]>>> = (0..section_count).map(|_| None).collect();
        let put = |data: &mut Vec<Option<Box<[u16; 4096]>>>, x: usize, y: i32, z: usize, s: u32| {
            if y < min_y || y >= max_y {
                return;
            }
            let rel = (y - min_y) as usize;
            let section = data[rel >> 4].get_or_insert_with(|| Box::new([blocks::AIR as u16; 4096]));
            section[((rel & 15) << 8) | (z << 4) | x] = s as u16;
        };
        for z in 0..16 {
            for x in 0..16 {
                let h = heights[z][x];
                let (wx, wz) = (bx + x as i32, bz + z as i32);
                let underwater = h < self.sea_level;
                let beach = h <= self.sea_level + 1;
                let snowy = h > 118;
                for y in min_y..=h {
                    let depth = h - y;
                    let s = if y == min_y || (y < min_y + 4 && hash2(self.seed ^ y as u64, wx, wz) % 4 < (min_y + 4 - y) as u64) {
                        self.bedrock
                    } else if depth == 0 {
                        if snowy {
                            self.snow_block
                        } else if beach {
                            if underwater && h < self.sea_level - 6 {
                                self.gravel
                            } else {
                                self.sand
                            }
                        } else {
                            self.grass
                        }
                    } else if depth < 4 {
                        if beach {
                            self.sand
                        } else if snowy {
                            self.stone
                        } else {
                            self.dirt
                        }
                    } else if y < 0 {
                        self.deepslate
                    } else {
                        self.stone
                    };
                    put(&mut data, x, y, z, s);
                }
                for y in h + 1..=self.sea_level {
                    put(&mut data, x, y, z, self.water);
                }
                // Sparse grass tufts on open ground.
                if !underwater && !beach && !snowy && hash2(self.seed ^ 0x6A55, wx, wz) % 9 == 0 && self.tree_at(wx, wz).is_none() {
                    put(&mut data, x, h + 1, z, self.short_grass);
                }
            }
        }

        // Trees: consider roots up to two blocks outside the chunk so canopies that reach
        // across the border are generated identically by both chunks.
        for tz in -2..18 {
            for tx in -2..18 {
                let (wx, wz) = (bx + tx, bz + tz);
                let Some(trunk) = self.tree_at(wx, wz) else { continue };
                let ground = self.height_at(wx, wz);
                let top = ground + trunk;
                for dy in -2i32..=1 {
                    let radius: i32 = if dy >= 0 { 1 } else { 2 };
                    for dz in -radius..=radius {
                        for dx in -radius..=radius {
                            // Trim the corners for a rounder canopy.
                            if dx.abs() == radius && dz.abs() == radius && (dy == 1 || hash2(self.seed ^ 0x1EAF, wx + dx + dy, wz + dz) % 2 == 0) {
                                continue;
                            }
                            let (lx, lz) = (tx + dx, tz + dz);
                            if !(0..16).contains(&lx) || !(0..16).contains(&lz) {
                                continue;
                            }
                            let y = top + dy;
                            let rel = (y - min_y) as usize;
                            let empty = data.get(rel >> 4).and_then(|s| s.as_ref()).is_none_or(|s| s[((rel & 15) << 8) | ((lz as usize) << 4) | lx as usize] == blocks::AIR as u16);
                            if empty {
                                put(&mut data, lx as usize, y, lz as usize, self.oak_leaves);
                            }
                        }
                    }
                }
                if (0..16).contains(&tx) && (0..16).contains(&tz) {
                    for y in ground + 1..=top {
                        put(&mut data, tx as usize, y, tz as usize, self.oak_log);
                    }
                    // Trees stand on dirt, like vanilla's.
                    put(&mut data, tx as usize, ground, tz as usize, self.dirt);
                }
            }
        }

        for (i, section_data) in data.into_iter().enumerate() {
            // Biomes at 4x4x4 resolution, sampled at cell centres.
            let mut biomes = Box::new([self.plains; 64]);
            for cz in 0..4 {
                for cx in 0..4 {
                    let (x, z) = (cx * 4 + 2, cz * 4 + 2);
                    let b = self.biome_at(bx + x as i32, bz + z as i32, heights[z][x]);
                    for cy in 0..4 {
                        biomes[(cy << 4) | (cz << 2) | cx] = b;
                    }
                }
            }
            let biomes = if biomes.iter().all(|b| *b == biomes[0]) { BiomeStorage::Uniform(biomes[0]) } else { BiomeStorage::Dense(biomes) };
            let blocks = match section_data {
                Some(d) => BlockStorage::Dense(d),
                None => BlockStorage::Uniform(blocks::AIR as u16),
            };
            let mut section = Section { blocks, biomes, non_air: 0, fluids: 0 };
            section.compact();
            section.recount();
            chunk.sections[i] = Arc::new(section);
        }
        chunk.recompute_derived();
        chunk
    }

    fn is_flat(&self) -> bool {
        false
    }

    fn sea_level(&self) -> i32 {
        self.sea_level
    }

    fn surface_y(&self, x: i32, z: i32) -> i32 {
        self.height_at(x, z).max(self.sea_level)
    }

    fn name(&self) -> &'static str {
        "noise"
    }
}
