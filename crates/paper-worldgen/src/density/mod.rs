//! Density functions: parsing, flattening, seeding and evaluation.

pub mod blended;
pub mod chunk;
pub mod end_islands;
pub mod ir;
pub mod stack;

use crate::data::{full_id, DataReader};
use crate::noise::{NoiseParameters, NormalNoise};
use crate::random::{LegacyRandom, PositionalRandom, RandomSource};
use anyhow::Result;
use blended::OldBlendedNoise;
use end_islands::EndIslands;
use ir::{prepare, Df, DfParser};
use serde_json::Value;
use stack::{seed_stack, NoiseFactory, Proto, StackBuilder};
use std::collections::HashMap;
use std::sync::Arc;

/// Seeds noises the way vanilla's `RandomState` does.
pub struct SeededNoises<'r, 'a> {
    data: &'r DataReader<'a>,
    seed: i64,
    legacy: bool,
    base: PositionalRandom,
    noises: HashMap<String, Arc<NormalNoise>>,
    end_islands: Option<Arc<EndIslands>>,
}

impl<'r, 'a> SeededNoises<'r, 'a> {
    pub fn new(data: &'r DataReader<'a>, seed: i64, legacy: bool, base: PositionalRandom) -> Self {
        Self { data, seed, legacy, base, noises: HashMap::new(), end_islands: None }
    }

    pub fn parameters(&self, id: &str) -> Result<NoiseParameters> {
        NoiseParameters::from_json(&*self.data.worldgen("noise", id)?)
    }
}

impl NoiseFactory for SeededNoises<'_, '_> {
    fn noise(&mut self, id: &str) -> Result<Arc<NormalNoise>> {
        let id = full_id(id);
        if let Some(n) = self.noises.get(&id) {
            return Ok(n.clone());
        }
        let params = self.parameters(&id)?;
        let noise = match id.as_str() {
            // The legacy nether biome noises keep their pre-1.18 construction.
            "minecraft:nether/temperature" => NormalNoise::new_legacy_nether(&mut LegacyRandom::new(self.seed), &params),
            "minecraft:nether/vegetation" => NormalNoise::new_legacy_nether(&mut LegacyRandom::new(self.seed.wrapping_add(1)), &params),
            _ => NormalNoise::new(&mut self.base.from_hash_of(&id), &params),
        };
        let noise = Arc::new(noise);
        self.noises.insert(id, noise.clone());
        Ok(noise)
    }

    fn end_islands(&mut self) -> Arc<EndIslands> {
        let seed = self.seed;
        self.end_islands.get_or_insert_with(|| Arc::new(EndIslands::new(seed))).clone()
    }

    fn old_blended(&mut self, xz_scale: f64, y_scale: f64, xz_factor: f64, y_factor: f64, smear: f64) -> OldBlendedNoise {
        let mut random = if self.legacy {
            RandomSource::Legacy(LegacyRandom::new(self.seed))
        } else {
            self.base.from_hash_of("minecraft:terrain")
        };
        OldBlendedNoise::new(&mut random, xz_scale, y_scale, xz_factor, y_factor, smear)
    }
}

/// A seeded component stack with named entry points.
pub struct SeededStack {
    pub protos: Vec<Proto>,
    roots: Vec<usize>,
}

impl SeededStack {
    /// The stack index of the `i`th function passed to [`build_stack`].
    pub fn root(&self, i: usize) -> usize {
        self.roots[i]
    }
}

/// Parses, folds and slices density functions (absent ones are the constant 0).
pub fn prepare_all(data: &DataReader, defs: &[Option<&Value>]) -> Result<Vec<Df>> {
    let mut parser = DfParser::new(data);
    defs.iter().map(|d| prepare(&mut parser, *d)).collect()
}

/// Flattens the given functions into one shared stack and seeds it.
pub fn build_stack(dfs: &[Df], noises: &mut SeededNoises) -> Result<SeededStack> {
    let mut builder = StackBuilder::default();
    let roots = dfs.iter().map(|d| builder.add(d)).collect();
    let protos = seed_stack(&builder.components, noises)?;
    Ok(SeededStack { protos, roots })
}
