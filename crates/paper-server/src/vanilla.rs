//! Vanilla game data read from Mojang's server jar: loot tables, recipes, biome definitions,
//! world generation settings and similar data-pack files.
//!
//! Like Paperclip, the server uses the official jar but does not ship it: it is taken from
//! `world.vanilla-jar` in `config/paper-rs.toml`, or downloaded once from Mojang (checked
//! against the SHA-1 Mojang publishes) into `cache/`. The jar is only read as a zip archive;
//! none of its code is run. Since 1.18 Mojang's `server.jar` is a bundler with the real jar
//! inside (`META-INF/versions/<version>/server-<version>.jar`); that inner jar is extracted
//! into `cache/` the first time.

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Read-only access to the data files of the vanilla server jar.
pub struct VanillaData {
    path: PathBuf,
    archive: Mutex<zip::ZipArchive<File>>,
    names: Vec<String>,
}

impl VanillaData {
    /// Opens a jar; a bundler jar is unpacked to `cache_dir` first.
    pub fn open(path: &Path, cache_dir: &Path) -> Result<Arc<VanillaData>> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut archive = zip::ZipArchive::new(file).with_context(|| format!("{} is not a jar", path.display()))?;
        let inner = archive.file_names().find(|n| n.starts_with("META-INF/versions/") && n.ends_with(".jar")).map(str::to_owned);
        if let Some(inner) = inner {
            let target = cache_dir.join(Path::new(&inner).file_name().unwrap());
            if !target.is_file() {
                std::fs::create_dir_all(cache_dir)?;
                let tmp = target.with_extension("jar.tmp");
                {
                    let mut entry = archive.by_name(&inner)?;
                    let mut out = File::create(&tmp)?;
                    std::io::copy(&mut entry, &mut out)?;
                    out.flush()?;
                }
                std::fs::rename(&tmp, &target)?;
            }
            return Self::open(&target, cache_dir);
        }
        if archive.by_name("data/minecraft/worldgen/noise_settings/overworld.json").is_err() {
            bail!("{} does not contain the vanilla data files", path.display());
        }
        let names = archive.file_names().filter(|n| n.starts_with("data/")).map(str::to_owned).collect();
        Ok(Arc::new(VanillaData { path: path.to_owned(), archive: Mutex::new(archive), names }))
    }

    /// Finds the jar the configuration names, else the cached copy, else downloads it.
    pub fn locate(configured: &str, root: &Path, download: bool) -> Result<Arc<VanillaData>> {
        let cache = root.join("cache");
        if !configured.is_empty() {
            let path = if Path::new(configured).is_absolute() { PathBuf::from(configured) } else { root.join(configured) };
            return Self::open(&path, &cache);
        }
        let version = paper_data::VERSION_NAME;
        for candidate in [cache.join(format!("server-{version}.jar")), cache.join(format!("mojang_{version}.jar"))] {
            if candidate.is_file() {
                return Self::open(&candidate, &cache);
            }
        }
        if !download {
            bail!("no vanilla server jar: set world.vanilla-jar in config/paper-rs.toml");
        }
        let target = cache.join(format!("mojang_{version}.jar"));
        download_server_jar(version, &target)?;
        Self::open(&target, &cache)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The bytes of a file, e.g. `data/minecraft/recipe/bread.json`.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let mut archive = self.archive.lock();
        let mut entry = archive.by_name(name).ok()?;
        let mut out = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut out).ok()?;
        Some(out)
    }

    pub fn read_json(&self, name: &str) -> Option<Value> {
        serde_json::from_slice(&self.read(name)?).ok()
    }

    /// Files whose path starts with `prefix`.
    pub fn list<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.names.iter().map(String::as_str).filter(move |n| n.starts_with(prefix) && !n.ends_with('/'))
    }

    /// Every JSON file of a registry directory, keyed by resource id: `list_json("recipe")`
    /// yields (`minecraft:bread`, {...}).
    pub fn registry(&self, directory: &str) -> Vec<(String, Value)> {
        let prefix = format!("data/minecraft/{directory}/");
        let names: Vec<String> = self.list(&prefix).filter(|n| n.ends_with(".json")).map(str::to_owned).collect();
        names
            .into_iter()
            .filter_map(|n| {
                let id = format!("minecraft:{}", n[prefix.len()..].trim_end_matches(".json"));
                Some((id, self.read_json(&n)?))
            })
            .collect()
    }
}

/// Downloads the official server jar for `version`, verifying Mojang's SHA-1.
fn download_server_jar(version: &str, target: &Path) -> Result<()> {
    log::info!("Downloading the vanilla {version} server jar from Mojang (used for game data only)");
    let manifest: Value = ureq::get(MANIFEST).call().context("fetching Mojang's version manifest")?.into_json()?;
    let url = manifest["versions"]
        .as_array()
        .and_then(|v| v.iter().find(|e| e["id"] == version))
        .and_then(|e| e["url"].as_str())
        .ok_or_else(|| anyhow!("version {version} is not in Mojang's manifest"))?;
    let meta: Value = ureq::get(url).call()?.into_json()?;
    let server = &meta["downloads"]["server"];
    let (jar_url, sha1) = (server["url"].as_str().ok_or_else(|| anyhow!("no server download"))?, server["sha1"].as_str().unwrap_or(""));
    let mut bytes = Vec::new();
    ureq::get(jar_url).call()?.into_reader().take(512 * 1024 * 1024).read_to_end(&mut bytes)?;
    use sha1::Digest;
    let digest: String = sha1::Sha1::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if !sha1.is_empty() && digest != sha1 {
        bail!("the downloaded server jar has SHA-1 {digest}, Mojang publishes {sha1}");
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("jar.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, target)?;
    Ok(())
}

/// The jar used by tests: `PAPER_RS_VANILLA_JAR`, else the development copy if present.
pub fn test_jar() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PAPER_RS_VANILLA_JAR") {
        return Some(PathBuf::from(p));
    }
    let dev = PathBuf::from("/home/user/upstream/vanilla/inner/META-INF/versions/26.3/server-26.3.jar");
    dev.is_file().then_some(dev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_vanilla_data() {
        let Some(jar) = test_jar() else {
            eprintln!("skipping: no vanilla jar");
            return;
        };
        let cache = std::env::temp_dir().join(format!("paper-rs-vanilla-{}", std::process::id()));
        let data = VanillaData::open(&jar, &cache).unwrap();
        let bread = data.read_json("data/minecraft/recipe/bread.json").unwrap();
        assert_eq!(bread["type"], "minecraft:crafting_shaped");
        assert!(data.registry("loot_table/blocks").len() > 1000);
        let _ = std::fs::remove_dir_all(&cache);
    }
}
