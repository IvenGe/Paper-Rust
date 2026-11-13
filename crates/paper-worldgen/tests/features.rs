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

/// Chunks outside the fixtures with features the fixtures do not cover, against Pumpkin
/// (commit e9dc12e9) with structures disabled. Not vanilla references: they pin the ported
/// behaviour down.
const PUMPKIN_CROSS_CHECK: &[(Dimension, i32, i32, &str)] = &[
    // End spikes (one per pillar), the arrival platform, a return gateway, chorus plants,
    // small end islands and the main island's center.
    (Dimension::End, 2, 0, "9a2c34045976591f60fce7788111e2a6a8f40aa32925692e3c0a672fd23a83f4"),
    (Dimension::End, 2, 1, "ac32db594c9e02ae07d6c7a0e5d2c87c496b1f8d17128d8e2d34e6d0a3e72a5b"),
    (Dimension::End, 0, 2, "cc8bb1e2347f03812aacb82b5a298b91103cc6d2ccf38e498d2002f6a4c841a5"),
    (Dimension::End, -1, 2, "51ae48a4452ef40fa943b00aef88042ad0cde71bbd3ee148cd93d678c7fd8d14"),
    (Dimension::End, -3, 1, "521ffe4c535398e614308bf84ca70b1f58da17a10b7ce735bdd6bbbff03bd40e"),
    (Dimension::End, -3, -1, "8853509fa73b787ecf6b10b38f08f9b0828eac1c67fcb5fc3d1df764a1da8980"),
    (Dimension::End, -3, -2, "d8e3610c8a8bd5e69b0d5e3d7e43e460917d93348fdeb1a5cc595eb03a377fba"),
    (Dimension::End, -1, -3, "0362cbb32c45746918ea3cd28fddc0556a6063e56930a52c46ed821472b728b7"),
    (Dimension::End, 0, -3, "7d47b08457b9f8f967400bc7325b82d5814f2f5ee579a872e1bd717ef691901a"),
    (Dimension::End, 2, -2, "a7f76996308749b949dd60c96e8385bdd2c23f1bfe7326ac65a914e462098007"),
    (Dimension::End, 6, 0, "3669dbd3755ee3cbc6ed951728270f0ab974498063b6f0de837e6ab4275e011f"),
    (Dimension::End, 68, -5, "cf2f9a5d99446eb826be8cddc630a566fee7e80539b761a31efb760cad3133e8"),
    (Dimension::End, -76, 0, "bef4f108fdedddab7d48877ce4739ae666ac72af33af00cbb1a04990978d3ef1"),
    (Dimension::End, -67, -80, "ae46ffeca9580b8770238c8e60cb741027cd9ebba194613872ffd0204121e52f"),
    (Dimension::End, -63, -80, "bb20c7761deae20497b9b04496f0d1b8a14c86c3f2aa7f526e865b1334c5f7b2"),
    (Dimension::End, -59, -80, "efd5b5b40d9cc6f49b586d419c0b53b883803da603c2a2e818809b9a505d780c"),
    (Dimension::End, 0, 0, "9d6a6e3bbd379cfb1b1030fb35954292239196f6ab3f210f916e92ba35a0b248"),
    // Overworld biomes with code-defined features. For these Pumpkin was first corrected
    // where it departs from vanilla 26.3: the ice spike's edge draw and replaceable tag,
    // the iceberg cut-out's and snow cap's random draws, the forest rock's
    // `forest_rock_can_place_on` tag and the huge mushrooms' height and clearance checks.
    // Ice spikes, deep frozen ocean, old growth pine taiga, mushroom fields, dark forest,
    // warm ocean (corals).
    (Dimension::Overworld, 208, -80, "b7c748c510298ac9383411187ddd67a951be353baf235b3f32d6dd9f9e13edc1"),
    (Dimension::Overworld, 208, -76, "ada3dd03d0065ed17c6ff72d55cd05258f56cc9747e201a91028a8b93d86aa12"),
    (Dimension::Overworld, -152, 152, "c00b55511021b8834e1a938f4053df5f8a295db25f9cc2fde0efda0d67e904ca"),
    (Dimension::Overworld, 172, 16, "c2a0a1ca2e17aa88793b6f42f19cce6b62d76e829582e1e566a7cc936b4649e6"),
    (Dimension::Overworld, -40, -60, "3293b219561aca17b462099ea91c7d22033d82a84853615fb7120b1cb004a4c7"),
    (Dimension::Overworld, -84, 72, "74344211e7b103505eb8afe525c9ad720872b2915cf70be830093646cc583bc3"),
    (Dimension::Overworld, 16, 4, "c66819538844cf67621dfb4dff5d71a7f0bc7329ffed180802bba987b01dc942"),
    (Dimension::Overworld, 20, 4, "dca117360b65af5213ce6299d454facb12e2563ea4c7e1d8fd511ccb7811a885"),
];

#[test]
fn pumpkin_cross_check() {
    let mut failures = Vec::new();
    for &(dimension, cx, cz, expected) in PUMPKIN_CROSS_CHECK {
        let Some(chunk) = decorated(dimension, cx, cz) else { return };
        let (sha, _) = hashes(&chunk);
        if sha != expected {
            failures.push(format!("{dimension:?} ({cx}, {cz})"));
        }
    }
    assert!(failures.is_empty(), "differ from Pumpkin: {}", failures.join(", "));
}

#[test]
fn end_spike_crystals() {
    let Some(g) = generator(Dimension::End, 0) else { return };
    let mut chunks = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks.push(g.generate_terrain(2 + dx, dz));
        }
    }
    let mut region = Region::new(2, 0, chunks);
    g.decorate(2, 0, &mut region);
    assert_eq!(region.entities.len(), 1, "{:?}", region.entities);
    let paper_worldgen::Entity::EndCrystal { x, z, .. } = region.entities[0];
    assert_eq!((x, z), (42.5, 0.5));
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
