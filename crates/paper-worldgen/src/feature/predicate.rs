//! Block predicates (`worldgen/block_predicate`) and the rule tests of ore targets.
//!
//! Derived from `pumpkin-world/src/generation/{block_predicate.rs,rule/*.rs}` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use super::blockinfo::{BlockSet, Fluid};
use super::parse::Parser;
use super::world::{Dir, Pos, World};
use crate::data::JsonExt;
use crate::providers::YAnchor;
use crate::random::{Random, WorldgenRandom};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::sync::Arc;

type Offset = (i32, i32, i32);

/// A test on the block at (an offset from) a position.
#[derive(Debug)]
pub enum BlockPredicate {
    True,
    MatchingBlocks(Offset, Arc<BlockSet>),
    MatchingTag(Offset, Arc<BlockSet>),
    MatchingFluids(Offset, Vec<Fluid>),
    HasSturdyFace(Offset, Dir),
    Solid(Offset),
    Replaceable(Offset),
    WouldSurvive(Offset, u32),
    InsideWorldBounds(Offset),
    HeightRange(YAnchor, YAnchor),
    VolumeMatch(Offset, Offset, Box<BlockPredicate>),
    MatchingBiomes(Offset, u128),
    AnyOf(Vec<BlockPredicate>),
    AllOf(Vec<BlockPredicate>),
    Not(Box<BlockPredicate>),
}

#[inline]
fn at(p: Pos, o: Offset) -> Pos {
    p.add(o.0, o.1, o.2)
}

impl BlockPredicate {
    pub fn test(&self, w: &World, p: Pos) -> bool {
        match self {
            BlockPredicate::True => true,
            BlockPredicate::MatchingBlocks(o, set) | BlockPredicate::MatchingTag(o, set) => set.has(w.get(at(p, *o))),
            BlockPredicate::MatchingFluids(o, fluids) => {
                let f = w.fluid(at(p, *o));
                fluids.contains(&f)
            }
            BlockPredicate::HasSturdyFace(o, d) => w.b.sturdy(w.get(at(p, *o)), *d),
            BlockPredicate::Solid(o) => w.b.is_solid(w.get(at(p, *o))),
            BlockPredicate::Replaceable(o) => w.b.is_replaceable(w.get(at(p, *o))),
            BlockPredicate::WouldSurvive(o, state) => w.b.can_survive(w, *state, at(p, *o)),
            BlockPredicate::InsideWorldBounds(o) => !w.out_of_height(at(p, *o).y),
            BlockPredicate::HeightRange(min, max) => {
                let ctx = w.gen.gen_context();
                p.y >= min.resolve(&ctx) && p.y <= max.resolve(&ctx)
            }
            BlockPredicate::VolumeMatch(min, max, m) => {
                for ox in min.0..=max.0 {
                    for oz in min.2..=max.2 {
                        for oy in min.1..=max.1 {
                            if !m.test(w, p.add(ox, oy, oz)) {
                                return false;
                            }
                        }
                    }
                }
                true
            }
            BlockPredicate::MatchingBiomes(o, set) => {
                let b = w.biome(at(p, *o));
                set & (1u128 << b) != 0
            }
            BlockPredicate::AnyOf(ps) => ps.iter().any(|q| q.test(w, p)),
            BlockPredicate::AllOf(ps) => ps.iter().all(|q| q.test(w, p)),
            BlockPredicate::Not(q) => !q.test(w, p),
        }
    }
}

fn offset(v: &Value, key: &str) -> Result<Offset> {
    match v.get(key) {
        None => Ok((0, 0, 0)),
        Some(Value::Array(a)) if a.len() == 3 => {
            let c = |i: usize| a[i].as_i64().map(|x| x as i32).ok_or_else(|| anyhow!("bad offset"));
            Ok((c(0)?, c(1)?, c(2)?))
        }
        Some(other) => bail!("bad offset {other}"),
    }
}

impl Parser<'_, '_> {
    pub fn predicate(&mut self, v: &Value) -> Result<BlockPredicate> {
        let t = v.str_field("type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        Ok(match t {
            "true" => BlockPredicate::True,
            "matching_blocks" => BlockPredicate::MatchingBlocks(offset(v, "offset")?, self.block_set(v.field("blocks")?)?),
            "matching_block_tag" => {
                let tag = v.str_field("tag")?;
                BlockPredicate::MatchingTag(offset(v, "offset")?, self.tag(tag)?)
            }
            "matching_fluids" => {
                let fluids = match v.field("fluids")? {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                    other => bail!("bad fluids {other}"),
                };
                let mut out = Vec::new();
                for f in fluids {
                    if let Some(tag) = f.strip_prefix('#') {
                        // Fluid tags: water and lava include their flowing variants.
                        match tag.strip_prefix("minecraft:").unwrap_or(tag) {
                            "water" => out.extend([Fluid::Water, Fluid::FlowingWater]),
                            "lava" => out.extend([Fluid::Lava, Fluid::FlowingLava]),
                            other => bail!("unknown fluid tag {other}"),
                        }
                    } else {
                        out.push(Fluid::parse(&f).ok_or_else(|| anyhow!("unknown fluid {f}"))?);
                    }
                }
                BlockPredicate::MatchingFluids(offset(v, "offset")?, out)
            }
            "has_sturdy_face" => {
                let d = v.str_field("direction")?;
                BlockPredicate::HasSturdyFace(offset(v, "offset")?, Dir::parse(d).ok_or_else(|| anyhow!("bad direction {d}"))?)
            }
            "solid" => BlockPredicate::Solid(offset(v, "offset")?),
            "replaceable" => BlockPredicate::Replaceable(offset(v, "offset")?),
            "would_survive" => BlockPredicate::WouldSurvive(offset(v, "offset")?, self.state(v.field("state")?)?),
            "inside_world_bounds" => BlockPredicate::InsideWorldBounds(offset(v, "offset")?),
            "height_range" => BlockPredicate::HeightRange(YAnchor::parse(v.field("min_inclusive")?)?, YAnchor::parse(v.field("max_inclusive")?)?),
            "volume_match" => BlockPredicate::VolumeMatch(offset(v, "min")?, offset(v, "max")?, Box::new(self.predicate(v.field("match")?)?)),
            "matching_biomes" => BlockPredicate::MatchingBiomes(offset(v, "offset")?, self.biome_set(v.field("biomes")?)?),
            "any_of" => BlockPredicate::AnyOf(self.predicates(v.field("predicates")?)?),
            "all_of" => BlockPredicate::AllOf(self.predicates(v.field("predicates")?)?),
            "not" => BlockPredicate::Not(Box::new(self.predicate(v.field("predicate")?)?)),
            "unobstructed" => BlockPredicate::True,
            other => bail!("unknown block predicate {other}"),
        })
    }

    fn predicates(&mut self, v: &Value) -> Result<Vec<BlockPredicate>> {
        v.as_array().ok_or_else(|| anyhow!("predicates must be a list"))?.iter().map(|p| self.predicate(p)).collect()
    }

    pub fn rule_test(&mut self, v: &Value) -> Result<RuleTest> {
        let t = v.str_field("predicate_type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        Ok(match t {
            "always_true" => RuleTest::True,
            "block_match" => RuleTest::Block(self.block_id(v.str_field("block")?)?),
            "blockstate_match" => RuleTest::State(self.state(v.field("block_state")?)?),
            "tag_match" => RuleTest::Tag(self.tag(v.str_field("tag")?)?),
            "random_block_match" => RuleTest::RandomBlock(self.block_id(v.str_field("block")?)?, v.f64_or("probability", 0.0) as f32),
            "random_blockstate_match" => RuleTest::RandomState(self.state(v.field("block_state")?)?, v.f64_or("probability", 0.0) as f32),
            "any_of" => RuleTest::AnyOf(self.rule_tests(v.field("rules")?)?),
            "all_of" => RuleTest::AllOf(self.rule_tests(v.field("rules")?)?),
            "not" => RuleTest::Not(Box::new(self.rule_test(v.field("rule")?)?)),
            "height_match" => RuleTest::Height(v.i64_or("min_inclusive", i32::MIN as i64) as i32, v.i64_or("max_inclusive", i32::MAX as i64) as i32),
            other => bail!("unknown rule test {other}"),
        })
    }

    fn rule_tests(&mut self, v: &Value) -> Result<Vec<RuleTest>> {
        v.as_array().ok_or_else(|| anyhow!("rules must be a list"))?.iter().map(|p| self.rule_test(p)).collect()
    }
}

/// `RuleTest` (ore targets, structure processors).
#[derive(Debug)]
pub enum RuleTest {
    True,
    Block(u32),
    State(u32),
    Tag(Arc<BlockSet>),
    RandomBlock(u32, f32),
    RandomState(u32, f32),
    AnyOf(Vec<RuleTest>),
    AllOf(Vec<RuleTest>),
    Not(Box<RuleTest>),
    Height(i32, i32),
}

impl RuleTest {
    pub fn test(&self, state: u32, y: i32, r: &mut WorldgenRandom) -> bool {
        match self {
            RuleTest::True => true,
            RuleTest::Block(b) => paper_data::blocks::block_id_of_state(state) == *b,
            RuleTest::State(s) => state == *s,
            RuleTest::Tag(t) => t.has(state),
            RuleTest::RandomBlock(b, p) => paper_data::blocks::block_id_of_state(state) == *b && r.next_f32() < *p,
            RuleTest::RandomState(s, p) => state == *s && r.next_f32() < *p,
            RuleTest::AnyOf(rs) => rs.iter().any(|t| t.test(state, y, r)),
            RuleTest::AllOf(rs) => rs.iter().all(|t| t.test(state, y, r)),
            RuleTest::Not(t) => !t.test(state, y, r),
            RuleTest::Height(min, max) => y >= *min && y <= *max,
        }
    }
}
