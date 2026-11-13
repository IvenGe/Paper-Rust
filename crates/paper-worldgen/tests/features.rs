//! The features stage compared with vanilla 26.3 (whole-chunk hashes after decoration).
//!
//! Vanilla's feature-stage chunks include structures, which this crate does not generate.
//! Chunks without structure pieces are compared with vanilla; for the others the reference
//! is Pumpkin (commit e9dc12e9) with structures disabled, which is not a vanilla reference
//! but pins down everything except the structures.

mod common;

use common::generator;
use paper_worldgen::{Dimension, ProtoChunk, Region};
use sha2::{Digest, Sha256};

fn hashes(chunk: &ProtoChunk) -> (String, Vec<String>) {
    let mut sha = Sha256::new();
    let mut sections = Vec::new();
    for s in 0..(chunk.height / 16) {
        let mut fnv: u64 = 0xcbf2_9ce4_8422_2325;
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let state = chunk.get(x, chunk.min_y + s * 16 + y, z);
                    fnv = (fnv ^ state as u64).wrapping_mul(0x0000_0100_0000_01B3);
                    sha.update(state.to_be_bytes());
                }
            }
        }
        sections.push(fnv.to_string());
    }
    let hex = sha.finalize().iter().map(|b| format!("{b:02x}")).collect();
    (hex, sections)
}

/// Feature-stage references that are Pumpkin without structures (the vanilla chunk has
/// structure pieces): (dimension, x, z, sha256).
const PUMPKIN_WITHOUT_STRUCTURES: &[(Dimension, i32, i32, &str)] = &[
    (Dimension::Overworld, -13, -7, "d244af3cd3f013c678097da142b795a4c2ea215be20cfb04e240fddb49474762"),
    (Dimension::Overworld, 14, 5, "0bb46d1f47b9f4e66892d1f3dd44cab87c5b8ecbee24262d9495409c6195eb72"),
    (Dimension::Overworld, -9, 12, "035042456c36f05b9445c96b0208424ae3eac862922685d3a381a6af9ebdb1cf"),
    (Dimension::Overworld, 99, 70, "d688a48ebe07be90121fa7e043e71ffe001d4e5fad7a8034e9637f03a9a516c2"),
    (Dimension::Nether, 14, 5, "545ab037d4aa8b08c14d4bf0bfe22b0956959165187c0f8c939e08e9b953409e"),
];

/// Decorates `(cx, cz)` with freshly generated neighbours and returns the center chunk.
pub fn decorated(dimension: Dimension, cx: i32, cz: i32) -> Option<ProtoChunk> {
    let g = generator(dimension, 0)?;
    let mut chunks = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks.push(g.generate_terrain(cx + dx, cz + dz));
        }
    }
    let mut region = Region::new(cx, cz, chunks);
    g.decorate(cx, cz, &mut region);
    Some(region.into_chunks().swap_remove(4))
}

/// Compares every chunk of a fixture; returns (matched, compared) and prints mismatches.
fn check(file: &str, dimension: Dimension) -> (usize, Vec<String>) {
    let data = common::json_fixture(file);
    let mut matched = 0;
    let mut failures = Vec::new();
    if generator(dimension, 0).is_none() {
        return (0, failures);
    }
    for entry in data.as_array().unwrap() {
        let (cx, cz) = (entry["x"].as_i64().unwrap() as i32, entry["z"].as_i64().unwrap() as i32);
        let chunk = decorated(dimension, cx, cz).unwrap();
        let (sha, sections) = hashes(&chunk);
        let pumpkin = PUMPKIN_WITHOUT_STRUCTURES.iter().find(|p| p.0 == dimension && (p.1, p.2) == (cx, cz));
        let expected = &entry["stages"]["minecraft:features"];
        let ok = match pumpkin {
            Some(p) => sha == p.3,
            None => sha == expected["hash"].as_str().unwrap(),
        };
        if ok {
            matched += 1;
            eprintln!("{dimension:?} ({cx}, {cz}): match");
            continue;
        }
        let bad: Vec<usize> = sections
            .iter()
            .zip(expected["sections"].as_array().unwrap())
            .enumerate()
            .filter(|(_, (a, b))| a.as_str() != b.as_str().unwrap())
            .map(|(i, _)| i)
            .collect();
        let what = if pumpkin.is_some() { "differs from Pumpkin without structures; vanilla sections differing" } else { "sections differing" };
        let msg = format!("{dimension:?} ({cx}, {cz}) {what}: {bad:?}");
        eprintln!("{msg}");
        failures.push(msg);
    }
    (matched, failures)
}

#[test]
fn overworld_features() {
    let (_, failures) = check("stage_hashes", Dimension::Overworld);
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn nether_features() {
    let (_, failures) = check("nether_stage_hashes", Dimension::Nether);
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn end_features() {
    let (_, failures) = check("end_stage_hashes", Dimension::End);
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn decoration_changes_blocks() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let before = g.generate_terrain(8, 8);
    let after = decorated(Dimension::Overworld, 8, 8).unwrap();
    let changed = before.blocks.iter().zip(&after.blocks).filter(|(a, b)| a != b).count();
    eprintln!("blocks changed by decoration: {changed}; unsupported: {:?}", g.unsupported_features());
    assert!(changed > 100);
}
