//! Operators, the whitelist and bans, stored in vanilla's JSON files (`ops.json`,
//! `whitelist.json`, `banned-players.json`, `banned-ips.json`).

use crate::config::ServerProperties;
use paper_protocol::text::Component;
use paper_protocol::types::GameProfile;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OpEntry {
    pub uuid: Uuid,
    pub name: String,
    pub level: u8,
    #[serde(rename = "bypassesPlayerLimit", default)]
    pub bypasses_player_limit: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WhitelistEntry {
    pub uuid: Uuid,
    pub name: String,
}

fn default_source() -> String {
    "Server".into()
}

fn default_expires() -> String {
    "forever".into()
}

fn default_reason() -> String {
    "Banned by an operator.".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BanEntry {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default)]
    pub created: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_expires")]
    pub expires: String,
    #[serde(default = "default_reason")]
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IpBanEntry {
    pub ip: String,
    #[serde(default)]
    pub created: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_expires")]
    pub expires: String,
    #[serde(default = "default_reason")]
    pub reason: String,
}

const DATE_FORMAT: &str = "%Y-%m-%d %H:%M:%S %z";

/// The current time in the format vanilla writes to the ban lists.
pub fn now_string() -> String {
    chrono::Local::now().format(DATE_FORMAT).to_string()
}

/// Whether a ban with this `expires` value is still in force.
fn in_force(expires: &str) -> bool {
    if expires.eq_ignore_ascii_case("forever") || expires.trim().is_empty() {
        return true;
    }
    match chrono::DateTime::parse_from_str(expires, DATE_FORMAT) {
        Ok(until) => until > chrono::Local::now(),
        // An unreadable date must not silently lift a ban.
        Err(_) => true,
    }
}

#[derive(Debug, Default)]
pub struct AccessLists {
    root: PathBuf,
    pub ops: Vec<OpEntry>,
    pub whitelist: Vec<WhitelistEntry>,
    pub banned_players: Vec<BanEntry>,
    pub banned_ips: Vec<IpBanEntry>,
}

fn load_list<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Vec::new(),
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("Could not read {}: {e}. The file is left untouched and treated as empty.", path.display());
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

fn save_list<T: Serialize>(path: &Path, list: &[T]) {
    match serde_json::to_string_pretty(list) {
        Ok(text) => {
            if let Err(e) = fs::write(path, text) {
                log::warn!("Could not write {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("Could not serialise {}: {e}", path.display()),
    }
}

impl AccessLists {
    /// Loads the lists from `root`, creating empty files for those that do not exist.
    pub fn load(root: &Path) -> Self {
        let lists = Self {
            root: root.to_path_buf(),
            ops: load_list(&root.join("ops.json")),
            whitelist: load_list(&root.join("whitelist.json")),
            banned_players: load_list(&root.join("banned-players.json")),
            banned_ips: load_list(&root.join("banned-ips.json")),
        };
        for (file, missing) in [("ops.json", true), ("whitelist.json", true), ("banned-players.json", true), ("banned-ips.json", true)] {
            let path = root.join(file);
            if missing && !path.exists() {
                let _ = fs::write(&path, "[]");
            }
        }
        lists
    }

    /// Lists that are never written to disk (tests).
    pub fn in_memory() -> Self {
        Self::default()
    }

    fn persistent(&self) -> bool {
        !self.root.as_os_str().is_empty()
    }

    fn save_ops(&self) {
        if self.persistent() {
            save_list(&self.root.join("ops.json"), &self.ops);
        }
    }

    fn save_whitelist(&self) {
        if self.persistent() {
            save_list(&self.root.join("whitelist.json"), &self.whitelist);
        }
    }

    fn save_bans(&self) {
        if self.persistent() {
            save_list(&self.root.join("banned-players.json"), &self.banned_players);
            save_list(&self.root.join("banned-ips.json"), &self.banned_ips);
        }
    }

    /// Operator permission level of a player (0 if not an operator).
    pub fn op_level(&self, uuid: Uuid) -> u8 {
        self.ops.iter().find(|op| op.uuid == uuid).map_or(0, |op| op.level)
    }

    /// Makes a player an operator. Returns `false` if they already were one.
    pub fn op(&mut self, uuid: Uuid, name: &str, level: u8) -> bool {
        if self.ops.iter().any(|op| op.uuid == uuid) {
            return false;
        }
        self.ops.push(OpEntry { uuid, name: name.to_owned(), level, bypasses_player_limit: false });
        self.save_ops();
        true
    }

    /// Removes operator status. Returns `false` if the player was not an operator.
    pub fn deop(&mut self, uuid: Uuid) -> bool {
        let before = self.ops.len();
        self.ops.retain(|op| op.uuid != uuid);
        let changed = self.ops.len() != before;
        if changed {
            self.save_ops();
        }
        changed
    }

    /// The ban in force for a player, if any.
    pub fn ban_of(&self, uuid: Uuid) -> Option<&BanEntry> {
        self.banned_players.iter().find(|b| b.uuid == uuid && in_force(&b.expires))
    }

    pub fn is_whitelisted(&self, uuid: Uuid) -> bool {
        self.whitelist.iter().any(|e| e.uuid == uuid)
    }

    pub fn whitelist_add(&mut self, uuid: Uuid, name: &str) -> bool {
        if self.is_whitelisted(uuid) {
            return false;
        }
        self.whitelist.push(WhitelistEntry { uuid, name: name.to_owned() });
        self.save_whitelist();
        true
    }

    pub fn whitelist_remove(&mut self, name: &str) -> bool {
        let before = self.whitelist.len();
        self.whitelist.retain(|e| !e.name.eq_ignore_ascii_case(name));
        let changed = self.whitelist.len() != before;
        if changed {
            self.save_whitelist();
        }
        changed
    }

    pub fn reload_whitelist(&mut self) {
        if self.persistent() {
            self.whitelist = load_list(&self.root.join("whitelist.json"));
        }
    }

    pub fn ban(&mut self, uuid: Uuid, name: &str, source: &str, reason: Option<&str>) -> bool {
        if self.banned_players.iter().any(|b| b.uuid == uuid && in_force(&b.expires)) {
            return false;
        }
        self.banned_players.retain(|b| b.uuid != uuid);
        self.banned_players.push(BanEntry {
            uuid,
            name: name.to_owned(),
            created: now_string(),
            source: source.to_owned(),
            expires: default_expires(),
            reason: reason.map_or_else(default_reason, str::to_owned),
        });
        self.save_bans();
        true
    }

    pub fn pardon(&mut self, name: &str) -> bool {
        let before = self.banned_players.len();
        self.banned_players.retain(|b| !b.name.eq_ignore_ascii_case(name));
        let changed = self.banned_players.len() != before;
        if changed {
            self.save_bans();
        }
        changed
    }

    pub fn ban_ip(&mut self, ip: &str, source: &str, reason: Option<&str>) -> bool {
        if self.banned_ips.iter().any(|b| b.ip == ip && in_force(&b.expires)) {
            return false;
        }
        self.banned_ips.retain(|b| b.ip != ip);
        self.banned_ips.push(IpBanEntry { ip: ip.to_owned(), created: now_string(), source: source.to_owned(), expires: default_expires(), reason: reason.map_or_else(default_reason, str::to_owned) });
        self.save_bans();
        true
    }

    pub fn pardon_ip(&mut self, ip: &str) -> bool {
        let before = self.banned_ips.len();
        self.banned_ips.retain(|b| b.ip != ip);
        let changed = self.banned_ips.len() != before;
        if changed {
            self.save_bans();
        }
        changed
    }

    /// Decides whether a player may log in, in the order vanilla checks: player ban,
    /// whitelist, IP ban, player limit.
    pub fn check_login(&self, profile: &GameProfile, ip: IpAddr, online: usize, props: &ServerProperties) -> Result<(), Component> {
        if let Some(ban) = self.banned_players.iter().find(|b| b.uuid == profile.uuid && in_force(&b.expires)) {
            let mut message = Component::translatable("multiplayer.disconnect.banned.reason", vec![Component::text(ban.reason.clone())]);
            if !ban.expires.eq_ignore_ascii_case("forever") {
                message = message.append(Component::translatable("multiplayer.disconnect.banned.expiration", vec![Component::text(ban.expires.clone())]));
            }
            return Err(message);
        }
        let op = self.ops.iter().find(|op| op.uuid == profile.uuid);
        if props.white_list && op.is_none() && !self.is_whitelisted(profile.uuid) {
            return Err(Component::translatable("multiplayer.disconnect.not_whitelisted", vec![]));
        }
        let ip_text = ip.to_string();
        if let Some(ban) = self.banned_ips.iter().find(|b| b.ip == ip_text && in_force(&b.expires)) {
            return Err(Component::translatable("multiplayer.disconnect.banned_ip.reason", vec![Component::text(ban.reason.clone())]));
        }
        if online >= props.max_players as usize && !op.is_some_and(|op| op.bypasses_player_limit) {
            return Err(Component::translatable("multiplayer.disconnect.server_full", vec![]));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn profile(n: u128, name: &str) -> GameProfile {
        GameProfile { uuid: Uuid::from_u128(n), name: name.into(), properties: vec![] }
    }

    fn props(extra: &[(&str, &str)]) -> ServerProperties {
        ServerProperties::from_map(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>())
    }

    #[test]
    fn login_checks() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let mut lists = AccessLists::in_memory();
        let steve = profile(1, "Steve");
        assert!(lists.check_login(&steve, ip, 0, &props(&[])).is_ok());

        // Player limit, with the operator bypass flag.
        assert!(lists.check_login(&steve, ip, 20, &props(&[])).is_err());
        lists.ops.push(OpEntry { uuid: steve.uuid, name: "Steve".into(), level: 4, bypasses_player_limit: true });
        assert!(lists.check_login(&steve, ip, 20, &props(&[])).is_ok());
        lists.ops.clear();

        // Whitelist: listed players and operators may join.
        let wl = props(&[("white-list", "true")]);
        assert!(lists.check_login(&steve, ip, 0, &wl).is_err());
        assert!(lists.whitelist_add(steve.uuid, "Steve"));
        assert!(!lists.whitelist_add(steve.uuid, "Steve"));
        assert!(lists.check_login(&steve, ip, 0, &wl).is_ok());
        assert!(lists.whitelist_remove("steve"));
        assert!(lists.op(steve.uuid, "Steve", 4));
        assert!(lists.check_login(&steve, ip, 0, &wl).is_ok());
        assert_eq!(lists.op_level(steve.uuid), 4);
        assert!(lists.deop(steve.uuid));
        assert!(!lists.deop(steve.uuid));

        // Bans.
        assert!(lists.ban(steve.uuid, "Steve", "Console", Some("griefing")));
        let err = lists.check_login(&steve, ip, 0, &props(&[])).unwrap_err();
        assert!(err.to_json_string().contains("griefing"));
        assert!(lists.pardon("STEVE"));
        assert!(lists.check_login(&steve, ip, 0, &props(&[])).is_ok());
        assert!(lists.ban_ip("10.0.0.1", "Console", None));
        assert!(lists.check_login(&steve, ip, 0, &props(&[])).is_err());
        assert!(lists.pardon_ip("10.0.0.1"));
    }

    #[test]
    fn ban_expiry() {
        assert!(in_force("forever"));
        assert!(in_force("2999-01-01 00:00:00 +0000"));
        assert!(!in_force("2001-01-01 00:00:00 +0000"));
        assert!(in_force("garbage"), "unparseable expiry keeps the ban");
        assert!(chrono::DateTime::parse_from_str(&now_string(), DATE_FORMAT).is_ok());
    }

    #[test]
    fn reads_vanilla_files() {
        let ops: Vec<OpEntry> = serde_json::from_str(r#"[{"uuid":"069a79f4-44e9-4726-a5be-fca90e38aaf5","name":"Notch","level":4,"bypassesPlayerLimit":false}]"#).unwrap();
        assert_eq!(ops[0].level, 4);
        let bans: Vec<BanEntry> = serde_json::from_str(r#"[{"uuid":"069a79f4-44e9-4726-a5be-fca90e38aaf5","name":"Notch","created":"2024-01-01 00:00:00 +0000","source":"Server","expires":"forever","reason":"x"}]"#).unwrap();
        assert_eq!(bans[0].reason, "x");
    }
}
