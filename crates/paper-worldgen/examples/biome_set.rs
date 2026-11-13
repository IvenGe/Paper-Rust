//! Prints the biomes present in the 3x3 chunks around a few chunks (seed 0).

use paper_worldgen::{DataSource, Dimension, Stage, WorldGenerator};
use std::collections::BTreeSet;
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
    let path = std::env::var("PAPER_RS_VANILLA_JAR").unwrap_or_else(|_| "/home/user/upstream/vanilla/inner/META-INF/versions/26.3/server-26.3.jar".into());
    let jar = Jar(Mutex::new(zip::ZipArchive::new(std::fs::File::open(&path)?)?));
    for dim in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let g = WorldGenerator::new(&jar, dim, 0)?;
        for (cx, cz) in [(8, 8), (-13, -7), (14, 5), (-9, 12), (99, 70)] {
            let mut set = BTreeSet::new();
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let c = g.generate_until(cx + dx, cz + dz, Stage::Biomes);
                    for b in &c.biomes {
                        set.insert(g.biome_name(*b).to_string());
                    }
                }
            }
            println!("{dim:?} ({cx},{cz}): {set:?}");
        }
    }
    Ok(())
}
