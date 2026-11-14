//! Whole-chunk hashes after each generation stage, compared with vanilla 26.3.
//!
//! The fixtures hold, per chunk and stage, a SHA-256 over every block state (as big-endian
//! u32, section by section, then y, z, x) and an FNV-1a hash per section.

mod common;

use common::generator;
use paper_worldgen::{Dimension, ProtoChunk, Stage};
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

/// SHA-256 of Pumpkin's output (commit e9dc12e9) with structures disabled, for chunks whose
/// vanilla terrain a structure shaped (the trial chambers' beardifier at (14, 5)). Not a
/// vanilla reference: it checks that turning structures off leaves the rest of generation
/// as it was.
const PUMPKIN_WITHOUT_STRUCTURES: &[(Dimension, Stage, i32, i32, &str)] = &[
    (Dimension::Overworld, Stage::Noise, 14, 5, "cfab3430f490a8584aa387c61a2880d1492df1601408091dd821caf745f289c7"),
    (Dimension::Overworld, Stage::Surface, 14, 5, "efa0620f86b542372fd443b36fd953c00c4a56af9a5e2332bcd9c0b9d1389729"),
    (Dimension::Overworld, Stage::Carvers, 14, 5, "97906a81c21f6932b2177eef4d3bed159dcbd3bcc97c5ba5fc4d0e9073e73b4e"),
];

#[test]
fn structures_disabled() {
    let mut failures = Vec::new();
    for &(dimension, stage, cx, cz, expected) in PUMPKIN_WITHOUT_STRUCTURES {
        let Some(g) = common::generator_without_structures(dimension, 0) else { return };
        let (sha, _) = hashes(&g.generate_until(cx, cz, stage));
        if sha != expected {
            failures.push(format!("{dimension:?} {stage:?} ({cx}, {cz})"));
        }
    }
    assert!(failures.is_empty(), "differ from Pumpkin without structures: {}", failures.join(", "));
}

fn check_stage(file: &str, dimension: Dimension, stage: Stage, key: &str) {
    let data = common::json_fixture(file);
    let Some(g) = generator(dimension, 0) else { return };
    let mut failures = Vec::new();
    for entry in data.as_array().unwrap() {
        let (cx, cz) = (entry["x"].as_i64().unwrap() as i32, entry["z"].as_i64().unwrap() as i32);
        let expected = &entry["stages"][key];
        let chunk = g.generate_until(cx, cz, stage);
        let (sha, sections) = hashes(&chunk);
        if sha != expected["hash"].as_str().unwrap() {
            let bad: Vec<usize> = sections
                .iter()
                .zip(expected["sections"].as_array().unwrap())
                .enumerate()
                .filter(|(_, (a, b))| a.as_str() != b.as_str().unwrap())
                .map(|(i, _)| i)
                .collect();
            failures.push(format!("({cx}, {cz}) sections {bad:?}"));
        }
    }
    assert!(failures.is_empty(), "{file} {key}: {}", failures.join("; "));
}

#[test]
fn overworld_noise() {
    check_stage("stage_hashes", Dimension::Overworld, Stage::Noise, "minecraft:noise");
}

#[test]
fn overworld_surface() {
    check_stage("stage_hashes", Dimension::Overworld, Stage::Surface, "minecraft:surface");
}

#[test]
fn overworld_carvers() {
    check_stage("stage_hashes", Dimension::Overworld, Stage::Carvers, "minecraft:carvers");
}

#[test]
fn nether_carvers() {
    check_stage("nether_stage_hashes", Dimension::Nether, Stage::Carvers, "minecraft:carvers");
}

#[test]
fn end_carvers() {
    check_stage("end_stage_hashes", Dimension::End, Stage::Carvers, "minecraft:carvers");
}

#[test]
fn nether_noise() {
    check_stage("nether_stage_hashes", Dimension::Nether, Stage::Noise, "minecraft:noise");
}

#[test]
fn nether_surface() {
    check_stage("nether_stage_hashes", Dimension::Nether, Stage::Surface, "minecraft:surface");
}

#[test]
fn end_noise() {
    check_stage("end_stage_hashes", Dimension::End, Stage::Noise, "minecraft:noise");
}

#[test]
fn end_surface() {
    check_stage("end_stage_hashes", Dimension::End, Stage::Surface, "minecraft:surface");
}
