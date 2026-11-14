//! Shared test helpers: the vanilla jar as a data source and fixture loading.
#![allow(dead_code)]

use paper_worldgen::{DataSource, Dimension, WorldGenerator};
use std::io::Read;
use std::sync::{Mutex, OnceLock};

const DEFAULT_JAR: &str = "/home/user/upstream/vanilla/inner/META-INF/versions/26.3/server-26.3.jar";

/// The vanilla server jar read as a zip of data files.
pub struct JarSource {
    archive: Mutex<zip::ZipArchive<std::fs::File>>,
}

impl DataSource for JarSource {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let mut archive = self.archive.lock().ok()?;
        let mut file = archive.by_name(path).ok()?;
        let mut out = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut out).ok()?;
        Some(out)
    }
}

/// The jar from `PAPER_RS_VANILLA_JAR` (or the default path), or `None` with a note on stderr.
pub fn jar() -> Option<&'static JarSource> {
    static JAR: OnceLock<Option<JarSource>> = OnceLock::new();
    JAR.get_or_init(|| {
        let path = std::env::var("PAPER_RS_VANILLA_JAR").unwrap_or_else(|_| DEFAULT_JAR.to_owned());
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("skipping: vanilla jar {path} not readable ({e}); set PAPER_RS_VANILLA_JAR");
                return None;
            }
        };
        match zip::ZipArchive::new(file) {
            Ok(archive) => Some(JarSource { archive: Mutex::new(archive) }),
            Err(e) => {
                eprintln!("skipping: {path} is not a zip ({e})");
                None
            }
        }
    })
    .as_ref()
}

/// A generator for the given dimension and seed, cached per test binary.
pub fn generator(dimension: Dimension, seed: i64) -> Option<&'static WorldGenerator> {
    type Cache = Mutex<Vec<(Dimension, i64, &'static WorldGenerator)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let jar = jar()?;
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let mut cache = cache.lock().unwrap();
    if let Some((_, _, g)) = cache.iter().find(|(d, s, _)| *d == dimension && *s == seed) {
        return Some(*g);
    }
    let g: &'static WorldGenerator = Box::leak(Box::new(WorldGenerator::new(jar, dimension, seed).expect("building the generator")));
    cache.push((dimension, seed, g));
    Some(g)
}

/// Like [`generator`], with structure generation turned off (for references made without
/// structures).
#[allow(dead_code)]
pub fn generator_without_structures(dimension: Dimension, seed: i64) -> Option<&'static WorldGenerator> {
    type Cache = Mutex<Vec<(Dimension, i64, &'static WorldGenerator)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let jar = jar()?;
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let mut cache = cache.lock().unwrap();
    if let Some((_, _, g)) = cache.iter().find(|(d, s, _)| *d == dimension && *s == seed) {
        return Some(*g);
    }
    let mut g = WorldGenerator::new(jar, dimension, seed).expect("building the generator");
    g.set_structures_enabled(false);
    let g: &'static WorldGenerator = Box::leak(Box::new(g));
    cache.push((dimension, seed, g));
    Some(g)
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name)
}

fn gunzip(name: &str) -> Vec<u8> {
    let file = std::fs::File::open(fixture_path(name)).unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(file).read_to_end(&mut out).expect("gzip fixture");
    out
}

/// A block dump: states indexed `(x * height + y) * 16 + z`.
pub fn chunk_dump(name: &str) -> Vec<u16> {
    gunzip(&format!("{name}.u16.gz")).as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect()
}

pub fn json_fixture(name: &str) -> serde_json::Value {
    serde_json::from_slice(&gunzip(&format!("{name}.json.gz"))).expect("json fixture")
}

pub fn block_name(state: u32) -> String {
    paper_data::blocks::state_to_string(state)
}
