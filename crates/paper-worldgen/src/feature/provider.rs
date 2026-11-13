//! Block state providers (`worldgen/block_state_provider`).
//!
//! Derived from `pumpkin-world/src/generation/block_state_provider.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, with the 26.x provider types.

use super::parse::Parser;
use super::predicate::BlockPredicate;
use super::world::{Dir, Pos, World};
use crate::data::JsonExt;
use crate::noise::{Noise, NoiseParameters, NormalNoise};
use crate::providers::IntProvider;
use crate::random::{LegacyRandom, Random, RandomSource, WorldgenRandom};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::sync::Arc;

pub struct NoiseBase {
    noise: NormalNoise,
    scale: f32,
}

impl NoiseBase {
    fn parse(v: &Value) -> Result<Self> {
        let seed = v.i64_or("seed", 0);
        let params = NoiseParameters::from_json(v.field("noise")?)?;
        Ok(Self { noise: NormalNoise::new(&mut RandomSource::Legacy(LegacyRandom::new(seed)), &params), scale: v.f64_or("scale", 1.0) as f32 })
    }

    /// `getNoiseValue(pos, scale)`.
    fn value(&self, p: Pos, scale: f64) -> f32 {
        self.noise.get(p.x as f64 * scale, p.y as f64 * scale, p.z as f64 * scale)
    }
}

/// `NoiseProvider.getRandomState(states, value)`.
fn by_noise(states: &[u32], value: f32) -> u32 {
    let v = ((1.0f32 + value) / 2.0).clamp(0.0, 0.9999);
    states[(v * states.len() as f32) as usize]
}

pub enum StateProvider {
    Simple(u32),
    Weighted(Vec<(u32, i32)>, i32),
    NoiseThreshold { base: NoiseBase, threshold: f32, high_chance: f32, default: u32, low: Vec<u32>, high: Vec<u32> },
    Noise { base: NoiseBase, states: Vec<u32> },
    DualNoise { base: NoiseBase, states: Vec<u32>, variety: (i32, i32), slow: NormalNoise, slow_scale: f32 },
    Rotated { inner: Arc<StateProvider>, direction: Option<Dir> },
    RandomizedInt { source: Arc<StateProvider>, property: String, values: IntProvider },
    RuleBased { rules: Vec<(BlockPredicate, Arc<StateProvider>)>, fallback: Option<Arc<StateProvider>> },
    RandomBlock(Vec<u32>),
    CopyProperties(Arc<StateProvider>),
}

impl StateProvider {
    /// `getState`.
    pub fn get(&self, w: &World, r: &mut WorldgenRandom, p: Pos) -> u32 {
        match self.get_optional(w, r, p) {
            Some(s) => s,
            None => w.get(p),
        }
    }

    /// `getOptionalState`: `None` where a rule-based provider has no matching rule.
    pub fn get_optional(&self, w: &World, r: &mut WorldgenRandom, p: Pos) -> Option<u32> {
        Some(match self {
            StateProvider::Simple(s) => *s,
            StateProvider::Weighted(entries, total) => {
                let mut i = r.next_bounded(*total);
                let mut out = entries[0].0;
                for (s, weight) in entries {
                    i -= weight;
                    if i < 0 {
                        out = *s;
                        break;
                    }
                }
                out
            }
            StateProvider::NoiseThreshold { base, threshold, high_chance, default, low, high } => {
                let v = base.value(p, base.scale as f64) as f64;
                if v < *threshold as f64 {
                    low[r.next_bounded(low.len() as i32) as usize]
                } else if r.next_f32() < *high_chance {
                    high[r.next_bounded(high.len() as i32) as usize]
                } else {
                    *default
                }
            }
            StateProvider::Noise { base, states } => by_noise(states, base.value(p, base.scale as f64)),
            StateProvider::DualNoise { base, states, variety, slow, slow_scale } => {
                let slow_value = |q: Pos| slow.get((q.x as f32 * slow_scale) as f64, (q.y as f32 * slow_scale) as f64, (q.z as f32 * slow_scale) as f64);
                let count = crate::math::clamped_map(slow_value(p) as f64, -1.0, 1.0, variety.0 as f64, (variety.1 + 1) as f64) as i32;
                let mut list = Vec::with_capacity(count.max(0) as usize);
                for i in 0..count {
                    list.push(by_noise(states, slow_value(p.add(i.wrapping_mul(54545), 0, i.wrapping_mul(34234)))));
                }
                by_noise(&list, base.value(p, base.scale as f64))
            }
            StateProvider::Rotated { inner, direction } => {
                let d = match direction {
                    Some(d) => *d,
                    None => Dir::random(r),
                };
                let b = w.b;
                let mut s = inner.get(w, r, p);
                s = b.with(s, "axis", d.axis());
                s = b.with(s, "facing", d.name());
                s
            }
            StateProvider::RandomizedInt { source, property, values } => {
                let s = source.get(w, r, p);
                if !w.b.has_prop(s, property) {
                    return Some(s);
                }
                w.b.with_int(s, property, values.sample(r))
            }
            StateProvider::RuleBased { rules, fallback } => {
                for (pred, then) in rules {
                    if pred.test(w, p) {
                        if let Some(s) = then.get_optional(w, r, p) {
                            return Some(s);
                        }
                    }
                }
                return match fallback {
                    Some(f) => f.get_optional(w, r, p),
                    None => None,
                };
            }
            StateProvider::RandomBlock(blocks) => {
                if blocks.is_empty() {
                    return None;
                }
                blocks[r.next_bounded(blocks.len() as i32) as usize]
            }
            StateProvider::CopyProperties(source) => {
                let s = source.get(w, r, p);
                copy_properties(s, w.get(p))
            }
        })
    }

    /// The state of a simple provider (used where vanilla requires one).
    pub fn simple_state(&self) -> Option<u32> {
        match self {
            StateProvider::Simple(s) => Some(*s),
            _ => None,
        }
    }
}

/// `BlockState.withPropertiesOf(other)`: copies the values of properties both states share.
fn copy_properties(state: u32, from: u32) -> u32 {
    let mut s = state;
    for (name, value) in paper_data::blocks::properties_of(from) {
        if let Some(n) = paper_data::blocks::with_property(s, name, value) {
            s = n;
        }
    }
    s
}

impl Parser<'_, '_> {
    /// A block state provider: a registry reference, an inline state or a typed provider.
    pub fn provider(&mut self, v: &Value) -> Result<Arc<StateProvider>> {
        if let Value::String(s) = v {
            let (ns, path) = crate::data::split_id(s);
            let file = format!("data/{ns}/worldgen/block_state_provider/{path}.json");
            if self.data.exists(&file) {
                let key = crate::data::full_id(s);
                if let Some(p) = self.providers.get(&key) {
                    return Ok(Arc::clone(p));
                }
                let json = self.data.json(&file)?;
                let p = self.provider(&json)?;
                self.providers.insert(key, Arc::clone(&p));
                return Ok(p);
            }
            return Ok(Arc::new(StateProvider::Simple(self.state(v)?)));
        }
        if v.get("type").is_none() {
            return Ok(Arc::new(StateProvider::Simple(self.state(v)?)));
        }
        let t = v.str_field("type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        Ok(Arc::new(match t {
            "simple_state_provider" | "simple" => StateProvider::Simple(self.state(v.get("state").unwrap_or(v))?),
            "weighted_state_provider" | "weighted" => {
                let entries = v
                    .field("entries")?
                    .as_array()
                    .ok_or_else(|| anyhow!("weighted entries"))?
                    .iter()
                    .map(|e| Ok((self.state(e.field("data")?)?, e.i64_or("weight", 1) as i32)))
                    .collect::<Result<Vec<_>>>()?;
                let total = entries.iter().map(|e| e.1).sum();
                if entries.is_empty() || total <= 0 {
                    bail!("empty weighted provider");
                }
                StateProvider::Weighted(entries, total)
            }
            "noise_threshold_provider" | "noise_threshold" => StateProvider::NoiseThreshold {
                base: NoiseBase::parse(v)?,
                threshold: v.f64_or("threshold", 0.0) as f32,
                high_chance: v.f64_or("high_chance", 0.0) as f32,
                default: self.state(v.field("default_state")?)?,
                low: self.states(v.field("low_states")?)?,
                high: self.states(v.field("high_states")?)?,
            },
            "noise_provider" | "noise" => StateProvider::Noise { base: NoiseBase::parse(v)?, states: self.states(v.field("states")?)? },
            "dual_noise_provider" | "dual_noise" => {
                let variety = v.field("variety")?;
                let variety = match variety {
                    Value::Array(a) if a.len() == 2 => (a[0].as_i64().unwrap_or(1) as i32, a[1].as_i64().unwrap_or(1) as i32),
                    Value::Number(n) => (n.as_i64().unwrap_or(1) as i32, n.as_i64().unwrap_or(1) as i32),
                    other => bail!("bad variety {other}"),
                };
                let seed = v.i64_or("seed", 0);
                let slow_params = NoiseParameters::from_json(v.field("slow_noise")?)?;
                StateProvider::DualNoise {
                    base: NoiseBase::parse(v)?,
                    states: self.states(v.field("states")?)?,
                    variety,
                    slow: NormalNoise::new(&mut RandomSource::Legacy(LegacyRandom::new(seed)), &slow_params),
                    slow_scale: v.f64_or("slow_scale", 1.0) as f32,
                }
            }
            "rotated_block_provider" | "rotated" => {
                let direction = match v.get("direction").and_then(Value::as_str) {
                    Some(d) => Some(Dir::parse(d).ok_or_else(|| anyhow!("bad direction {d}"))?),
                    None => None,
                };
                StateProvider::Rotated { inner: self.provider(v.field("state")?)?, direction }
            }
            "randomized_int_state_provider" | "randomized_int" => StateProvider::RandomizedInt {
                source: self.provider(v.field("source")?)?,
                property: v.str_field("property")?.to_owned(),
                values: self.int_provider(v.field("values")?)?,
            },
            "rule_based_state_provider" | "rule_based" => {
                let mut rules = Vec::new();
                for rule in v.field("rules")?.as_array().ok_or_else(|| anyhow!("rules"))? {
                    rules.push((self.predicate(rule.field("if_true")?)?, self.provider(rule.field("then")?)?));
                }
                let fallback = match v.get("fallback") {
                    Some(f) if !f.is_null() => Some(self.provider(f)?),
                    _ => None,
                };
                StateProvider::RuleBased { rules, fallback }
            }
            "random_block" => StateProvider::RandomBlock(self.block_list(v.field("blocks")?)?),
            "copy_properties" => StateProvider::CopyProperties(self.provider(v.field("source")?)?),
            other => bail!("unknown block state provider {other}"),
        }))
    }

    fn states(&mut self, v: &Value) -> Result<Vec<u32>> {
        let list = v.as_array().ok_or_else(|| anyhow!("expected a list of states"))?;
        if list.is_empty() {
            bail!("empty state list");
        }
        list.iter().map(|s| self.state(s)).collect()
    }
}
