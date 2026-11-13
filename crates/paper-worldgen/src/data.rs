//! Access to the vanilla data files the generator is built from.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Read access to the files of the vanilla server jar (or an extracted datapack).
///
/// Paths are jar-relative, e.g. `data/minecraft/worldgen/noise_settings/overworld.json`.
pub trait DataSource: Send + Sync {
    /// The bytes of the file at `path`, or `None` if it does not exist.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// Splits `ns:path` into its namespace and path; bare paths are in `minecraft`.
pub fn split_id(id: &str) -> (&str, &str) {
    match id.split_once(':') {
        Some((ns, path)) => (ns, path),
        None => ("minecraft", id),
    }
}

/// `ns:path` with the namespace made explicit.
pub fn full_id(id: &str) -> String {
    let (ns, path) = split_id(id);
    format!("{ns}:{path}")
}

/// A caching JSON reader over a [`DataSource`], used while a generator is being built.
pub struct DataReader<'a> {
    source: &'a dyn DataSource,
    cache: RefCell<HashMap<String, Rc<Value>>>,
    block_tags: RefCell<HashMap<String, Rc<HashSet<u32>>>>,
}

impl<'a> DataReader<'a> {
    pub fn new(source: &'a dyn DataSource) -> Self {
        Self { source, cache: RefCell::new(HashMap::new()), block_tags: RefCell::new(HashMap::new()) }
    }

    /// Parses the JSON file at `path`.
    pub fn json(&self, path: &str) -> Result<Rc<Value>> {
        if let Some(v) = self.cache.borrow().get(path) {
            return Ok(v.clone());
        }
        let bytes = self.source.read(path).ok_or_else(|| anyhow!("missing data file {path}"))?;
        let value: Value = serde_json::from_slice(&bytes).with_context(|| format!("parsing {path}"))?;
        let value = Rc::new(value);
        self.cache.borrow_mut().insert(path.to_owned(), value.clone());
        Ok(value)
    }

    pub fn exists(&self, path: &str) -> bool {
        self.cache.borrow().contains_key(path) || self.source.read(path).is_some()
    }

    /// An entry of a worldgen registry, e.g. `worldgen("density_function", "minecraft:y")`.
    pub fn worldgen(&self, registry: &str, id: &str) -> Result<Rc<Value>> {
        let (ns, path) = split_id(id);
        self.json(&format!("data/{ns}/worldgen/{registry}/{path}.json"))
    }

    /// An entry of a non-worldgen registry, e.g. `registry("dimension_type", "minecraft:overworld")`.
    pub fn registry(&self, registry: &str, id: &str) -> Result<Rc<Value>> {
        let (ns, path) = split_id(id);
        self.json(&format!("data/{ns}/{registry}/{path}.json"))
    }

    /// The block state ids of all blocks in a block tag (without the leading `#`).
    pub fn block_tag(&self, tag: &str) -> Result<Rc<HashSet<u32>>> {
        let key = full_id(tag);
        if let Some(v) = self.block_tags.borrow().get(&key) {
            return Ok(v.clone());
        }
        let mut blocks = HashSet::new();
        self.collect_tag("block", &key, &mut blocks, &mut Vec::new())?;
        let blocks = Rc::new(blocks);
        self.block_tags.borrow_mut().insert(key, blocks.clone());
        Ok(blocks)
    }

    fn collect_tag(&self, kind: &str, tag: &str, out: &mut HashSet<u32>, stack: &mut Vec<String>) -> Result<()> {
        if stack.iter().any(|t| t == tag) {
            return Err(anyhow!("cyclic tag {tag}"));
        }
        stack.push(tag.to_owned());
        let (ns, path) = split_id(tag);
        let file = format!("data/{ns}/tags/{kind}/{path}.json");
        let json = self.json(&file)?;
        let values = json.get("values").and_then(Value::as_array).ok_or_else(|| anyhow!("tag {tag} has no values"))?;
        for v in values {
            let (id, required) = match v {
                Value::String(s) => (s.as_str(), true),
                Value::Object(o) => (
                    o.get("id").and_then(Value::as_str).ok_or_else(|| anyhow!("tag entry without id in {tag}"))?,
                    o.get("required").and_then(Value::as_bool).unwrap_or(true),
                ),
                _ => return Err(anyhow!("bad tag entry in {tag}")),
            };
            if let Some(nested) = id.strip_prefix('#') {
                let nested = full_id(nested);
                let (nns, npath) = split_id(&nested);
                if !required && !self.exists(&format!("data/{nns}/tags/{kind}/{npath}.json")) {
                    continue;
                }
                self.collect_tag(kind, &nested, out, stack)?;
            } else {
                match paper_data::blocks::id(id) {
                    Some(block) => {
                        out.insert(block);
                    }
                    None if !required => {}
                    None => return Err(anyhow!("unknown block {id} in tag {tag}")),
                }
            }
        }
        stack.pop();
        Ok(())
    }

    /// Resolves a block list: `#tag`, a single block id, or an array of block ids.
    pub fn block_set(&self, v: &Value) -> Result<HashSet<u32>> {
        match v {
            Value::String(s) => {
                if let Some(tag) = s.strip_prefix('#') {
                    Ok((*self.block_tag(tag)?).clone())
                } else {
                    let id = paper_data::blocks::id(s).ok_or_else(|| anyhow!("unknown block {s}"))?;
                    Ok(HashSet::from([id]))
                }
            }
            Value::Array(a) => {
                let mut out = HashSet::new();
                for e in a {
                    out.extend(self.block_set(e)?);
                }
                Ok(out)
            }
            _ => Err(anyhow!("bad block list {v}")),
        }
    }
}

/// Parses a block state as written in worldgen data: either `"minecraft:stone"`,
/// `"minecraft:x[prop=value]"` or `{"Name": ..., "Properties": {...}}`.
pub fn parse_block_state(v: &Value) -> Result<u32> {
    match v {
        Value::String(s) => paper_data::blocks::parse_state(s).ok_or_else(|| anyhow!("unknown block state {s}")),
        Value::Object(o) => {
            let name = o.get("Name").or_else(|| o.get("name")).and_then(Value::as_str).ok_or_else(|| anyhow!("block state without Name: {v}"))?;
            let mut state = paper_data::blocks::default_state(name).ok_or_else(|| anyhow!("unknown block {name}"))?;
            if let Some(props) = o.get("Properties").or_else(|| o.get("properties")).and_then(Value::as_object) {
                for (k, val) in props {
                    let val = match val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    state = paper_data::blocks::with_property(state, k, &val).ok_or_else(|| anyhow!("bad property {k}={val} for {name}"))?;
                }
            }
            Ok(state)
        }
        _ => Err(anyhow!("bad block state {v}")),
    }
}

/// Small helpers for reading JSON fields.
pub trait JsonExt {
    fn f64_or(&self, key: &str, default: f64) -> f64;
    fn i64_or(&self, key: &str, default: i64) -> i64;
    fn bool_or(&self, key: &str, default: bool) -> bool;
    fn str_field(&self, key: &str) -> Result<&str>;
    fn field(&self, key: &str) -> Result<&Value>;
}

impl JsonExt for Value {
    fn f64_or(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(Value::as_f64).unwrap_or(default)
    }

    fn i64_or(&self, key: &str, default: i64) -> i64 {
        self.get(key).and_then(Value::as_i64).unwrap_or(default)
    }

    fn bool_or(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    fn str_field(&self, key: &str) -> Result<&str> {
        self.get(key).and_then(Value::as_str).ok_or_else(|| anyhow!("missing string field {key} in {}", short(self)))
    }

    fn field(&self, key: &str) -> Result<&Value> {
        self.get(key).ok_or_else(|| anyhow!("missing field {key} in {}", short(self)))
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 200 {
        format!("{}...", &s[..200])
    } else {
        s
    }
}
