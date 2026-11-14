//! Terrain heights and columns from the noise alone, before any chunk exists: what
//! structure starts measure (vanilla's `NoiseBasedChunkGenerator.iterateNoiseColumn`,
//! `getBaseHeight`, `getBaseColumn`).
//!
//! Written to vanilla 26.3: the final density is sampled over a one block wide column and
//! the aquifer decides the fluid; there is no beardifier and there are no ore veins.

use crate::aquifer::Aquifer;
use crate::chunk::Heightmap;
use crate::feature::world::Pos;
use crate::generator::WorldGenerator;
use crate::math::FxMap;
use crate::noise::DensityVolume;
use crate::router::{ChunkRouter, ClimateSampler, SurfaceEstimator};
use std::sync::Arc;

/// Samples noise columns for one structure start; caches columns by position.
pub struct Columns<'a> {
    gen: &'a WorldGenerator,
    router: ChunkRouter<'a>,
    estimator: SurfaceEstimator<'a>,
    aquifers: FxMap<(i32, i32), Aquifer>,
    columns: FxMap<(i32, i32), Arc<[u16]>>,
    density: Vec<f32>,
    climate: ClimateSampler<'a>,
}

impl<'a> Columns<'a> {
    pub fn new(gen: &'a WorldGenerator) -> Self {
        Self { gen, router: ChunkRouter::terrain(&gen.terrain), estimator: gen.surface_estimator(), aquifers: FxMap::default(), columns: FxMap::default(), density: Vec::new(), climate: gen.climate_sampler() }
    }

    /// The block states of the column at `(x, z)` from the generation bottom up
    /// (`getBaseColumn`).
    pub fn column(&mut self, x: i32, z: i32) -> Arc<[u16]> {
        if let Some(c) = self.columns.get(&(x, z)) {
            return c.clone();
        }
        let g = self.gen;
        let height = g.gen_height.max(0) as usize;
        let mut out = vec![0u16; height];
        if height > 0 {
            let volume = DensityVolume::blocks(1, height, 1, x, g.gen_min_y, z);
            self.density.resize(volume.size(), 0.0);
            self.router.sample_volume(g.terrain_fns.final_density, &mut self.density, &volume);
            let picker = g.fluid_picker();
            let chunk = (x >> 4, z >> 4);
            if g.aquifers_enabled && !self.aquifers.contains_key(&chunk) {
                let a = Aquifer::new(chunk.0, chunk.1, &g.aquifer_random, g.gen_min_y, g.gen_height, picker, g.terrain_fns.aquifer, g.water, g.lava);
                self.aquifers.insert(chunk, a);
            }
            for y in (0..height).rev() {
                let by = volume.block_y(y);
                let d = self.density[volume.index(0, y, 0)];
                let state = match self.aquifers.get_mut(&chunk) {
                    Some(a) => a.compute(&mut self.router, &mut self.estimator, x, by, z, d).0,
                    None => (d <= 0.0).then(|| picker.level(by).at(by)),
                };
                out[y] = state.unwrap_or(g.default_block) as u16;
            }
        }
        let c: Arc<[u16]> = out.into();
        self.columns.insert((x, z), c.clone());
        c
    }

    /// The block at `y` of a column (air outside the generated range, `NoiseColumn.getBlock`).
    pub fn block(&mut self, x: i32, y: i32, z: i32) -> u32 {
        let c = self.column(x, z);
        let i = y - self.gen.gen_min_y;
        if i >= 0 && (i as usize) < c.len() {
            c[i as usize] as u32
        } else {
            paper_data::blocks::AIR
        }
    }

    /// `getBaseHeight`: one above the highest block the heightmap counts, or the
    /// dimension's bottom.
    pub fn base_height(&mut self, x: i32, z: i32, hm: Heightmap) -> i32 {
        let c = self.column(x, z);
        let flags = &self.gen.flags;
        for y in (0..c.len()).rev() {
            let s = c[y] as u32;
            let hit = match hm {
                Heightmap::OceanFloorWg | Heightmap::OceanFloor => flags.get(s) & crate::blocks::BLOCKS_MOTION != 0,
                _ => !flags.is_air(s),
            };
            if hit {
                return self.gen.gen_min_y + y as i32 + 1;
            }
        }
        self.gen.min_y
    }

    /// `getFirstFreeHeight`.
    pub fn first_free(&mut self, x: i32, z: i32, hm: Heightmap) -> i32 {
        self.base_height(x, z, hm)
    }

    /// `getFirstOccupiedHeight`.
    pub fn first_occupied(&mut self, x: i32, z: i32, hm: Heightmap) -> i32 {
        self.base_height(x, z, hm) - 1
    }

    /// `Structure.getCornerHeights` (first occupied, WORLD_SURFACE_WG).
    pub fn corner_heights(&mut self, min_x: i32, size_x: i32, min_z: i32, size_z: i32) -> [i32; 4] {
        let hm = Heightmap::WorldSurfaceWg;
        [
            self.first_occupied(min_x, min_z, hm),
            self.first_occupied(min_x, min_z + size_z, hm),
            self.first_occupied(min_x + size_x, min_z, hm),
            self.first_occupied(min_x + size_x, min_z + size_z, hm),
        ]
    }

    /// `Structure.getLowestY(context, minX, minZ, sizeX, sizeZ)`.
    pub fn lowest_y(&mut self, min_x: i32, min_z: i32, size_x: i32, size_z: i32) -> i32 {
        let c = self.corner_heights(min_x, size_x, min_z, size_z);
        c[0].min(c[1]).min(c[2].min(c[3]))
    }

    /// `Structure.getMeanFirstOccupiedHeight`.
    pub fn mean_first_occupied(&mut self, min_x: i32, min_z: i32, size_x: i32, size_z: i32) -> i32 {
        let c = self.corner_heights(min_x, size_x, min_z, size_z);
        (c[0] + c[1] + c[2] + c[3]) / 4
    }

    /// The biome at a block position, from the biome source (`isValidBiome`'s lookup).
    pub fn biome_at(&mut self, p: Pos) -> u8 {
        self.biome_at_quart(p.x >> 2, p.y >> 2, p.z >> 2)
    }

    /// The biome at a quart position (no clamping to the build height, like vanilla's
    /// biome resolver).
    pub fn biome_at_quart(&mut self, qx: i32, qy: i32, qz: i32) -> u8 {
        self.gen.biome_source.biome(qx, qy, qz, &mut self.climate)
    }
}
