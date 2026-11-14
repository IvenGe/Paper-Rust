//! Structure placement and structure starts compared with vanilla 26.3.
//!
//! `structure_starts.json.gz` is a vanilla dump (from Pumpkin's `assets/tests/`, seed 0):
//! the starts vanilla generated in 2000 chunks per dimension, with every piece's type,
//! generation depth, orientation and bounding box (and for jigsaw pieces their position,
//! template and junctions).

mod common;

use common::generator;
use paper_worldgen::structure::transform::Rotation;
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
        let pieces = s.pieces();
        let expected = start["p"].as_array().unwrap();
        let mut bad = None;
        if ours != bb(&start["bb"]) {
            bad = Some(format!("{id} at ({x}, {z}): bounding box {ours:?}, vanilla {:?}", bb(&start["bb"])));
        } else if pieces.len() != expected.len() {
            bad = Some(format!("{id} at ({x}, {z}): {} pieces, vanilla {}", pieces.len(), expected.len()));
        }
        for (i, (p, e)) in pieces.iter().zip(expected).enumerate() {
            let pb = [p.bbox.min_x, p.bbox.min_y, p.bbox.min_z, p.bbox.max_x, p.bbox.max_y, p.bbox.max_z];
            let kind = p.kind.strip_prefix("minecraft:").unwrap_or(p.kind);
            if pb != bb(&e["bb"]) || kind != e["t"].as_str().unwrap() || p.gen_depth as i64 != e["d"].as_i64().unwrap() || p.orientation as i64 != e["o"].as_i64().unwrap() {
                bad = Some(format!("{}{id} at ({x}, {z}) piece {i}: {kind} {pb:?} depth {} orientation {}, vanilla {e}", bad.map(|b| b + "; ").unwrap_or_default(), p.gen_depth, p.orientation));
                break;
            }
            if let Some(pos) = e.get("pos") {
                let jig = p.jigsaw.as_ref().expect("jigsaw piece");
                let rot = match p.rotation {
                    Some(Rotation::None) => "NONE",
                    Some(Rotation::Clockwise90) => "CLOCKWISE_90",
                    Some(Rotation::Clockwise180) => "CLOCKWISE_180",
                    Some(Rotation::CounterClockwise90) => "COUNTERCLOCKWISE_90",
                    None => "?",
                };
                let junctions: Vec<Value> = jig
                    .junctions
                    .iter()
                    .map(|j| serde_json::json!([j.source_x, j.source_ground_y, j.source_z, j.delta_y, if j.dest_rigid { "rigid" } else { "terrain_matching" }]))
                    .collect();
                let ours = serde_json::json!({
                    "pos": p.position.unwrap(),
                    "loc": p.element.clone(),
                    "rot": rot,
                    "gld": jig.ground_level_delta,
                    "j": junctions,
                });
                let theirs = serde_json::json!({"pos": pos, "loc": e.get("loc"), "rot": e["rot"], "gld": e["gld"], "j": e["j"]});
                if ours != theirs {
                    bad = Some(format!("{}{id} at ({x}, {z}) piece {i}: {ours}, vanilla {theirs}", bad.map(|b| b + "; ").unwrap_or_default()));
                    break;
                }
            }
        }
        if let (Some(_), Ok(dir)) = (&bad, std::env::var("STRUCTURE_DUMP_DIR")) {
            let ours: Vec<Value> = pieces
                .iter()
                .map(|p| serde_json::json!({"bb": [p.bbox.min_x, p.bbox.min_y, p.bbox.min_z, p.bbox.max_x, p.bbox.max_y, p.bbox.max_z], "pos": p.position, "loc": p.element, "j": p.jigsaw.as_ref().map(|j| j.junctions.iter().map(|j| [j.source_x, j.source_ground_y, j.source_z, j.delta_y]).collect::<Vec<_>>())}))
                .collect();
            let file = format!("{dir}/{}_{x}_{z}.json", id.replace(':', "_"));
            std::fs::write(file, serde_json::to_string(&serde_json::json!({"ours": ours, "vanilla": expected})).unwrap()).unwrap();
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

/// Decorates `(cx, cz)` with freshly generated neighbours.
fn decorate(g: &paper_worldgen::WorldGenerator, cx: i32, cz: i32) -> paper_worldgen::Region {
    let mut chunks = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks.push(g.generate_terrain(cx + dx, cz + dz));
        }
    }
    let mut region = paper_worldgen::Region::new(cx, cz, chunks);
    g.decorate(cx, cz, &mut region);
    region
}

/// Counts the blocks of `name` in the center chunk of a decorated region.
fn count(region: &paper_worldgen::Region, name: &str) -> usize {
    let c = region.center();
    let mut n = 0;
    for y in c.min_y..c.min_y + c.height {
        for z in 0..16 {
            for x in 0..16 {
                if paper_data::blocks::block_of_state(c.get(x, y, z)).name == name {
                    n += 1;
                }
            }
        }
    }
    n
}

#[test]
fn locate_finds_fixture_starts() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let data = common::json_fixture("structure_starts");
    for start in data["overworld"].as_array().unwrap() {
        let id = start["s"].as_str().unwrap();
        if !matches!(id, "minecraft:village_plains" | "minecraft:trial_chambers" | "minecraft:shipwreck") {
            continue;
        }
        let (x, z) = (start["x"].as_i64().unwrap() as i32, start["z"].as_i64().unwrap() as i32);
        // Searching from the start's chunk finds that start.
        let found = g.locate(id, x * 16 + 8, z * 16 + 8, 2).unwrap_or_else(|| panic!("{id} near ({x}, {z})"));
        assert_eq!((found.chunk_x, found.chunk_z), (x, z), "{id}");
    }
    // The nearest stronghold to the origin is a start on its ring.
    let rings = g.ring_positions("minecraft:stronghold");
    assert_eq!(rings.len(), 128);
    let s = g.locate("minecraft:stronghold", 0, 0, 100).expect("a stronghold");
    assert!(rings.contains(&(s.chunk_x, s.chunk_z)));
    assert!(g.structure_starts(s.chunk_x, s.chunk_z).iter().any(|st| st.id() == "minecraft:stronghold"));
}

#[test]
fn scattered_structures_are_placed() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let checks = [
        ("minecraft:swamp_hut", "minecraft:spruce_planks"),
        ("minecraft:jungle_pyramid", "minecraft:mossy_cobblestone"),
        ("minecraft:igloo", "minecraft:snow_block"),
        ("minecraft:desert_pyramid", "minecraft:cut_sandstone"),
    ];
    for (id, block) in checks {
        let Some(found) = g.locate(id, 0, 0, 100) else { panic!("no {id}") };
        let start = g.structure_starts(found.chunk_x, found.chunk_z).into_iter().find(|s| s.id() == id).unwrap();
        let piece = start.pieces().last().unwrap().bbox;
        let region = decorate(g, piece.min_x >> 4, piece.min_z >> 4);
        let n = count(&region, block);
        eprintln!("{id} at chunk ({}, {}): {n} {block}", found.chunk_x, found.chunk_z);
        assert!(n > 0, "{id}: no {block} placed");
    }
}

#[test]
fn template_features_are_placed() {
    // Fossils (`fossil_upper` in deserts, `fossil_lower` deep down) around the desert pyramid
    // the placement test finds.
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let mut bones = 0;
    'scan: for cz in -192..-184 {
        for cx in -4..4 {
            bones += count(&decorate(g, cx, cz), "minecraft:bone_block");
            if bones > 0 {
                break 'scan;
            }
        }
    }
    eprintln!("bone blocks: {bones}");
    assert!(bones > 0);
}
