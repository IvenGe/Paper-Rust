//! The surface stage: vanilla 26.x "material rules" (surface rules, bedrock, deepslate and
//! ore veins), read from `worldgen/material_rule` and `worldgen/material_condition`.
//!
//! Derived from `pumpkin-world/src/generation/surface/{mod,rule}.rs`,
//! `pumpkin-world/src/generation/proto_chunk.rs` (`build_surface`) and
//! `tools/pumpkin-codegen/src/material_rule.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

pub mod terrain;

use crate::biome::{fuzzy_quart, Biomes};
use crate::blocks::BlockFlags;
use crate::chunk::ProtoChunk;
use crate::data::{parse_block_state, DataReader, JsonExt};
use crate::density::ir::Df;
use crate::math::{lerp2, map};
use crate::noise::{DensityVolume, Noise, NormalNoise};
use crate::random::{PositionalRandom, Random};
use crate::router::{ChunkRouter, SurfaceEstimator};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// A y coordinate relative to the generation range (`VerticalAnchor`).
#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

impl Anchor {
    pub fn parse(v: &Value) -> Result<Self> {
        if let Some(a) = v.get("absolute").and_then(Value::as_i64) {
            Ok(Anchor::Absolute(a as i32))
        } else if let Some(a) = v.get("above_bottom").and_then(Value::as_i64) {
            Ok(Anchor::AboveBottom(a as i32))
        } else if let Some(a) = v.get("below_top").and_then(Value::as_i64) {
            Ok(Anchor::BelowTop(a as i32))
        } else {
            Err(anyhow!("bad vertical anchor {v}"))
        }
    }

    #[inline]
    pub fn resolve(self, min_y: i32, height: i32) -> i32 {
        match self {
            Anchor::Absolute(y) => y,
            Anchor::AboveBottom(d) => min_y + d,
            Anchor::BelowTop(d) => min_y + height - 1 - d,
        }
    }
}

pub enum Condition {
    /// Bit set over biome network ids.
    Biome(u128),
    NoiseThreshold { noise: usize, min: f64, max: f64, is_3d: bool },
    VerticalGradient { random: PositionalRandom, true_at: Anchor, false_at: Anchor },
    YAbove { anchor: Anchor, surface_depth_multiplier: i32, add_stone_depth: bool },
    Water { offset: i32, surface_depth_multiplier: i32, add_stone_depth: bool },
    Temperature,
    Steep,
    Not(Box<Condition>),
    Hole,
    AbovePreliminarySurface,
    StoneDepth { offset: i32, add_surface_depth: bool, secondary_depth_range: i32, ceiling: bool },
}

pub struct OreVeinRule {
    density: usize,
    richness: usize,
    filler_gap: usize,
    ore: u32,
    raw_ore: u32,
    filler: u32,
    raw_ore_chance: f32,
}

pub enum MaterialRule {
    Block(u32),
    Sequence(Vec<MaterialRule>),
    Condition(Condition, Box<MaterialRule>),
    Badlands,
    OreVein(OreVeinRule),
}

/// Collects what parsing material rules needs from the rest of the generator.
pub struct RuleParser<'r, 'a> {
    data: &'r DataReader<'a>,
    biomes: &'r Biomes,
    base_random: PositionalRandom,
    /// Noise ids referenced by noise thresholds.
    pub noises: Vec<String>,
    /// Density functions referenced by ore veins.
    pub vein_functions: Vec<Df>,
    /// Whether a `bandlands` rule occurs.
    pub uses_badlands: bool,
    vein_parser: crate::density::ir::DfParser<'r, 'a>,
}

impl<'r, 'a> RuleParser<'r, 'a> {
    pub fn new(data: &'r DataReader<'a>, biomes: &'r Biomes, base_random: PositionalRandom) -> Self {
        Self { data, biomes, base_random, noises: Vec::new(), vein_functions: Vec::new(), uses_badlands: false, vein_parser: crate::density::ir::DfParser::new(data) }
    }

    pub fn rule(&mut self, v: &Value) -> Result<MaterialRule> {
        if let Value::String(id) = v {
            let json = self.data.worldgen("material_rule", id)?;
            return self.rule(&json);
        }
        let ty = v.str_field("type")?;
        Ok(match ty.strip_prefix("minecraft:").unwrap_or(ty) {
            "block" => MaterialRule::Block(parse_block_state(v.field("result_state")?)?),
            "sequence" => MaterialRule::Sequence(
                v.field("sequence")?.as_array().ok_or_else(|| anyhow!("bad sequence"))?.iter().map(|r| self.rule(r)).collect::<Result<_>>()?,
            ),
            "condition" => MaterialRule::Condition(self.condition(v.field("if_true")?)?, Box::new(self.rule(v.field("then_run")?)?)),
            "bandlands" | "badlands" => {
                self.uses_badlands = true;
                MaterialRule::Badlands
            }
            "ore_vein" => {
                let mut df = |key: &str| -> Result<usize> {
                    let mut f = self.vein_parser.parse(v.field(key)?)?;
                    f.optimize();
                    f.slice_uniform_axes(crate::density::ir::AXES_ALL);
                    self.vein_functions.push(f);
                    Ok(self.vein_functions.len() - 1)
                };
                let density = df("density")?;
                let richness = df("richness")?;
                let filler_gap = df("filler_gap")?;
                MaterialRule::OreVein(OreVeinRule {
                    density,
                    richness,
                    filler_gap,
                    ore: parse_block_state(v.field("ore_block")?)?,
                    raw_ore: parse_block_state(v.field("raw_ore_block")?)?,
                    filler: parse_block_state(v.field("filler_block")?)?,
                    raw_ore_chance: v.f64_or("raw_ore_chance", 0.02) as f32,
                })
            }
            other => bail!("unknown material rule {other}"),
        })
    }

    fn condition(&mut self, v: &Value) -> Result<Condition> {
        if let Value::String(id) = v {
            let json = self.data.worldgen("material_condition", id)?;
            return self.condition(&json);
        }
        let ty = v.str_field("type")?;
        Ok(match ty.strip_prefix("minecraft:").unwrap_or(ty) {
            "biome" => {
                let names: Vec<&str> = match v.field("biome_is")? {
                    Value::String(s) => vec![s.as_str()],
                    Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                    _ => bail!("bad biome_is"),
                };
                let mut mask = 0u128;
                for n in names {
                    let id = self.biomes.id(n).ok_or_else(|| anyhow!("unknown biome {n}"))?;
                    mask |= 1u128 << id;
                }
                Condition::Biome(mask)
            }
            "noise_threshold" => {
                let id = crate::data::full_id(v.str_field("noise")?);
                let noise = match self.noises.iter().position(|n| *n == id) {
                    Some(i) => i,
                    None => {
                        self.noises.push(id);
                        self.noises.len() - 1
                    }
                };
                Condition::NoiseThreshold { noise, min: v.f64_or("min_threshold", f64::MIN), max: v.f64_or("max_threshold", f64::MAX), is_3d: v.bool_or("is_3d", false) }
            }
            "vertical_gradient" => Condition::VerticalGradient {
                random: self.base_random.from_hash_of(v.str_field("random_name")?).fork_positional(),
                true_at: Anchor::parse(v.field("true_at_and_below")?)?,
                false_at: Anchor::parse(v.field("false_at_and_above")?)?,
            },
            "y_above" => Condition::YAbove {
                anchor: Anchor::parse(v.field("anchor")?)?,
                surface_depth_multiplier: v.i64_or("surface_depth_multiplier", 0) as i32,
                add_stone_depth: v.bool_or("add_stone_depth", false),
            },
            "water" => Condition::Water {
                offset: v.i64_or("offset", 0) as i32,
                surface_depth_multiplier: v.i64_or("surface_depth_multiplier", 0) as i32,
                add_stone_depth: v.bool_or("add_stone_depth", false),
            },
            "temperature" => Condition::Temperature,
            "steep" => Condition::Steep,
            "not" => Condition::Not(Box::new(self.condition(v.field("invert")?)?)),
            "hole" => Condition::Hole,
            "above_preliminary_surface" => Condition::AbovePreliminarySurface,
            "stone_depth" => Condition::StoneDepth {
                offset: v.i64_or("offset", 0) as i32,
                add_surface_depth: v.bool_or("add_surface_depth", false),
                secondary_depth_range: v.i64_or("secondary_depth_range", 0) as i32,
                ceiling: v.str_field("surface_type")? == "ceiling",
            },
            other => bail!("unknown material condition {other}"),
        })
    }
}

/// The immutable surface configuration of a dimension.
pub struct SurfaceSystem {
    pub rule: MaterialRule,
    pub noises: Vec<Arc<NormalNoise>>,
    pub surface_noise: Arc<NormalNoise>,
    pub secondary_noise: Arc<NormalNoise>,
    pub terrain: Option<terrain::TerrainBuilder>,
    pub vein_stack: Option<crate::density::SeededStack>,
    pub eroded_badlands: Option<u8>,
    pub frozen_oceans: u128,
}

/// Biome lookups for the surface stage: the chunk's own cells, and neighbouring cells
/// computed directly from the climate.
pub struct BiomeLookup<'g> {
    gen: &'g crate::WorldGenerator,
    neighbours: HashMap<(i32, i32, i32), u8>,
}

impl<'g> BiomeLookup<'g> {
    pub fn new(gen: &'g crate::WorldGenerator) -> Self {
        Self { gen, neighbours: HashMap::new() }
    }

    pub fn quart(&mut self, chunk: &ProtoChunk, qx: i32, qy: i32, qz: i32) -> u8 {
        if qx >> 2 == chunk.chunk_x && qz >> 2 == chunk.chunk_z {
            return chunk.biome(qx, qy, qz);
        }
        let gen = self.gen;
        *self.neighbours.entry((qx, qy, qz)).or_insert_with(|| gen.biome_at_quart(qx, qy, qz))
    }

    /// The biome a block uses (vanilla's fuzzy `BiomeManager.getBiome`).
    pub fn block(&mut self, chunk: &ProtoChunk, x: i32, y: i32, z: i32) -> u8 {
        let g = self.gen;
        let (qx, qy, qz) = fuzzy_quart(g.biome_zoom_seed, x, y, z, g.min_y, g.height);
        self.quart(chunk, qx, qy, qz)
    }
}

/// Lazily evaluated ore vein functions over the chunk's vein volume.
struct VeinState<'a> {
    router: ChunkRouter<'a>,
    stack: &'a crate::density::SeededStack,
    volume: DensityVolume,
    buffers: HashMap<usize, Vec<f32>>,
}

impl VeinState<'_> {
    fn value(&mut self, function: usize, x: i32, y: i32, z: i32) -> Option<f32> {
        let index = self.volume.index_of(x, y, z)?;
        let root = self.stack.root(function);
        if !self.buffers.contains_key(&function) {
            let mut buf = vec![0.0f32; self.volume.size()];
            self.router.sample_volume(root, &mut buf, &self.volume);
            self.buffers.insert(function, buf);
        }
        Some(self.buffers[&function][index])
    }

    fn point(&mut self, function: usize, x: i32, y: i32, z: i32) -> f32 {
        let root = self.stack.root(function);
        self.router.sample(root, x, y, z)
    }
}

/// Per-chunk state while material rules are applied (`SurfaceRules.Context`).
pub struct RuleContext<'a> {
    gen: &'a crate::WorldGenerator,
    system: &'a SurfaceSystem,
    biomes: BiomeLookup<'a>,
    min_y: i32,
    height: i32,
    x: i32,
    y: i32,
    z: i32,
    fluid_height: i32,
    stone_depth_above: i32,
    stone_depth_below: i32,
    run_depth: i32,
    biome: Option<u8>,
    column: i64,
    secondary_column: i64,
    secondary_depth: f32,
    surface_column: i64,
    surface_chunk: Option<(i32, i32)>,
    surface_corners: [i32; 4],
    surface_min_y: i32,
    veins: Option<VeinState<'a>>,
}

impl<'a> RuleContext<'a> {
    pub fn new(gen: &'a crate::WorldGenerator, system: &'a SurfaceSystem, chunk: &ProtoChunk) -> Self {
        let veins = system.vein_stack.as_ref().map(|stack| {
            let chunk_min = gen.min_y;
            let chunk_max = gen.min_y + gen.height;
            let min_y = (-60).max(chunk_min);
            let max_y = 52.min(chunk_max);
            VeinState {
                router: ChunkRouter::terrain(stack),
                stack,
                volume: DensityVolume::blocks(16, (max_y - min_y).max(0) as usize, 16, chunk.start_x(), min_y, chunk.start_z()),
                buffers: HashMap::new(),
            }
        });
        Self {
            gen,
            system,
            biomes: BiomeLookup::new(gen),
            min_y: gen.gen_min_y,
            height: gen.gen_height,
            x: 0,
            y: 0,
            z: 0,
            fluid_height: 0,
            stone_depth_above: 0,
            stone_depth_below: 0,
            run_depth: 0,
            biome: None,
            column: i64::MIN,
            secondary_column: i64::MIN,
            secondary_depth: 0.0,
            surface_column: i64::MIN,
            surface_chunk: None,
            surface_corners: [0; 4],
            surface_min_y: 0,
            veins,
        }
    }

    fn init_horizontal(&mut self, x: i32, z: i32) {
        self.column += 1;
        self.x = x;
        self.z = z;
        let noise = self.system.surface_noise.get(x as f64, 0.0, z as f64) as f64;
        let r = self.gen.base_random.at(x, 0, z).next_f64();
        self.run_depth = (noise * 2.75 + 3.0 + r * 0.25) as i32;
    }

    fn init_vertical(&mut self, above: i32, below: i32, y: i32, fluid_height: i32) {
        self.y = y;
        self.fluid_height = fluid_height;
        self.stone_depth_below = below;
        self.stone_depth_above = above;
        self.biome = None;
    }

    fn biome(&mut self, chunk: &ProtoChunk) -> u8 {
        if let Some(b) = self.biome {
            return b;
        }
        let b = self.biomes.block(chunk, self.x, self.y, self.z);
        self.biome = Some(b);
        b
    }

    fn secondary_depth(&mut self) -> f32 {
        if self.secondary_column != self.column {
            self.secondary_column = self.column;
            self.secondary_depth = self.system.secondary_noise.get(self.x as f64, 0.0, self.z as f64);
        }
        self.secondary_depth
    }

    /// `getMinSurfaceLevel`: the interpolated preliminary surface minus 8 plus the run depth.
    fn min_surface_level(&mut self, estimator: &mut SurfaceEstimator) -> i32 {
        if self.surface_column != self.column {
            self.surface_column = self.column;
            let (cx, cz) = (self.x >> 4, self.z >> 4);
            if self.surface_chunk != Some((cx, cz)) {
                self.surface_chunk = Some((cx, cz));
                self.surface_corners = [
                    estimator.estimate_height(cx << 4, cz << 4),
                    estimator.estimate_height((cx + 1) << 4, cz << 4),
                    estimator.estimate_height(cx << 4, (cz + 1) << 4),
                    estimator.estimate_height((cx + 1) << 4, (cz + 1) << 4),
                ];
            }
            let c = self.surface_corners;
            let surface = lerp2(
                ((self.x & 15) as f32 / 16.0) as f64,
                ((self.z & 15) as f32 / 16.0) as f64,
                c[0] as f64,
                c[1] as f64,
                c[2] as f64,
                c[3] as f64,
            )
            .floor() as i32;
            self.surface_min_y = surface.saturating_add(self.run_depth) - 8;
        }
        self.surface_min_y
    }

    fn steep(chunk: &ProtoChunk, x: i32, z: i32) -> bool {
        use crate::chunk::Heightmap::WorldSurface;
        let (lx, lz) = (x & 15, z & 15);
        let s = chunk.top_y(WorldSurface, lx, 0.max(lz - 1));
        let a = chunk.top_y(WorldSurface, lx, 15.min(lz + 1));
        if a >= s + 4 {
            return true;
        }
        let s = chunk.top_y(WorldSurface, 0.max(lx - 1), lz);
        let a = chunk.top_y(WorldSurface, 15.min(lx + 1), lz);
        s >= a + 4
    }

    fn test(&mut self, c: &Condition, chunk: &ProtoChunk, estimator: &mut SurfaceEstimator) -> bool {
        match c {
            Condition::Biome(mask) => mask & (1u128 << self.biome(chunk)) != 0,
            Condition::NoiseThreshold { noise, min, max, is_3d } => {
                let y = if *is_3d { self.y as f64 } else { 0.0 };
                let v = self.system.noises[*noise].get(self.x as f64, y, self.z as f64) as f64;
                v >= *min && v <= *max
            }
            Condition::VerticalGradient { random, true_at, false_at } => {
                let t = true_at.resolve(self.min_y, self.height);
                let f = false_at.resolve(self.min_y, self.height);
                if self.y <= t {
                    return true;
                }
                if self.y >= f {
                    return false;
                }
                let mapped = map(self.y as f32, t as f32, f as f32, 1.0, 0.0);
                random.at(self.x, self.y, self.z).next_f32() < mapped
            }
            Condition::YAbove { anchor, surface_depth_multiplier, add_stone_depth } => {
                self.y + if *add_stone_depth { self.stone_depth_above } else { 0 } >= anchor.resolve(self.min_y, self.height) + self.run_depth * surface_depth_multiplier
            }
            Condition::Water { offset, surface_depth_multiplier, add_stone_depth } => {
                self.fluid_height == i32::MIN
                    || self.y + if *add_stone_depth { self.stone_depth_above } else { 0 } >= self.fluid_height + offset + self.run_depth * surface_depth_multiplier
            }
            Condition::Temperature => {
                let b = self.biome(chunk);
                self.gen.biomes.get(b).temperature_at(self.x, self.y, self.z, self.gen.sea_level) < 0.15
            }
            Condition::Steep => Self::steep(chunk, self.x, self.z),
            Condition::Not(inner) => !self.test(inner, chunk, estimator),
            Condition::Hole => self.run_depth <= 0,
            Condition::AbovePreliminarySurface => self.y >= self.min_surface_level(estimator),
            Condition::StoneDepth { offset, add_surface_depth, secondary_depth_range, ceiling } => {
                let depth = if *ceiling { self.stone_depth_below } else { self.stone_depth_above };
                let surface = if *add_surface_depth { self.run_depth } else { 0 };
                let range = if *secondary_depth_range == 0 {
                    0
                } else {
                    let s = self.secondary_depth() as f64;
                    ((s + 1.0) / 2.0 * *secondary_depth_range as f64) as i32
                };
                depth <= 1 + offset + surface + range
            }
        }
    }

    fn ore_vein(&mut self, r: &OreVeinRule) -> Option<u32> {
        let (x, y, z) = (self.x, self.y, self.z);
        let ore_random = self.gen.ore_random;
        let veins = self.veins.as_mut()?;
        let density = veins.value(r.density, x, y, z)?;
        if density <= 0.0 {
            return None;
        }
        let mut random = ore_random.at(x, y, z);
        if random.next_f32() > density {
            return None;
        }
        let richness = veins.value(r.richness, x, y, z).unwrap_or(0.0);
        if random.next_f32() < richness && veins.point(r.filler_gap, x, y, z) < 0.0 {
            Some(if random.next_f32() < r.raw_ore_chance { r.raw_ore } else { r.ore })
        } else {
            Some(r.filler)
        }
    }

    fn apply(&mut self, rule: &MaterialRule, chunk: &ProtoChunk, estimator: &mut SurfaceEstimator) -> Option<u32> {
        match rule {
            MaterialRule::Block(s) => Some(*s),
            MaterialRule::Sequence(rules) => rules.iter().find_map(|r| self.apply(r, chunk, estimator)),
            MaterialRule::Condition(c, then) => {
                if self.test(c, chunk, estimator) {
                    self.apply(then, chunk, estimator)
                } else {
                    None
                }
            }
            MaterialRule::Badlands => self.system.terrain.as_ref().map(|t| t.terracotta(self.x, self.y, self.z)),
            MaterialRule::OreVein(r) => self.ore_vein(r),
        }
    }
}

/// Runs the surface stage on a chunk whose noise stage is done.
pub fn build_surface(gen: &crate::WorldGenerator, system: &SurfaceSystem, chunk: &mut ProtoChunk, estimator: &mut SurfaceEstimator) {
    let flags: &BlockFlags = &gen.flags;
    let mut ctx = RuleContext::new(gen, system, chunk);
    let (sx, sz) = (chunk.start_x(), chunk.start_z());
    let min_y = chunk.min_y;
    let check_column = system.eroded_badlands.is_some() || system.frozen_oceans != 0;
    for lx in 0..16 {
        for lz in 0..16 {
            let x = sx + lx;
            let z = sz + lz;
            let top = chunk.top_y(crate::chunk::Heightmap::WorldSurface, lx, lz);
            let column_biome = if check_column {
                let biome_y = if gen.legacy_random { 0 } else { top };
                let b = ctx.biomes.block(chunk, x, biome_y, z);
                if Some(b) == system.eroded_badlands {
                    if let Some(t) = &system.terrain {
                        t.badlands_pillar(chunk, flags, gen.default_block, x, z, top);
                    }
                }
                Some(b)
            } else {
                None
            };
            let top = chunk.top_y(crate::chunk::Heightmap::WorldSurface, lx, lz);
            ctx.init_horizontal(x, z);
            let mut stone_above = 0;
            let mut stone_min = i32::MAX;
            let mut fluid_height = i32::MIN;
            let mut y = top;
            while y >= min_y {
                let s = chunk.get(x, y, z);
                if flags.is_air(s) {
                    stone_above = 0;
                    fluid_height = i32::MIN;
                    y -= 1;
                    continue;
                }
                if flags.has_fluid(s) {
                    if fluid_height == i32::MIN {
                        fluid_height = y + 1;
                    }
                    y -= 1;
                    continue;
                }
                if stone_min >= y {
                    stone_min = min_y;
                    let mut sy = y - 1;
                    while sy >= min_y - 1 {
                        if sy < min_y {
                            stone_min = sy + 1;
                            break;
                        }
                        let b = chunk.get(x, sy, z);
                        if flags.is_air(b) || flags.has_fluid(b) {
                            stone_min = sy + 1;
                            break;
                        }
                        sy -= 1;
                    }
                }
                stone_above += 1;
                let stone_below = y - stone_min + 1;
                ctx.init_vertical(stone_above, stone_below, y, fluid_height);
                if let Some(new) = ctx.apply(&system.rule, chunk, estimator) {
                    chunk.set(x, y, z, new, flags);
                }
                y -= 1;
            }
            if let Some(b) = column_biome {
                if system.frozen_oceans & (1u128 << b) != 0 {
                    let estimate = ctx.min_surface_level(estimator);
                    if let Some(t) = &system.terrain {
                        t.iceberg(chunk, flags, gen.biomes.get(b), x, z, estimate, top, gen.sea_level, &gen.base_random);
                    }
                }
            }
        }
    }
}
