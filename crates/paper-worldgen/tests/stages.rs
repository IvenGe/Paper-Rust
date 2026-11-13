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

/// Chunks whose vanilla terrain was shaped by a structure (structures are out of scope): the
/// overworld chunk (14, 5) is affected by a structure's beardifier. Pumpkin with structures
/// disabled differs from vanilla in exactly these chunks too.
fn affected_by_structures(dimension: Dimension, cx: i32, cz: i32) -> bool {
    dimension == Dimension::Overworld && (cx, cz) == (14, 5)
}

/// For the structure-affected chunks: SHA-256 of Pumpkin's output with structures disabled
/// (Pumpkin commit e9dc12e9). Not a vanilla reference, but it pins down everything except
/// the structures.
const PUMPKIN_WITHOUT_STRUCTURES: &[(Dimension, &str, i32, i32, &str)] = &[
    (Dimension::Overworld, "minecraft:noise", 14, 5, "cfab3430f490a8584aa387c61a2880d1492df1601408091dd821caf745f289c7"),
    (Dimension::Overworld, "minecraft:surface", 14, 5, "efa0620f86b542372fd443b36fd953c00c4a56af9a5e2332bcd9c0b9d1389729"),
    (Dimension::Overworld, "minecraft:carvers", 14, 5, "97906a81c21f6932b2177eef4d3bed159dcbd3bcc97c5ba5fc4d0e9073e73b4e"),
    (Dimension::Overworld, "minecraft:features", -13, -7, "d244af3cd3f013c678097da142b795a4c2ea215be20cfb04e240fddb49474762"),
    (Dimension::Overworld, "minecraft:features", 14, 5, "0bb46d1f47b9f4e66892d1f3dd44cab87c5b8ecbee24262d9495409c6195eb72"),
    (Dimension::Overworld, "minecraft:features", -9, 12, "035042456c36f05b9445c96b0208424ae3eac862922685d3a381a6af9ebdb1cf"),
    (Dimension::Overworld, "minecraft:features", 99, 70, "d688a48ebe07be90121fa7e043e71ffe001d4e5fad7a8034e9637f03a9a516c2"),
    (Dimension::Nether, "minecraft:features", 14, 5, "545ab037d4aa8b08c14d4bf0bfe22b0956959165187c0f8c939e08e9b953409e"),
];

fn check_stage(file: &str, dimension: Dimension, stage: Stage, key: &str) {
    let data = common::json_fixture(file);
    let Some(g) = generator(dimension, 0) else { return };
    let mut failures = Vec::new();
    for entry in data.as_array().unwrap() {
        let (cx, cz) = (entry["x"].as_i64().unwrap() as i32, entry["z"].as_i64().unwrap() as i32);
        if let Some(p) = PUMPKIN_WITHOUT_STRUCTURES.iter().find(|p| p.0 == dimension && p.1 == key && (p.2, p.3) == (cx, cz)) {
            let (sha, _) = hashes(&g.generate_until(cx, cz, stage));
            if sha != p.4 {
                failures.push(format!("({cx}, {cz}) differs from Pumpkin without structures"));
            }
            continue;
        }
        if affected_by_structures(dimension, cx, cz) {
            continue;
        }
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
