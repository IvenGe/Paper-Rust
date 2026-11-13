//! Int, float and height providers (`IntProvider`, `FloatProvider`, `HeightProvider`).
//!
//! Derived from `pumpkin-util/src/math/{int_provider,float_provider}.rs` and
//! `pumpkin-world/src/generation/height_provider.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::data::JsonExt;
use crate::random::Random;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;

fn type_of(v: &Value) -> Result<&str> {
    let t = v.str_field("type")?;
    Ok(t.strip_prefix("minecraft:").unwrap_or(t))
}

#[derive(Clone, Debug)]
pub enum IntProvider {
    Constant(i32),
    Uniform(i32, i32),
    BiasedToBottom(i32, i32),
    VeryBiasedToBottom(i32, i32),
    Clamped(Box<IntProvider>, i32, i32),
    ClampedNormal { mean: f32, deviation: f32, min: i32, max: i32 },
    Trapezoid { min: i32, max: i32, plateau: i32 },
    WeightedList(Vec<(IntProvider, i32)>),
}

impl IntProvider {
    pub fn parse(v: &Value) -> Result<Self> {
        if let Some(i) = v.as_i64() {
            return Ok(IntProvider::Constant(i as i32));
        }
        let mi = |k: &str| v.i64_or(k, 0) as i32;
        if v.get("type").is_none() && v.get("min_inclusive").is_some() {
            return Ok(IntProvider::Uniform(mi("min_inclusive"), mi("max_inclusive")));
        }
        Ok(match type_of(v)? {
            "constant" => IntProvider::Constant(mi("value")),
            "uniform" => IntProvider::Uniform(mi("min_inclusive"), mi("max_inclusive")),
            "biased_to_bottom" => IntProvider::BiasedToBottom(mi("min_inclusive"), mi("max_inclusive")),
            "very_biased_to_bottom" => IntProvider::VeryBiasedToBottom(mi("min_inclusive"), mi("max_inclusive")),
            "clamped" => IntProvider::Clamped(Box::new(IntProvider::parse(v.field("source")?)?), mi("min_inclusive"), mi("max_inclusive")),
            "clamped_normal" => IntProvider::ClampedNormal {
                mean: v.f64_or("mean", 0.0) as f32,
                deviation: v.f64_or("deviation", 0.0) as f32,
                min: mi("min_inclusive"),
                max: mi("max_inclusive"),
            },
            "trapezoid" => IntProvider::Trapezoid {
                min: v.get("min").map_or_else(|| mi("min_inclusive"), |_| mi("min")),
                max: v.get("max").map_or_else(|| mi("max_inclusive"), |_| mi("max")),
                plateau: mi("plateau"),
            },
            "weighted_list" => IntProvider::WeightedList(
                v.field("distribution")?
                    .as_array()
                    .ok_or_else(|| anyhow!("bad distribution"))?
                    .iter()
                    .map(|e| Ok((IntProvider::parse(e.field("data")?)?, e.i64_or("weight", 1) as i32)))
                    .collect::<Result<_>>()?,
            ),
            other => bail!("unknown int provider {other}"),
        })
    }

    pub fn sample(&self, r: &mut impl Random) -> i32 {
        match self {
            IntProvider::Constant(c) => *c,
            IntProvider::Uniform(min, max) => r.next_bounded(max - min + 1) + min,
            IntProvider::BiasedToBottom(min, max) => {
                let b = r.next_bounded(max - min + 1) + 1;
                min + r.next_bounded(b)
            }
            IntProvider::VeryBiasedToBottom(min, max) => {
                if min > max {
                    return *min;
                }
                let b1 = r.next_bounded(max - min + 1) + 1;
                let b2 = r.next_bounded(b1) + 1;
                min + r.next_bounded(b2)
            }
            IntProvider::Clamped(source, min, max) => source.sample(r).clamp(*min, *max),
            IntProvider::ClampedNormal { mean, deviation, min, max } => {
                let n = mean + r.next_gaussian() as f32 * deviation;
                n.clamp(*min as f32, *max as f32) as i32
            }
            IntProvider::Trapezoid { min, max, plateau } => {
                if min > max {
                    return *min;
                }
                if *plateau == 0 && *max == -*min {
                    return r.next_bounded(max + 1) - r.next_bounded(max + 1);
                }
                let range = max - min;
                if *plateau >= range {
                    return min + r.next_bounded(range + 1);
                }
                let start = (range - plateau) / 2;
                let end = range - start;
                min + r.next_bounded(end + 1) + r.next_bounded(start + 1)
            }
            IntProvider::WeightedList(entries) => {
                let total: i32 = entries.iter().map(|e| e.1).sum();
                if total <= 0 {
                    return 0;
                }
                let mut i = r.next_bounded(total);
                for (p, w) in entries {
                    i -= w;
                    if i < 0 {
                        return p.sample(r);
                    }
                }
                0
            }
        }
    }

    pub fn min_value(&self) -> i32 {
        match self {
            IntProvider::Constant(c) => *c,
            IntProvider::Uniform(min, _) | IntProvider::BiasedToBottom(min, _) | IntProvider::VeryBiasedToBottom(min, _) => *min,
            IntProvider::Clamped(s, min, _) => (*min).max(s.min_value()),
            IntProvider::ClampedNormal { min, .. } | IntProvider::Trapezoid { min, .. } => *min,
            IntProvider::WeightedList(e) => e.iter().map(|x| x.0.min_value()).min().unwrap_or(0),
        }
    }

    pub fn max_value(&self) -> i32 {
        match self {
            IntProvider::Constant(c) => *c,
            IntProvider::Uniform(_, max) | IntProvider::BiasedToBottom(_, max) | IntProvider::VeryBiasedToBottom(_, max) => *max,
            IntProvider::Clamped(s, _, max) => (*max).min(s.max_value()),
            IntProvider::ClampedNormal { max, .. } | IntProvider::Trapezoid { max, .. } => *max,
            IntProvider::WeightedList(e) => e.iter().map(|x| x.0.max_value()).max().unwrap_or(0),
        }
    }
}

#[derive(Clone, Debug)]
pub enum FloatProvider {
    Constant(f32),
    Uniform(f32, f32),
    ClampedNormal { mean: f32, deviation: f32, min: f32, max: f32 },
    Trapezoid { min: f32, max: f32, plateau: f32 },
}

impl FloatProvider {
    pub fn parse(v: &Value) -> Result<Self> {
        if let Some(f) = v.as_f64() {
            return Ok(FloatProvider::Constant(f as f32));
        }
        let mf = |k: &str| v.f64_or(k, 0.0) as f32;
        Ok(match type_of(v)? {
            "constant" => FloatProvider::Constant(mf("value")),
            "uniform" => FloatProvider::Uniform(mf("min_inclusive"), mf("max_exclusive")),
            "clamped_normal" => FloatProvider::ClampedNormal { mean: mf("mean"), deviation: mf("deviation"), min: mf("min"), max: mf("max") },
            "trapezoid" => FloatProvider::Trapezoid { min: mf("min"), max: mf("max"), plateau: mf("plateau") },
            other => bail!("unknown float provider {other}"),
        })
    }

    pub fn sample(&self, r: &mut impl Random) -> f32 {
        match self {
            FloatProvider::Constant(c) => *c,
            FloatProvider::Uniform(min, max) => r.next_f32() * (max - min) + min,
            FloatProvider::ClampedNormal { mean, deviation, min, max } => (mean + r.next_gaussian() as f32 * deviation).clamp(*min, *max),
            FloatProvider::Trapezoid { min, max, plateau } => {
                let range = max - min;
                let start = (range - plateau) / 2.0;
                let end = range - start;
                min + r.next_f32() * end + r.next_f32() * start
            }
        }
    }

    pub fn min_value(&self) -> f32 {
        match self {
            FloatProvider::Constant(c) => *c,
            FloatProvider::Uniform(min, _) => *min,
            FloatProvider::ClampedNormal { min, .. } | FloatProvider::Trapezoid { min, .. } => *min,
        }
    }

    pub fn max_value(&self) -> f32 {
        match self {
            FloatProvider::Constant(c) => *c,
            FloatProvider::Uniform(_, max) => *max,
            FloatProvider::ClampedNormal { max, .. } | FloatProvider::Trapezoid { max, .. } => *max,
        }
    }
}

/// `VerticalAnchor`.
#[derive(Clone, Copy, Debug)]
pub enum YAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
    RelativeToSeaLevel(i32),
}

impl YAnchor {
    pub fn parse(v: &Value) -> Result<Self> {
        let get = |k: &str| v.get(k).and_then(Value::as_i64).map(|x| x as i32);
        if let Some(a) = get("absolute") {
            Ok(YAnchor::Absolute(a))
        } else if let Some(a) = get("above_bottom") {
            Ok(YAnchor::AboveBottom(a))
        } else if let Some(a) = get("below_top") {
            Ok(YAnchor::BelowTop(a))
        } else if let Some(a) = get("relative_to_sea_level") {
            Ok(YAnchor::RelativeToSeaLevel(a))
        } else {
            Err(anyhow!("bad vertical anchor {v}"))
        }
    }

    /// Resolves against the generation range (`WorldGenerationContext`).
    #[inline]
    pub fn resolve(self, ctx: &GenContext) -> i32 {
        match self {
            YAnchor::Absolute(y) => y,
            YAnchor::AboveBottom(d) => ctx.min_y + d,
            YAnchor::BelowTop(d) => ctx.min_y + ctx.height - 1 - d,
            YAnchor::RelativeToSeaLevel(d) => ctx.sea_level + d,
        }
    }
}

/// The vertical range height providers and anchors resolve against.
#[derive(Clone, Copy, Debug)]
pub struct GenContext {
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
}

/// `Mth.nextInt(random, min, max)`: inclusive, no draw for an empty or single range.
#[inline]
fn mth_next_int(r: &mut impl Random, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        r.next_bounded(max - min + 1) + min
    }
}

#[derive(Clone, Debug)]
pub enum HeightProvider {
    Constant(YAnchor),
    Uniform(YAnchor, YAnchor),
    BiasedToBottom(YAnchor, YAnchor, i32),
    VeryBiasedToBottom(YAnchor, YAnchor, i32),
    Trapezoid(YAnchor, YAnchor, i32),
    WeightedList(Vec<(HeightProvider, i32)>),
}

impl HeightProvider {
    pub fn parse(v: &Value) -> Result<Self> {
        if v.get("type").is_none() {
            return Ok(HeightProvider::Constant(YAnchor::parse(v)?));
        }
        let a = |k: &str| -> Result<YAnchor> { YAnchor::parse(v.field(k)?) };
        Ok(match type_of(v)? {
            "constant" => HeightProvider::Constant(a("value")?),
            "uniform" => HeightProvider::Uniform(a("min_inclusive")?, a("max_inclusive")?),
            "biased_to_bottom" => HeightProvider::BiasedToBottom(a("min_inclusive")?, a("max_inclusive")?, v.i64_or("inner", 1) as i32),
            "very_biased_to_bottom" => HeightProvider::VeryBiasedToBottom(a("min_inclusive")?, a("max_inclusive")?, v.i64_or("inner", 1) as i32),
            "trapezoid" => HeightProvider::Trapezoid(a("min_inclusive")?, a("max_inclusive")?, v.i64_or("plateau", 0) as i32),
            "weighted_list" => HeightProvider::WeightedList(
                v.field("distribution")?
                    .as_array()
                    .ok_or_else(|| anyhow!("bad distribution"))?
                    .iter()
                    .map(|e| Ok((HeightProvider::parse(e.field("data")?)?, e.i64_or("weight", 1) as i32)))
                    .collect::<Result<_>>()?,
            ),
            other => bail!("unknown height provider {other}"),
        })
    }

    pub fn sample(&self, r: &mut impl Random, ctx: &GenContext) -> i32 {
        match self {
            HeightProvider::Constant(a) => a.resolve(ctx),
            HeightProvider::Uniform(lo, hi) => {
                let (i, j) = (lo.resolve(ctx), hi.resolve(ctx));
                if i > j {
                    i
                } else {
                    r.next_bounded(j - i + 1) + i
                }
            }
            HeightProvider::BiasedToBottom(lo, hi, inner) => {
                let (i, j) = (lo.resolve(ctx), hi.resolve(ctx));
                if j - i - inner < 0 {
                    return i;
                }
                let k = r.next_bounded(j - i - inner + 1);
                r.next_bounded(k + inner) + i
            }
            HeightProvider::VeryBiasedToBottom(lo, hi, inner) => {
                let (i, j) = (lo.resolve(ctx), hi.resolve(ctx));
                if j - i - inner < 0 {
                    return i;
                }
                let k = mth_next_int(r, i + inner, j);
                let l = mth_next_int(r, i, k - 1);
                mth_next_int(r, i, l - 1 + inner)
            }
            HeightProvider::Trapezoid(lo, hi, plateau) => {
                let (i, j) = (lo.resolve(ctx), hi.resolve(ctx));
                if i > j {
                    return i;
                }
                let k = j - i;
                if *plateau >= k {
                    return r.next_bounded(k + 1) + i;
                }
                let l = (k - plateau) / 2;
                let m = k - l;
                i + r.next_bounded(m + 1) + r.next_bounded(l + 1)
            }
            HeightProvider::WeightedList(entries) => {
                let total: i32 = entries.iter().map(|e| e.1).sum();
                if total <= 0 {
                    return 0;
                }
                let mut i = r.next_bounded(total);
                for (p, w) in entries {
                    i -= w;
                    if i < 0 {
                        return p.sample(r, ctx);
                    }
                }
                0
            }
        }
    }
}
