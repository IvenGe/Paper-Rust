//! Game rules of a world, saved like vanilla 26.x in `data/minecraft/game_rules.dat`.

use paper_data::game_rules::{self, Value};
use paper_nbt::{Compound, Tag};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct GameRules {
    values: BTreeMap<&'static str, Value>,
}

impl Default for GameRules {
    fn default() -> Self {
        Self { values: game_rules::all().iter().map(|r| (r.name.as_str(), r.default)).collect() }
    }
}

impl GameRules {
    /// The value of a rule (`keep_inventory` or `minecraft:keep_inventory`). Unknown rules
    /// read as `false` / 0.
    pub fn get(&self, name: &str) -> Value {
        match game_rules::get(name) {
            Some(rule) => self.values.get(rule.name.as_str()).copied().unwrap_or(rule.default),
            None => Value::Bool(false),
        }
    }

    pub fn bool(&self, name: &str) -> bool {
        self.get(name).as_bool()
    }

    pub fn int(&self, name: &str) -> i32 {
        self.get(name).as_int()
    }

    /// Sets a rule; returns false if the rule does not exist or the value has the wrong type.
    pub fn set(&mut self, name: &str, value: Value) -> bool {
        let Some(rule) = game_rules::get(name) else { return false };
        let ok = matches!((rule.default, value), (Value::Bool(_), Value::Bool(_)) | (Value::Int(_), Value::Int(_)));
        if ok {
            self.values.insert(rule.name.as_str(), value);
        }
        ok
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, Value)> + '_ {
        self.values.iter().map(|(k, v)| (*k, *v))
    }

    /// `{data: {"minecraft:keep_inventory": 0b, ...}, DataVersion: ...}` as vanilla writes it.
    pub fn to_nbt(&self) -> Compound {
        let mut data = Compound::new();
        for (name, value) in &self.values {
            data.insert(*name, match value {
                Value::Bool(b) => Tag::Byte(*b as i8),
                Value::Int(i) => Tag::Int(*i),
            });
        }
        Compound::new().with("data", Tag::Compound(data)).with("DataVersion", Tag::Int(super::storage::data_version()))
    }

    pub fn from_nbt(root: &Compound) -> Self {
        let mut rules = Self::default();
        let data = root.compound("data").unwrap_or(root);
        for rule in game_rules::all() {
            let value = match (rule.default, data.get(&rule.name)) {
                (Value::Bool(_), Some(Tag::Byte(b))) => Value::Bool(*b != 0),
                (Value::Int(_), Some(Tag::Int(i))) => Value::Int(*i),
                _ => continue,
            };
            rules.values.insert(rule.name.as_str(), value);
        }
        rules
    }

    /// Reads the pre-26 `GameRules` compound of `level.dat` (camelCase names, string values).
    pub fn from_legacy(old: &Compound) -> Self {
        let mut rules = Self::default();
        for (key, tag) in old.iter() {
            let Tag::String(text) = tag else { continue };
            let Some(rule) = game_rules::get(key) else { continue };
            let Some(mut value) = rule.parse(text) else { continue };
            // disableRaids became raids: the meaning flipped.
            if key == "disableRaids" {
                value = Value::Bool(!value.as_bool());
            }
            // doFireTick=false is a spread radius of 0.
            if key == "doFireTick" {
                value = Value::Int(if text == "false" { 0 } else { rule.default.as_int() });
            }
            rules.values.insert(rule.name.as_str(), value);
        }
        rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let mut rules = GameRules::default();
        assert!(!rules.bool("keep_inventory"));
        assert!(rules.bool("minecraft:natural_health_regeneration"));
        assert_eq!(rules.int("random_tick_speed"), 3);
        assert!(rules.set("keepInventory", Value::Bool(true)));
        assert!(!rules.set("random_tick_speed", Value::Bool(true)), "type mismatch");
        assert!(!rules.set("no_such_rule", Value::Int(1)));
        assert_eq!(GameRules::from_nbt(&rules.to_nbt()), rules);
    }

    #[test]
    fn imports_legacy_rules() {
        let old = Compound::new()
            .with("keepInventory", Tag::String("true".into()))
            .with("doDaylightCycle", Tag::String("false".into()))
            .with("disableRaids", Tag::String("true".into()))
            .with("randomTickSpeed", Tag::String("10".into()));
        let rules = GameRules::from_legacy(&old);
        assert!(rules.bool("keep_inventory"));
        assert!(!rules.bool("advance_time"));
        assert!(!rules.bool("raids"));
        assert_eq!(rules.int("random_tick_speed"), 10);
    }
}
