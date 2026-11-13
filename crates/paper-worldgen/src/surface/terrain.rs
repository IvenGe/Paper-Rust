//! Badlands terracotta bands and pillars, and frozen ocean icebergs.
//!
//! Derived from `pumpkin-world/src/generation/surface/terrain.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::biome::BiomeInfo;
use crate::blocks::{state, BlockFlags};
use crate::chunk::ProtoChunk;
use crate::noise::{Noise, NormalNoise};
use crate::random::{PositionalRandom, Random, RandomSource};
use std::sync::Arc;

pub struct TerrainBuilder {
    bands: Vec<u32>,
    bands_offset: Arc<NormalNoise>,
    badlands_pillar: Arc<NormalNoise>,
    badlands_surface: Arc<NormalNoise>,
    badlands_pillar_roof: Arc<NormalNoise>,
    iceberg_pillar: Arc<NormalNoise>,
    iceberg_pillar_roof: Arc<NormalNoise>,
    iceberg_surface: Arc<NormalNoise>,
    water: u32,
    snow_block: u32,
    packed_ice: u32,
}

/// The noises the builder needs, by id.
pub struct TerrainNoises {
    pub clay_bands_offset: Arc<NormalNoise>,
    pub badlands_pillar: Arc<NormalNoise>,
    pub badlands_surface: Arc<NormalNoise>,
    pub badlands_pillar_roof: Arc<NormalNoise>,
    pub iceberg_pillar: Arc<NormalNoise>,
    pub iceberg_pillar_roof: Arc<NormalNoise>,
    pub iceberg_surface: Arc<NormalNoise>,
}

impl TerrainBuilder {
    pub fn new(random: &PositionalRandom, noises: TerrainNoises) -> Self {
        Self {
            bands: Self::bands(random.from_hash_of("minecraft:clay_bands")),
            bands_offset: noises.clay_bands_offset,
            badlands_pillar: noises.badlands_pillar,
            badlands_surface: noises.badlands_surface,
            badlands_pillar_roof: noises.badlands_pillar_roof,
            iceberg_pillar: noises.iceberg_pillar,
            iceberg_pillar_roof: noises.iceberg_pillar_roof,
            iceberg_surface: noises.iceberg_surface,
            water: state("minecraft:water"),
            snow_block: state("minecraft:snow_block"),
            packed_ice: state("minecraft:packed_ice"),
        }
    }

    fn bands(mut random: RandomSource) -> Vec<u32> {
        let terracotta = state("minecraft:terracotta");
        let orange = state("minecraft:orange_terracotta");
        let mut b = vec![terracotta; 192];
        let mut i = 0;
        while i < b.len() {
            i += random.next_bounded(5) as usize + 1;
            if i >= b.len() {
                break;
            }
            b[i] = orange;
            i += 1;
        }
        Self::add_bands(&mut random, &mut b, 1, state("minecraft:yellow_terracotta"));
        Self::add_bands(&mut random, &mut b, 2, state("minecraft:brown_terracotta"));
        Self::add_bands(&mut random, &mut b, 1, state("minecraft:red_terracotta"));
        let white = state("minecraft:white_terracotta");
        let light_gray = state("minecraft:light_gray_terracotta");
        let count = random.next_between(9, 15);
        let mut n = 0;
        let mut k = 0usize;
        while n < count && k < b.len() {
            b[k] = white;
            if k > 1 && random.next_bool() {
                b[k - 1] = light_gray;
            }
            if k + 1 < b.len() && random.next_bool() {
                b[k + 1] = light_gray;
            }
            k += random.next_bounded(16) as usize + 4;
            n += 1;
        }
        b
    }

    fn add_bands(random: &mut RandomSource, b: &mut [u32], min_size: i32, s: u32) {
        let count = random.next_between(6, 15);
        for _ in 0..count {
            let width = min_size + random.next_bounded(3);
            let start = random.next_bounded(b.len() as i32);
            let mut m = 0;
            while start + m < b.len() as i32 && m < width {
                b[(start + m) as usize] = s;
                m += 1;
            }
        }
    }

    /// The `minecraft:bandlands` material rule.
    pub fn terracotta(&self, x: i32, y: i32, z: i32) -> u32 {
        let offset = crate::math::java_round_f32(self.bands_offset.get(x as f64, 0.0, z as f64) * 4.0);
        self.bands[(y + offset).rem_euclid(self.bands.len() as i32) as usize]
    }

    /// Raises eroded badlands pillars in a column.
    pub fn badlands_pillar(&self, chunk: &mut ProtoChunk, flags: &BlockFlags, default_block: u32, x: i32, z: i32, surface_y: i32) {
        let surface = (self.badlands_surface.get(x as f64, 0.0, z as f64) * 8.25).abs();
        let pillar = self.badlands_pillar.get(x as f64 * 0.2, 0.0, z as f64 * 0.2) * 15.0;
        let threshold = surface.min(pillar);
        if threshold <= 0.0 {
            return;
        }
        let roof = (self.badlands_pillar_roof.get(x as f64 * 0.75, 0.0, z as f64 * 0.75) * 1.5).abs();
        let elevation = 64.0 + (threshold * threshold * 2.5).min((roof * 50.0).ceil() + 24.0);
        let top = elevation.floor() as i32;
        if surface_y > top {
            return;
        }
        let default = paper_data::blocks::block_id_of_state(default_block);
        let water = paper_data::blocks::block_id_of_state(self.water);
        let mut y = top;
        while y >= chunk.min_y {
            let b = paper_data::blocks::block_id_of_state(chunk.get(x, y, z));
            if b == default {
                break;
            }
            if b == water {
                return;
            }
            y -= 1;
        }
        let mut y = top;
        while y >= chunk.min_y && flags.is_air(chunk.get(x, y, z)) {
            chunk.set(x, y, z, default_block, flags);
            y -= 1;
        }
    }

    /// Builds frozen ocean icebergs in a column.
    #[allow(clippy::too_many_arguments)]
    pub fn iceberg(&self, chunk: &mut ProtoChunk, flags: &BlockFlags, biome: &BiomeInfo, x: i32, z: i32, surface_estimate: i32, top: i32, sea_level: i32, random: &PositionalRandom) {
        let surface = (self.iceberg_surface.get(x as f64, 0.0, z as f64) * 8.25).abs();
        let pillar = self.iceberg_pillar.get(x as f64 * 1.28, 0.0, z as f64 * 1.28) * 15.0;
        let threshold = surface.min(pillar);
        if threshold <= 1.8 {
            return;
        }
        let roof = (self.iceberg_pillar_roof.get(x as f64 * 1.17, 0.0, z as f64 * 1.17) * 1.5).abs();
        let mut t = (threshold * threshold * 1.2).min((roof * 40.0).ceil() + 14.0);
        if biome.temperature_at(x, sea_level, z, sea_level) > 0.1 {
            t -= 2.0;
        }
        let (top_block, bottom_block) = if t > 2.0 { (t as i32 + sea_level, (sea_level as f32 - t - 7.0) as i32) } else { (0, 0) };
        let mut r = random.at(x, 0, z);
        let snow_count = 2 + r.next_bounded(4);
        let snow_bottom = sea_level + 18 + r.next_bounded(10);
        let mut snow = 0;
        let water = paper_data::blocks::block_id_of_state(self.water);
        let mut y = top.max(top_block + 1);
        while y >= surface_estimate {
            let s = chunk.get(x, y, z);
            if (flags.is_air(s) && y < top_block && r.next_f64() > 0.01)
                || (paper_data::blocks::block_id_of_state(s) == water && y > bottom_block && y < sea_level && bottom_block != 0 && r.next_f64() > 0.15)
            {
                if snow <= snow_count && y > snow_bottom {
                    chunk.set(x, y, z, self.snow_block, flags);
                    snow += 1;
                } else {
                    chunk.set(x, y, z, self.packed_ice, flags);
                }
            }
            y -= 1;
        }
    }
}
