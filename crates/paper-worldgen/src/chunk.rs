//! The chunk being generated: block states, biomes and heightmaps.
//!
//! Heightmap maintenance is derived from `pumpkin-world/src/generation/proto_chunk.rs` of
//! Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::blocks::BlockFlags;

/// Heightmap types (vanilla's `Heightmap.Types`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Heightmap {
    WorldSurfaceWg,
    WorldSurface,
    OceanFloorWg,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

impl Heightmap {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "WORLD_SURFACE_WG" => Self::WorldSurfaceWg,
            "WORLD_SURFACE" => Self::WorldSurface,
            "OCEAN_FLOOR_WG" => Self::OceanFloorWg,
            "OCEAN_FLOOR" => Self::OceanFloor,
            "MOTION_BLOCKING" => Self::MotionBlocking,
            "MOTION_BLOCKING_NO_LEAVES" => Self::MotionBlockingNoLeaves,
            _ => return None,
        })
    }
}

const SURFACE: usize = 0;
const OCEAN: usize = 1;
const MOTION: usize = 2;
const MOTION_NO_LEAVES: usize = 3;

/// A chunk column during generation.
///
/// Block states are indexed `(y - min_y) << 8 | z << 4 | x`, so each 4096-entry slice is
/// one section in the server's layout. Biomes (network ids of `minecraft:worldgen/biome`)
/// are stored per 4x4x4 cell, indexed `((y - min_y) >> 2) << 4 | (z >> 2) << 2 | x >> 2`.
#[derive(Clone)]
pub struct ProtoChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub min_y: i32,
    pub height: i32,
    pub blocks: Vec<u16>,
    pub biomes: Vec<u8>,
    /// Highest y of each heightmap per column (`x * 16 + z`), `min_y - 1` when empty.
    heights: [[i16; 256]; 4],
    /// WORLD_SURFACE_WG and OCEAN_FLOOR_WG, frozen after the carver stage.
    wg_heights: [[i16; 256]; 2],
}

impl ProtoChunk {
    pub fn new(chunk_x: i32, chunk_z: i32, min_y: i32, height: i32) -> Self {
        let empty = (min_y - 1) as i16;
        Self {
            chunk_x,
            chunk_z,
            min_y,
            height,
            blocks: vec![0; (height as usize) << 8],
            biomes: vec![0; ((height as usize) >> 2) << 4],
            heights: [[empty; 256]; 4],
            wg_heights: [[empty; 256]; 2],
        }
    }

    #[inline]
    pub fn start_x(&self) -> i32 {
        self.chunk_x << 4
    }

    #[inline]
    pub fn start_z(&self) -> i32 {
        self.chunk_z << 4
    }

    #[inline]
    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        (((y - self.min_y) as usize) << 8) | (((z & 15) as usize) << 4) | (x & 15) as usize
    }

    #[inline]
    pub fn in_height(&self, y: i32) -> bool {
        y >= self.min_y && y < self.min_y + self.height
    }

    /// The state at a block position (x and z are taken modulo 16); air outside the height.
    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> u32 {
        if !self.in_height(y) {
            return paper_data::blocks::AIR;
        }
        self.blocks[self.index(x, y, z)] as u32
    }

    /// Sets a state without touching heightmaps.
    #[inline]
    pub fn set_raw(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let i = self.index(x, y, z);
        self.blocks[i] = state as u16;
    }

    /// Sets a state and keeps the heightmaps up to date.
    pub fn set(&mut self, x: i32, y: i32, z: i32, state: u32, flags: &BlockFlags) {
        if !self.in_height(y) {
            return;
        }
        let i = self.index(x, y, z);
        if self.blocks[i] as u32 == state {
            return;
        }
        self.blocks[i] = state as u16;
        self.update_heightmaps(x & 15, y, z & 15, state, flags);
    }

    fn update_heightmaps(&mut self, lx: i32, y: i32, lz: i32, state: u32, flags: &BlockFlags) {
        let hm = (lx * 16 + lz) as usize;
        let y16 = y as i16;
        if y16 < self.heights[OCEAN][hm].min(self.heights[MOTION_NO_LEAVES][hm]) {
            return;
        }
        let f = flags.get(state);
        let not_air = f & crate::blocks::AIR == 0;
        let ocean = not_air && f & crate::blocks::BLOCKS_MOTION != 0;
        let liquid = not_air && f & crate::blocks::FLUID != 0;
        let leaves = not_air && f & crate::blocks::LEAVES != 0;
        let motion = ocean || liquid;
        let motion_no_leaves = motion && !leaves;
        let checks = [not_air, ocean, motion, motion_no_leaves];
        let mut lower = [false; 4];
        let mut any_lower = false;
        for k in 0..4 {
            if checks[k] && y16 > self.heights[k][hm] {
                self.heights[k][hm] = y16;
            }
            lower[k] = !checks[k] && y16 == self.heights[k][hm];
            any_lower |= lower[k];
        }
        if !any_lower {
            return;
        }
        let empty = (self.min_y - 1) as i16;
        for k in 0..4 {
            if lower[k] {
                self.heights[k][hm] = empty;
            }
        }
        let mut sy = y - 1;
        while sy >= self.min_y {
            let s = self.blocks[self.index(lx, sy, lz)] as u32;
            let f = flags.get(s);
            if f & crate::blocks::AIR == 0 {
                let bm = f & crate::blocks::BLOCKS_MOTION != 0;
                let mb = bm || f & crate::blocks::FLUID != 0;
                let found = [true, bm, mb, mb && f & crate::blocks::LEAVES == 0];
                for k in 0..4 {
                    if lower[k] && found[k] {
                        self.heights[k][hm] = sy as i16;
                        lower[k] = false;
                    }
                }
                if !lower.iter().any(|l| *l) {
                    break;
                }
            }
            sy -= 1;
        }
    }

    /// Recomputes all heightmaps from the blocks.
    pub fn prime_heightmaps(&mut self, flags: &BlockFlags) {
        let empty = (self.min_y - 1) as i16;
        for lx in 0..16 {
            for lz in 0..16 {
                let hm = (lx * 16 + lz) as usize;
                let mut found = [false; 4];
                for heights in self.heights.iter_mut().take(4) {
                    heights[hm] = empty;
                }
                let mut y = self.min_y + self.height - 1;
                while y >= self.min_y {
                    let s = self.blocks[self.index(lx, y, lz)] as u32;
                    let f = flags.get(s);
                    if f & crate::blocks::AIR == 0 {
                        let bm = f & crate::blocks::BLOCKS_MOTION != 0;
                        let mb = bm || f & crate::blocks::FLUID != 0;
                        let hits = [true, bm, mb, mb && f & crate::blocks::LEAVES == 0];
                        for k in 0..4 {
                            if hits[k] && !found[k] {
                                self.heights[k][hm] = y as i16;
                                found[k] = true;
                            }
                        }
                        if found.iter().all(|f| *f) {
                            break;
                        }
                    }
                    y -= 1;
                }
            }
        }
    }

    /// The worldgen heightmaps (WORLD_SURFACE_WG, OCEAN_FLOOR_WG): highest y per column
    /// (`x * 16 + z`), `min_y - 1` when empty. Saved with chunks that are not finished yet.
    pub fn wg_heightmaps(&self) -> &[[i16; 256]; 2] {
        &self.wg_heights
    }

    /// Restores the worldgen heightmaps of a chunk read back from disk.
    pub fn set_wg_heightmaps(&mut self, maps: [[i16; 256]; 2]) {
        self.wg_heights = maps;
    }

    /// Freezes the worldgen heightmaps (done after carving).
    pub fn snapshot_wg_heightmaps(&mut self) {
        self.wg_heights[0] = self.heights[SURFACE];
        self.wg_heights[1] = self.heights[OCEAN];
    }

    /// The first free y above the heightmap at a column (`getHeight + 1`).
    pub fn top_y(&self, heightmap: Heightmap, x: i32, z: i32) -> i32 {
        let hm = (((x & 15) * 16) + (z & 15)) as usize;
        let h = match heightmap {
            Heightmap::WorldSurfaceWg => self.wg_heights[0][hm],
            Heightmap::OceanFloorWg => self.wg_heights[1][hm],
            Heightmap::WorldSurface => self.heights[SURFACE][hm],
            Heightmap::OceanFloor => self.heights[OCEAN][hm],
            Heightmap::MotionBlocking => self.heights[MOTION][hm],
            Heightmap::MotionBlockingNoLeaves => self.heights[MOTION_NO_LEAVES][hm],
        };
        h as i32 + 1
    }

    #[inline]
    fn biome_index(&self, qx: i32, qy: i32, qz: i32) -> usize {
        (((qy - (self.min_y >> 2)) as usize) << 4) | (((qz & 3) as usize) << 2) | (qx & 3) as usize
    }

    /// The biome at a quart (4x4x4 cell) position; y is clamped to the chunk.
    pub fn biome(&self, qx: i32, qy: i32, qz: i32) -> u8 {
        let min = self.min_y >> 2;
        let max = min + (self.height >> 2) - 1;
        self.biomes[self.biome_index(qx, qy.clamp(min, max), qz)]
    }

    pub fn set_biome(&mut self, qx: i32, qy: i32, qz: i32, biome: u8) {
        let i = self.biome_index(qx, qy, qz);
        self.biomes[i] = biome;
    }

    /// One 16x16x16 section's block states (`y << 8 | z << 4 | x`).
    pub fn section_blocks(&self, section: usize) -> &[u16] {
        &self.blocks[section << 12..(section + 1) << 12]
    }

    /// One section's biomes (`y << 4 | z << 2 | x` in cells).
    pub fn section_biomes(&self, section: usize) -> &[u8] {
        &self.biomes[section << 6..(section + 1) << 6]
    }
}
