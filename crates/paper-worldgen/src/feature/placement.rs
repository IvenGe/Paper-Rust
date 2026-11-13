//! Placed features and their placement modifiers.
//!
//! Derived from `pumpkin-world/src/generation/feature/placed_features.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0; the modifier set and the
//! depth-first placement order follow vanilla 26.3 (`FeaturePlacer`).

use super::parse::Parser;
use super::predicate::BlockPredicate;
use super::world::{Dir, Pos, World};
use super::Feature;
use crate::chunk::Heightmap;
use crate::data::JsonExt;
use crate::providers::{HeightProvider, IntProvider};
use crate::random::{Random, WorldgenRandom};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::sync::Arc;

pub enum Modifier {
    BlockPredicateFilter(BlockPredicate),
    Rarity(i32),
    RandomChance(f32),
    SurfaceRelativeThreshold { heightmap: Heightmap, min: i32, max: i32 },
    SurfaceWaterDepth(i32),
    Biome,
    Count(IntProvider),
    NoiseBasedCount { ratio: i32, factor: f64, offset: f64 },
    NoiseThresholdCount { level: f64, below: i32, above: i32 },
    CountOnEveryLayer(IntProvider),
    Cuboid { xz: IntProvider, y: IntProvider, edges: bool, interior: bool },
    EnvironmentScan { dir: Dir, target: BlockPredicate, allowed: BlockPredicate, max_steps: i32 },
    Heightmap(Heightmap),
    HeightRange(HeightProvider),
    InSquare,
    Offset(IntProvider, IntProvider, IntProvider),
    RandomlySelected(Vec<Modifier>),
    Fixed(Vec<Pos>),
}

/// The feature a placed feature places plus its modifiers. `biomes` holds the biomes whose
/// generation settings contain it (for the biome filter of top-level features).
pub struct PlacedFeature {
    pub feature: Arc<Feature>,
    pub modifiers: Vec<Modifier>,
    pub biomes: u128,
}

impl PlacedFeature {
    /// `FeaturePlacer.place`: positions are expanded depth first, each modifier's output in
    /// order, and the feature is placed at every position the last modifier produces.
    pub fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        if self.modifiers.is_empty() {
            return self.feature.place(w, r, origin);
        }
        let mut placed = false;
        let mut stack: Vec<(Pos, usize)> = vec![(origin, 0)];
        let mut out = Vec::new();
        while let Some((pos, index)) = stack.pop() {
            out.clear();
            self.modifiers[index].modify(w, r, pos, self.biomes, &mut out);
            let next = index + 1;
            if next < self.modifiers.len() {
                for p in out.iter().rev() {
                    stack.push((*p, next));
                }
            } else {
                for p in out.iter() {
                    placed |= self.feature.place(w, r, *p);
                }
            }
        }
        placed
    }
}

impl Modifier {
    fn modify(&self, w: &World, r: &mut WorldgenRandom, pos: Pos, biomes: u128, out: &mut Vec<Pos>) {
        let keep = |b: bool, out: &mut Vec<Pos>| {
            if b {
                out.push(pos)
            }
        };
        match self {
            Modifier::BlockPredicateFilter(p) => keep(p.test(w, pos), out),
            Modifier::Rarity(chance) => keep(r.next_f32() < 1.0 / *chance as f32, out),
            Modifier::RandomChance(chance) => keep(r.next_f32() < *chance, out),
            Modifier::SurfaceRelativeThreshold { heightmap, min, max } => {
                let y = w.height_at(*heightmap, pos.x, pos.z) as i64;
                keep(y + *min as i64 <= pos.y as i64 && pos.y as i64 <= y + *max as i64, out)
            }
            Modifier::SurfaceWaterDepth(max) => {
                let floor = w.height_at(Heightmap::OceanFloor, pos.x, pos.z);
                let surface = w.height_at(Heightmap::WorldSurface, pos.x, pos.z);
                keep(surface - floor <= *max, out)
            }
            Modifier::Biome => keep(biomes & (1u128 << w.biome(pos)) != 0, out),
            Modifier::Count(c) => {
                let n = c.sample(r);
                for _ in 0..n {
                    out.push(pos);
                }
            }
            Modifier::NoiseBasedCount { ratio, factor, offset } => {
                let noise = crate::biome::climate::biome_info_noise().get_value(pos.x as f64 / factor, pos.z as f64 / factor, false) as f32 as f64;
                let n = ((noise + offset) * *ratio as f64).ceil() as i32;
                for _ in 0..n {
                    out.push(pos);
                }
            }
            Modifier::NoiseThresholdCount { level, below, above } => {
                let noise = crate::biome::climate::biome_info_noise().get_value(pos.x as f64 / 200.0, pos.z as f64 / 200.0, false) as f32 as f64;
                let n = if noise < *level { *below } else { *above };
                for _ in 0..n {
                    out.push(pos);
                }
            }
            Modifier::CountOnEveryLayer(count) => {
                let mut layer = 0;
                loop {
                    let mut found = false;
                    let mut i = 0;
                    while i < count.sample(r) {
                        let x = r.next_bounded(16) + pos.x;
                        let z = r.next_bounded(16) + pos.z;
                        let top = w.height_at(Heightmap::MotionBlocking, x, z);
                        let y = find_on_ground(w, x, top, z, layer);
                        if y != i32::MAX {
                            out.push(Pos::new(x, y, z));
                            found = true;
                        }
                        i += 1;
                    }
                    layer += 1;
                    if !found {
                        break;
                    }
                }
            }
            Modifier::Cuboid { xz, y, edges, interior } => {
                let ys = y.sample(r);
                let xs = xz.sample(r);
                let zs = xz.sample(r);
                for dx in 0..=xs {
                    for dy in 0..=ys {
                        for dz in 0..=zs {
                            let ex = dx == 0 || dx == xs;
                            let ey = dy == 0 || dy == ys;
                            let ez = dz == 0 || dz == zs;
                            if !edges && ((ex && ey) || (ez && ey) || (ex && ez)) {
                                continue;
                            }
                            if !interior && !ex && !ey && !ez {
                                continue;
                            }
                            out.push(pos.add(dx, dy, dz));
                        }
                    }
                }
            }
            Modifier::EnvironmentScan { dir, target, allowed, max_steps } => {
                let mut p = pos;
                if !allowed.test(w, p) {
                    return;
                }
                for _ in 0..*max_steps {
                    if target.test(w, p) {
                        out.push(p);
                        return;
                    }
                    p = p.rel(*dir);
                    if w.out_of_height(p.y) {
                        return;
                    }
                    if !allowed.test(w, p) {
                        break;
                    }
                }
                if target.test(w, p) {
                    out.push(p);
                }
            }
            Modifier::Heightmap(hm) => {
                let y = w.height_at(*hm, pos.x, pos.z);
                if y > w.min_y {
                    out.push(pos.with_y(y));
                }
            }
            Modifier::HeightRange(h) => {
                let y = h.sample(r, &w.gen.gen_context());
                out.push(pos.with_y(y));
            }
            Modifier::InSquare => {
                let x = r.next_bounded(16) + pos.x;
                let z = r.next_bounded(16) + pos.z;
                out.push(Pos::new(x, pos.y, z));
            }
            Modifier::Offset(x, y, z) => {
                let dx = x.sample(r);
                let dy = y.sample(r);
                let dz = z.sample(r);
                out.push(pos.add(dx, dy, dz));
            }
            Modifier::RandomlySelected(list) => {
                let m = &list[r.next_bounded(list.len() as i32) as usize];
                m.modify(w, r, pos, biomes, out);
            }
            Modifier::Fixed(positions) => {
                let (cx, cz) = (pos.x >> 4, pos.z >> 4);
                for p in positions {
                    if p.x >> 4 == cx && p.z >> 4 == cz {
                        out.push(*p);
                    }
                }
            }
        }
    }
}

/// `CountOnEveryLayerPlacement.findOnGroundYPosition`.
fn find_on_ground(w: &World, x: i32, top: i32, z: i32, target: i32) -> i32 {
    let empty = |s: u32| w.b.is_air(s) || w.b.is_block(s, w.b.known.water) || w.b.is_block(s, w.b.known.lava);
    let mut found = 0;
    let mut current = w.get(Pos::new(x, top, z));
    let mut y = top;
    while y >= w.min_y + 1 {
        let next = w.get(Pos::new(x, y - 1, z));
        if !empty(next) && empty(current) && !w.b.is_block(next, w.b.known.bedrock) {
            if found == target {
                return y;
            }
            found += 1;
        }
        current = next;
        y -= 1;
    }
    i32::MAX
}

impl Parser<'_, '_> {
    pub fn modifier(&mut self, v: &Value) -> Result<Modifier> {
        let t = v.str_field("type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        let hm = |v: &Value| -> Result<Heightmap> {
            let s = v.str_field("heightmap")?;
            Heightmap::parse(s).ok_or_else(|| anyhow!("unknown heightmap {s}"))
        };
        Ok(match t {
            "block_predicate_filter" => Modifier::BlockPredicateFilter(self.predicate(v.field("predicate")?)?),
            "rarity_filter" => Modifier::Rarity(v.i64_or("chance", 1) as i32),
            "random_chance" => Modifier::RandomChance(v.f64_or("chance", 1.0) as f32),
            "surface_relative_threshold_filter" => Modifier::SurfaceRelativeThreshold {
                heightmap: hm(v)?,
                min: v.i64_or("min_inclusive", i32::MIN as i64) as i32,
                max: v.i64_or("max_inclusive", i32::MAX as i64) as i32,
            },
            "surface_water_depth_filter" => Modifier::SurfaceWaterDepth(v.i64_or("max_water_depth", 0) as i32),
            "biome" => Modifier::Biome,
            "count" => Modifier::Count(self.int_provider(v.field("count")?)?),
            "noise_based_count" => Modifier::NoiseBasedCount {
                ratio: v.i64_or("noise_to_count_ratio", 0) as i32,
                factor: v.f64_or("noise_factor", 1.0),
                offset: v.f64_or("noise_offset", 0.0),
            },
            "noise_threshold_count" => Modifier::NoiseThresholdCount {
                level: v.f64_or("noise_level", 0.0),
                below: v.i64_or("below_noise", 0) as i32,
                above: v.i64_or("above_noise", 0) as i32,
            },
            "count_on_every_layer" => Modifier::CountOnEveryLayer(self.int_provider(v.field("count")?)?),
            "cuboid" => Modifier::Cuboid {
                xz: self.int_provider(v.field("xz_size")?)?,
                y: self.int_provider(v.field("y_size")?)?,
                edges: v.bool_or("include_edges", true),
                interior: v.bool_or("include_interior", true),
            },
            "environment_scan" => {
                let d = v.str_field("direction_of_search")?;
                Modifier::EnvironmentScan {
                    dir: Dir::parse(d).ok_or_else(|| anyhow!("bad direction {d}"))?,
                    target: self.predicate(v.field("target_condition")?)?,
                    allowed: match v.get("allowed_search_condition") {
                        Some(p) => self.predicate(p)?,
                        None => BlockPredicate::True,
                    },
                    max_steps: v.i64_or("max_steps", 1) as i32,
                }
            }
            "heightmap" => Modifier::Heightmap(hm(v)?),
            "height_range" => Modifier::HeightRange(HeightProvider::parse(v.field("height")?)?),
            "in_square" => Modifier::InSquare,
            "offset" => Modifier::Offset(
                self.int_provider(v.get("x").unwrap_or(&Value::from(0)))?,
                self.int_provider(v.get("y").unwrap_or(&Value::from(0)))?,
                self.int_provider(v.get("z").unwrap_or(&Value::from(0)))?,
            ),
            "randomly_selected" => {
                let list = v.field("placements")?.as_array().ok_or_else(|| anyhow!("placements"))?;
                if list.is_empty() {
                    bail!("randomly_selected without placements");
                }
                Modifier::RandomlySelected(list.iter().map(|m| self.modifier(m)).collect::<Result<_>>()?)
            }
            "fixed_placement" => {
                let mut out = Vec::new();
                for p in v.field("positions")?.as_array().ok_or_else(|| anyhow!("positions"))? {
                    let a = p.as_array().ok_or_else(|| anyhow!("position"))?;
                    let c = |i: usize| a.get(i).and_then(Value::as_i64).unwrap_or(0) as i32;
                    out.push(Pos::new(c(0), c(1), c(2)));
                }
                Modifier::Fixed(out)
            }
            other => bail!("unknown placement modifier {other}"),
        })
    }

    /// A placed feature: a reference to the `placed_feature` registry or an inline object.
    pub fn placed(&mut self, v: &Value) -> Result<Arc<PlacedFeature>> {
        if let Value::String(s) = v {
            let key = crate::data::full_id(s);
            if let Some(p) = self.placed.get(&key) {
                return Ok(Arc::clone(p));
            }
            let json = self.data.worldgen("placed_feature", &key)?;
            let p = self.placed_inline(&json, self.biomes_of(&key))?;
            self.placed.insert(key, Arc::clone(&p));
            return Ok(p);
        }
        self.placed_inline(v, 0)
    }

    fn placed_inline(&mut self, v: &Value, biomes: u128) -> Result<Arc<PlacedFeature>> {
        let feature = self.feature(v.field("feature")?)?;
        let mut modifiers = Vec::new();
        if let Some(list) = v.get("placement").and_then(Value::as_array) {
            for m in list {
                modifiers.push(self.modifier(m)?);
            }
        }
        Ok(Arc::new(PlacedFeature { feature, modifiers, biomes }))
    }
}
