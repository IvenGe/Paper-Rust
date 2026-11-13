//! Biomes: the registry data the generator needs, the biome sources and the fuzzy biome
//! lookup used for surfaces and features.
//!
//! Derived from `pumpkin-world/src/biome/{mod,end}.rs` and
//! `pumpkin-world/src/generation/biome.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

pub mod climate;

use crate::data::{DataReader, JsonExt};
use crate::router::ClimateSampler;
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `Biome.TemperatureModifier`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemperatureModifier {
    None,
    Frozen,
}

/// The parts of a `worldgen/biome` entry that generation reads.
#[derive(Clone, Debug)]
pub struct BiomeInfo {
    pub name: String,
    pub temperature: f32,
    pub temperature_modifier: TemperatureModifier,
    pub has_precipitation: bool,
    pub carvers: Vec<String>,
    /// Placed feature ids per decoration step.
    pub features: Vec<Vec<String>>,
}

impl BiomeInfo {
    /// `Biome.getHeightAdjustedTemperature`.
    pub fn temperature_at(&self, x: i32, y: i32, z: i32, sea_level: i32) -> f32 {
        climate::height_adjusted_temperature(self.temperature, self.temperature_modifier, x, y, z, sea_level)
    }

    /// `Biome.coldEnoughToSnow`.
    pub fn cold_enough_to_snow(&self, x: i32, y: i32, z: i32, sea_level: i32) -> bool {
        !self.warm_enough_to_rain(x, y, z, sea_level)
    }

    /// `Biome.warmEnoughToRain`.
    pub fn warm_enough_to_rain(&self, x: i32, y: i32, z: i32, sea_level: i32) -> bool {
        self.temperature_at(x, y, z, sea_level) >= 0.15
    }
}

/// All biomes by network id (the order of the synchronised `worldgen/biome` registry).
pub struct Biomes {
    pub list: Vec<BiomeInfo>,
    by_name: HashMap<String, u8>,
}

impl Biomes {
    pub fn load(data: &DataReader) -> Result<Self> {
        let names = paper_data::synced::entries("minecraft:worldgen/biome").ok_or_else(|| anyhow!("paper-data has no biome registry"))?;
        let mut list = Vec::with_capacity(names.len());
        let mut by_name = HashMap::new();
        for (i, name) in names.iter().enumerate() {
            let json = data.worldgen("biome", name).with_context(|| format!("biome {name}"))?;
            let carvers = match json.get("carvers") {
                Some(Value::Array(a)) => a.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect(),
                Some(Value::String(s)) => vec![s.clone()],
                _ => Vec::new(),
            };
            let features = json
                .get("features")
                .and_then(Value::as_array)
                .map(|steps| {
                    steps
                        .iter()
                        .map(|s| match s {
                            Value::Array(a) => a.iter().filter_map(|f| f.as_str().map(str::to_owned)).collect(),
                            Value::String(t) => vec![t.clone()],
                            _ => Vec::new(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let modifier = match json.get("temperature_modifier").and_then(Value::as_str) {
                Some("frozen") => TemperatureModifier::Frozen,
                _ => TemperatureModifier::None,
            };
            list.push(BiomeInfo {
                name: (*name).to_owned(),
                temperature: json.f64_or("temperature", 0.5) as f32,
                temperature_modifier: modifier,
                has_precipitation: json.bool_or("has_precipitation", true),
                carvers,
                features,
            });
            by_name.insert((*name).to_owned(), i as u8);
        }
        Ok(Self { list, by_name })
    }

    pub fn id(&self, name: &str) -> Option<u8> {
        self.by_name.get(&crate::data::full_id(name)).copied()
    }

    pub fn get(&self, id: u8) -> &BiomeInfo {
        &self.list[id as usize]
    }
}

#[derive(Clone, Copy, Default)]
struct FlatNode {
    min: [i32; 7],
    max: [i32; 7],
    /// Index of the first child for branches, `!leaf` for leaves.
    child_or_leaf: i32,
    len: u32,
}

impl FlatNode {
    #[inline]
    fn distance(&self, p: &[i32; 7]) -> i64 {
        let mut sum = 0i64;
        for i in 0..7 {
            let d = (p[i] - self.max[i]).max(self.min[i] - p[i]).max(0) as i64;
            sum += d * d;
        }
        sum
    }
}

/// A multi-noise parameter R-tree, flattened for fast searching.
pub struct BiomeTree {
    id: usize,
    nodes: Vec<FlatNode>,
    leaves: Vec<u8>,
    root_len: usize,
}

#[derive(Clone, Copy)]
struct LastLeaf {
    tree: usize,
    node: usize,
    point: [i32; 7],
    biome: u8,
}

thread_local! {
    /// Vanilla keeps the last search result per thread and starts the next search from it.
    static LAST_LEAF: RefCell<Vec<LastLeaf>> = const { RefCell::new(Vec::new()) };
}

static TREE_IDS: AtomicUsize = AtomicUsize::new(0);

impl BiomeTree {
    fn parse_params(v: &Value) -> Result<([i32; 7], [i32; 7])> {
        let params = v.field("parameters")?.as_array().ok_or_else(|| anyhow!("bad parameters"))?;
        if params.len() != 7 {
            return Err(anyhow!("expected 7 parameters"));
        }
        let mut min = [0; 7];
        let mut max = [0; 7];
        for (i, p) in params.iter().enumerate() {
            min[i] = p.i64_or("min", 0) as i32;
            max[i] = p.i64_or("max", 0) as i32;
        }
        Ok((min, max))
    }

    fn build(children: &[Value], nodes: &mut Vec<FlatNode>, leaves: &mut Vec<u8>, biomes: &Biomes) -> Result<usize> {
        let start = nodes.len();
        nodes.resize(start + children.len(), FlatNode::default());
        for (i, child) in children.iter().enumerate() {
            let (min, max) = Self::parse_params(child)?;
            if child.str_field("_type")? == "leaf" {
                let name = child.str_field("biome")?;
                let biome = biomes.id(name).ok_or_else(|| anyhow!("unknown biome {name} in biome tree"))?;
                let leaf = leaves.len();
                leaves.push(biome);
                nodes[start + i] = FlatNode { min, max, child_or_leaf: !(leaf as i32), len: 0 };
            } else {
                let sub = child.field("subTree")?.as_array().ok_or_else(|| anyhow!("bad subTree"))?;
                let first = Self::build(sub, nodes, leaves, biomes)?;
                nodes[start + i] = FlatNode { min, max, child_or_leaf: first as i32, len: sub.len() as u32 };
            }
        }
        Ok(start)
    }

    pub fn from_json(root: &Value, biomes: &Biomes) -> Result<Self> {
        let children = root.field("subTree")?.as_array().ok_or_else(|| anyhow!("bad biome tree root"))?;
        let mut nodes = Vec::new();
        let mut leaves = Vec::new();
        Self::build(children, &mut nodes, &mut leaves, biomes)?;
        Ok(Self { id: TREE_IDS.fetch_add(1, Ordering::Relaxed), nodes, leaves, root_len: children.len() })
    }

    /// The trees vanilla builds in code for the `overworld` and `nether` presets.
    pub fn preset(name: &str, biomes: &Biomes) -> Result<Self> {
        static TREES: &[u8] = include_bytes!("../../../../data/26.3/worldgen/multi_noise_biome_tree.json.gz");
        let mut json = Vec::new();
        flate2::read::GzDecoder::new(TREES).read_to_end(&mut json)?;
        let all: Value = serde_json::from_slice(&json)?;
        let key = name.strip_prefix("minecraft:").unwrap_or(name);
        let root = all.get(key).ok_or_else(|| anyhow!("no biome tree for preset {name}"))?;
        Self::from_json(root, biomes)
    }

    fn search(&self, start: usize, len: usize, p: &[i32; 7], best: &mut i64, best_node: &mut usize) {
        for i in start..start + len {
            let node = &self.nodes[i];
            let d = node.distance(p);
            if d >= *best {
                continue;
            }
            if node.child_or_leaf < 0 {
                *best = d;
                *best_node = i;
                if d == 0 {
                    return;
                }
            } else {
                self.search(node.child_or_leaf as usize, node.len as usize, p, best, best_node);
                if *best == 0 {
                    return;
                }
            }
        }
    }

    /// The biome closest to a climate point.
    pub fn get(&self, point: [i64; 7]) -> u8 {
        let p: [i32; 7] = point.map(|v| v.clamp(-1_000_000_000, 1_000_000_000) as i32);
        LAST_LEAF.with_borrow_mut(|cache| {
            let slot = cache.iter().position(|l| l.tree == self.id);
            let (mut best, mut best_node) = (i64::MAX, 0);
            if let Some(s) = slot {
                let last = cache[s];
                if last.point == p {
                    return last.biome;
                }
                let d = self.nodes[last.node].distance(&p);
                if d == 0 {
                    cache[s].point = p;
                    return last.biome;
                }
                best = d;
                best_node = last.node;
            }
            self.search(0, self.root_len, &p, &mut best, &mut best_node);
            let biome = self.leaves[(!self.nodes[best_node].child_or_leaf) as usize];
            let entry = LastLeaf { tree: self.id, node: best_node, point: p, biome };
            match slot {
                Some(s) => cache[s] = entry,
                None => cache.push(entry),
            }
            biome
        })
    }
}

/// Where a dimension's biomes come from.
pub enum BiomeSource {
    MultiNoise(BiomeTree),
    End { the_end: u8, highlands: u8, midlands: u8, small_islands: u8, barrens: u8 },
    Fixed(u8),
}

impl BiomeSource {
    pub fn from_json(v: &Value, data: &DataReader, biomes: &Biomes) -> Result<Self> {
        let ty = v.str_field("type")?;
        Ok(match ty.strip_prefix("minecraft:").unwrap_or(ty) {
            "multi_noise" => {
                let preset = match v.get("preset") {
                    Some(Value::String(p)) => p.clone(),
                    _ => return Err(anyhow!("inline multi-noise biome lists are not supported")),
                };
                // The parameter list entry names the code-built preset.
                let list = data.worldgen("multi_noise_biome_source_parameter_list", &preset)?;
                let preset = list.get("preset").and_then(Value::as_str).unwrap_or(&preset).to_owned();
                BiomeSource::MultiNoise(BiomeTree::preset(&preset, biomes)?)
            }
            "the_end" => {
                let id = |n: &str| biomes.id(n).ok_or_else(|| anyhow!("missing biome {n}"));
                BiomeSource::End {
                    the_end: id("minecraft:the_end")?,
                    highlands: id("minecraft:end_highlands")?,
                    midlands: id("minecraft:end_midlands")?,
                    small_islands: id("minecraft:small_end_islands")?,
                    barrens: id("minecraft:end_barrens")?,
                }
            }
            "fixed" => {
                let name = v.str_field("biome")?;
                BiomeSource::Fixed(biomes.id(name).ok_or_else(|| anyhow!("unknown biome {name}"))?)
            }
            other => return Err(anyhow!("unsupported biome source {other}")),
        })
    }

    /// Every biome the source can produce.
    pub fn possible_biomes(&self) -> Vec<u8> {
        match self {
            BiomeSource::MultiNoise(tree) => {
                let mut v = tree.leaves.clone();
                v.sort_unstable();
                v.dedup();
                v
            }
            BiomeSource::End { the_end, highlands, midlands, small_islands, barrens } => vec![*the_end, *highlands, *midlands, *small_islands, *barrens],
            BiomeSource::Fixed(b) => vec![*b],
        }
    }

    /// The biome at a quart position.
    pub fn biome(&self, qx: i32, qy: i32, qz: i32, climate: &mut ClimateSampler) -> u8 {
        match self {
            BiomeSource::MultiNoise(tree) => tree.get(climate.sample(qx, qy, qz).to_array()),
            BiomeSource::End { the_end, highlands, midlands, small_islands, barrens } => {
                let (x, y, z) = (qx << 2, qy << 2, qz << 2);
                let (sx, sz) = (x >> 4, z >> 4);
                if (sx as i64) * (sx as i64) + (sz as i64) * (sz as i64) <= 4096 {
                    return *the_end;
                }
                let nx = sx.wrapping_mul(2).wrapping_add(1).wrapping_mul(8);
                let nz = sz.wrapping_mul(2).wrapping_add(1).wrapping_mul(8);
                let e = climate.erosion(nx, y, nz);
                if e > 0.25 {
                    *highlands
                } else if e >= -0.0625 {
                    *midlands
                } else if e < -0.21875 {
                    *small_islands
                } else {
                    *barrens
                }
            }
            BiomeSource::Fixed(b) => *b,
        }
    }
}

/// `BiomeManager.obfuscateSeed`: the first 8 bytes of SHA-256 of the seed (little endian).
pub fn obfuscate_seed(seed: i64) -> i64 {
    let digest = Sha256::digest(seed.to_le_bytes());
    i64::from_le_bytes(digest[..8].try_into().expect("sha256 has 32 bytes"))
}

#[inline]
const fn salt_mix(seed: i64, salt: i64) -> i64 {
    seed.wrapping_mul(seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407)).wrapping_add(salt)
}

#[inline]
fn fiddle(seed: i64) -> f64 {
    let d = ((seed >> 24) & 1023) as f64 / 1024.0;
    (d - 0.5) * 0.9
}

fn corner_offset(seed: i64, x: i32, y: i32, z: i32) -> [f64; 3] {
    let mut m = salt_mix(seed, x as i64);
    m = salt_mix(m, y as i64);
    m = salt_mix(m, z as i64);
    m = salt_mix(m, x as i64);
    m = salt_mix(m, y as i64);
    m = salt_mix(m, z as i64);
    let ox = fiddle(m);
    m = salt_mix(m, seed);
    let oy = fiddle(m);
    m = salt_mix(m, seed);
    let oz = fiddle(m);
    [ox, oy, oz]
}

/// `BiomeManager.getBiome`'s fuzzy lookup: the quart position whose biome a block uses.
/// The quart y is clamped to the dimension.
pub fn fuzzy_quart(seed: i64, x: i32, y: i32, z: i32, min_y: i32, height: i32) -> (i32, i32, i32) {
    FuzzyZoom::new(seed, min_y, height).quart(x, y, z)
}

/// A cell's quart position and the fiddled offsets of its eight corners.
type CachedCorners = ((i32, i32, i32), [[f64; 3]; 8]);

/// The fuzzy lookup with the corner offsets of the last cell kept (columns are scanned
/// top-down, so consecutive blocks mostly share a cell or its upper neighbour).
pub struct FuzzyZoom {
    seed: i64,
    min_y: i32,
    height: i32,
    last: Option<CachedCorners>,
}

impl FuzzyZoom {
    pub fn new(seed: i64, min_y: i32, height: i32) -> Self {
        Self { seed, min_y, height, last: None }
    }

    fn offsets(&mut self, bx: i32, by: i32, bz: i32) -> [[f64; 3]; 8] {
        let seed = self.seed;
        let mut out = [[0.0; 3]; 8];
        match self.last {
            Some((key, prev)) if key == (bx, by, bz) => return prev,
            Some((key, prev)) if key == (bx, by + 1, bz) => {
                for (p, o) in out.iter_mut().enumerate() {
                    *o = if p & 2 != 0 {
                        prev[p & !2]
                    } else {
                        corner_offset(seed, if p & 4 == 0 { bx } else { bx + 1 }, by, if p & 1 == 0 { bz } else { bz + 1 })
                    };
                }
            }
            _ => {
                for (p, o) in out.iter_mut().enumerate() {
                    *o = corner_offset(seed, if p & 4 == 0 { bx } else { bx + 1 }, if p & 2 == 0 { by } else { by + 1 }, if p & 1 == 0 { bz } else { bz + 1 });
                }
            }
        }
        self.last = Some(((bx, by, bz), out));
        out
    }

    pub fn quart(&mut self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        let (ox, oy, oz) = (x - 2, y - 2, z - 2);
        let (bx, by, bz) = (ox >> 2, oy >> 2, oz >> 2);
        let fx = (ox & 3) as f64 * 0.25;
        let fy = (oy & 3) as f64 * 0.25;
        let fz = (oz & 3) as f64 * 0.25;
        let offsets = self.offsets(bx, by, bz);
        let mut best = 0;
        let mut best_score = f64::INFINITY;
        for (p, o) in offsets.iter().enumerate() {
            let sx = if p & 4 == 0 { fx } else { fx - 1.0 };
            let sy = if p & 2 == 0 { fy } else { fy - 1.0 };
            let sz = if p & 1 == 0 { fz } else { fz - 1.0 };
            let score = (sz + o[2]) * (sz + o[2]) + (sy + o[1]) * (sy + o[1]) + (sx + o[0]) * (sx + o[0]);
            if best_score > score {
                best_score = score;
                best = p;
            }
        }
        let qx = if best & 4 == 0 { bx } else { bx + 1 };
        let qy = if best & 2 == 0 { by } else { by + 1 };
        let qz = if best & 1 == 0 { bz } else { bz + 1 };
        let bottom = self.min_y >> 2;
        let top = bottom + (self.height >> 2) - 1;
        (qx, qy.clamp(bottom, top), qz)
    }
}
