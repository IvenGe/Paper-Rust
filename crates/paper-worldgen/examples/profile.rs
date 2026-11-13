//! Generates a few chunks up to a stage, for use under a profiler.
//!
//! `profile <overworld|nether|end> <biomes|noise|surface|carvers|features> <chunks>`

use paper_worldgen::{DataSource, Dimension, Stage, WorldGenerator};
use std::io::Read;
use std::sync::Mutex;

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
    let dim = match args.get(1).map(String::as_str) {
        Some("nether") => Dimension::Nether,
        Some("end") => Dimension::End,
        _ => Dimension::Overworld,
    };
    let stage = match args.get(2).map(String::as_str) {
        Some("biomes") => Stage::Biomes,
        Some("noise") => Stage::Noise,
        Some("surface") => Stage::Surface,
        _ => Stage::Carvers,
    };
    let n: i32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);
    let path = std::env::var("PAPER_RS_VANILLA_JAR").unwrap_or_else(|_| "/home/user/upstream/vanilla/inner/META-INF/versions/26.3/server-26.3.jar".into());
    let jar = Jar(Mutex::new(zip::ZipArchive::new(std::fs::File::open(&path)?)?));
    let g = WorldGenerator::new(&jar, dim, 12345)?;
    for i in 0..n {
        std::hint::black_box(g.generate_until(i, i * 3, stage));
    }
    Ok(())
}
