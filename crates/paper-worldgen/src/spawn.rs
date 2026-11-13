//! The initial world spawn (`MinecraftServer.setInitialSpawn`).
//!
//! Written from vanilla 26.3: `NoiseBasedChunkGenerator.getOrigin` with `NoiseSpawnFinder`
//! and `SpawnTargetPoint` pick a chunk from the climate, then `setInitialSpawn` spirals
//! over the chunks around it and takes the first `PlayerSpawnFinder.getSpawnPosInChunk`
//! hit. The chunks examined are generated and decorated here (without structures).

use crate::chunk::{Heightmap, ProtoChunk};
use crate::data::JsonExt;
use crate::feature::RegionAccess;
use crate::generator::{Dimension, WorldGenerator};
use crate::router::quantize;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// One `SpawnTargetPoint`: (climate parameter index, quantized min, quantized max).
pub(crate) type SpawnTarget = Vec<(usize, i64, i64)>;

/// Parses the noise settings' `spawn_target` list. Its density functions must be climate
/// parameters of the noise router (as they are in vanilla).
pub(crate) fn parse_spawn_targets(settings: &Value) -> Result<Vec<SpawnTarget>> {
    let Some(list) = settings.get("spawn_target").and_then(Value::as_array) else { return Ok(Vec::new()) };
    let router = settings.field("noise_router")?;
    let fields = ["temperature", "vegetation", "continents", "erosion", "depth", "ridges"];
    let mut out = Vec::new();
    for point in list {
        let map = point.as_object().ok_or_else(|| anyhow!("spawn target point is not an object"))?;
        let mut target = Vec::new();
        for (function, range) in map {
            let index = fields
                .iter()
                .position(|f| router.get(*f).and_then(Value::as_str).map(crate::data::full_id) == Some(crate::data::full_id(function)))
                .ok_or_else(|| anyhow!("spawn target {function} is not a climate parameter of the noise router"))?;
            let (min, max) = match range {
                Value::Number(n) => {
                    let v = n.as_f64().unwrap_or(0.0) as f32;
                    (v, v)
                }
                Value::Array(a) if a.len() == 2 => (a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32),
                other => bail!("bad spawn target parameter {other}"),
            };
            target.push((index, quantize(min), quantize(max)));
        }
        out.push(target);
    }
    Ok(out)
}

/// `Climate.Parameter.distance`.
fn distance(min: i64, max: i64, value: i64) -> i64 {
    let above = value - max;
    let below = min - value;
    if above > 0 {
        above
    } else {
        below.max(0)
    }
}

/// The 3x3 chunks around a center, out of a larger set.
struct Window<'a> {
    chunks: &'a mut HashMap<(i32, i32), ProtoChunk>,
    center: (i32, i32),
}

impl Window<'_> {
    fn near(&self, x: i32, z: i32) -> bool {
        (x - self.center.0).abs() <= 1 && (z - self.center.1).abs() <= 1
    }
}

impl RegionAccess for Window<'_> {
    fn chunk(&self, x: i32, z: i32) -> Option<&ProtoChunk> {
        if self.near(x, z) {
            self.chunks.get(&(x, z))
        } else {
            None
        }
    }

    fn chunk_mut(&mut self, x: i32, z: i32) -> Option<&mut ProtoChunk> {
        if self.near(x, z) {
            self.chunks.get_mut(&(x, z))
        } else {
            None
        }
    }
}

/// Chunks generated on demand; a chunk counts as finished once it and its eight
/// neighbours have been decorated.
struct Chunks<'g> {
    gen: &'g WorldGenerator,
    terrain: HashMap<(i32, i32), ProtoChunk>,
    decorated: HashSet<(i32, i32)>,
}

impl Chunks<'_> {
    fn finished(&mut self, cx: i32, cz: i32) -> &ProtoChunk {
        for z in cz - 2..=cz + 2 {
            for x in cx - 2..=cx + 2 {
                if !self.terrain.contains_key(&(x, z)) {
                    self.terrain.insert((x, z), self.gen.generate_terrain(x, z));
                }
            }
        }
        for z in cz - 1..=cz + 1 {
            for x in cx - 1..=cx + 1 {
                if self.decorated.insert((x, z)) {
                    let mut window = Window { chunks: &mut self.terrain, center: (x, z) };
                    self.gen.decorate(x, z, &mut window);
                }
            }
        }
        &self.terrain[&(cx, cz)]
    }
}

impl WorldGenerator {
    /// `ChunkGenerator.getOrigin`: the chunk whose climate best matches the noise settings'
    /// spawn targets, near the world origin (chunk (0, 0) without targets).
    pub fn spawn_origin_chunk(&self) -> (i32, i32) {
        if self.spawn_targets.is_empty() {
            return (0, 0);
        }
        let mut climate = self.climate_sampler();
        let mut fitness = |x: i32, z: i32| -> i64 {
            let p = climate.sample(x >> 2, 0, z >> 2).to_array();
            let min = self
                .spawn_targets
                .iter()
                .map(|t| t.iter().map(|&(i, lo, hi)| distance(lo, hi, p[i]).pow(2)).sum::<i64>())
                .min()
                .unwrap_or(i64::MAX);
            min.wrapping_mul(2048 * 2048).wrapping_add((x as i64).pow(2) + (z as i64).pow(2))
        };
        let mut best = ((0, 0), fitness(0, 0));
        for (max_radius, increment) in [(2048.0f32, 512.0f32), (512.0, 32.0)] {
            let mut angle = 0.0f32;
            let mut radius = increment;
            let origin = best.0;
            while radius <= max_radius {
                let x = origin.0 + ((angle as f64).sin() * radius as f64) as i32;
                let z = origin.1 + ((angle as f64).cos() * radius as f64) as i32;
                let f = fitness(x, z);
                if f < best.1 {
                    best = ((x, z), f);
                }
                angle += increment / radius;
                if angle as f64 > std::f64::consts::TAU {
                    angle = 0.0;
                    radius += increment;
                }
            }
        }
        (best.0 .0 >> 4, best.0 .1 >> 4)
    }

    /// The world spawn vanilla would choose for a new world with this seed
    /// (`MinecraftServer.setInitialSpawn`), as the block position a player stands in.
    ///
    /// Chunks around the spawn are generated and decorated to find it (typically a few
    /// hundred milliseconds). Structures are not generated, so a structure at the spawn can
    /// make vanilla choose differently; and the top-face test approximates
    /// `Block.isFaceFull` of the collision shape with full cubes and sturdy top faces.
    /// The End returns its fixed arrival point.
    pub fn find_spawn(&self) -> (i32, i32, i32) {
        if self.dimension == Dimension::End {
            // `ServerLevel.END_SPAWN_POINT`.
            return (100, 50, 0);
        }
        let (ox, oz) = self.spawn_origin_chunk();
        // `ChunkGenerator.getSpawnHeight` is 64 for every generator in 26.3.
        let spawn_height = 64;
        let fallback = (ox * 16 + 8, spawn_height, oz * 16 + 8);
        let mut chunks = Chunks { gen: self, terrain: HashMap::new(), decorated: HashSet::new() };
        let (mut dx, mut dz, mut x_step, mut z_step) = (0i32, 0i32, 0i32, -1i32);
        for _ in 0..11 * 11 {
            if (-5..=5).contains(&dx) && (-5..=5).contains(&dz) {
                if let Some(p) = self.spawn_pos_in_chunk(&mut chunks, ox + dx, oz + dz, spawn_height) {
                    return p;
                }
            }
            if dx == dz || (dx < 0 && dx == -dz) || (dx > 0 && dx == 1 - dz) {
                let swap = x_step;
                x_step = -z_step;
                z_step = swap;
            }
            dx += x_step;
            dz += z_step;
        }
        fallback
    }

    /// `PlayerSpawnFinder.getSpawnPosInChunk`.
    fn spawn_pos_in_chunk(&self, chunks: &mut Chunks, cx: i32, cz: i32, spawn_height: i32) -> Option<(i32, i32, i32)> {
        let has_ceiling = self.has_ceiling;
        let blocks = &self.decoration.blocks;
        let chunk = chunks.finished(cx, cz);
        for x in cx * 16..cx * 16 + 16 {
            for z in cz * 16..cz * 16 + 16 {
                // `ChunkAccess.getHeight` is the top block, one below the heightmap.
                let top = if has_ceiling { spawn_height } else { chunk.top_y(Heightmap::MotionBlocking, x, z) - 1 };
                if top < self.min_y {
                    continue;
                }
                let surface = chunk.top_y(Heightmap::WorldSurface, x, z) - 1;
                if surface <= top && surface > chunk.top_y(Heightmap::OceanFloor, x, z) - 1 {
                    continue;
                }
                let mut y = top + 1;
                while y >= self.min_y {
                    let state = if y < self.min_y + self.height { chunk.get(x, y, z) } else { blocks.known.void_air };
                    if blocks.fluid(state) != crate::feature::blockinfo::Fluid::Empty {
                        break;
                    }
                    if blocks.is_full_cube(state) || blocks.sturdy(state, crate::feature::world::Dir::Up) {
                        return Some((x, y + 1, z));
                    }
                    y -= 1;
                }
            }
        }
        None
    }
}
