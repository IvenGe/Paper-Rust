//! Terrain stages compared with vanilla 26.3 reference dumps.

mod common;

use common::{block_name, chunk_dump, generator};
use paper_worldgen::{Dimension, ProtoChunk, Stage};

/// Compares a chunk with a dump indexed `(x * height + y) * 16 + z` starting at the chunk's
/// minimum y, and checks that everything above the dumped window is air.
fn mismatches(chunk: &ProtoChunk, expected: &[u16], name: &str) -> usize {
    let window = expected.len() / 256;
    let mut count = 0;
    for x in 0..16 {
        for ly in 0..window {
            for z in 0..16 {
                let want = expected[(x * window + ly) * 16 + z] as u32;
                let y = chunk.min_y + ly as i32;
                let got = chunk.get(x as i32, y, z as i32);
                if got != want {
                    if count < 10 {
                        eprintln!("[{name}] ({x}, {y}, {z}): got {} expected {}", block_name(got), block_name(want));
                    }
                    count += 1;
                }
            }
        }
    }
    for ly in window..chunk.height as usize {
        for x in 0..16 {
            for z in 0..16 {
                let y = chunk.min_y + ly as i32;
                assert!(paper_data::blocks::is_air(chunk.get(x, y, z)), "[{name}] block above the dumped window at ({x}, {y}, {z})");
            }
        }
    }
    if count > 0 {
        eprintln!("[{name}] {count} mismatches");
    }
    count
}

fn check(dimension: Dimension, seed: i64, cx: i32, cz: i32, stage: Stage, fixture: &str, allowed: usize) {
    let Some(g) = generator(dimension, seed) else { return };
    let chunk = g.generate_until(cx, cz, stage);
    let n = mismatches(&chunk, &chunk_dump(fixture), fixture);
    assert!(n <= allowed, "{fixture}: {n} mismatches (allowed {allowed})");
}

#[test]
fn noise_overworld() {
    check(Dimension::Overworld, 0, 0, 0, Stage::Noise, "noise_no_blend_no_beard_0_0", 0);
    check(Dimension::Overworld, 0, 7, 4, Stage::Noise, "noise_no_blend_no_beard_7_4", 0);
    check(Dimension::Overworld, 0, -595, 544, Stage::Noise, "noise_no_blend_no_beard_-595_544", 0);
    check(Dimension::Overworld, 0, -119, 183, Stage::Noise, "noise_no_blend_no_beard_-119_183", 0);
    check(Dimension::Overworld, 13579, -6, 11, Stage::Noise, "noise_no_blend_no_beard_13579_-6_11", 0);
    check(Dimension::Overworld, 13579, -2, 15, Stage::Noise, "noise_no_blend_no_beard_13579_-2_15", 0);
    check(Dimension::Overworld, 13579, -7, 9, Stage::Noise, "noise_no_blend_no_beard_13579_-7_9", 0);
}

#[test]
fn noise_nether() {
    check(Dimension::Nether, 0, 0, 0, Stage::Noise, "noise_nether_no_blend_no_beard_0_0", 0);
    check(Dimension::Nether, 0, 7, 4, Stage::Noise, "noise_nether_no_blend_no_beard_7_4", 0);
}

#[test]
fn noise_end() {
    check(Dimension::End, 0, 0, 0, Stage::Noise, "noise_end_no_blend_no_beard_0_0", 0);
    check(Dimension::End, 0, 7, 4, Stage::Noise, "noise_end_no_blend_no_beard_7_4", 0);
}

/// `[{x, z, data: [[quart x, quart y, quart z, biome network id], ...]}, ...]`
fn biome_points(name: &str, dimension: Dimension, seed: i64) {
    let Some(g) = generator(dimension, seed) else { return };
    let data = common::json_fixture(name);
    let mut wrong = 0;
    for entry in data.as_array().unwrap() {
        let cx = entry["x"].as_i64().unwrap() as i32;
        let cz = entry["z"].as_i64().unwrap() as i32;
        let chunk = g.generate_until(cx, cz, Stage::Biomes);
        for p in entry["data"].as_array().unwrap() {
            let p = p.as_array().unwrap();
            let (qx, qy, qz) = (p[0].as_i64().unwrap() as i32, p[1].as_i64().unwrap() as i32, p[2].as_i64().unwrap() as i32);
            let id = p[3].as_u64().unwrap() as u8;
            let got = chunk.biome(qx, qy, qz);
            if got != id {
                if wrong < 10 {
                    eprintln!("[{name}] chunk ({cx}, {cz}) quart ({qx}, {qy}, {qz}): got {} expected {}", g.biome_name(got), g.biome_name(id));
                }
                wrong += 1;
            }
        }
    }
    assert_eq!(wrong, 0, "{name}: {wrong} biome mismatches");
}

/// Quantized climate parameters at single positions: `[x, y, z, t, h, c, e, d, w]`.
#[test]
fn climate_samples() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let data = common::json_fixture("multi_noise_sample_no_blend_no_beard_0_0_0");
    let mut sampler = g.climate_sampler();
    for p in data.as_array().unwrap() {
        let v: Vec<i64> = p.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect();
        let c = sampler.sample(v[0] as i32, v[1] as i32, v[2] as i32);
        assert_eq!(
            [c.temperature, c.humidity, c.continentalness, c.erosion, c.depth, c.weirdness],
            [v[3], v[4], v[5], v[6], v[7], v[8]],
            "climate at {:?}",
            &v[..3]
        );
    }
}

#[test]
fn biomes_chunk_5_5() {
    biome_points("biome_no_blend_no_beard_0", Dimension::Overworld, 0);
}

#[test]
fn biomes_grid_8x8() {
    biome_points("grid_8x8_biomes", Dimension::Overworld, 0);
}

/// MD5 over the biome names of whole chunks, compared with vanilla for 400 random chunks per
/// dimension.
fn biome_hashes(dimension: Dimension, key: &str) {
    use md5::{Digest, Md5};
    let data = common::json_fixture("biome_hashes");
    let seed = data["seed"].as_i64().unwrap();
    let Some(g) = generator(dimension, seed) else { return };
    let d = &data[key];
    let (min_s, max_s) = (d["min_section_y"].as_i64().unwrap() as i32, d["max_section_y"].as_i64().unwrap() as i32);
    let mut wrong = 0;
    for h in d["hashes"].as_array().unwrap() {
        let (cx, cz) = (h[0].as_i64().unwrap() as i32, h[1].as_i64().unwrap() as i32);
        let chunk = g.generate_until(cx, cz, Stage::Biomes);
        let mut md5 = Md5::new();
        for sy in min_s..=max_s {
            md5.update([sy as u8]);
            for y in 0..4 {
                for z in 0..4 {
                    for x in 0..4 {
                        let b = g.biome_name(chunk.biome(x, sy * 4 + y, z));
                        md5.update(b.strip_prefix("minecraft:").unwrap_or(b).as_bytes());
                    }
                }
            }
        }
        let got: String = md5.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != h[2].as_str().unwrap() {
            wrong += 1;
            if wrong < 5 {
                eprintln!("[{key}] biome hash mismatch at chunk ({cx}, {cz})");
            }
        }
    }
    assert_eq!(wrong, 0, "{key}: {wrong} chunks with different biomes");
}

#[test]
fn biome_hashes_overworld() {
    biome_hashes(Dimension::Overworld, "overworld");
}

#[test]
fn biome_hashes_nether() {
    biome_hashes(Dimension::Nether, "the_nether");
}

#[test]
fn biome_hashes_end() {
    biome_hashes(Dimension::End, "the_end");
}
