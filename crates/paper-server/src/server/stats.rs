//! Player statistics (vanilla's `ServerStatsCounter`): values by stat type
//! (`minecraft:custom`, `minecraft:mined`, `minecraft:killed`, ...) and key, counted as players
//! play, saved in the main world's `stats/<uuid>.json`, shown on the statistics screen
//! (`ClientboundAwardStatsPacket`), and followed by scoreboard objectives with stat criteria
//! (`minecraft.mined:minecraft.stone`), `deathCount`, `playerKillCount` and `totalKillCount`.

use super::Server;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use std::collections::BTreeMap;

pub const CUSTOM: &str = "minecraft:custom";

/// A player's statistics.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Stat type to key to value.
    pub values: BTreeMap<String, BTreeMap<String, i32>>,
    /// Changed since the client last asked.
    pub dirty: bool,
}

impl Stats {
    pub fn get(&self, kind: &str, key: &str) -> i32 {
        self.values.get(kind).and_then(|m| m.get(key)).copied().unwrap_or(0)
    }

    pub fn set(&mut self, kind: &str, key: &str, value: i32) {
        self.values.entry(kind.to_owned()).or_default().insert(key.to_owned(), value);
        self.dirty = true;
    }

    /// The `stats/<uuid>.json` form.
    pub fn to_json(&self) -> serde_json::Value {
        let stats: serde_json::Map<String, serde_json::Value> = self.values.iter().map(|(kind, keys)| (kind.clone(), serde_json::Value::Object(keys.iter().map(|(k, v)| (k.clone(), serde_json::Value::from(*v))).collect()))).collect();
        serde_json::json!({ "stats": stats, "DataVersion": crate::world::storage::data_version() })
    }

    pub fn from_json(v: &serde_json::Value) -> Stats {
        let mut out = Stats::default();
        if let Some(stats) = v.get("stats").and_then(|s| s.as_object()) {
            for (kind, keys) in stats {
                for (key, value) in keys.as_object().into_iter().flatten() {
                    if let Some(n) = value.as_i64() {
                        out.values.entry(kind.clone()).or_default().insert(key.clone(), n.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
                    }
                }
            }
        }
        out
    }
}

/// The scoreboard criteria naming a statistic (`minecraft.custom:minecraft.jump`).
pub fn criteria_of(kind: &str, key: &str) -> String {
    format!("{}:{}", kind.replace(':', "."), key.replace(':', "."))
}

/// The registry a stat type's keys come from.
fn key_registry(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "minecraft:mined" => "minecraft:block",
        "minecraft:crafted" | "minecraft:used" | "minecraft:broken" | "minecraft:picked_up" | "minecraft:dropped" => "minecraft:item",
        "minecraft:killed" | "minecraft:killed_by" => "minecraft:entity_type",
        CUSTOM => "minecraft:custom_stat",
        _ => return None,
    })
}

impl Server {
    /// Adds to a player's statistic (`awardStat`) unless a plugin cancels the increase;
    /// scoreboards following it change too.
    pub fn award_stat(&mut self, player: i32, kind: &str, key: &str, amount: i32) {
        if amount == 0 {
            return;
        }
        let Some(p) = self.players.get(&player) else { return };
        let old = p.stats.get(kind, key);
        let value = old.saturating_add(amount);
        if !crate::plugins::events::on_statistic_increment(self, player, kind, key, old, value) {
            return;
        }
        let Some(p) = self.players.get_mut(&player) else { return };
        p.stats.set(kind, key, value);
        self.add_to_criteria(player, &criteria_of(kind, key), amount);
    }

    /// Sets a statistic back to zero (`resetStat`); scores following it are reset.
    pub(super) fn reset_stat(&mut self, player: i32, kind: &str, key: &str) {
        let Some(p) = self.players.get_mut(&player) else { return };
        p.stats.set(kind, key, 0);
        let name = p.name().to_owned();
        for (board, objective) in self.criteria_objectives(&criteria_of(kind, key)) {
            let _ = self.reset_score(board, &name, Some(&objective));
        }
    }

    /// Sets a player's statistic for a plugin (`CraftStatistic.setStatistic`); scores following
    /// it are set to the same value.
    pub fn set_stat(&mut self, player: i32, kind: &str, key: &str, value: i32) {
        let Some(p) = self.players.get_mut(&player) else { return };
        p.stats.set(kind, key, value);
        let name = p.name().to_owned();
        for (board, objective) in self.criteria_objectives(&criteria_of(kind, key)) {
            let _ = self.set_score(board, &objective, &name, Some(value), None);
        }
    }

    /// The objectives of every scoreboard with `criteria`.
    fn criteria_objectives(&self, criteria: &str) -> Vec<(u64, String)> {
        let mut out = Vec::new();
        for board in self.scoreboards.ids() {
            if let Some(b) = self.scoreboards.get(board) {
                out.extend(b.objectives.values().filter(|o| o.criteria == criteria).map(|o| (board, o.name.clone())));
            }
        }
        out
    }

    /// Adds to the player's scores of objectives with `criteria` (`ScoreAccess.add`); a missing
    /// score starts from zero.
    fn add_to_criteria(&mut self, player: i32, criteria: &str, amount: i32) {
        let Some(name) = self.players.get(&player).map(|p| p.name().to_owned()) else { return };
        for (board, objective) in self.criteria_objectives(criteria) {
            let old = self.scoreboards.get(board).and_then(|b| b.scores.get(&name)).and_then(|s| s.get(&objective)).map_or(0, |s| s.value);
            let _ = self.set_score(board, &objective, &name, Some(old.wrapping_add(amount)), None);
        }
    }

    /// Adds one to the player's scores of objectives with `criteria` (`deathCount`,
    /// `playerKillCount`, `totalKillCount`, ...).
    pub(super) fn bump_criteria(&mut self, player: i32, criteria: &str) {
        self.add_to_criteria(player, criteria, 1);
    }

    /// The statistics screen asked for them (`ClientboundAwardStatsPacket`).
    pub(super) fn send_stats(&mut self, player: i32) {
        let Some(p) = self.players.get_mut(&player) else { return };
        p.stats.dirty = false;
        let mut stats = Vec::new();
        for (kind, keys) in &p.stats.values {
            let (Some(kind_id), Some(registry)) = (paper_data::registries::id("minecraft:stat_type", kind), key_registry(kind)) else { continue };
            for (key, value) in keys {
                if let Some(key_id) = paper_data::registries::id(registry, key) {
                    stats.push((VarInt(kind_id as i32), VarInt(key_id as i32), VarInt(*value)));
                }
            }
        }
        p.conn.send(&play::AwardStats { stats });
    }

    /// Time statistics, every tick (`ServerPlayer.tick`'s `awardStat` calls).
    pub(super) fn tick_stats(&mut self) {
        let ids: Vec<(i32, bool, bool)> = self.players.values().filter(|p| !p.dead).map(|p| (p.entity_id, p.sneaking, p.sleeping.is_some())).collect();
        for (id, sneaking, sleeping) in ids {
            self.award_stat(id, CUSTOM, "minecraft:play_time", 1);
            self.award_stat(id, CUSTOM, "minecraft:total_world_time", 1);
            self.award_stat(id, CUSTOM, "minecraft:time_since_death", 1);
            if sleeping {
                self.reset_stat(id, CUSTOM, "minecraft:time_since_rest");
            } else {
                self.award_stat(id, CUSTOM, "minecraft:time_since_rest", 1);
            }
            if sneaking {
                self.award_stat(id, CUSTOM, "minecraft:sneak_time", 1);
            }
        }
    }

    /// `checkMovementStatistics`: how the player moved, in centimetres.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn movement_stats(&mut self, id: i32, dx: f64, dy: f64, dz: f64, in_water: bool, eyes_in_water: bool, climbing: bool) {
        let Some(p) = self.players.get(&id) else { return };
        if p.vehicle.is_some() || dx * dx + dy * dy + dz * dz < 1.0e-7 {
            return;
        }
        let (on_ground, sprinting, sneaking, flying) = (p.on_ground, p.sprinting, p.sneaking, p.flying);
        let swimming = sprinting && eyes_in_water;
        let cm3 = ((dx * dx + dy * dy + dz * dz).sqrt() * 100.0).round() as i32;
        let cm2 = ((dx * dx + dz * dz).sqrt() * 100.0).round() as i32;
        let (key, amount) = if swimming {
            ("minecraft:swim_one_cm", cm3)
        } else if eyes_in_water {
            ("minecraft:walk_under_water_one_cm", cm3)
        } else if in_water {
            ("minecraft:walk_on_water_one_cm", cm2)
        } else if climbing {
            ("minecraft:climb_one_cm", if dy > 0.0 { (dy * 100.0).round() as i32 } else { 0 })
        } else if on_ground {
            (if sprinting { "minecraft:sprint_one_cm" } else if sneaking { "minecraft:crouch_one_cm" } else { "minecraft:walk_one_cm" }, cm2)
        } else if flying && cm2 > 25 {
            ("minecraft:fly_one_cm", cm2)
        } else if dy < 0.0 {
            ("minecraft:fall_one_cm", (-dy * 100.0).round() as i32)
        } else {
            return;
        };
        if amount > 0 {
            self.award_stat(id, CUSTOM, key, amount);
        }
    }

    /// Distance ridden on a mount (`checkRidingStatistics`).
    pub(super) fn riding_stats(&mut self, id: i32, vehicle: &str, dx: f64, dy: f64, dz: f64) {
        let key = match vehicle {
            "minecraft:pig" => "minecraft:pig_one_cm",
            "minecraft:strider" => "minecraft:strider_one_cm",
            "minecraft:happy_ghast" => "minecraft:happy_ghast_one_cm",
            "minecraft:nautilus" | "minecraft:zombie_nautilus" => "minecraft:nautilus_one_cm",
            n if super::riding::is_horse(n) => "minecraft:horse_one_cm",
            n if n.ends_with("_boat") || n.ends_with("_raft") => "minecraft:boat_one_cm",
            n if n.contains("minecart") => "minecraft:minecart_one_cm",
            _ => return,
        };
        let cm = ((dx * dx + dy * dy + dz * dz).sqrt() * 100.0).round() as i32;
        if cm > 0 {
            self.award_stat(id, CUSTOM, key, cm);
        }
    }

    /// Loads a joining player's statistics from the main world's `stats/<uuid>.json`.
    pub(super) fn load_stats(&mut self, id: i32) {
        let Some(uuid) = self.players.get(&id).map(|p| p.uuid()) else { return };
        let Some(storage) = self.worlds.first().and_then(|w| w.storage.clone()) else { return };
        if let Some(json) = storage.read_stats(uuid) {
            if let Some(p) = self.players.get_mut(&id) {
                p.stats = Stats::from_json(&json);
            }
        }
    }

    /// Saves a player's statistics with their data.
    pub(super) fn save_stats(&self, player: &super::player::Player) {
        if let Some(storage) = self.worlds.first().and_then(|w| w.storage.as_ref()) {
            storage.save_stats(player.uuid(), &player.stats.to_json());
        }
    }

    /// A player's statistic, online or not (`CraftOfflinePlayer` reads the saved file).
    pub fn statistic(&self, uuid: uuid::Uuid, kind: &str, key: &str) -> i32 {
        if let Some(p) = self.by_uuid.get(&uuid).and_then(|id| self.players.get(id)) {
            return p.stats.get(kind, key);
        }
        let storage = self.worlds.first().and_then(|w| w.storage.as_ref());
        storage.and_then(|s| s.read_stats(uuid)).map_or(0, |json| Stats::from_json(&json).get(kind, key))
    }

    /// Sets a player's statistic, online or not; an offline player's file is saved at once.
    pub fn set_statistic(&mut self, uuid: uuid::Uuid, kind: &str, key: &str, value: i32) {
        if let Some(id) = self.by_uuid.get(&uuid).copied().filter(|id| self.players.contains_key(id)) {
            self.set_stat(id, kind, key, value);
            return;
        }
        let Some(storage) = self.worlds.first().and_then(|w| w.storage.clone()) else { return };
        let mut stats = storage.read_stats(uuid).map(|json| Stats::from_json(&json)).unwrap_or_default();
        stats.set(kind, key, value);
        storage.save_stats(uuid, &stats.to_json());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_round_trip_and_name_their_criteria() {
        let mut stats = Stats::default();
        stats.set(CUSTOM, "minecraft:jump", 5);
        stats.set("minecraft:mined", "minecraft:stone", 3);
        let back = Stats::from_json(&stats.to_json());
        assert_eq!((back.get(CUSTOM, "minecraft:jump"), back.get("minecraft:mined", "minecraft:stone"), back.get(CUSTOM, "minecraft:deaths")), (5, 3, 0));
        assert_eq!(criteria_of("minecraft:mined", "minecraft:stone"), "minecraft.mined:minecraft.stone");
    }
}
