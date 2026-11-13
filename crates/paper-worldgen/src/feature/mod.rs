//! The features stage ("decoration"): placed features per biome and generation step.
//!
//! The driver follows `ProtoChunk::generate_features_and_structure` and
//! `generation/feature_order.rs` of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin),
//! GPL-3.0, checked against vanilla 26.3's `ChunkGenerator.applyBiomeDecoration` and
//! `FeatureSorter`. Structures are not generated.

pub mod blockinfo;
pub mod javaset;
mod kinds;
pub mod parse;
pub mod placement;
pub mod predicate;
pub mod provider;
pub mod world;

use crate::biome::Biomes;
use crate::chunk::ProtoChunk;
use crate::data::{full_id, DataReader, JsonExt};
use crate::generator::{Dimension, WorldGenerator};
use crate::random::WorldgenRandom;
use anyhow::Result;
use blockinfo::Blocks;
use parse::Parser;
use placement::PlacedFeature;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use world::{Pos, World};

/// Data a feature attaches to a block it placed (for the caller's block entities).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockEntity {
    /// A mob spawner (dungeons) spawning the given entity type.
    Spawner { entity: &'static str },
    /// A container or brushable block with a loot table and the seed vanilla rolled for it.
    Loot { table: String, seed: i64 },
    /// A bee nest or beehive with the given number of bees (each with its ticks in the hive).
    Beehive { occupants: Vec<i32> },
}

/// The chunks a decoration step may read and write: the chunk being decorated and its
/// eight neighbours, all past the terrain stages.
///
/// Reads of chunks the region does not provide see air; writes to them are dropped.
pub trait RegionAccess {
    fn chunk(&self, chunk_x: i32, chunk_z: i32) -> Option<&ProtoChunk>;
    fn chunk_mut(&mut self, chunk_x: i32, chunk_z: i32) -> Option<&mut ProtoChunk>;
    /// Called for the block entities features create. Ignored by default.
    fn add_block_entity(&mut self, _x: i32, _y: i32, _z: i32, _entity: BlockEntity) {}
}

/// A ready-made [`RegionAccess`]: nine chunks around a center, collecting block entities.
pub struct Region {
    pub center_x: i32,
    pub center_z: i32,
    /// Row-major over `dz` then `dx` (`(dz + 1) * 3 + dx + 1`).
    pub chunks: Vec<ProtoChunk>,
    pub block_entities: Vec<((i32, i32, i32), BlockEntity)>,
}

impl Region {
    /// Takes the nine chunks around `(center_x, center_z)` (in any order).
    pub fn new(center_x: i32, center_z: i32, chunks: Vec<ProtoChunk>) -> Self {
        let mut slots: Vec<Option<ProtoChunk>> = (0..9).map(|_| None).collect();
        for c in chunks {
            let (dx, dz) = (c.chunk_x - center_x, c.chunk_z - center_z);
            assert!(dx.abs() <= 1 && dz.abs() <= 1, "chunk ({}, {}) is not next to ({center_x}, {center_z})", c.chunk_x, c.chunk_z);
            slots[((dz + 1) * 3 + dx + 1) as usize] = Some(c);
        }
        let chunks = slots.into_iter().map(|c| c.expect("a region needs all nine chunks")).collect();
        Self { center_x, center_z, chunks, block_entities: Vec::new() }
    }

    fn index(&self, chunk_x: i32, chunk_z: i32) -> Option<usize> {
        let (dx, dz) = (chunk_x - self.center_x, chunk_z - self.center_z);
        (dx.abs() <= 1 && dz.abs() <= 1).then(|| ((dz + 1) * 3 + dx + 1) as usize)
    }

    pub fn center(&self) -> &ProtoChunk {
        &self.chunks[4]
    }

    pub fn into_chunks(self) -> Vec<ProtoChunk> {
        self.chunks
    }
}

impl RegionAccess for Region {
    fn chunk(&self, chunk_x: i32, chunk_z: i32) -> Option<&ProtoChunk> {
        self.index(chunk_x, chunk_z).map(|i| &self.chunks[i])
    }

    fn chunk_mut(&mut self, chunk_x: i32, chunk_z: i32) -> Option<&mut ProtoChunk> {
        self.index(chunk_x, chunk_z).map(move |i| &mut self.chunks[i])
    }

    fn add_block_entity(&mut self, x: i32, y: i32, z: i32, entity: BlockEntity) {
        self.block_entities.push(((x, y, z), entity));
    }
}

impl RegionAccess for HashMap<(i32, i32), ProtoChunk> {
    fn chunk(&self, chunk_x: i32, chunk_z: i32) -> Option<&ProtoChunk> {
        self.get(&(chunk_x, chunk_z))
    }

    fn chunk_mut(&mut self, chunk_x: i32, chunk_z: i32) -> Option<&mut ProtoChunk> {
        self.get_mut(&(chunk_x, chunk_z))
    }
}

/// A configured feature's placement logic.
pub(crate) trait Place: Send + Sync {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool;
}

/// A configured feature (`worldgen/feature`).
pub struct Feature {
    pub(crate) kind: String,
    imp: Box<dyn Place>,
}

impl Feature {
    pub(crate) fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        self.imp.place(w, r, pos)
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }
}

/// Features vanilla 26.3 has no data-driven equivalent for are never generated here.
struct NoOp;

impl Place for NoOp {
    fn place(&self, _w: &mut World, _r: &mut WorldgenRandom, _pos: Pos) -> bool {
        false
    }
}

impl Parser<'_, '_> {
    fn feature_inline(&mut self, v: &Value) -> Result<Feature> {
        let t = v.str_field("type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t).to_owned();
        // Older data wraps the settings in "config".
        let config = match v.get("config") {
            Some(c) if c.is_object() => c,
            _ => v,
        };
        let imp = match kinds::parse(self, &t, config)? {
            Some(imp) => imp,
            None => {
                if !self.unsupported.contains(&t) {
                    self.unsupported.push(t.clone());
                }
                Box::new(NoOp)
            }
        };
        Ok(Feature { kind: t, imp })
    }
}

/// `possibleBiomes()` of the vanilla biome sources, in order (the order breaks ties
/// between otherwise unordered features in `FeatureSorter`).
const OVERWORLD_BIOMES: &[&str] = &[
    "mushroom_fields", "deep_frozen_ocean", "frozen_ocean", "deep_cold_ocean", "cold_ocean", "deep_ocean", "ocean",
    "deep_lukewarm_ocean", "lukewarm_ocean", "warm_ocean", "stony_shore", "swamp", "mangrove_swamp", "snowy_slopes",
    "snowy_plains", "snowy_beach", "windswept_gravelly_hills", "grove", "windswept_hills", "snowy_taiga",
    "windswept_forest", "taiga", "plains", "meadow", "beach", "forest", "old_growth_spruce_taiga", "flower_forest",
    "birch_forest", "dark_forest", "pale_garden", "savanna_plateau", "savanna", "jungle", "badlands", "desert",
    "wooded_badlands", "jagged_peaks", "stony_peaks", "frozen_river", "river", "ice_spikes", "dappled_forest",
    "old_growth_pine_taiga", "sunflower_plains", "old_growth_birch_forest", "sparse_jungle", "bamboo_jungle",
    "eroded_badlands", "windswept_savanna", "cherry_grove", "frozen_peaks", "dripstone_caves", "lush_caves",
    "sulfur_caves", "deep_dark",
];
const NETHER_BIOMES: &[&str] = &["nether_wastes", "soul_sand_valley", "crimson_forest", "warped_forest", "basalt_deltas"];
const END_BIOMES: &[&str] = &["the_end", "end_highlands", "end_midlands", "small_end_islands", "end_barrens"];

/// The decoration data of one dimension: features sorted per step and, per biome, the
/// indices of its features in that order.
pub struct Decoration {
    pub(crate) blocks: Blocks,
    steps: Vec<Vec<Arc<PlacedFeature>>>,
    biome_steps: Vec<Vec<Vec<u32>>>,
    possible: u128,
    /// Feature types the data uses that are not implemented (they place nothing).
    pub unsupported: Vec<String>,
}

impl Decoration {
    pub(crate) fn load(data: &DataReader, biomes: &Biomes, dimension: Dimension, possible: &[u8]) -> Result<Self> {
        let mut blocks = Blocks::load(data)?;
        let order: &[&str] = match dimension {
            Dimension::Overworld => OVERWORLD_BIOMES,
            Dimension::Nether => NETHER_BIOMES,
            Dimension::End => END_BIOMES,
        };
        let mut ordered: Vec<u8> = order.iter().filter_map(|n| biomes.id(&format!("minecraft:{n}"))).filter(|b| possible.contains(b)).collect();
        for b in possible {
            if !ordered.contains(b) {
                ordered.push(*b);
            }
        }
        let mut possible_bits = 0u128;
        for b in possible {
            possible_bits |= 1u128 << b;
        }

        // `FeatureSorter.buildFeaturesPerStep`: identical placed features count as one.
        let mut canonical: HashMap<String, String> = HashMap::new();
        let mut by_content: HashMap<String, String> = HashMap::new();
        for &b in &ordered {
            for step in &biomes.get(b).features {
                for f in step {
                    let id = full_id(f);
                    if canonical.contains_key(&id) {
                        continue;
                    }
                    let content = data.worldgen("placed_feature", &id)?.to_string();
                    let c = by_content.entry(content).or_insert_with(|| id.clone()).clone();
                    canonical.insert(id, c);
                }
            }
        }
        let mut feature_index: HashMap<String, usize> = HashMap::new();
        let mut edges: BTreeMap<(usize, usize), BTreeSet<(usize, usize)>> = BTreeMap::new();
        let mut names: HashMap<(usize, usize), String> = HashMap::new();
        let mut max_steps = 0;
        for &b in &ordered {
            let features = &biomes.get(b).features;
            max_steps = max_steps.max(features.len());
            let mut in_order = Vec::new();
            for (step, list) in features.iter().enumerate() {
                for f in list {
                    let id = canonical[&full_id(f)].clone();
                    let next = feature_index.len();
                    let index = *feature_index.entry(id.clone()).or_insert(next);
                    names.insert((step, index), id);
                    in_order.push((step, index));
                }
            }
            for (i, node) in in_order.iter().enumerate() {
                let children = edges.entry(*node).or_default();
                if let Some(next) = in_order.get(i + 1) {
                    children.insert(*next);
                }
            }
        }
        let mut discovered = BTreeSet::new();
        let mut visiting = BTreeSet::new();
        let mut sorted = Vec::new();
        fn visit(
            node: (usize, usize),
            edges: &BTreeMap<(usize, usize), BTreeSet<(usize, usize)>>,
            discovered: &mut BTreeSet<(usize, usize)>,
            visiting: &mut BTreeSet<(usize, usize)>,
            sorted: &mut Vec<(usize, usize)>,
        ) -> Result<()> {
            if discovered.contains(&node) {
                return Ok(());
            }
            if !visiting.insert(node) {
                anyhow::bail!("feature order cycle at {node:?}");
            }
            if let Some(next) = edges.get(&node) {
                for n in next {
                    visit(*n, edges, discovered, visiting, sorted)?;
                }
            }
            visiting.remove(&node);
            discovered.insert(node);
            sorted.push(node);
            Ok(())
        }
        for node in edges.keys() {
            visit(*node, &edges, &mut discovered, &mut visiting, &mut sorted)?;
        }
        sorted.reverse();

        let mut parser = Parser::new(data, &mut blocks, biomes);
        let mut steps: Vec<Vec<Arc<PlacedFeature>>> = vec![Vec::new(); max_steps];
        let mut position: HashMap<(usize, String), u32> = HashMap::new();
        for node in &sorted {
            let id = &names[node];
            let placed = parser.placed(&Value::String(id.clone())).map_err(|e| e.context(format!("placed feature {id}")))?;
            position.insert((node.0, id.clone()), steps[node.0].len() as u32);
            steps[node.0].push(placed);
        }
        let unsupported = std::mem::take(&mut parser.unsupported);
        drop(parser);

        let mut biome_steps = vec![Vec::new(); biomes.list.len()];
        for &b in possible {
            let features = &biomes.get(b).features;
            let mut per_step = Vec::with_capacity(features.len());
            for (step, list) in features.iter().enumerate() {
                let mut indices: Vec<u32> = list.iter().map(|f| position[&(step, canonical[&full_id(f)].clone())]).collect();
                indices.sort_unstable();
                indices.dedup();
                per_step.push(indices);
            }
            biome_steps[b as usize] = per_step;
        }
        Ok(Self { blocks, steps, biome_steps, possible: possible_bits, unsupported })
    }

    /// Places the features of the chunk at the center of `region`.
    pub(crate) fn decorate(&self, gen: &WorldGenerator, chunk_x: i32, chunk_z: i32, region: &mut dyn RegionAccess) {
        let origin = Pos::new(chunk_x << 4, gen.min_y, chunk_z << 4);
        let mut possible = 0u128;
        for dz in -1..=1 {
            for dx in -1..=1 {
                if let Some(c) = region.chunk(chunk_x + dx, chunk_z + dz) {
                    for b in &c.biomes {
                        possible |= 1u128 << b;
                    }
                }
            }
        }
        possible &= self.possible;
        let biomes: Vec<usize> = (0..128).filter(|b| possible & (1u128 << b) != 0).collect();

        let mut r = WorldgenRandom::xoroshiro(0);
        let decoration_seed = r.set_decoration_seed(gen.seed, origin.x, origin.z);
        let mut w = World::new(gen, &self.blocks, region, chunk_x, chunk_z);
        let step_count = self.steps.len().max(11);
        let mut indices: Vec<u32> = Vec::new();
        for step in 0..step_count {
            if step >= self.steps.len() {
                continue;
            }
            indices.clear();
            for &b in &biomes {
                if let Some(list) = self.biome_steps[b].get(step) {
                    indices.extend_from_slice(list);
                }
            }
            indices.sort_unstable();
            indices.dedup();
            for &index in &indices {
                r.set_feature_seed(decoration_seed, index as i32, step as i32);
                self.steps[step][index as usize].place(&mut w, &mut r, origin);
            }
        }
    }
}
