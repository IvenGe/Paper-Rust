//! The initial spawn search.

mod common;

use common::generator;
use paper_worldgen::Dimension;

#[test]
fn overworld_spawn_is_on_the_surface() {
    for seed in [0, 1, 12345] {
        let Some(g) = generator(Dimension::Overworld, seed) else { return };
        let start = std::time::Instant::now();
        let origin = g.spawn_origin_chunk();
        let (x, y, z) = g.find_spawn();
        eprintln!("seed {seed}: origin chunk {origin:?}, spawn ({x}, {y}, {z}) in {:?}", start.elapsed());
        // Within the 11x11 chunks searched around the climate origin.
        assert!((x >> 4) - origin.0 <= 5 && origin.0 - (x >> 4) <= 5);
        assert!((z >> 4) - origin.1 <= 5 && origin.1 - (z >> 4) <= 5);
        assert!(y > g.sea_level() - 8 && y < 200, "spawn height {y}");
    }
}

#[test]
fn end_spawn_is_the_platform() {
    let Some(g) = generator(Dimension::End, 0) else { return };
    assert_eq!(g.find_spawn(), (100, 50, 0));
}
