//! Structure set placements: where a structure set may start.
//!
//! Written to vanilla 26.3's `AbstractSpreadingStructurePlacement` (frequency reducers,
//! exclusion zones), `RandomSpreadStructurePlacement`, `ConcentricRingsStructurePlacement`
//! and `ChunkGeneratorStructureState.generateRingPositions`. The ring search follows the
//! approach of `pumpkin-world/src/generation/structure/placement.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0: each ring position's biome search runs
//! only when a chunk near it asks.

use crate::data::JsonExt;
use crate::random::{LegacyRandom, Random, WorldgenRandom};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpreadType {
    Linear,
    Triangular,
}

impl SpreadType {
    /// `RandomSpreadType.evaluate`.
    fn evaluate(self, r: &mut WorldgenRandom, limit: i32) -> i32 {
        match self {
            SpreadType::Linear => r.next_bounded(limit),
            SpreadType::Triangular => (r.next_bounded(limit) + r.next_bounded(limit)) / 2,
        }
    }
}

/// `FrequencyReductionMethod`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrequencyReduction {
    Default,
    LegacyType1,
    LegacyType2,
    LegacyType3,
}

impl FrequencyReduction {
    fn should_generate(self, seed: i64, salt: i32, x: i32, z: i32, p: f32) -> bool {
        let mut r = WorldgenRandom::legacy(0);
        match self {
            FrequencyReduction::Default => {
                // Vanilla passes the salt as the x coordinate here.
                r.set_large_feature_with_salt(seed, salt, x, z);
                r.next_f32() < p
            }
            FrequencyReduction::LegacyType1 => {
                let (cx, cz) = (x >> 4, z >> 4);
                r.set_seed((cx ^ (cz << 4)) as i64 ^ seed);
                r.next_i32();
                r.next_bounded((1.0 / p) as i32) == 0
            }
            FrequencyReduction::LegacyType2 => {
                r.set_large_feature_with_salt(seed, x, z, 10387320);
                r.next_f32() < p
            }
            FrequencyReduction::LegacyType3 => {
                r.set_large_feature_seed(seed, x, z);
                r.next_f64() < p as f64
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum PlacementKind {
    RandomSpread { spacing: i32, separation: i32, spread: SpreadType },
    ConcentricRings { distance: i32, spread: i32, count: i32, preferred: u128 },
    /// `dimension_origin`; vanilla's data does not use it and it is not generated.
    DimensionOrigin,
}

/// A structure set's placement.
#[derive(Clone, Debug)]
pub struct Placement {
    pub kind: PlacementKind,
    pub locate_offset: (i32, i32, i32),
    pub reduction: FrequencyReduction,
    pub frequency: f32,
    pub salt: i32,
    /// The excluded set (index into the structure sets) and its range in chunks.
    pub exclusion: Option<(usize, i32)>,
}

impl Placement {
    pub fn parse(v: &Value, set_index: &dyn Fn(&str) -> Option<usize>, biomes: &dyn Fn(&Value) -> Result<u128>) -> Result<Self> {
        let t = v.str_field("type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        let kind = match t {
            "random_spread" => {
                let spacing = v.i64_or("spacing", 1) as i32;
                let separation = v.i64_or("separation", 0) as i32;
                if spacing <= separation {
                    bail!("spacing {spacing} must be larger than separation {separation}");
                }
                let spread = match v.get("spread_type").and_then(Value::as_str).unwrap_or("linear") {
                    "triangular" => SpreadType::Triangular,
                    _ => SpreadType::Linear,
                };
                PlacementKind::RandomSpread { spacing, separation, spread }
            }
            "concentric_rings" => PlacementKind::ConcentricRings {
                distance: v.i64_or("distance", 32) as i32,
                spread: v.i64_or("spread", 3) as i32,
                count: v.i64_or("count", 128) as i32,
                preferred: biomes(v.field("preferred_biomes")?)?,
            },
            "dimension_origin" => PlacementKind::DimensionOrigin,
            other => bail!("unknown structure placement {other}"),
        };
        let locate_offset = match v.get("locate_offset").and_then(Value::as_array) {
            Some(a) if a.len() == 3 => (a[0].as_i64().unwrap_or(0) as i32, a[1].as_i64().unwrap_or(0) as i32, a[2].as_i64().unwrap_or(0) as i32),
            _ => (0, 0, 0),
        };
        let reduction = match v.get("frequency_reduction_method").and_then(Value::as_str).unwrap_or("default") {
            "legacy_type_1" => FrequencyReduction::LegacyType1,
            "legacy_type_2" => FrequencyReduction::LegacyType2,
            "legacy_type_3" => FrequencyReduction::LegacyType3,
            _ => FrequencyReduction::Default,
        };
        let exclusion = match v.get("exclusion_zone") {
            Some(z) => {
                let other = z.str_field("other_set")?;
                Some((set_index(other).ok_or_else(|| anyhow!("unknown structure set {other}"))?, z.i64_or("chunk_count", 0) as i32))
            }
            None => None,
        };
        Ok(Self { kind, locate_offset, reduction, frequency: v.f64_or("frequency", 1.0) as f32, salt: v.i64_or("salt", 0) as i32, exclusion })
    }

    /// `RandomSpreadStructurePlacement.getPotentialStructureChunk`.
    pub fn potential_chunk(&self, seed: i64, x: i32, z: i32) -> Option<(i32, i32)> {
        let PlacementKind::RandomSpread { spacing, separation, spread } = self.kind else { return None };
        let gx = x.div_euclid(spacing);
        let gz = z.div_euclid(spacing);
        let mut r = WorldgenRandom::legacy(0);
        r.set_large_feature_with_salt(seed, gx, gz, self.salt);
        let limit = spacing - separation;
        let sx = spread.evaluate(&mut r, limit);
        let sz = spread.evaluate(&mut r, limit);
        Some((gx * spacing + sx, gz * spacing + sz))
    }

    /// `applyAdditionalChunkRestrictions`.
    pub fn passes_frequency(&self, seed: i64, x: i32, z: i32) -> bool {
        self.frequency >= 1.0 || self.reduction.should_generate(seed, self.salt, x, z, self.frequency)
    }

    /// `getLocatePos`.
    pub fn locate_pos(&self, chunk_x: i32, chunk_z: i32) -> (i32, i32, i32) {
        ((chunk_x << 4) + self.locate_offset.0, self.locate_offset.1, (chunk_z << 4) + self.locate_offset.2)
    }
}

/// `Math.round(double)` (half up), as an int.
pub fn java_round(v: f64) -> i32 {
    let f = v.floor();
    (if v - f >= 0.5 { f + 1.0 } else { f }) as i32
}

#[derive(Clone, Copy, Debug)]
pub struct RingTask {
    pub x: i32,
    pub z: i32,
    seed: i64,
}

/// The ring positions of a concentric rings placement: initial positions are computed up
/// front, the biome search of each only on demand.
pub struct Rings {
    pub tasks: Vec<RingTask>,
    resolved: Vec<OnceLock<(i32, i32)>>,
    pub preferred: u128,
}

impl Rings {
    /// The positions before the biome search (`generateRingPositions`'s random draws).
    pub fn new(seed: i64, distance: i32, spread: i32, count: i32, preferred: u128) -> Self {
        let mut tasks = Vec::with_capacity(count.max(0) as usize);
        let mut r = LegacyRandom::new(seed);
        let mut angle = r.next_f64() * std::f64::consts::PI * 2.0;
        let mut spread = spread;
        let mut position_in_circle = 0;
        let mut circle = 0;
        let distance_f = distance as f64;
        for i in 0..count {
            let dist = (4 * distance + distance * circle * 6) as f64 + (r.next_f64() - 0.5) * (distance_f * 2.5);
            let x = java_round(angle.cos() * dist);
            let z = java_round(angle.sin() * dist);
            let fork_seed = r.next_i64();
            tasks.push(RingTask { x, z, seed: fork_seed });
            angle += std::f64::consts::PI * 2.0 / spread as f64;
            position_in_circle += 1;
            if position_in_circle == spread {
                circle += 1;
                position_in_circle = 0;
                spread += 2 * spread / (circle + 1);
                spread = spread.min(count - i);
                angle += r.next_f64() * std::f64::consts::PI * 2.0;
            }
        }
        let resolved = (0..tasks.len()).map(|_| OnceLock::new()).collect();
        Self { tasks, resolved, preferred }
    }

    /// The chunk of ring position `i` after the biome search (`findBiomeHorizontal` over
    /// 112 blocks around the initial chunk's center, at y 0).
    pub fn resolve(&self, i: usize, biome_at_quart: &mut dyn FnMut(i32, i32, i32) -> u8) -> (i32, i32) {
        *self.resolved[i].get_or_init(|| {
            let t = self.tasks[i];
            let mut r = LegacyRandom::new(t.seed);
            let cx = ((t.x << 4) + 8) >> 2;
            let cz = ((t.z << 4) + 8) >> 2;
            let radius = 112 >> 2;
            let mut result = None;
            let mut found = 0;
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let (qx, qz) = (cx + dx, cz + dz);
                    let b = biome_at_quart(qx, 0, qz);
                    if self.preferred & (1u128 << b) != 0 {
                        if result.is_none() || r.next_bounded(found + 1) == 0 {
                            result = Some(((qx << 2) >> 4, (qz << 2) >> 4));
                        }
                        found += 1;
                    }
                }
            }
            result.unwrap_or((t.x, t.z))
        })
    }

    /// Ring positions within `range` chunks (Chebyshev) of a chunk, resolving only the ring
    /// positions whose search area can reach it.
    pub fn near(&self, chunk_x: i32, chunk_z: i32, range: i32, biome_at_quart: &mut dyn FnMut(i32, i32, i32) -> u8) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for (i, t) in self.tasks.iter().enumerate() {
            // The search moves a position at most 7 chunks (112 blocks around the center).
            if (t.x - chunk_x).abs() > range + 7 || (t.z - chunk_z).abs() > range + 7 {
                continue;
            }
            let (x, z) = self.resolve(i, biome_at_quart);
            if (x - chunk_x).abs() <= range && (z - chunk_z).abs() <= range && !out.contains(&(x, z)) {
                out.push((x, z));
            }
        }
        out
    }
}

/// fastutil's `HashCommon.mix(long)`.
fn mix(x: i64) -> i64 {
    let h = x.wrapping_mul(0x9E37_79B9_7F4A_7C15_u64 as i64);
    let h = h ^ ((h as u64) >> 32) as i64;
    h ^ ((h as u64) >> 16) as i64
}

/// `ChunkPos.pack`.
pub fn pack_chunk(x: i32, z: i32) -> i64 {
    (x as u32 as i64) | ((z as u32 as i64) << 32)
}

/// The iteration order of a fastutil `LongOpenHashSet` (default capacity) after inserting
/// `keys` in order: vanilla iterates structure references in this order.
pub fn long_open_hash_set_order(keys: &[i64]) -> Vec<i64> {
    fn array_size(expected: usize, f: f32) -> usize {
        let s = ((expected as f64) / (f as f64)).ceil() as usize;
        s.next_power_of_two().max(2)
    }
    let f = 0.75f32;
    let mut n = array_size(16, f);
    let mut table = vec![0i64; n];
    let mut contains_null = false;
    let mut size = 0usize;
    let max_fill = |n: usize| ((n as f64 * f as f64).ceil() as usize).min(n - 1);
    for &k in keys {
        if k == 0 {
            if !contains_null {
                contains_null = true;
                size += 1;
            }
            continue;
        }
        let mask = n - 1;
        let mut pos = (mix(k) as usize) & mask;
        let mut dup = false;
        while table[pos] != 0 {
            if table[pos] == k {
                dup = true;
                break;
            }
            pos = (pos + 1) & mask;
        }
        if dup {
            continue;
        }
        table[pos] = k;
        let before = size;
        size += 1;
        if before >= max_fill(n) {
            // rehash(arraySize(size + 1, f)): moves entries from the top down.
            let new_n = array_size(size + 1, f);
            let new_mask = new_n - 1;
            let mut new_table = vec![0i64; new_n];
            let mut i = n;
            let real = size - contains_null as usize;
            for _ in 0..real {
                loop {
                    i -= 1;
                    if table[i] != 0 {
                        break;
                    }
                }
                let mut p = (mix(table[i]) as usize) & new_mask;
                while new_table[p] != 0 {
                    p = (p + 1) & new_mask;
                }
                new_table[p] = table[i];
            }
            table = new_table;
            n = new_n;
        }
    }
    let mut out = Vec::with_capacity(size);
    if contains_null {
        out.push(0);
    }
    for i in (0..n).rev() {
        if table[i] != 0 {
            out.push(table[i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_matches_pumpkin_reference() {
        // Pumpkin's test: seed 123, region (1, 1) of the desert pyramid placement.
        let p = Placement {
            kind: PlacementKind::RandomSpread { spacing: 32, separation: 8, spread: SpreadType::Linear },
            locate_offset: (0, 0, 0),
            reduction: FrequencyReduction::Default,
            frequency: 1.0,
            salt: 14357620,
            exclusion: None,
        };
        assert_eq!(p.potential_chunk(123, 1, 1), Some((5, 4)));
    }

    #[test]
    fn hash_set_order() {
        // Small sets keep the slot order of fastutil's 32 slot table, read top down (keys
        // picked to land in distinct slots, so no probing is involved).
        let mut keys = vec![0];
        let mut used = Vec::new();
        for x in 1..64 {
            let k = pack_chunk(x, -2 * x);
            let slot = (mix(k) as usize) & 31;
            if !used.contains(&slot) {
                used.push(slot);
                keys.push(k);
            }
            if keys.len() == 4 {
                break;
            }
        }
        let order = long_open_hash_set_order(&keys);
        assert_eq!(order.len(), 4);
        assert_eq!(order[0], 0);
        let mut slots: Vec<(usize, i64)> = keys.iter().filter(|k| **k != 0).map(|k| ((mix(*k) as usize) & 31, *k)).collect();
        slots.sort_by_key(|s| std::cmp::Reverse(s.0));
        assert_eq!(order[1..].to_vec(), slots.iter().map(|s| s.1).collect::<Vec<_>>());
    }

    #[test]
    fn rounding() {
        assert_eq!(java_round(0.49999999999999994), 0);
        assert_eq!(java_round(-0.5), 0);
        assert_eq!(java_round(-1.5), -1);
        assert_eq!(java_round(2.5), 3);
    }
}
