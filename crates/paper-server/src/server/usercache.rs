//! The name cache (vanilla's `usercache.json`): the names and UUIDs of players who joined,
//! most recently used first. It answers name lookups for offline players
//! (`Bukkit.getOfflinePlayer(String)`) without asking Mojang.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Vanilla keeps at most this many entries in the file.
const MAX_ENTRIES: usize = 1000;
/// Entries expire a month after the player was last seen, as in vanilla.
const EXPIRY_DAYS: i64 = 30;
const DATE_FORMAT: &str = "%Y-%m-%d %H:%M:%S %z";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UserCacheEntry {
    pub name: String,
    pub uuid: Uuid,
    #[serde(rename = "expiresOn", default)]
    pub expires_on: String,
}

/// When a player who joined since the server started first played and last logged in, in
/// milliseconds since the epoch (saved in their player data as `bukkit.firstPlayed` and
/// `Paper.LastLogin`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Session {
    pub first_played: i64,
    pub last_login: i64,
    /// They had a player data file when they joined.
    pub played_before: bool,
}

#[derive(Debug, Default)]
pub struct UserCache {
    /// `usercache.json`, or `None` for servers that keep nothing on disk (tests).
    path: Option<PathBuf>,
    /// Most recently used first.
    entries: Vec<UserCacheEntry>,
    sessions: std::collections::HashMap<Uuid, Session>,
}

impl UserCache {
    /// Reads `usercache.json` from the server directory (an empty `root` keeps the cache in
    /// memory). Expired entries are dropped, as vanilla does when it loads the file.
    pub fn load(root: &Path) -> Self {
        if root.as_os_str().is_empty() {
            return Self::default();
        }
        let path = root.join("usercache.json");
        let mut entries: Vec<UserCacheEntry> = match std::fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("Could not read {}: {e}; starting with an empty name cache", path.display());
                Vec::new()
            }),
            _ => Vec::new(),
        };
        let now = chrono::Local::now();
        entries.retain(|e| chrono::DateTime::parse_from_str(&e.expires_on, DATE_FORMAT).map_or(true, |until| until > now));
        Self { path: Some(path), entries, sessions: Default::default() }
    }

    pub fn session(&self, uuid: Uuid) -> Option<&Session> {
        self.sessions.get(&uuid)
    }

    /// Records a login.
    pub fn start_session(&mut self, uuid: Uuid, session: Session) {
        self.sessions.insert(uuid, session);
    }

    /// Records that a player was seen with this name, and saves the file.
    pub fn add(&mut self, name: &str, uuid: Uuid) {
        self.entries.retain(|e| e.uuid != uuid && !e.name.eq_ignore_ascii_case(name));
        let expires_on = (chrono::Local::now() + chrono::Duration::days(EXPIRY_DAYS)).format(DATE_FORMAT).to_string();
        self.entries.insert(0, UserCacheEntry { name: name.to_owned(), uuid, expires_on });
        self.entries.truncate(MAX_ENTRIES);
        self.save();
    }

    pub fn by_name(&self, name: &str) -> Option<&UserCacheEntry> {
        self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }

    pub fn by_uuid(&self, uuid: Uuid) -> Option<&UserCacheEntry> {
        self.entries.iter().find(|e| e.uuid == uuid)
    }

    pub fn entries(&self) -> &[UserCacheEntry] {
        &self.entries
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        match serde_json::to_string(&self.entries) {
            Ok(text) => {
                if let Err(e) = std::fs::write(path, text) {
                    log::warn!("Could not write {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("Could not serialise the name cache: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_first_and_case_insensitive() {
        let mut cache = UserCache::default();
        cache.add("Alice", Uuid::from_u128(1));
        cache.add("Bob", Uuid::from_u128(2));
        cache.add("alice", Uuid::from_u128(1));
        assert_eq!(cache.entries().len(), 2);
        assert_eq!(cache.entries()[0].name, "alice");
        assert_eq!(cache.by_name("BOB").map(|e| e.uuid), Some(Uuid::from_u128(2)));
        assert_eq!(cache.by_uuid(Uuid::from_u128(1)).map(|e| e.name.as_str()), Some("alice"));
    }

    #[test]
    fn reads_and_writes_vanilla_files() {
        let dir = std::env::temp_dir().join(format!("paper-rs-usercache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("usercache.json"), r#"[{"name":"Notch","uuid":"069a79f4-44e9-4726-a5be-fca90e38aaf5","expiresOn":"2999-01-01 00:00:00 +0000"},{"name":"Old","uuid":"00000000-0000-0000-0000-000000000001","expiresOn":"2000-01-01 00:00:00 +0000"}]"#).unwrap();
        let mut cache = UserCache::load(&dir);
        assert!(cache.by_name("notch").is_some());
        assert!(cache.by_name("Old").is_none(), "expired entries are dropped");
        cache.add("Steve", Uuid::from_u128(7));
        let again = UserCache::load(&dir);
        assert_eq!(again.entries()[0].name, "Steve");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
