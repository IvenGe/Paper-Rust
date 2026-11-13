//! Generates and decorates one chunk, then prints its hash (the test fixtures' scheme),
//! what decoration produced and how long it took.
//!
//! `cargo run --release -p paper-worldgen --example decorate -- [overworld|nether|end] [chunk_x] [chunk_z] [seed]`

use paper_worldgen::{DataSource, Dimension, Region, WorldGenerator};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::sync::Mutex;
use std::time::Instant;

struct Jar(Mutex<zip::ZipArchive<std::fs::File>>);

impl DataSource for Jar {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let mut a = self.0.lock().ok()?;
        let mut f = a.by_name(path).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(v)
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dimension = match args.first().map(String::as_str).unwrap_or("overworld") {
        "nether" | "the_nether" => Dimension::Nether,
        "end" | "the_end" => Dimension::End,
        _ => Dimension::Overworld,
    };
    let arg = |i: usize, default: i64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(default);
    let (cx, cz, seed) = (arg(1, 0) as i32, arg(2, 0) as i32, arg(3, 0));
    let path = std::env::var("PAPER_RS_VANILLA_JAR").unwrap_or_else(|_| "/home/user/upstream/vanilla/inner/META-INF/versions/26.3/server-26.3.jar".into());
    let jar = Jar(Mutex::new(zip::ZipArchive::new(std::fs::File::open(&path)?)?));
    let g = WorldGenerator::new(&jar, dimension, seed)?;

    let start = Instant::now();
    let mut chunks = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks.push(g.generate_terrain(cx + dx, cz + dz));
        }
    }
    let terrain = start.elapsed();
    let before = chunks[4].blocks.clone();
    let mut region = Region::new(cx, cz, chunks);
    let start = Instant::now();
    g.decorate(cx, cz, &mut region);
    let decoration = start.elapsed();
    let center = region.center();

    let mut sha = Sha256::new();
    let mut sections = Vec::new();
    for s in 0..center.height / 16 {
        let mut fnv: u64 = 0xcbf2_9ce4_8422_2325;
        for y in center.min_y + s * 16..center.min_y + s * 16 + 16 {
            for z in 0..16 {
                for x in 0..16 {
                    let state = center.get(x, y, z);
                    fnv = (fnv ^ state as u64).wrapping_mul(0x0000_0100_0000_01B3);
                    sha.update(state.to_be_bytes());
                }
            }
        }
        sections.push(fnv);
    }
    let hash: String = sha.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let changed = before.iter().zip(&center.blocks).filter(|(a, b)| a != b).count();
    println!("{dimension:?} chunk ({cx}, {cz}) seed {seed}: {hash}");
    if std::env::var_os("SECTIONS").is_some() {
        println!("section hashes: {sections:?}");
    }
    if let Some(dir) = std::env::var_os("DUMP") {
        // The center chunk's state ids as little-endian u16, y-major then z then x.
        let mut out = Vec::new();
        for y in center.min_y..center.min_y + center.height {
            for z in 0..16 {
                for x in 0..16 {
                    out.extend_from_slice(&(center.get(x, y, z) as u16).to_le_bytes());
                }
            }
        }
        std::fs::write(std::path::Path::new(&dir).join(format!("mine_{cx}_{cz}.bin")), out)?;
    }
    println!("terrain of 9 chunks: {terrain:?}; decoration: {decoration:?}; blocks changed in the center: {changed}");
    for (pos, e) in &region.block_entities {
        println!("block entity at {pos:?}: {e:?}");
    }
    for e in &region.entities {
        println!("entity: {e:?}");
    }
    if !g.unsupported_features().is_empty() {
        println!("feature types without an implementation: {:?}", g.unsupported_features());
    }
    Ok(())
}
