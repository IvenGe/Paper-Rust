//! Building features from the jar's JSON: shared parsing state and small helpers.

use super::blockinfo::{BlockSet, Blocks};
use super::placement::PlacedFeature;
use super::provider::StateProvider;
use super::Feature;
use crate::biome::Biomes;
use crate::data::{full_id, split_id, DataReader};
use crate::providers::{FloatProvider, IntProvider};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub struct Parser<'a, 'd> {
    pub data: &'a DataReader<'d>,
    pub blocks: &'a mut Blocks,
    pub biomes: &'a Biomes,
    pub(crate) placed: HashMap<String, Arc<PlacedFeature>>,
    pub(crate) configured: HashMap<String, Arc<Feature>>,
    pub(crate) providers: HashMap<String, Arc<StateProvider>>,
    /// For each placed feature id, the biomes whose feature lists contain it.
    pub(crate) biome_features: HashMap<String, u128>,
    pub(crate) unsupported: Vec<String>,
    /// Structure templates and processor lists of template features.
    pub(crate) templates: crate::structure::template::Templates,
    pub(crate) processor_lists: crate::structure::processor::ProcessorLists,
}

impl<'a, 'd> Parser<'a, 'd> {
    pub fn new(data: &'a DataReader<'d>, blocks: &'a mut Blocks, biomes: &'a Biomes) -> Self {
        let mut biome_features: HashMap<String, u128> = HashMap::new();
        for (id, b) in biomes.list.iter().enumerate() {
            for step in &b.features {
                for f in step {
                    *biome_features.entry(full_id(f)).or_default() |= 1u128 << id;
                }
            }
        }
        Self {
            data,
            blocks,
            biomes,
            placed: HashMap::new(),
            configured: HashMap::new(),
            providers: HashMap::new(),
            biome_features,
            unsupported: Vec::new(),
            templates: Default::default(),
            processor_lists: Default::default(),
        }
    }

    pub fn biomes_of(&self, placed_id: &str) -> u128 {
        self.biome_features.get(placed_id).copied().unwrap_or(0)
    }

    /// A block state: `"id"`, `"id[prop=value]"`, `{"Name"/"id": ..., "Properties": {...}}`.
    pub fn state(&self, v: &Value) -> Result<u32> {
        crate::data::parse_block_state(v)
    }

    pub fn block_id(&self, name: &str) -> Result<u32> {
        paper_data::blocks::id(name).ok_or_else(|| anyhow!("unknown block {name}"))
    }

    pub fn tag(&mut self, name: &str) -> Result<Arc<BlockSet>> {
        let name = name.strip_prefix('#').unwrap_or(name);
        self.blocks.tag(self.data, name)
    }

    /// A holder set of blocks: `"#tag"`, a block id or a list of block ids.
    pub fn block_set(&mut self, v: &Value) -> Result<Arc<BlockSet>> {
        match v {
            Value::String(s) if s.starts_with('#') => self.tag(s),
            Value::String(s) => Ok(Arc::new(BlockSet::from_ids([self.block_id(s)?]))),
            Value::Array(a) => {
                let mut set = BlockSet::default();
                for e in a {
                    let s = e.as_str().ok_or_else(|| anyhow!("bad block list entry {e}"))?;
                    set.insert(self.block_id(s)?);
                }
                Ok(Arc::new(set))
            }
            other => bail!("bad block set {other}"),
        }
    }

    /// The default states of a holder set of blocks, in order (tags are expanded in file
    /// order, without duplicates).
    pub fn block_list(&mut self, v: &Value) -> Result<Vec<u32>> {
        let mut ids: Vec<u32> = Vec::new();
        match v {
            Value::String(s) if s.starts_with('#') => self.collect_tag(&full_id(&s[1..]), &mut ids, 0)?,
            Value::String(s) => ids.push(self.block_id(s)?),
            Value::Array(a) => {
                for e in a {
                    let s = e.as_str().ok_or_else(|| anyhow!("bad block list entry {e}"))?;
                    ids.push(self.block_id(s)?);
                }
            }
            other => bail!("bad block list {other}"),
        }
        Ok(ids.into_iter().map(|b| paper_data::blocks::BLOCKS[b as usize].default_state).collect())
    }

    fn collect_tag(&self, tag: &str, out: &mut Vec<u32>, depth: usize) -> Result<()> {
        if depth > 16 {
            bail!("tag nesting too deep at {tag}");
        }
        let (ns, path) = split_id(tag);
        let json = self.data.json(&format!("data/{ns}/tags/block/{path}.json"))?;
        for v in json.get("values").and_then(Value::as_array).ok_or_else(|| anyhow!("tag {tag} without values"))? {
            let id = match v {
                Value::String(s) => s.as_str(),
                Value::Object(o) => o.get("id").and_then(Value::as_str).unwrap_or(""),
                _ => continue,
            };
            if let Some(nested) = id.strip_prefix('#') {
                self.collect_tag(&full_id(nested), out, depth + 1)?;
            } else if let Some(b) = paper_data::blocks::id(id) {
                if !out.contains(&b) {
                    out.push(b);
                }
            }
        }
        Ok(())
    }

    /// A holder set of biomes as a bit set of biome ids.
    pub fn biome_set(&self, v: &Value) -> Result<u128> {
        let mut out = 0u128;
        let mut add = |name: &str| -> Result<()> {
            if let Some(tag) = name.strip_prefix('#') {
                let (ns, path) = split_id(tag);
                let json = self.data.json(&format!("data/{ns}/tags/worldgen/biome/{path}.json"))?;
                for e in json.get("values").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(b) = e.as_str().and_then(|s| self.biomes.id(&full_id(s))) {
                        out |= 1u128 << b;
                    }
                }
            } else if let Some(b) = self.biomes.id(&full_id(name)) {
                out |= 1u128 << b;
            }
            Ok(())
        };
        match v {
            Value::String(s) => add(s)?,
            Value::Array(a) => {
                for e in a {
                    add(e.as_str().unwrap_or(""))?;
                }
            }
            other => bail!("bad biome set {other}"),
        }
        Ok(out)
    }

    pub fn int_provider(&self, v: &Value) -> Result<IntProvider> {
        IntProvider::parse(v)
    }

    pub fn float_provider(&self, v: &Value) -> Result<FloatProvider> {
        if let Some(f) = v.as_f64() {
            return Ok(FloatProvider::Constant(f as f32));
        }
        FloatProvider::parse(v)
    }

    /// A configured feature: a reference to the `feature` registry or an inline object.
    pub fn feature(&mut self, v: &Value) -> Result<Arc<Feature>> {
        if let Value::String(s) = v {
            let key = full_id(s);
            if let Some(f) = self.configured.get(&key) {
                return Ok(Arc::clone(f));
            }
            let json = self.data.worldgen("feature", &key)?;
            let f = Arc::new(self.feature_inline(&json).map_err(|e| e.context(format!("feature {key}")))?);
            self.configured.insert(key, Arc::clone(&f));
            return Ok(f);
        }
        Ok(Arc::new(self.feature_inline(v)?))
    }
}
