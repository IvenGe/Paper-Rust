//! Block placement and composition features: `simple_block`, `block_column`, the
//! selectors, `sequence`, `overlay`, `single_block_pillar`,
//! `projected_random_patchy_square`, `random_neighbor_spread`, `block_pile`.
//!
//! Derived from `pumpkin-world/src/generation/feature/features/{simple_block,block_column,
//! random_selector,simple_random_selector,weighted_random_selector,random_boolean_selector,
//! overlay,single_block_pillar,projected_random_patchy_square,random_neighbor_spread,
//! block_pile}.rs` of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use super::placed_list;
use crate::data::JsonExt;
use crate::feature::parse::Parser;
use crate::feature::placement::PlacedFeature;
use crate::feature::predicate::BlockPredicate;
use crate::feature::provider::StateProvider;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::Place;
use crate::providers::IntProvider;
use crate::random::{Random, WorldgenRandom};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::sync::Arc;

pub(super) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(Some(match t {
        "no_op" => Box::new(NoOp),
        "simple_block" => Box::new(SimpleBlock { to_place: p.provider(v.field("to_place")?)? }),
        "block_column" => {
            let mut layers = Vec::new();
            for l in v.field("layers")?.as_array().ok_or_else(|| anyhow!("layers"))? {
                layers.push((p.int_provider(l.field("height")?)?, p.provider(l.get("provider").or_else(|| l.get("state")).ok_or_else(|| anyhow!("layer provider"))?)?));
            }
            let d = v.get("direction").and_then(Value::as_str).unwrap_or("up");
            Box::new(BlockColumn {
                layers,
                direction: Dir::parse(d).ok_or_else(|| anyhow!("bad direction {d}"))?,
                allowed: p.predicate(v.field("allowed_placement")?)?,
                prioritize_tip: v.bool_or("prioritize_tip", false),
            })
        }
        "random_selector" => {
            let mut features = Vec::new();
            for e in v.field("features")?.as_array().ok_or_else(|| anyhow!("features"))? {
                features.push((e.f64_or("chance", 0.0) as f32, p.placed(e.field("feature")?)?));
            }
            Box::new(RandomSelector { features, default: p.placed(v.field("default")?)? })
        }
        "simple_random_selector" => {
            let features = placed_list(p, v, "features")?;
            if features.is_empty() {
                bail!("simple_random_selector without features");
            }
            Box::new(SimpleRandomSelector { features })
        }
        "weighted_random_selector" => {
            let mut features = Vec::new();
            for e in v.field("features")?.as_array().ok_or_else(|| anyhow!("features"))? {
                features.push((p.placed(e.field("data")?)?, e.i64_or("weight", 1) as i32));
            }
            Box::new(WeightedSelector { total: features.iter().map(|f| f.1).sum(), features })
        }
        "random_boolean_selector" => Box::new(RandomBoolean { yes: p.placed(v.field("feature_true")?)?, no: p.placed(v.field("feature_false")?)? }),
        "sequence" => Box::new(Sequence { features: placed_list(p, v, "features")? }),
        "overlay" => Box::new(Overlay { features: placed_list(p, v, "features")? }),
        "single_block_pillar" => {
            let d = v.get("direction").and_then(Value::as_str).unwrap_or("up");
            Box::new(SingleBlockPillar {
                block: p.provider(v.field("block")?)?,
                can_replace: p.predicate(v.field("can_replace")?)?,
                direction: Dir::parse(d).ok_or_else(|| anyhow!("bad direction {d}"))?,
                chance: v.f64_or("chance_to_continue", 1.0) as f32,
                cap: match v.get("cap_feature") {
                    Some(c) => Some(p.placed(c)?),
                    None => None,
                },
            })
        }
        "projected_random_patchy_square" => Box::new(ProjectedPatchySquare {
            block: p.provider(v.field("block")?)?,
            project_through: p.predicate(v.field("project_through")?)?,
            size: p.int_provider(v.field("size")?)?,
            max_projection: v.i64_or("max_projection_height", 0) as i32,
        }),
        "random_neighbor_spread" => Box::new(RandomNeighborSpread {
            block: p.provider(v.field("block")?)?,
            accepted: p.block_set(v.field("accepted_neighbors")?)?,
            can_replace: p.predicate(v.field("can_replace")?)?,
            attempts: p.int_provider(v.field("attempts")?)?,
            xz: p.int_provider(v.field("xz_offset")?)?,
            y: p.int_provider(v.field("y_offset")?)?,
        }),
        "block_pile" => Box::new(BlockPile { provider: p.provider(v.field("state_provider")?)? }),
        _ => return Ok(None),
    }))
}

struct NoOp;

impl Place for NoOp {
    fn place(&self, _w: &mut World, _r: &mut WorldgenRandom, _pos: Pos) -> bool {
        false
    }
}

/// Places a double plant (`DoublePlantBlock.placeAt`), copying water into both halves.
pub(crate) fn place_double_plant(w: &mut World, state: u32, pos: Pos) {
    let b = w.b;
    let lower = b.with(state, "half", "lower");
    let upper = b.with(state, "half", "upper");
    let lower = if b.has_prop(lower, "waterlogged") { b.waterlogged(lower, w.is_water_at(pos)) } else { lower };
    w.set(pos, lower);
    let up = pos.up();
    let upper = if b.has_prop(upper, "waterlogged") { b.waterlogged(upper, w.is_water_at(up)) } else { upper };
    w.set(up, upper);
}

/// The fluid state of a block, compared by `SimpleBlockFeature` (type and level).
fn fluid_key(w: &World, state: u32) -> (crate::feature::blockinfo::Fluid, i32) {
    let f = w.b.fluid(state);
    let level = if matches!(f, crate::feature::blockinfo::Fluid::FlowingWater | crate::feature::blockinfo::Fluid::FlowingLava) {
        w.b.prop_int(state, "level").unwrap_or(0)
    } else {
        0
    };
    (f, level)
}

pub(crate) struct SimpleBlock {
    pub to_place: Arc<StateProvider>,
}

impl SimpleBlock {
    pub(crate) fn place_state(w: &mut World, state: u32, pos: Pos) -> bool {
        let b = w.b;
        if !b.can_survive(w, state, pos) {
            return false;
        }
        if b.is_double_plant(state) {
            let above = w.get(pos.up());
            if b.is_air(above) || (fluid_key(w, state) == fluid_key(w, above) && b.is_replaceable(above)) {
                place_double_plant(w, state, pos);
            } else {
                return false;
            }
        } else if b.is_block(state, b.known.pale_moss_carpet) {
            super::misc::place_mossy_carpet(w, pos);
        } else {
            w.set(pos, state);
        }
        true
    }
}

impl Place for SimpleBlock {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let Some(state) = self.to_place.get_optional(w, r, pos) else {
            return false;
        };
        Self::place_state(w, state, pos)
    }
}

struct BlockColumn {
    layers: Vec<(IntProvider, Arc<StateProvider>)>,
    direction: Dir,
    allowed: BlockPredicate,
    prioritize_tip: bool,
}

impl Place for BlockColumn {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let mut heights: Vec<i32> = self.layers.iter().map(|l| l.0.sample(r)).collect();
        let total: i32 = heights.iter().sum();
        if total == 0 {
            return false;
        }
        let mut probe = pos.rel(self.direction);
        for placed in 0..total {
            if !self.allowed.test(w, probe) {
                truncate(&mut heights, total, placed, self.prioritize_tip);
                break;
            }
            probe = probe.rel(self.direction);
        }
        let mut p = pos;
        for (i, h) in heights.iter().enumerate() {
            for _ in 0..*h {
                let s = self.layers[i].1.get(w, r, p);
                w.set(p, s);
                p = p.rel(self.direction);
            }
        }
        true
    }
}

/// `BlockColumnFeature.truncate`.
fn truncate(heights: &mut [i32], total: i32, actual: i32, prioritize_tip: bool) {
    let mut to_remove = total - actual;
    let n = heights.len() as i32;
    let (step, start, end) = if prioritize_tip { (1, 0, n) } else { (-1, n - 1, -1) };
    let mut i = start;
    while i != end && to_remove > 0 {
        let h = heights[i as usize];
        let take = h.min(to_remove);
        heights[i as usize] -= take;
        to_remove -= take;
        i += step;
    }
}

struct RandomSelector {
    features: Vec<(f32, Arc<PlacedFeature>)>,
    default: Arc<PlacedFeature>,
}

impl Place for RandomSelector {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        for (chance, f) in &self.features {
            if r.next_f32() < *chance {
                return f.place(w, r, pos);
            }
        }
        self.default.place(w, r, pos)
    }
}

struct SimpleRandomSelector {
    features: Vec<Arc<PlacedFeature>>,
}

impl Place for SimpleRandomSelector {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let i = r.next_bounded(self.features.len() as i32) as usize;
        self.features[i].place(w, r, pos)
    }
}

struct WeightedSelector {
    features: Vec<(Arc<PlacedFeature>, i32)>,
    total: i32,
}

impl Place for WeightedSelector {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        if self.total <= 0 {
            return false;
        }
        let mut i = r.next_bounded(self.total);
        for (f, weight) in &self.features {
            i -= weight;
            if i < 0 {
                return f.place(w, r, pos);
            }
        }
        false
    }
}

struct RandomBoolean {
    yes: Arc<PlacedFeature>,
    no: Arc<PlacedFeature>,
}

impl Place for RandomBoolean {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        if r.next_bool() {
            self.yes.place(w, r, pos)
        } else {
            self.no.place(w, r, pos)
        }
    }
}

struct Sequence {
    features: Vec<Arc<PlacedFeature>>,
}

impl Place for Sequence {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        for f in &self.features {
            if !f.place(w, r, pos) {
                return false;
            }
        }
        true
    }
}

struct Overlay {
    features: Vec<Arc<PlacedFeature>>,
}

impl Place for Overlay {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let mut any = false;
        for f in &self.features {
            any |= f.place(w, r, pos);
        }
        any
    }
}

struct SingleBlockPillar {
    block: Arc<StateProvider>,
    can_replace: BlockPredicate,
    direction: Dir,
    chance: f32,
    cap: Option<Arc<PlacedFeature>>,
}

impl Place for SingleBlockPillar {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let mut p = pos;
        while self.can_replace.test(w, p) && r.next_f32() < self.chance && !w.out_of_height(p.y) {
            let s = self.block.get(w, r, p);
            w.set(p, s);
            p = p.rel(self.direction);
        }
        p = p.rel(self.direction.opposite());
        if let Some(cap) = &self.cap {
            cap.place(w, r, p);
        }
        true
    }
}

struct ProjectedPatchySquare {
    block: Arc<StateProvider>,
    project_through: BlockPredicate,
    size: IntProvider,
    max_projection: i32,
}

impl Place for ProjectedPatchySquare {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let size = self.size.sample(r);
        let bound = size * size + 1;
        for dx in -size..=size {
            for dz in -size..=size {
                let probability = dx.abs() * dz.abs();
                if r.next_bounded(bound) >= bound - probability {
                    continue;
                }
                let mut p = pos.add(dx, 0, dz);
                let mut budget = self.max_projection;
                while self.project_through.test(w, p.down()) {
                    p = p.down();
                    budget -= 1;
                    if budget <= 0 {
                        break;
                    }
                }
                if let Some(s) = self.block.get_optional(w, r, p) {
                    w.set(p, s);
                }
            }
        }
        true
    }
}

struct RandomNeighborSpread {
    block: Arc<StateProvider>,
    accepted: Arc<crate::feature::blockinfo::BlockSet>,
    can_replace: BlockPredicate,
    attempts: IntProvider,
    xz: IntProvider,
    y: IntProvider,
}

impl Place for RandomNeighborSpread {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let s = self.block.get(w, r, pos);
        w.set(pos, s);
        let attempts = self.attempts.sample(r);
        for _ in 0..attempts {
            let dx = self.xz.sample(r);
            let dy = self.y.sample(r);
            let dz = self.xz.sample(r);
            let p = pos.add(dx, dy, dz);
            if !self.can_replace.test(w, p) {
                continue;
            }
            let mut neighbours = 0;
            for d in Dir::ALL {
                if self.accepted.has(w.get(p.rel(d))) {
                    neighbours += 1;
                }
                if neighbours > 1 {
                    break;
                }
            }
            if neighbours == 1 {
                let s = self.block.get(w, r, p);
                w.set(p, s);
            }
        }
        true
    }
}

struct BlockPile {
    provider: Arc<StateProvider>,
}

impl BlockPile {
    fn try_place(&self, w: &mut World, r: &mut WorldgenRandom, p: Pos) {
        if !w.is_air(p) {
            return;
        }
        let below = w.get(p.down());
        let may = if w.b.is_block(below, paper_data::blocks::default_state("minecraft:dirt_path").unwrap_or(0)) { r.next_bool() } else { w.b.sturdy(below, Dir::Up) };
        if may {
            let s = self.provider.get(w, r, p);
            w.set(p, s);
        }
    }
}

impl Place for BlockPile {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        if pos.y < w.min_y + 5 {
            return false;
        }
        let xr = 2 + r.next_bounded(2);
        let zr = 2 + r.next_bounded(2);
        for z in pos.z - zr..=pos.z + zr {
            for y in pos.y..=pos.y + 1 {
                for x in pos.x - xr..=pos.x + xr {
                    let p = Pos::new(x, y, z);
                    let dx = pos.x - x;
                    let dz = pos.z - z;
                    if ((dx * dx + dz * dz) as f32) <= r.next_f32() * 10.0 - r.next_f32() * 6.0 || (r.next_f32() as f64) < 0.031 {
                        self.try_place(w, r, p);
                    }
                }
            }
        }
        true
    }
}
