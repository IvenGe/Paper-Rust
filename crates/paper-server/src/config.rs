//! Server configuration.
//!
//! * `server.properties` keeps vanilla's file name, keys and defaults, so an existing
//!   server directory can be reused. Keys this server does not (yet) act on are preserved
//!   when the file is rewritten.
//! * `config/paper-rs.toml` holds what is specific to this implementation: thread counts,
//!   the world generator and the separate-mode plugin host.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The player-facing game modes, with their protocol ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GameMode {
    Survival = 0,
    Creative = 1,
    Adventure = 2,
    Spectator = 3,
}

impl GameMode {
    pub fn id(self) -> i32 {
        self as i32
    }

    pub fn from_id(id: i32) -> Option<GameMode> {
        Some(match id {
            0 => GameMode::Survival,
            1 => GameMode::Creative,
            2 => GameMode::Adventure,
            3 => GameMode::Spectator,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            GameMode::Survival => "survival",
            GameMode::Creative => "creative",
            GameMode::Adventure => "adventure",
            GameMode::Spectator => "spectator",
        }
    }

    /// Accepts names, legacy numeric ids and the one-letter abbreviations.
    pub fn parse(s: &str) -> Option<GameMode> {
        Some(match s.to_ascii_lowercase().as_str() {
            "survival" | "s" | "0" => GameMode::Survival,
            "creative" | "c" | "1" => GameMode::Creative,
            "adventure" | "a" | "2" => GameMode::Adventure,
            "spectator" | "sp" | "3" => GameMode::Spectator,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Peaceful = 0,
    Easy = 1,
    Normal = 2,
    Hard = 3,
}

impl Difficulty {
    pub fn id(self) -> u8 {
        self as u8
    }

    pub fn name(self) -> &'static str {
        match self {
            Difficulty::Peaceful => "peaceful",
            Difficulty::Easy => "easy",
            Difficulty::Normal => "normal",
            Difficulty::Hard => "hard",
        }
    }

    pub fn parse(s: &str) -> Option<Difficulty> {
        Some(match s.to_ascii_lowercase().as_str() {
            "peaceful" | "p" | "0" => Difficulty::Peaceful,
            "easy" | "e" | "1" => Difficulty::Easy,
            "normal" | "n" | "2" => Difficulty::Normal,
            "hard" | "h" | "3" => Difficulty::Hard,
            _ => return None,
        })
    }
}

/// `server.properties`.
#[derive(Clone, Debug)]
pub struct ServerProperties {
    pub server_ip: String,
    pub server_port: u16,
    pub motd: String,
    pub max_players: i32,
    pub online_mode: bool,
    pub level_name: String,
    pub level_seed: String,
    pub level_type: String,
    pub gamemode: GameMode,
    pub force_gamemode: bool,
    pub difficulty: Difficulty,
    pub hardcore: bool,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub network_compression_threshold: i32,
    pub white_list: bool,
    pub enforce_whitelist: bool,
    pub enable_status: bool,
    pub hide_online_players: bool,
    pub prevent_proxy_connections: bool,
    pub allow_flight: bool,
    /// `allow-nether`: create the Nether (`<level-name>_nether`) and let portals lead there.
    pub allow_nether: bool,
    pub player_idle_timeout: i32,
    pub op_permission_level: u8,
    pub spawn_protection: i32,
    pub accepts_transfers: bool,
    pub log_ips: bool,
    /// Every key that was in the file, including ones this server does not act on.
    pub raw: BTreeMap<String, String>,
}

/// Keys written to a fresh `server.properties`, with vanilla's defaults.
const DEFAULTS: &[(&str, &str)] = &[
    ("accepts-transfers", "false"),
    ("allow-flight", "false"),
    ("allow-nether", "true"),
    ("difficulty", "easy"),
    ("enable-status", "true"),
    ("enforce-whitelist", "false"),
    ("force-gamemode", "false"),
    ("gamemode", "survival"),
    ("hardcore", "false"),
    ("hide-online-players", "false"),
    ("level-name", "world"),
    ("level-seed", ""),
    ("level-type", "minecraft:normal"),
    ("log-ips", "true"),
    ("max-players", "20"),
    ("motd", "A Minecraft Server"),
    ("network-compression-threshold", "256"),
    ("online-mode", "true"),
    ("op-permission-level", "4"),
    ("player-idle-timeout", "0"),
    ("prevent-proxy-connections", "false"),
    ("server-ip", ""),
    ("server-port", "25565"),
    ("simulation-distance", "10"),
    ("spawn-protection", "16"),
    ("view-distance", "10"),
    ("white-list", "false"),
];

/// Parses the Java `.properties` subset vanilla writes: `key=value` lines, `#`/`!`
/// comments and backslash escapes.
pub fn parse_properties(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let (key, value) = match line.find(['=', ':']) {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (line, ""),
        };
        out.insert(unescape(key.trim()), unescape(value.trim_start()));
    }
    out
}

fn unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('u') => {
                // Java escapes are UTF-16 code units; a supplementary character arrives as
                // two consecutive \uXXXX escapes forming a surrogate pair.
                let mut units = vec![u16::from_str_radix(&chars.by_ref().take(4).collect::<String>(), 16).unwrap_or(0xFFFD)];
                if (0xD800..0xDC00).contains(&units[0]) {
                    let mut ahead = chars.clone();
                    if ahead.next() == Some('\\') && ahead.next() == Some('u') {
                        if let Ok(low) = u16::from_str_radix(&ahead.by_ref().take(4).collect::<String>(), 16) {
                            if (0xDC00..0xE000).contains(&low) {
                                units.push(low);
                                chars = ahead;
                            }
                        }
                    }
                }
                out.extend(char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')));
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7E => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

impl ServerProperties {
    pub fn from_map(mut raw: BTreeMap<String, String>) -> Self {
        for (key, value) in DEFAULTS {
            raw.entry((*key).to_owned()).or_insert_with(|| (*value).to_owned());
        }
        let get = |key: &str| raw.get(key).cloned().unwrap_or_default();
        let get_bool = |key: &str, default: bool| raw.get(key).and_then(|v| v.trim().parse::<bool>().ok()).unwrap_or(default);
        let get_int = |key: &str, default: i32| raw.get(key).and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(default);
        Self {
            server_ip: get("server-ip").trim().to_owned(),
            server_port: get_int("server-port", 25565).clamp(0, 65535) as u16,
            motd: get("motd"),
            max_players: get_int("max-players", 20).max(0),
            online_mode: get_bool("online-mode", true),
            level_name: {
                let name = get("level-name");
                if name.trim().is_empty() {
                    "world".to_owned()
                } else {
                    name.trim().to_owned()
                }
            },
            level_seed: get("level-seed").trim().to_owned(),
            level_type: get("level-type").trim().to_ascii_lowercase(),
            gamemode: GameMode::parse(get("gamemode").trim()).unwrap_or(GameMode::Survival),
            force_gamemode: get_bool("force-gamemode", false),
            difficulty: Difficulty::parse(get("difficulty").trim()).unwrap_or(Difficulty::Easy),
            hardcore: get_bool("hardcore", false),
            view_distance: get_int("view-distance", 10).clamp(2, 32),
            simulation_distance: get_int("simulation-distance", 10).clamp(2, 32),
            network_compression_threshold: get_int("network-compression-threshold", 256),
            white_list: get_bool("white-list", false),
            enforce_whitelist: get_bool("enforce-whitelist", false),
            enable_status: get_bool("enable-status", true),
            hide_online_players: get_bool("hide-online-players", false),
            prevent_proxy_connections: get_bool("prevent-proxy-connections", false),
            allow_flight: get_bool("allow-flight", false),
            allow_nether: get_bool("allow-nether", true),
            player_idle_timeout: get_int("player-idle-timeout", 0).max(0),
            op_permission_level: get_int("op-permission-level", 4).clamp(0, 4) as u8,
            spawn_protection: get_int("spawn-protection", 16),
            accepts_transfers: get_bool("accepts-transfers", false),
            log_ips: get_bool("log-ips", true),
            raw,
        }
    }

    /// Loads `server.properties`, creating it with defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        let existed = path.exists();
        let raw = if existed {
            let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            // The format is nominally ISO-8859-1 with \u escapes; accept UTF-8 as vanilla does.
            parse_properties(&String::from_utf8_lossy(&bytes))
        } else {
            BTreeMap::new()
        };
        let props = Self::from_map(raw);
        if !existed {
            props.save(path)?;
        }
        Ok(props)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let mut out = String::from("#Minecraft server properties\n");
        out.push_str(&format!("#{}\n", chrono::Local::now().format("%a %b %d %H:%M:%S %Z %Y")));
        for (key, value) in &self.raw {
            out.push_str(&escape(key));
            out.push('=');
            out.push_str(&escape(value));
            out.push('\n');
        }
        fs::write(path, out).with_context(|| format!("writing {}", path.display()))
    }

    /// The numeric world seed: the configured value parsed as a number, otherwise Java's
    /// `String.hashCode` of the text (as vanilla does), otherwise a random one.
    pub fn seed(&self) -> i64 {
        let s = self.level_seed.trim();
        if s.is_empty() {
            return rand::random::<i64>();
        }
        if let Ok(n) = s.parse::<i64>() {
            return n;
        }
        s.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32)) as i64
    }
}

/// What to do when the plugin host process dies unexpectedly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnHostCrash {
    /// Stop the server (safe default: protection plugins are gone).
    Shutdown,
    /// Keep running without plugins.
    Continue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ThreadsConfig {
    /// Network I/O threads (0 = one per CPU core).
    pub io: usize,
    /// Worker threads for chunk generation, lighting, packet encoding and saving
    /// (0 = one per CPU core).
    pub workers: usize,
}

impl Default for ThreadsConfig {
    fn default() -> Self {
        Self { io: 0, workers: 0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct NetworkConfig {
    /// zlib level for packet compression (1 fastest - 9 smallest).
    pub compression_level: u32,
    /// Seconds a connection may take to get from handshake to play.
    pub login_timeout_seconds: u64,
    /// Seconds without any packet before a connection is dropped.
    pub read_timeout_seconds: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self { compression_level: 4, login_timeout_seconds: 30, read_timeout_seconds: 30 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct WorldConfig {
    /// `auto` follows `level-type` in server.properties (`minecraft:flat` -> flat, anything
    /// else -> noise). `flat` and `noise` force a generator.
    pub generator: String,
    /// Ticks between automatic world saves (0 disables autosave).
    pub autosave_interval_ticks: u64,
    /// Mojang's server jar, read for game data (loot tables, recipes, world generation).
    /// Empty: use `cache/`, downloading it from Mojang if needed.
    pub vanilla_jar: String,
    /// Allow downloading the server jar from Mojang when it is not present.
    pub download_vanilla_jar: bool,
    /// Create the End (`<level-name>_the_end`), like Bukkit's `settings.allow-end`.
    pub allow_end: bool,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self { generator: "auto".into(), autosave_interval_ticks: 6000, vanilla_jar: String::new(), download_vanilla_jar: true, allow_end: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PluginsConfig {
    /// Run Java plugins in a separate JVM process ("separate mode").
    pub enabled: bool,
    /// The Java runtime to launch (Java 25 or newer, as required by the Paper API).
    pub java: String,
    /// Directory holding the plugin host jars (relative to the server directory, or absolute).
    pub host_dir: String,
    /// Extra JVM arguments for the plugin host.
    pub jvm_args: Vec<String>,
    /// Directory scanned for plugin jars.
    pub plugins_dir: String,
    pub startup_timeout_seconds: u64,
    pub on_host_crash: OnHostCrash,
    /// What `Bukkit.getName()` reports to plugins.
    pub server_name: String,
    /// Warn when a single call into the plugin host blocks the main thread this long.
    pub slow_call_warning_millis: u64,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            java: "java".into(),
            host_dir: "plugin-host".into(),
            jvm_args: vec!["-Xmx1G".into()],
            plugins_dir: "plugins".into(),
            startup_timeout_seconds: 120,
            on_host_crash: OnHostCrash::Shutdown,
            server_name: "Paper".into(),
            slow_call_warning_millis: 1000,
        }
    }
}

/// `config/paper-rs.toml`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PaperRsConfig {
    pub threads: ThreadsConfig,
    pub network: NetworkConfig,
    pub world: WorldConfig,
    pub plugins: PluginsConfig,
}

const CONFIG_HEADER: &str = "\
# paper-rs configuration.
#
# server.properties keeps the vanilla settings (port, MOTD, game mode, ...).
# This file holds what is specific to the Rust server.
#
# [threads]  io / workers: 0 picks one thread per CPU core.
# [plugins]  Java plugins run in a separate JVM process (\"separate mode\") that talks to
#            the server over a local socket. Set enabled = false to run without plugins.

";

impl PaperRsConfig {
    /// Loads the file, creating it with defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            return toml::from_str(&text).with_context(|| format!("parsing {}", path.display()));
        }
        let config = Self::default();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = toml::to_string_pretty(&config).context("serialising default configuration")?;
        fs::write(path, format!("{CONFIG_HEADER}{body}")).with_context(|| format!("writing {}", path.display()))?;
        Ok(config)
    }
}

/// Everything the server is configured with.
#[derive(Clone, Debug)]
pub struct Config {
    /// The server directory all relative paths resolve against.
    pub root: PathBuf,
    pub properties: ServerProperties,
    pub paper: PaperRsConfig,
}

impl Config {
    pub fn load(root: &Path) -> Result<Self> {
        let properties = ServerProperties::load(&root.join("server.properties"))?;
        let paper = PaperRsConfig::load(&root.join("config").join("paper-rs.toml"))?;
        Ok(Self { root: root.to_path_buf(), properties, paper })
    }

    /// A configuration for tests: offline mode, ephemeral port, plugins disabled.
    pub fn for_tests(root: &Path) -> Self {
        let mut raw = BTreeMap::new();
        raw.insert("online-mode".to_owned(), "false".to_owned());
        raw.insert("server-port".to_owned(), "0".to_owned());
        raw.insert("server-ip".to_owned(), "127.0.0.1".to_owned());
        raw.insert("level-type".to_owned(), "minecraft:flat".to_owned());
        raw.insert("gamemode".to_owned(), "creative".to_owned());
        raw.insert("view-distance".to_owned(), "4".to_owned());
        let mut paper = PaperRsConfig::default();
        paper.plugins.enabled = false;
        paper.threads.io = 2;
        paper.threads.workers = 2;
        // Tests never download: they use a local jar when one is available.
        paper.world.download_vanilla_jar = false;
        if let Some(jar) = crate::vanilla::test_jar() {
            paper.world.vanilla_jar = jar.display().to_string();
        }
        Self { root: root.to_path_buf(), properties: ServerProperties::from_map(raw), paper }
    }

    pub fn resolve(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        }
    }

    pub fn io_threads(&self) -> usize {
        match self.paper.threads.io {
            0 => std::thread::available_parallelism().map_or(2, |n| n.get()),
            n => n,
        }
    }

    pub fn worker_threads(&self) -> usize {
        match self.paper.threads.workers {
            0 => std::thread::available_parallelism().map_or(2, |n| n.get()),
            n => n,
        }
    }

    /// Compression threshold, or `None` when compression is disabled (negative value).
    pub fn compression_threshold(&self) -> Option<u32> {
        u32::try_from(self.properties.network_compression_threshold).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_properties() {
        let text = "#comment\n! other comment\nmotd=Hello \\u00a7aWorld\\: \\\\o/\nserver-port = 25570\nwhite-list:true\nempty=\nnoequals\n";
        let map = parse_properties(text);
        assert_eq!(map["motd"], "Hello §aWorld: \\o/");
        assert_eq!(map["server-port"], "25570");
        assert_eq!(map["white-list"], "true");
        assert_eq!(map["empty"], "");
        assert_eq!(map["noequals"], "");

        let props = ServerProperties::from_map(map);
        assert_eq!(props.server_port, 25570);
        assert!(props.white_list);
        assert!(props.online_mode, "vanilla default");
        assert_eq!(props.gamemode, GameMode::Survival);
        assert_eq!(props.view_distance, 10);
        // Unknown keys survive.
        assert_eq!(props.raw["noequals"], "");
    }

    #[test]
    fn escape_round_trip() {
        for s in ["plain", "§a colour", "back\\slash", "new\nline", "emoji \u{1F600}"] {
            assert_eq!(unescape(&escape(s)), s);
        }
    }

    #[test]
    fn seed_parsing() {
        let mut p = ServerProperties::from_map(BTreeMap::new());
        p.level_seed = "12345".into();
        assert_eq!(p.seed(), 12345);
        p.level_seed = "-7".into();
        assert_eq!(p.seed(), -7);
        // Java: "hello".hashCode() == 99162322
        p.level_seed = "hello".into();
        assert_eq!(p.seed(), 99162322);
    }

    #[test]
    fn default_toml_round_trips() {
        let text = toml::to_string_pretty(&PaperRsConfig::default()).unwrap();
        let back: PaperRsConfig = toml::from_str(&format!("{CONFIG_HEADER}{text}")).unwrap();
        assert!(back.plugins.enabled);
        assert_eq!(back.plugins.on_host_crash, OnHostCrash::Shutdown);
        // Partial files fall back to defaults for everything else.
        let partial: PaperRsConfig = toml::from_str("[plugins]\nenabled = false\n").unwrap();
        assert!(!partial.plugins.enabled);
        assert_eq!(partial.network.compression_level, 4);
    }

    #[test]
    fn game_mode_parsing() {
        assert_eq!(GameMode::parse("Creative"), Some(GameMode::Creative));
        assert_eq!(GameMode::parse("3"), Some(GameMode::Spectator));
        assert_eq!(GameMode::parse("nope"), None);
        assert_eq!(GameMode::from_id(2), Some(GameMode::Adventure));
    }
}
