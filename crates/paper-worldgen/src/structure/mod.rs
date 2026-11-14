//! Structures: where structure sets start, the pieces of each structure start, their
//! placement during decoration and the terrain adaptation ("beardifier") around them.
//!
//! Written to vanilla 26.3's `ChunkGenerator.createStructures` / `tryGenerateStructure` /
//! `createReferences` / `applyBiomeDecoration`, `Structure.generate`, `StructureStart` and
//! `StructureManager`. Starts are a pure function of the seed and the noise (heights come
//! from [`height::Columns`]), so they are computed on demand and cached; the chunk
//! references vanilla stores are recomputed from the starts within 8 chunks.

pub mod bbox;
mod beard;
pub(crate) mod height;
pub(crate) mod piece;
pub(crate) mod placement;
pub mod transform;

mod fortress;
mod jigsaw;
mod mineshaft;
pub(crate) mod processor;
mod stronghold;
pub(crate) mod template;

pub use bbox::BoundingBox;
pub(crate) use beard::Beardifier;

use crate::biome::Biomes;
use crate::data::{full_id, split_id, DataReader, JsonExt};
use crate::feature::world::{Pos, World};
use crate::generator::WorldGenerator;
use crate::random::WorldgenRandom;
use anyhow::{anyhow, Context, Result};
use height::Columns;
use piece::{PieceBase, PlaceCtx};
use placement::{Placement, PlacementKind, Rings};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// `GenerationStep.Decoration`, by name.
pub const STEPS: [&str; 11] = [
    "raw_generation",
    "lakes",
    "local_modifications",
    "underground_structures",
    "surface_structures",
    "strongholds",
    "underground_ores",
    "underground_decoration",
    "fluid_springs",
    "vegetal_decoration",
    "top_layer_modification",
];

/// The vanilla structure sets (the registry is read from these files; the data source
/// cannot list directories).
const VANILLA_SETS: &[&str] = &[
    "abandoned_camp",
    "ancient_cities",
    "buried_treasures",
    "desert_pyramids",
    "end_cities",
    "igloos",
    "jungle_temples",
    "mineshafts",
    "nether_complexes",
    "nether_fossils",
    "ocean_monuments",
    "ocean_ruins",
    "pillager_outposts",
    "ruined_portals",
    "shipwrecks",
    "strongholds",
    "swamp_huts",
    "trail_ruins",
    "trial_chambers",
    "villages",
    "woodland_mansions",
];

/// `TerrainAdjustment`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainAdjustment {
    None,
    Bury,
    BeardThin,
    BeardBox,
    Encapsulate,
}

impl TerrainAdjustment {
    fn parse(s: &str) -> Self {
        match s {
            "bury" => Self::Bury,
            "beard_thin" => Self::BeardThin,
            "beard_box" => Self::BeardBox,
            "encapsulate" => Self::Encapsulate,
            _ => Self::None,
        }
    }
}

/// The jigsaw data of a piece the beardifier reads (`PoolElementStructurePiece`).
#[derive(Clone, Debug, Default)]
pub struct JigsawInfo {
    /// The element's projection is `rigid` (only rigid pieces are beardified as boxes).
    pub rigid: bool,
    pub ground_level_delta: i32,
    pub junctions: Vec<Junction>,
}

/// `JigsawJunction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Junction {
    pub source_x: i32,
    pub source_ground_y: i32,
    pub source_z: i32,
    pub delta_y: i32,
    pub dest_rigid: bool,
}

/// A structure piece (`StructurePiece`): its common state and its placement.
pub(crate) trait Piece: Send + Sync {
    fn base(&self) -> &PieceBase;
    fn base_mut(&mut self) -> &mut PieceBase;
    /// The current bounding box (pieces that settle their height when first placed report
    /// the settled box).
    fn bbox(&self) -> BoundingBox {
        self.base().bbox
    }
    /// `postProcess`: places the part of the piece inside `ctx.chunk_box`.
    fn post_process(&self, ctx: &mut PlaceCtx);
    fn jigsaw(&self) -> Option<&JigsawInfo> {
        None
    }
    /// For jigsaw pieces: the element's template, the position and the rotation.
    fn jigsaw_details(&self) -> Option<(Option<&str>, Pos, transform::Rotation)> {
        None
    }
    /// `move`.
    fn translate(&mut self, dx: i32, dy: i32, dz: i32) {
        self.base_mut().bbox.move_by(dx, dy, dz);
    }
}

impl AsRef<PieceBase> for dyn Piece {
    fn as_ref(&self) -> &PieceBase {
        self.base()
    }
}

/// What a structure start generation sees (`Structure.GenerationContext`).
pub(crate) struct GenCtx<'a> {
    pub gen: &'a WorldGenerator,
    pub seed: i64,
    pub chunk_x: i32,
    pub chunk_z: i32,
    /// `makeRandom`: a legacy random seeded with the large feature seed of the chunk.
    pub r: WorldgenRandom,
    /// The structure's biomes.
    pub biomes: u128,
    pub cols: Columns<'a>,
}

impl<'a> GenCtx<'a> {
    fn new(gen: &'a WorldGenerator, chunk_x: i32, chunk_z: i32, biomes: u128) -> Self {
        let mut r = WorldgenRandom::legacy(0);
        r.set_large_feature_seed(gen.seed, chunk_x, chunk_z);
        Self { gen, seed: gen.seed, chunk_x, chunk_z, r, biomes, cols: Columns::new(gen) }
    }

    pub fn min_block_x(&self) -> i32 {
        self.chunk_x << 4
    }

    pub fn min_block_z(&self) -> i32 {
        self.chunk_z << 4
    }

    pub fn middle_block_x(&self) -> i32 {
        (self.chunk_x << 4) + 8
    }

    pub fn middle_block_z(&self) -> i32 {
        (self.chunk_z << 4) + 8
    }

    /// The dimension's bottom (`heightAccessor.getMinY()`).
    pub fn min_y(&self) -> i32 {
        self.gen.min_y
    }

    pub fn max_y(&self) -> i32 {
        self.gen.min_y + self.gen.height - 1
    }

    pub fn sea_level(&self) -> i32 {
        self.gen.sea_level
    }

    /// `isValidBiome`: the structure's biomes contain the biome at the stub position.
    pub fn valid_biome(&mut self, p: Pos) -> bool {
        let b = self.cols.biome_at(p);
        self.biomes & (1u128 << b) != 0
    }

    /// `couldStructureExistInColumn`: a valid biome at some quart of the column.
    pub fn could_exist_in_column(&mut self, x: i32, z: i32, min_y: i32, max_y: i32) -> bool {
        let (qx, qz) = (x >> 2, z >> 2);
        (min_y >> 2..=max_y >> 2).any(|qy| self.biomes & (1u128 << self.cols.biome_at_quart(qx, qy, qz)) != 0)
    }

    /// `couldValidBiomeExistOnTopOfChunkCenter`.
    pub fn could_valid_biome_exist_on_top_of_chunk_center(&mut self) -> bool {
        let (x, z) = (self.middle_block_x(), self.middle_block_z());
        let (min_y, max_y) = (self.min_y() - 1, self.max_y());
        self.could_exist_in_column(x, z, min_y, max_y)
    }

    /// `onTopOfChunkCenter`: the first occupied block at the chunk center, if a valid
    /// biome could be there.
    pub fn on_top_of_chunk_center(&mut self, hm: crate::chunk::Heightmap) -> Option<Pos> {
        if !self.could_valid_biome_exist_on_top_of_chunk_center() {
            return None;
        }
        let (x, z) = (self.middle_block_x(), self.middle_block_z());
        Some(Pos::new(x, self.cols.first_occupied(x, z, hm), z))
    }
}

/// How a structure's pieces are generated (`Structure.findGenerationPoint` and the piece
/// builder it returns).
pub(crate) trait StructureGen: Send + Sync {
    /// The pieces of a start in `ctx`'s chunk, or `None`. Implementations call
    /// `ctx.valid_biome` where vanilla's generation stub is checked.
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>>;
    /// `Structure.afterPlace`.
    fn after_place(&self, _start: &StructureStart, _ctx: &mut PlaceCtx) {}
}

/// A structure type this crate does not generate.
struct Unsupported;

impl StructureGen for Unsupported {
    fn generate(&self, _ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        None
    }
}

/// A configured structure (`worldgen/structure`).
pub(crate) struct StructureDef {
    pub id: Arc<str>,
    pub kind: String,
    pub generator: Box<dyn StructureGen>,
    pub biomes: u128,
    pub step: usize,
    pub adaptation: TerrainAdjustment,
    pub supported: bool,
}

/// A structure set (`worldgen/structure_set`).
pub(crate) struct SetDef {
    pub id: String,
    /// (structure index, weight)
    pub entries: Vec<(usize, i32)>,
    pub placement: Placement,
    /// Some structure of the set can occur in the dimension's biomes.
    pub possible: bool,
    /// Some structure of the set adapts the terrain.
    pub adapts_terrain: bool,
}

/// A generated structure start (`StructureStart`).
pub struct StructureStart {
    pub(crate) structure: usize,
    id: Arc<str>,
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub(crate) pieces: Vec<Box<dyn Piece>>,
    /// The bounding box of all pieces, inflated by 12 for structures that adapt the
    /// terrain (`getBoundingBox`).
    pub bbox: BoundingBox,
}

impl StructureStart {
    /// `placeInChunk`.
    fn place_in_chunk(&self, def: &StructureDef, w: &mut World, r: &mut WorldgenRandom, chunk_x: i32, chunk_z: i32) {
        let Some(first) = self.pieces.first() else { return };
        let fb = first.bbox();
        let c = fb.center();
        let reference = Pos::new(c.x, fb.min_y, c.z);
        let chunk_box = BoundingBox::writable_area(chunk_x, chunk_z, w.min_y, w.max_y());
        let mut ctx = PlaceCtx { w, r, chunk_box, chunk_x, chunk_z, reference };
        for p in &self.pieces {
            if p.bbox().intersects(&chunk_box) {
                p.post_process(&mut ctx);
            }
        }
        def.generator.after_place(self, &mut ctx);
    }

    /// The structure's id (`minecraft:stronghold`, ...).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The pieces: bounding box, piece type id (`minecraft:shstart`, ...), generation depth
    /// and orientation (`Direction.get2DDataValue`, -1 for none).
    pub fn pieces(&self) -> Vec<PieceInfo> {
        self.pieces
            .iter()
            .map(|p| {
                let b = p.base();
                let orientation = match b.orientation {
                    Some(crate::feature::world::Dir::South) => 0,
                    Some(crate::feature::world::Dir::West) => 1,
                    Some(crate::feature::world::Dir::North) => 2,
                    Some(crate::feature::world::Dir::East) => 3,
                    _ => -1,
                };
                let details = p.jigsaw_details();
                PieceInfo {
                    bbox: p.bbox(),
                    kind: b.kind,
                    gen_depth: b.gen_depth,
                    orientation,
                    jigsaw: p.jigsaw().cloned(),
                    element: details.and_then(|d| d.0.map(str::to_owned)),
                    position: details.map(|d| [d.1.x, d.1.y, d.1.z]),
                    rotation: details.map(|d| d.2),
                }
            })
            .collect()
    }
}

/// A piece of a structure start, as [`StructureStart::pieces`] reports it.
#[derive(Clone, Debug)]
pub struct PieceInfo {
    pub bbox: BoundingBox,
    pub kind: &'static str,
    pub gen_depth: i32,
    pub orientation: i32,
    pub jigsaw: Option<JigsawInfo>,
    /// Jigsaw pieces: the template of the element (`location`), if it has one.
    pub element: Option<String>,
    /// Jigsaw pieces: the element's position.
    pub position: Option<[i32; 3]>,
    /// Jigsaw pieces: the element's rotation.
    pub rotation: Option<transform::Rotation>,
}

impl StructureStart {
}

type StartCell = Arc<OnceLock<Option<Arc<StructureStart>>>>;

/// Structure starts by (set, chunk), computed once.
struct StartCache {
    shards: Vec<Mutex<HashMap<(u16, i32, i32), StartCell>>>,
}

impl StartCache {
    const SHARDS: usize = 64;

    fn new() -> Self {
        Self { shards: (0..Self::SHARDS).map(|_| Mutex::new(HashMap::new())).collect() }
    }

    fn get_or_compute(&self, key: (u16, i32, i32), f: impl FnOnce() -> Option<Arc<StructureStart>>) -> Option<Arc<StructureStart>> {
        let h = (key.0 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (key.1 as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ (key.2 as u32 as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
        let shard = &self.shards[(h >> 58) as usize % Self::SHARDS];
        let cell = {
            let mut m = shard.lock().unwrap_or_else(|e| e.into_inner());
            m.entry(key).or_default().clone()
        };
        cell.get_or_init(f).clone()
    }
}

/// The structures of one dimension.
pub struct Structures {
    pub(crate) defs: Vec<StructureDef>,
    pub(crate) sets: Vec<SetDef>,
    /// Per decoration step, the structures of the registry in that step (vanilla seeds
    /// structure placement with the index in this list).
    pub(crate) steps: Vec<Vec<usize>>,
    rings: Vec<Option<Rings>>,
    starts: StartCache,
    pub(crate) enabled: bool,
}

/// What structure generators may need while they are being built.
pub(crate) struct Loader<'a, 'd> {
    pub data: &'a DataReader<'d>,
    pub biomes: &'a Biomes,
    pub parser: crate::feature::parse::Parser<'a, 'd>,
    pub procs: processor::ProcessorLists,
    pub templates: template::Templates,
    pub pools: jigsaw::PoolLoader,
    pub builtins: Arc<processor::BuiltinProcessors>,
}

impl Loader<'_, '_> {
    /// A biome holder set (`#tag`, id or list) as a bit set of biome ids; nested tags and
    /// optional entries are resolved.
    pub fn biome_set(&self, v: &Value) -> Result<u128> {
        fn tag(l: &Loader, name: &str, depth: usize, out: &mut u128) -> Result<()> {
            if depth > 16 {
                return Err(anyhow!("biome tag {name} nests too deeply"));
            }
            let (ns, path) = split_id(name);
            let file = format!("data/{ns}/tags/worldgen/biome/{path}.json");
            let json = l.data.json(&file)?;
            for e in json.get("values").and_then(Value::as_array).into_iter().flatten() {
                let (id, required) = match e {
                    Value::String(s) => (s.as_str(), true),
                    Value::Object(o) => (o.get("id").and_then(Value::as_str).unwrap_or(""), o.get("required").and_then(Value::as_bool).unwrap_or(true)),
                    _ => continue,
                };
                if let Some(t) = id.strip_prefix('#') {
                    let (tns, tpath) = split_id(t);
                    if !required && !l.data.exists(&format!("data/{tns}/tags/worldgen/biome/{tpath}.json")) {
                        continue;
                    }
                    tag(l, t, depth + 1, out)?;
                } else if let Some(b) = l.biomes.id(&full_id(id)) {
                    *out |= 1u128 << b;
                }
            }
            Ok(())
        }
        let mut out = 0u128;
        match v {
            Value::String(s) => {
                if let Some(t) = s.strip_prefix('#') {
                    tag(self, t, 0, &mut out)?;
                } else if let Some(b) = self.biomes.id(&full_id(s)) {
                    out |= 1u128 << b;
                }
            }
            Value::Array(a) => {
                for e in a {
                    out |= self.biome_set(e)?;
                }
            }
            _ => return Err(anyhow!("bad biome holder set {v}")),
        }
        Ok(out)
    }
}

impl Structures {
    pub(crate) fn load(data: &DataReader, biomes: &Biomes, possible: &[u8], seed: i64, blocks: &mut crate::feature::blockinfo::Blocks) -> Result<Self> {
        let mut loader = Loader {
            data,
            biomes,
            parser: crate::feature::parse::Parser::new(data, blocks, biomes),
            procs: Default::default(),
            templates: Default::default(),
            pools: Default::default(),
            builtins: Arc::new(processor::BuiltinProcessors::new()),
        };
        let mut possible_bits = 0u128;
        for b in possible {
            possible_bits |= 1u128 << b;
        }
        // The structure registry: every structure of a set, sorted by id.
        let mut set_jsons = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        for name in VANILLA_SETS {
            let id = format!("minecraft:{name}");
            let json = data.worldgen("structure_set", &id)?;
            for e in json.get("structures").and_then(Value::as_array).into_iter().flatten() {
                let s = full_id(e.str_field("structure")?);
                if !ids.contains(&s) {
                    ids.push(s);
                }
            }
            set_jsons.push((id, json));
        }
        ids.sort_by(|a, b| {
            let (an, ap) = split_id(a);
            let (bn, bp) = split_id(b);
            ap.cmp(bp).then(an.cmp(bn))
        });
        let mut defs = Vec::with_capacity(ids.len());
        for id in &ids {
            let json = data.worldgen("structure", id)?;
            defs.push(Self::load_def(&mut loader, id, &json).with_context(|| format!("structure {id}"))?);
        }
        let Loader { data, parser, procs, templates, pools, .. } = &mut loader;
        pools.finish(data, parser, procs, templates).context("loading template pools")?;
        let mut steps = vec![Vec::new(); STEPS.len()];
        for (i, d) in defs.iter().enumerate() {
            steps[d.step].push(i);
        }
        let set_index = |name: &str| set_jsons.iter().position(|(id, _)| *id == full_id(name));
        let mut sets = Vec::with_capacity(set_jsons.len());
        for (id, json) in &set_jsons {
            let placement = Placement::parse(json.field("placement")?, &set_index, &|v| loader.biome_set(v)).with_context(|| format!("structure set {id}"))?;
            let mut entries = Vec::new();
            for e in json.get("structures").and_then(Value::as_array).into_iter().flatten() {
                let s = full_id(e.str_field("structure")?);
                let index = ids.iter().position(|i| *i == s).expect("structure in registry");
                entries.push((index, e.i64_or("weight", 1) as i32));
            }
            let possible = entries.iter().any(|(s, _)| defs[*s].biomes & possible_bits != 0);
            let adapts_terrain = entries.iter().any(|(s, _)| defs[*s].adaptation != TerrainAdjustment::None);
            sets.push(SetDef { id: id.clone(), entries, placement, possible, adapts_terrain });
        }
        let rings = sets
            .iter()
            .map(|s| match s.placement.kind {
                PlacementKind::ConcentricRings { distance, spread, count, preferred } if s.possible => Some(Rings::new(seed, distance, spread, count, preferred)),
                _ => None,
            })
            .collect();
        Ok(Self { defs, sets, steps, rings, starts: StartCache::new(), enabled: true })
    }

    fn load_def(l: &mut Loader, id: &str, json: &Value) -> Result<StructureDef> {
        let kind = json.str_field("type")?;
        let kind = kind.strip_prefix("minecraft:").unwrap_or(kind).to_owned();
        let biomes = l.biome_set(json.field("biomes")?)?;
        let step_name = json.str_field("step")?;
        let step = STEPS.iter().position(|s| *s == step_name).ok_or_else(|| anyhow!("unknown generation step {step_name}"))?;
        let adaptation = TerrainAdjustment::parse(json.get("terrain_adaptation").and_then(Value::as_str).unwrap_or("none"));
        let generator: Option<Box<dyn StructureGen>> = match kind.as_str() {
            "stronghold" => Some(Box::new(stronghold::Stronghold)),
            "fortress" => Some(Box::new(fortress::Fortress)),
            "mineshaft" => Some(Box::new(mineshaft::Mineshaft {
                mesa: json.get("mineshaft_type").and_then(Value::as_str) == Some("mesa"),
                blocking: l.biome_set(&Value::String("#minecraft:mineshaft_blocking".into()))?,
            })),
            "jigsaw" => Some(Box::new(jigsaw::Jigsaw::parse(json, &mut l.pools, l.builtins.clone())?)),
            _ => None,
        };
        let supported = generator.is_some();
        Ok(StructureDef { id: id.into(), kind, generator: generator.unwrap_or_else(|| Box::new(Unsupported)), biomes, step, adaptation, supported })
    }

    /// Structure types the data uses that are not generated.
    pub fn unsupported(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.sets {
            if !s.possible {
                continue;
            }
            for (i, _) in &s.entries {
                let d = &self.defs[*i];
                if !d.supported && !out.contains(&d.kind) {
                    out.push(d.kind.clone());
                }
            }
        }
        out
    }

    fn biome_lookup(gen: &WorldGenerator) -> impl FnMut(i32, i32, i32) -> u8 + '_ {
        let mut climate = gen.climate_sampler();
        move |qx, qy, qz| gen.biome_source.biome(qx, qy, qz, &mut climate)
    }

    /// `isPlacementChunk`.
    fn is_placement_chunk(&self, gen: &WorldGenerator, set: usize, x: i32, z: i32) -> bool {
        let p = &self.sets[set].placement;
        match &p.kind {
            PlacementKind::RandomSpread { .. } => p.potential_chunk(gen.seed, x, z) == Some((x, z)),
            PlacementKind::ConcentricRings { .. } => match &self.rings[set] {
                Some(rings) => rings.near(x, z, 0, &mut Self::biome_lookup(gen)).contains(&(x, z)),
                None => false,
            },
            PlacementKind::DimensionOrigin => false,
        }
    }

    /// `StructurePlacement.isStructureChunk`: placement, frequency and exclusion zone.
    pub(crate) fn is_structure_chunk(&self, gen: &WorldGenerator, set: usize, x: i32, z: i32) -> bool {
        let p = &self.sets[set].placement;
        if !self.is_placement_chunk(gen, set, x, z) || !p.passes_frequency(gen.seed, x, z) {
            return false;
        }
        if let Some((other, range)) = p.exclusion {
            for tx in x - range..=x + range {
                for tz in z - range..=z + range {
                    if self.is_structure_chunk(gen, other, tx, tz) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// The start of `set` in a chunk (`createStructures` for one set), cached.
    pub(crate) fn start(&self, gen: &WorldGenerator, set: usize, x: i32, z: i32) -> Option<Arc<StructureStart>> {
        self.starts.get_or_compute((set as u16, x, z), || {
            if !self.is_structure_chunk(gen, set, x, z) {
                return None;
            }
            let entries = &self.sets[set].entries;
            if entries.len() == 1 {
                return self.try_generate(gen, entries[0].0, x, z);
            }
            let mut options = entries.clone();
            let mut r = WorldgenRandom::legacy(0);
            r.set_large_feature_seed(gen.seed, x, z);
            let mut total: i32 = options.iter().map(|o| o.1).sum();
            while !options.is_empty() {
                let mut choice = crate::random::Random::next_bounded(&mut r, total);
                let mut selected = 0;
                for o in &options {
                    choice -= o.1;
                    if choice < 0 {
                        break;
                    }
                    selected += 1;
                }
                let option = options[selected.min(options.len() - 1)];
                if let Some(s) = self.try_generate(gen, option.0, x, z) {
                    return Some(s);
                }
                options.remove(selected.min(options.len() - 1));
                total -= option.1;
            }
            None
        })
    }

    /// `tryGenerateStructure`: `Structure.generate` and the validity check.
    fn try_generate(&self, gen: &WorldGenerator, structure: usize, x: i32, z: i32) -> Option<Arc<StructureStart>> {
        let def = &self.defs[structure];
        let mut ctx = GenCtx::new(gen, x, z, def.biomes);
        let pieces = def.generator.generate(&mut ctx)?;
        if pieces.is_empty() {
            return None;
        }
        let bb = BoundingBox::encapsulating(pieces.iter().map(|p| p.bbox()))?;
        let bbox = if def.adaptation != TerrainAdjustment::None { bb.inflated(12) } else { bb };
        Some(Arc::new(StructureStart { structure, id: def.id.clone(), chunk_x: x, chunk_z: z, pieces, bbox }))
    }

    /// Candidate start chunks of a set within `range` chunks of a chunk.
    fn candidates(&self, gen: &WorldGenerator, set: usize, cx: i32, cz: i32, range: i32) -> Vec<(i32, i32)> {
        let p = &self.sets[set].placement;
        match &p.kind {
            PlacementKind::RandomSpread { spacing, .. } => {
                let mut out = Vec::new();
                for gx in (cx - range).div_euclid(*spacing)..=(cx + range).div_euclid(*spacing) {
                    for gz in (cz - range).div_euclid(*spacing)..=(cz + range).div_euclid(*spacing) {
                        let (px, pz) = p.potential_chunk(gen.seed, gx * spacing, gz * spacing).expect("random spread");
                        if (px - cx).abs() <= range && (pz - cz).abs() <= range {
                            out.push((px, pz));
                        }
                    }
                }
                out
            }
            PlacementKind::ConcentricRings { .. } => match &self.rings[set] {
                Some(rings) => rings.near(cx, cz, range, &mut Self::biome_lookup(gen)),
                None => Vec::new(),
            },
            PlacementKind::DimensionOrigin => Vec::new(),
        }
    }

    /// The structure starts whose bounding box reaches into a chunk, per structure, in the
    /// order vanilla visits them (`createReferences`, then the chunk's reference sets).
    pub(crate) fn references(&self, gen: &WorldGenerator, cx: i32, cz: i32, filter: impl Fn(&SetDef) -> bool) -> Vec<(usize, Vec<Arc<StructureStart>>)> {
        let (min_x, min_z) = (cx << 4, cz << 4);
        let mut found: Vec<((i32, i32), Arc<StructureStart>)> = Vec::new();
        for (si, set) in self.sets.iter().enumerate() {
            if !set.possible || !filter(set) {
                continue;
            }
            for (px, pz) in self.candidates(gen, si, cx, cz, 8) {
                // Checked before the cache, which then only holds placement chunks.
                if !self.is_structure_chunk(gen, si, px, pz) {
                    continue;
                }
                if let Some(s) = self.start(gen, si, px, pz) {
                    if s.bbox.intersects_xz(min_x, min_z, min_x + 15, min_z + 15) {
                        found.push(((px, pz), s));
                    }
                }
            }
        }
        found.sort_by_key(|((x, z), s)| (s.structure, *x, *z));
        let mut out: Vec<(usize, Vec<Arc<StructureStart>>)> = Vec::new();
        let mut i = 0;
        while i < found.len() {
            let structure = found[i].1.structure;
            let mut j = i;
            while j < found.len() && found[j].1.structure == structure {
                j += 1;
            }
            let group = &found[i..j];
            let keys: Vec<i64> = group.iter().map(|((x, z), _)| placement::pack_chunk(*x, *z)).collect();
            let order = placement::long_open_hash_set_order(&keys);
            let starts = order.iter().filter_map(|k| group.iter().find(|((x, z), _)| placement::pack_chunk(*x, *z) == *k).map(|(_, s)| s.clone())).collect();
            out.push((structure, starts));
            i = j;
        }
        out
    }

    /// The terrain adaptation of a chunk (`Beardifier.forStructuresInChunk`).
    pub(crate) fn beardifier(&self, gen: &WorldGenerator, cx: i32, cz: i32) -> Option<Beardifier> {
        if !self.enabled || !self.sets.iter().any(|s| s.possible && s.adapts_terrain) {
            return None;
        }
        let refs = self.references(gen, cx, cz, |s| s.adapts_terrain);
        let starts: Vec<(TerrainAdjustment, Arc<StructureStart>)> =
            refs.into_iter().flat_map(|(si, starts)| starts.into_iter().map(move |s| (si, s))).filter_map(|(si, s)| (self.defs[si].adaptation != TerrainAdjustment::None).then(|| (self.defs[si].adaptation, s))).collect();
        Beardifier::for_chunk(cx, cz, &starts)
    }

    /// Places the structure pieces of one decoration step into the center chunk of `w`
    /// (the structure part of `applyBiomeDecoration`'s step).
    pub(crate) fn place_step(&self, step: usize, w: &mut World, r: &mut WorldgenRandom, decoration_seed: i64, refs: &[(usize, Vec<Arc<StructureStart>>)]) {
        if !self.enabled || refs.is_empty() {
            return;
        }
        let Some(list) = self.steps.get(step) else { return };
        let (cx, cz) = (w.center_x, w.center_z);
        for (index, &structure) in list.iter().enumerate() {
            let Some((_, starts)) = refs.iter().find(|(s, _)| *s == structure) else { continue };
            r.set_feature_seed(decoration_seed, index as i32, step as i32);
            for start in starts {
                start.place_in_chunk(&self.defs[structure], w, r, cx, cz);
            }
        }
    }

    /// The id of a structure.
    pub fn structure_id(&self, structure: usize) -> &str {
        &self.defs[structure].id
    }

    pub(crate) fn structure_index(&self, id: &str) -> Option<usize> {
        let id = full_id(id);
        self.defs.iter().position(|d| *d.id == *id)
    }

    /// The structure starts of a chunk, one per structure set that starts there.
    pub(crate) fn starts_in_chunk(&self, gen: &WorldGenerator, x: i32, z: i32) -> Vec<Arc<StructureStart>> {
        if !self.enabled {
            return Vec::new();
        }
        (0..self.sets.len()).filter(|&s| self.sets[s].possible && self.is_structure_chunk(gen, s, x, z)).filter_map(|s| self.start(gen, s, x, z)).collect()
    }

    /// Ids of the structures of the dimension that are generated.
    pub(crate) fn supported(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for s in self.sets.iter().filter(|s| s.possible) {
            for (i, _) in &s.entries {
                if self.defs[*i].supported {
                    out.push(&*self.defs[*i].id);
                }
            }
        }
        out
    }

    /// The resolved ring positions of the set with the concentric rings placement of
    /// `structure` (all 128 strongholds' chunks, vanilla's `getRingPositionsFor`).
    pub(crate) fn ring_positions(&self, gen: &WorldGenerator, set: usize) -> Vec<(i32, i32)> {
        let Some(rings) = &self.rings[set] else { return Vec::new() };
        let mut lookup = Self::biome_lookup(gen);
        (0..rings.tasks.len()).map(|i| rings.resolve(i, &mut lookup)).collect()
    }
}
