//! Lighting.
//!
//! Two entry points:
//!
//! * [`compute_chunk_light`] lights one freshly generated or loaded chunk from the terrain
//!   of its 3x3 neighbourhood. It is a pure function and runs on worker threads. Because
//!   light travels at most 15 blocks and every neighbour is 16 blocks wide, the result for
//!   the centre chunk is exact.
//! * [`update_after_block_change`] repairs sky and block light around a single changed
//!   block with the usual remove-then-repropagate flood fill. It runs on the main thread
//!   and reports which light sections changed so they can be resent.
//!
//! Rules (matching vanilla's observable behaviour for full-cube opacity):
//! light entering a block is the source level minus `max(1, opacity)`; sky light of level
//! 15 travelling straight down through a fully transparent block stays 15. Directional
//! occlusion by partial shapes (slabs, stairs) is not modelled.

use super::chunk::{BlockStorage, Chunk, ChunkPos, LightSection, Section};
use paper_data::blocks;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// The terrain of one chunk, as needed for lighting.
#[derive(Clone)]
pub struct Terrain {
    pub sections: Vec<Arc<Section>>,
}

impl Terrain {
    pub fn of(chunk: &Chunk) -> Self {
        Self { sections: chunk.sections.clone() }
    }
}

const W: usize = 48;
const DIRS: [(i32, i32, i32); 6] = [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1), (0, 1, 0), (0, -1, 0)];

#[inline]
fn widx(x: usize, y: usize, z: usize) -> usize {
    (y * W + z) * W + x
}

/// Computes sky and block light for the centre chunk of a 3x3 neighbourhood.
///
/// `grid[dz + 1][dx + 1]` holds the terrain of the chunk at offset `(dx, dz)`; missing
/// neighbours are treated as empty. Returns `(sky_light, block_light)`, each with one entry
/// per section plus one below and one above the world.
pub fn compute_chunk_light(grid: &[[Option<Terrain>; 3]; 3], has_skylight: bool) -> (Vec<Arc<LightSection>>, Vec<Arc<LightSection>>) {
    let center = grid[1][1].as_ref().expect("centre chunk terrain is required");
    let section_count = center.sections.len();
    let light_sections = section_count + 2;

    // Highest section with any block anywhere in the window bounds the work: above it the
    // sky is 15 and there is no block light.
    let mut top_section = None;
    for row in grid {
        for terrain in row.iter().flatten() {
            if let Some(i) = terrain.sections.iter().rposition(|s| !s.is_empty()) {
                top_section = top_section.max(Some(i));
            }
        }
    }
    let Some(top_section) = top_section else {
        // Nothing but air: full sky light everywhere.
        let sky = if has_skylight { 15 } else { 0 };
        return (
            (0..light_sections).map(|_| Arc::new(LightSection::Uniform(sky))).collect(),
            (0..light_sections).map(|_| Arc::new(LightSection::Uniform(0))).collect(),
        );
    };
    // Window height in blocks: the section below the world, every section up to the
    // highest occupied one, and one section of headroom for light to flow over the top.
    let wh = ((top_section + 3) * 16).min(light_sections * 16);

    let mut opacity = vec![0u8; W * W * wh];
    let mut emitters: Vec<(u32, u8)> = Vec::new();
    for (gz, row) in grid.iter().enumerate() {
        for (gx, terrain) in row.iter().enumerate() {
            let Some(terrain) = terrain else { continue };
            for (si, section) in terrain.sections.iter().enumerate() {
                if section.is_empty() {
                    continue;
                }
                let base_y = (si + 1) * 16;
                if base_y >= wh {
                    continue;
                }
                match &section.blocks {
                    BlockStorage::Uniform(state) => {
                        let state = *state as u32;
                        let (op, lum) = (blocks::opacity(state), blocks::luminance(state));
                        if op == 0 && lum == 0 {
                            continue;
                        }
                        for y in 0..16 {
                            for z in 0..16 {
                                let start = widx(gx * 16, base_y + y, gz * 16 + z);
                                opacity[start..start + 16].fill(op);
                                if lum > 0 {
                                    emitters.extend((0..16).map(|x| ((start + x) as u32, lum)));
                                }
                            }
                        }
                    }
                    BlockStorage::Dense(data) => {
                        for y in 0..16 {
                            for z in 0..16 {
                                let start = widx(gx * 16, base_y + y, gz * 16 + z);
                                let row = &data[(y << 8) | (z << 4)..][..16];
                                for (x, state) in row.iter().enumerate() {
                                    let state = *state as u32;
                                    if state == blocks::AIR {
                                        continue;
                                    }
                                    opacity[start + x] = blocks::opacity(state);
                                    let lum = blocks::luminance(state);
                                    if lum > 0 {
                                        emitters.push(((start + x) as u32, lum));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let flood = |light: &mut [u8], queue: &mut VecDeque<u32>| {
        while let Some(i) = queue.pop_front() {
            let i = i as usize;
            let level = light[i];
            if level <= 1 {
                continue;
            }
            let (x, rest) = (i % W, i / W);
            let (z, y) = (rest % W, rest / W);
            for (dx, dy, dz) in DIRS {
                let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                if nx < 0 || nz < 0 || ny < 0 || nx >= W as i32 || nz >= W as i32 || ny >= wh as i32 {
                    continue;
                }
                let n = widx(nx as usize, ny as usize, nz as usize);
                let op = opacity[n];
                if op >= 15 {
                    continue;
                }
                let next = level.saturating_sub(op.max(1));
                if next > light[n] {
                    light[n] = next;
                    queue.push_back(n as u32);
                }
            }
        }
    };

    // --- sky light ---
    let mut sky = vec![0u8; if has_skylight { W * W * wh } else { 0 }];
    if has_skylight {
        // heights[col] = first y from which the column is open to the sky.
        let mut heights = vec![0u16; W * W];
        for z in 0..W {
            for x in 0..W {
                let mut h = wh;
                while h > 0 && opacity[widx(x, h - 1, z)] == 0 {
                    h -= 1;
                }
                heights[z * W + x] = h as u16;
                for y in h..wh {
                    sky[widx(x, y, z)] = 15;
                }
            }
        }
        let mut queue: VecDeque<u32> = VecDeque::new();
        for z in 0..W {
            for x in 0..W {
                let h = heights[z * W + x] as usize;
                // Light spills sideways into neighbouring columns that are shadowed higher up.
                let mut reach = h;
                for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, nz) = (x as i32 + dx, z as i32 + dz);
                    if nx < 0 || nz < 0 || nx >= W as i32 || nz >= W as i32 {
                        continue;
                    }
                    reach = reach.max(heights[nz as usize * W + nx as usize] as usize);
                }
                for y in h..reach.min(wh) {
                    queue.push_back(widx(x, y, z) as u32);
                }
                // Light enters a translucent block (water, leaves) from directly above.
                if h > 0 && opacity[widx(x, h - 1, z)] < 15 {
                    let below = widx(x, h - 1, z);
                    let level = 15u8.saturating_sub(opacity[below].max(1));
                    if level > sky[below] {
                        sky[below] = level;
                        queue.push_back(below as u32);
                    }
                }
            }
        }
        flood(&mut sky, &mut queue);
    }

    // --- block light ---
    let mut block = Vec::new();
    if !emitters.is_empty() {
        block = vec![0u8; W * W * wh];
        let mut queue: VecDeque<u32> = VecDeque::with_capacity(emitters.len());
        for (i, lum) in &emitters {
            if *lum > block[*i as usize] {
                block[*i as usize] = *lum;
                queue.push_back(*i);
            }
        }
        flood(&mut block, &mut queue);
    }

    // --- extract the centre chunk ---
    let extract = |buffer: &[u8], above: u8| -> Vec<Arc<LightSection>> {
        (0..light_sections)
            .map(|ls| {
                let base_y = ls * 16;
                if buffer.is_empty() {
                    return Arc::new(LightSection::Uniform(0));
                }
                if base_y >= wh {
                    return Arc::new(LightSection::Uniform(above));
                }
                let mut section = LightSection::Uniform(0);
                for y in 0..16 {
                    for z in 0..16 {
                        let start = widx(16, base_y + y, 16 + z);
                        for x in 0..16 {
                            let level = buffer[start + x];
                            if level != 0 {
                                section.set(x, y, z, level);
                            }
                        }
                    }
                }
                section.compact();
                Arc::new(section)
            })
            .collect()
    };
    (extract(&sky, 15), extract(&block, 0))
}

/// Which light array a change belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LightKind {
    Sky,
    Block,
}

/// A light section that changed: chunk, light-section index, and which kind.
pub type ChangedSection = (ChunkPos, usize, LightKind);

struct Access<'a> {
    chunks: &'a mut HashMap<ChunkPos, Chunk>,
    kind: LightKind,
    changed: &'a mut HashSet<ChangedSection>,
}

impl Access<'_> {
    /// Light at a block, or `None` where there is no lit chunk (treated as a wall).
    fn get(&self, x: i32, y: i32, z: i32) -> Option<u8> {
        let chunk = self.chunks.get(&ChunkPos::of_block(x, z)).filter(|c| c.lit)?;
        let rel = y - chunk.min_y + 16;
        if rel < 0 {
            return None;
        }
        let sections = match self.kind {
            LightKind::Sky => &chunk.sky_light,
            LightKind::Block => &chunk.block_light,
        };
        // Cells above the stored range are never read as sources: the topmost stored
        // section lies above the build limit, is always fully sky-lit and never cleared.
        sections.get((rel >> 4) as usize).map(|s| s.get((x & 15) as usize, (rel & 15) as usize, (z & 15) as usize))
    }

    fn set(&mut self, x: i32, y: i32, z: i32, level: u8) {
        let pos = ChunkPos::of_block(x, z);
        let Some(chunk) = self.chunks.get_mut(&pos).filter(|c| c.lit) else { return };
        let rel = y - chunk.min_y + 16;
        if rel < 0 {
            return;
        }
        let sections = match self.kind {
            LightKind::Sky => &mut chunk.sky_light,
            LightKind::Block => &mut chunk.block_light,
        };
        let index = (rel >> 4) as usize;
        let Some(section) = sections.get_mut(index) else { return };
        Arc::make_mut(section).set((x & 15) as usize, (rel & 15) as usize, (z & 15) as usize, level);
        chunk.dirty = true;
        chunk.revision += 1;
        self.changed.insert((pos, index, self.kind));
    }

    fn opacity(&self, x: i32, y: i32, z: i32) -> u8 {
        match self.chunks.get(&ChunkPos::of_block(x, z)) {
            Some(chunk) => blocks::opacity(chunk.get_block((x & 15) as usize, y, (z & 15) as usize)),
            None => 15,
        }
    }

    /// Clears light that depended on the cell at `(x, y, z)` (whose level was `level`) and
    /// returns the cells bordering the cleared region, from which light must flow back.
    fn remove(&mut self, x: i32, y: i32, z: i32, level: u8) -> Vec<(i32, i32, i32)> {
        let mut border = Vec::new();
        let mut queue: VecDeque<(i32, i32, i32, u8)> = VecDeque::new();
        self.set(x, y, z, 0);
        queue.push_back((x, y, z, level));
        while let Some((x, y, z, level)) = queue.pop_front() {
            for (dx, dy, dz) in DIRS {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                let Some(n) = self.get(nx, ny, nz) else { continue };
                if n == 0 {
                    continue;
                }
                // A neighbour dimmer than us was lit through us. Direct sky light (15) below
                // a cell that lost it is also gone.
                let fed_by_us = n < level || (self.kind == LightKind::Sky && dy == -1 && level == 15 && n == 15);
                if fed_by_us {
                    self.set(nx, ny, nz, 0);
                    queue.push_back((nx, ny, nz, n));
                } else {
                    border.push((nx, ny, nz));
                }
            }
        }
        border
    }

    /// Spreads light outwards from the given cells.
    fn propagate(&mut self, seeds: impl IntoIterator<Item = (i32, i32, i32)>) {
        let mut queue: VecDeque<(i32, i32, i32)> = seeds.into_iter().collect();
        while let Some((x, y, z)) = queue.pop_front() {
            let Some(level) = self.get(x, y, z) else { continue };
            if level <= 1 {
                continue;
            }
            for (dx, dy, dz) in DIRS {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                let Some(current) = self.get(nx, ny, nz) else { continue };
                let op = self.opacity(nx, ny, nz);
                if op >= 15 {
                    continue;
                }
                let next = if self.kind == LightKind::Sky && dy == -1 && level == 15 && op == 0 { 15 } else { level.saturating_sub(op.max(1)) };
                if next > current {
                    self.set(nx, ny, nz, next);
                    queue.push_back((nx, ny, nz));
                }
            }
        }
    }
}

/// Repairs lighting after the block at `(x, y, z)` changed from `old_state` to `new_state`
/// (the chunk must already hold the new state). Returns the light sections that changed.
pub fn update_after_block_change(
    chunks: &mut HashMap<ChunkPos, Chunk>,
    has_skylight: bool,
    x: i32,
    y: i32,
    z: i32,
    old_state: u32,
    new_state: u32,
) -> HashSet<ChangedSection> {
    let mut changed = HashSet::new();
    let (old_op, new_op) = (blocks::opacity(old_state), blocks::opacity(new_state));
    let (old_lum, new_lum) = (blocks::luminance(old_state), blocks::luminance(new_state));
    if old_op == new_op && old_lum == new_lum {
        return changed;
    }
    let neighbours = || DIRS.iter().map(move |(dx, dy, dz)| (x + dx, y + dy, z + dz));

    for kind in [LightKind::Block, LightKind::Sky] {
        if kind == LightKind::Sky && !has_skylight {
            continue;
        }
        let mut access = Access { chunks, kind, changed: &mut changed };
        let Some(level) = access.get(x, y, z) else { continue };
        // Drop whatever light the cell held and everything fed by it, then let light flow
        // back in from the surroundings (and from the cell itself if it now emits).
        let mut seeds: Vec<(i32, i32, i32)> = if level > 0 { access.remove(x, y, z, level) } else { Vec::new() };
        seeds.extend(neighbours());
        if kind == LightKind::Block && new_lum > 0 {
            access.set(x, y, z, new_lum);
            seeds.push((x, y, z));
        }
        access.propagate(seeds);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::{BiomeStorage, Chunk};

    const MIN_Y: i32 = -64;
    const HEIGHT: i32 = 384;

    fn stone() -> u32 {
        blocks::default_state("minecraft:stone").unwrap()
    }

    /// A chunk with stone from the bottom of the world up to and including `top_y`.
    fn slab_chunk(pos: ChunkPos, top_y: i32) -> Chunk {
        let mut chunk = Chunk::empty(pos, MIN_Y, HEIGHT, 0);
        for y in MIN_Y..=top_y {
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set_block(x, y, z, stone());
                }
            }
        }
        chunk
    }

    fn light(chunk: &mut Chunk, neighbours: &[&Chunk]) {
        let mut grid: [[Option<Terrain>; 3]; 3] = Default::default();
        grid[1][1] = Some(Terrain::of(chunk));
        for n in neighbours {
            let (dx, dz) = (n.pos.x - chunk.pos.x, n.pos.z - chunk.pos.z);
            grid[(dz + 1) as usize][(dx + 1) as usize] = Some(Terrain::of(n));
        }
        let (sky, block) = compute_chunk_light(&grid, true);
        chunk.sky_light = sky;
        chunk.block_light = block;
        chunk.lit = true;
    }

    fn world_of(chunks: Vec<Chunk>) -> HashMap<ChunkPos, Chunk> {
        chunks.into_iter().map(|c| (c.pos, c)).collect()
    }

    #[test]
    fn open_sky_and_solid_ground() {
        let mut chunk = slab_chunk(ChunkPos::new(0, 0), 63);
        light(&mut chunk, &[]);
        assert_eq!(chunk.sky_light_at(5, 64, 5), 15);
        assert_eq!(chunk.sky_light_at(5, 300, 5), 15);
        assert_eq!(chunk.sky_light_at(5, 63, 5), 0, "inside stone");
        assert_eq!(chunk.sky_light_at(5, 0, 5), 0);
        assert_eq!(chunk.block_light_at(5, 64, 5), 0);
        // Everything above the terrain compacts to uniform sections.
        assert!(matches!(*chunk.sky_light[chunk.sky_light.len() - 1], LightSection::Uniform(15)));
        assert!(chunk.block_light.iter().all(|s| matches!(**s, LightSection::Uniform(0))));
    }

    #[test]
    fn empty_chunk_is_fully_lit() {
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), MIN_Y, HEIGHT, 0);
        light(&mut chunk, &[]);
        assert!(chunk.sky_light.iter().all(|s| matches!(**s, LightSection::Uniform(15))));
    }

    #[test]
    fn sky_light_attenuates_under_a_roof() {
        // Ground at 63 and a roof at y=70 over the centre chunk and every neighbour except
        // the western one, so sky light can only enter from the west.
        let roofed = |pos: ChunkPos| {
            let mut chunk = slab_chunk(pos, 63);
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set_block(x, 70, z, stone());
                }
            }
            chunk
        };
        let mut chunk = roofed(ChunkPos::new(0, 0));
        let west = slab_chunk(ChunkPos::new(-1, 0), 63);
        let others: Vec<Chunk> = [(-1, -1), (0, -1), (1, -1), (1, 0), (-1, 1), (0, 1), (1, 1)].iter().map(|(x, z)| roofed(ChunkPos::new(*x, *z))).collect();
        let mut neighbours: Vec<&Chunk> = others.iter().collect();
        neighbours.push(&west);
        light(&mut chunk, &neighbours);
        // Just inside the roof edge next to the open western chunk: one step from 15.
        assert_eq!(chunk.sky_light_at(0, 64, 8), 14);
        assert_eq!(chunk.sky_light_at(1, 64, 8), 13);
        assert_eq!(chunk.sky_light_at(13, 64, 8), 1);
        assert_eq!(chunk.sky_light_at(14, 64, 8), 0);
        // On top of the roof the sky is open again.
        assert_eq!(chunk.sky_light_at(8, 71, 8), 15);
        // Missing neighbours count as open air: with only the centre chunk known, light
        // enters from all four sides.
        let mut alone = roofed(ChunkPos::new(0, 0));
        light(&mut alone, &[]);
        assert_eq!(alone.sky_light_at(13, 64, 8), 12);
        assert_eq!(alone.sky_light_at(7, 64, 8), 7);
    }

    #[test]
    fn block_light_crosses_chunk_borders() {
        let glowstone = blocks::default_state("minecraft:glowstone").unwrap();
        assert_eq!(blocks::luminance(glowstone), 15);
        let mut east = slab_chunk(ChunkPos::new(1, 0), 63);
        east.set_block(0, 64, 8, glowstone);
        let mut chunk = slab_chunk(ChunkPos::new(0, 0), 63);
        light(&mut chunk, &[&east]);
        // The emitter sits at x=16 (in the eastern chunk); x=15 is its direct neighbour.
        assert_eq!(chunk.block_light_at(15, 64, 8), 14);
        assert_eq!(chunk.block_light_at(14, 64, 8), 13);
        assert_eq!(chunk.block_light_at(15, 65, 8), 13);
        assert_eq!(chunk.block_light_at(2, 64, 8), 1);
        assert_eq!(chunk.block_light_at(1, 64, 8), 0);
    }

    #[test]
    fn incremental_updates_match_full_recompute() {
        let glowstone = blocks::default_state("minecraft:glowstone").unwrap();
        let mut chunks = Vec::new();
        for z in -1..=1 {
            for x in -1..=1 {
                chunks.push(slab_chunk(ChunkPos::new(x, z), 63));
            }
        }
        let mut world = world_of(chunks);
        let relight_all = |world: &mut HashMap<ChunkPos, Chunk>| {
            let positions: Vec<ChunkPos> = world.keys().copied().collect();
            for pos in positions {
                let mut grid: [[Option<Terrain>; 3]; 3] = Default::default();
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        grid[(dz + 1) as usize][(dx + 1) as usize] = world.get(&ChunkPos::new(pos.x + dx, pos.z + dz)).map(Terrain::of);
                    }
                }
                let (sky, block) = compute_chunk_light(&grid, true);
                let chunk = world.get_mut(&pos).unwrap();
                chunk.sky_light = sky;
                chunk.block_light = block;
                chunk.lit = true;
            }
        };
        relight_all(&mut world);

        // A sequence of edits around a chunk corner: build a roof, place and remove lights,
        // dig a shaft.
        let mut edits: Vec<(i32, i32, i32, u32)> = Vec::new();
        for z in -4..4 {
            for x in -4..4 {
                edits.push((x, 68, z, stone()));
            }
        }
        edits.push((0, 64, 0, glowstone));
        edits.push((-3, 65, 2, glowstone));
        edits.push((0, 64, 0, blocks::AIR));
        edits.push((1, 68, 1, blocks::AIR));
        for y in (40..=63).rev() {
            edits.push((2, y, -2, blocks::AIR));
        }
        edits.push((2, 45, -2, glowstone));
        edits.push((2, 60, -2, stone()));

        for (x, y, z, state) in edits {
            let pos = ChunkPos::of_block(x, z);
            let old = world.get_mut(&pos).unwrap().set_block((x & 15) as usize, y, (z & 15) as usize, state).unwrap();
            update_after_block_change(&mut world, true, x, y, z, old, state);

            // Compare the centre chunk region against a from-scratch computation.
            let mut reference = world.clone();
            relight_all(&mut reference);
            for cz in -1i32..=0 {
                for cx in -1i32..=0 {
                    let pos = ChunkPos::new(cx, cz);
                    let (a, b) = (&world[&pos], &reference[&pos]);
                    for yy in 36..76 {
                        for zz in 0..16 {
                            for xx in 0..16 {
                                assert_eq!(a.sky_light_at(xx, yy, zz), b.sky_light_at(xx, yy, zz), "sky at chunk {pos:?} {xx},{yy},{zz} after edit {x},{y},{z}");
                                assert_eq!(a.block_light_at(xx, yy, zz), b.block_light_at(xx, yy, zz), "block at chunk {pos:?} {xx},{yy},{zz} after edit {x},{y},{z}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unchanged_optics_do_nothing() {
        let mut world = world_of(vec![slab_chunk(ChunkPos::new(0, 0), 63)]);
        let dirt = blocks::default_state("minecraft:dirt").unwrap();
        let changed = update_after_block_change(&mut world, true, 1, 60, 1, stone(), dirt);
        assert!(changed.is_empty());
    }

    #[allow(dead_code)]
    fn _biome_storage_is_used(_: BiomeStorage) {}
}
