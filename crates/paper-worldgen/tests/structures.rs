//! Structure placement and structure starts compared with vanilla 26.3.
//!
//! `structure_starts.json.gz` is a vanilla dump (from Pumpkin's `assets/tests/`, seed 0):
//! the starts vanilla generated in 2000 chunks per dimension, with every piece's type,
//! generation depth, orientation and bounding box (and for jigsaw pieces their position,
//! template and junctions).

mod common;

use common::generator;
use paper_worldgen::Dimension;
use serde_json::Value;
use std::collections::BTreeMap;

fn bb(v: &Value) -> [i32; 6] {
    let a = v.as_array().unwrap();
    std::array::from_fn(|i| a[i].as_i64().unwrap() as i32)
}

/// Compares the starts of the dump for the structures this crate generates; returns the
/// number of matching starts per structure.
fn check_starts(dimension: Dimension, key: &str) -> BTreeMap<String, usize> {
    let mut matched = BTreeMap::new();
    let Some(g) = generator(dimension, 0) else { return matched };
    let data = common::json_fixture("structure_starts");
    let supported = g.supported_structures();
    let mut failures = Vec::new();
    for start in data[key].as_array().unwrap() {
        let id = start["s"].as_str().unwrap();
        if !supported.contains(&id) {
            continue;
        }
        let (x, z) = (start["x"].as_i64().unwrap() as i32, start["z"].as_i64().unwrap() as i32);
        let starts = g.structure_starts(x, z);
        let Some(s) = starts.iter().find(|s| s.id() == id) else {
            failures.push(format!("{id} at ({x}, {z}): no start (have {:?})", starts.iter().map(|s| s.id().to_owned()).collect::<Vec<_>>()));
            continue;
        };
        let b = s.bbox;
        let ours = [b.min_x, b.min_y, b.min_z, b.max_x, b.max_y, b.max_z];
        if ours != bb(&start["bb"]) {
            failures.push(format!("{id} at ({x}, {z}): bounding box {ours:?}, vanilla {:?}", bb(&start["bb"])));
            continue;
        }
        let pieces = s.pieces();
        let expected = start["p"].as_array().unwrap();
        if pieces.len() != expected.len() {
            failures.push(format!("{id} at ({x}, {z}): {} pieces, vanilla {}", pieces.len(), expected.len()));
            continue;
        }
        let mut bad = None;
        for (i, (p, e)) in pieces.iter().zip(expected).enumerate() {
            let pb = [p.bbox.min_x, p.bbox.min_y, p.bbox.min_z, p.bbox.max_x, p.bbox.max_y, p.bbox.max_z];
            let kind = p.kind.strip_prefix("minecraft:").unwrap_or(p.kind);
            if pb != bb(&e["bb"]) || kind != e["t"].as_str().unwrap() || p.gen_depth as i64 != e["d"].as_i64().unwrap() || p.orientation as i64 != e["o"].as_i64().unwrap() {
                bad = Some(format!("{id} at ({x}, {z}) piece {i}: {kind} {pb:?} depth {} orientation {}, vanilla {e}", p.gen_depth, p.orientation));
                break;
            }
        }
        match bad {
            Some(m) => failures.push(m),
            None => *matched.entry(id.to_owned()).or_insert(0) += 1,
        }
    }
    eprintln!("{dimension:?}: matched {matched:?}");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
    matched
}

#[test]
fn overworld_starts() {
    check_starts(Dimension::Overworld, "overworld");
}

#[test]
fn nether_starts() {
    check_starts(Dimension::Nether, "the_nether");
}

#[test]
fn end_starts() {
    check_starts(Dimension::End, "the_end");
}
