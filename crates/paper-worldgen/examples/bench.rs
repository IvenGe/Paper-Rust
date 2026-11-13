//! Times terrain generation (stages 1-4) and decoration.
//!
//! `cargo run --release -p paper-worldgen --example bench [jar] [chunks per side]`

use paper_worldgen::{DataSource, Dimension, WorldGenerator};
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
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| {
        std::env::var("PAPER_RS_VANILLA_JAR").unwrap_or_else(|_| "/home/user/upstream/vanilla/inner/META-INF/versions/26.3/server-26.3.jar".into())
    });
    let side: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(12);
    let jar = Jar(Mutex::new(zip::ZipArchive::new(std::fs::File::open(&path)?)?));
    for dim in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let t = Instant::now();
        let g = WorldGenerator::new(&jar, dim, 12345)?;
        let build = t.elapsed();
        // Warm up thread-local pools.
        g.generate_terrain(1000, 1000);
        let t = Instant::now();
        for x in 0..side {
            for z in 0..side {
                std::hint::black_box(g.generate_terrain(x, z));
            }
        }
        let single = t.elapsed() / (side * side) as u32;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let t = Instant::now();
        std::thread::scope(|s| {
            for i in 0..threads {
                let g = &g;
                s.spawn(move || {
                    for x in 0..side {
                        for z in 0..side {
                            if ((x * side + z) as usize) % threads == i {
                                std::hint::black_box(g.generate_terrain(x + 100, z));
                            }
                        }
                    }
                });
            }
        });
        let multi = t.elapsed();
        println!(
            "{dim:?}: build {build:?}; terrain {single:?}/chunk single-threaded; {} chunks on {threads} threads in {multi:?} ({:?}/chunk)",
            side * side,
            multi / (side * side) as u32
        );
    }
    Ok(())
}
