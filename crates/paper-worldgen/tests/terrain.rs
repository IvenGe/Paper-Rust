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
