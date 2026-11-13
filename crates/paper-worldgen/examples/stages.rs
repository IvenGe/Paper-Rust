//! Times each terrain stage separately (cumulative stage timings minus the previous ones).

use paper_worldgen::{DataSource, Dimension, Stage, WorldGenerator};
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
    let path = std::env::var("PAPER_RS_VANILLA_JAR").unwrap_or_else(|_| "/home/user/upstream/vanilla/inner/META-INF/versions/26.3/server-26.3.jar".into());
    let jar = Jar(Mutex::new(zip::ZipArchive::new(std::fs::File::open(&path)?)?));
    let side = 8;
    for dim in [Dimension::Overworld, Dimension::Nether] {
        let g = WorldGenerator::new(&jar, dim, 12345)?;
        g.generate_terrain(1000, 1000);
        let mut prev = std::time::Duration::ZERO;
        for stage in [Stage::Biomes, Stage::Noise, Stage::Surface, Stage::Carvers] {
            let t = Instant::now();
            for x in 0..side {
                for z in 0..side {
                    std::hint::black_box(g.generate_until(x, z, stage));
                }
            }
            let per = t.elapsed() / (side * side) as u32;
            println!("{dim:?} {stage:?}: {:?} (+{:?})", per, per.saturating_sub(prev));
            prev = per;
        }
    }
    Ok(())
}
