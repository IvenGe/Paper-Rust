//! The feature types, parsed from their JSON configuration.

mod basic;
mod cave;
mod end;
mod misc;
mod nether;
mod terrain;
mod tree;

use super::parse::Parser;
use super::Place;
use anyhow::Result;
use serde_json::Value;

/// Parses a feature of type `t`; `None` for types without an implementation.
pub(crate) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    if let Some(f) = basic::parse(p, t, v)? {
        return Ok(Some(f));
    }
    if let Some(f) = terrain::parse(p, t, v)? {
        return Ok(Some(f));
    }
    if let Some(f) = tree::parse(p, t, v)? {
        return Ok(Some(f));
    }
    if let Some(f) = cave::parse(p, t, v)? {
        return Ok(Some(f));
    }
    if let Some(f) = nether::parse(p, t, v)? {
        return Ok(Some(f));
    }
    if let Some(f) = end::parse(p, t, v)? {
        return Ok(Some(f));
    }
    misc::parse(p, t, v)
}

/// Placed features listed under `key`.
pub(crate) fn placed_list(p: &mut Parser, v: &Value, key: &str) -> Result<Vec<std::sync::Arc<super::placement::PlacedFeature>>> {
    use crate::data::JsonExt;
    match v.field(key)? {
        Value::Array(a) => a.iter().map(|f| p.placed(f)).collect(),
        single => Ok(vec![p.placed(single)?]),
    }
}
