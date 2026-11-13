//! Nether features.

use crate::feature::parse::Parser;
use crate::feature::Place;
use anyhow::Result;
use serde_json::Value;

pub(super) fn parse(_p: &mut Parser, _t: &str, _v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(None)
}
