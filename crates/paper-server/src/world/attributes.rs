//! Entity attributes (vanilla's `AttributeMap`): base values from the entity type, modifiers
//! from equipment, effects, sprinting and plugins, and the value vanilla computes from them.

use paper_nbt::{Compound, Tag};
use std::collections::{BTreeMap, BTreeSet};

/// How a modifier changes the value (`AttributeModifier.Operation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Adds the amount to the base value.
    AddValue,
    /// Adds the amount times the base (after `AddValue`s).
    AddMultipliedBase,
    /// Multiplies the result by one plus the amount.
    AddMultipliedTotal,
}

impl Operation {
    pub fn id(self) -> i8 {
        match self {
            Self::AddValue => 0,
            Self::AddMultipliedBase => 1,
            Self::AddMultipliedTotal => 2,
        }
    }

    pub fn from_id(id: i64) -> Option<Self> {
        match id {
            0 => Some(Self::AddValue),
            1 => Some(Self::AddMultipliedBase),
            2 => Some(Self::AddMultipliedTotal),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::AddValue => "add_value",
            Self::AddMultipliedBase => "add_multiplied_base",
            Self::AddMultipliedTotal => "add_multiplied_total",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "add_value" => Some(Self::AddValue),
            "add_multiplied_base" => Some(Self::AddMultipliedBase),
            "add_multiplied_total" => Some(Self::AddMultipliedTotal),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Modifier {
    /// A namespaced id (`minecraft:sprinting`, `minecraft:armor.chestplate`, ...).
    pub id: String,
    pub amount: f64,
    pub operation: Operation,
}

impl Modifier {
    pub fn new(id: impl Into<String>, amount: f64, operation: Operation) -> Self {
        Self { id: id.into(), amount, operation }
    }
}

/// Where a modifier comes from. Only `Saved` ones are written with the entity; the others are
/// applied again from their source (equipment, effects, sprinting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Saved,
    Transient,
    Equipment,
    Effect,
}

#[derive(Clone, Debug, PartialEq)]
struct Instance {
    base: f64,
    modifiers: Vec<(Modifier, Source)>,
}

/// The attributes of a living entity, by full name (`minecraft:max_health`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attributes {
    instances: BTreeMap<String, Instance>,
    /// Attributes changed since the last [`Attributes::take_dirty`].
    dirty: BTreeSet<String>,
}

fn full(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

/// `RangedAttribute`'s default, minimum and maximum, for the attributes vanilla clamps.
fn range(name: &str) -> Option<(f64, f64, f64)> {
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    Some(match path {
        "armor" => (0.0, 0.0, 30.0),
        "armor_toughness" => (0.0, 0.0, 20.0),
        "attack_damage" => (2.0, 0.0, 2048.0),
        "attack_knockback" => (0.0, 0.0, 5.0),
        "attack_speed" => (4.0, 0.0, 1024.0),
        "block_break_speed" => (1.0, 0.0, 1024.0),
        "block_interaction_range" => (4.5, 0.0, 64.0),
        "burning_time" => (1.0, 0.0, 1024.0),
        "explosion_knockback_resistance" => (0.0, 0.0, 1.0),
        "entity_interaction_range" => (3.0, 0.0, 64.0),
        "fall_damage_multiplier" => (1.0, 0.0, 100.0),
        "flying_speed" => (0.4, 0.0, 1024.0),
        "follow_range" => (32.0, 0.0, 2048.0),
        "gravity" => (0.08, -1.0, 1.0),
        "jump_strength" => (0.41999998688697815, 0.0, 32.0),
        "knockback_resistance" => (0.0, 0.0, 1.0),
        "luck" => (0.0, -1024.0, 1024.0),
        "max_absorption" => (0.0, 0.0, 2048.0),
        "max_health" => (20.0, 1.0, 1024.0),
        "mining_efficiency" => (0.0, 0.0, 1024.0),
        "movement_efficiency" => (0.0, 0.0, 1.0),
        "movement_speed" => (0.7, 0.0, 1024.0),
        "oxygen_bonus" => (0.0, 0.0, 1024.0),
        "safe_fall_distance" => (3.0, -1024.0, 1024.0),
        "scale" => (1.0, 0.0625, 16.0),
        "sneaking_speed" => (0.3, 0.0, 1.0),
        "spawn_reinforcements" => (0.0, 0.0, 1.0),
        "step_height" => (0.6, 0.0, 10.0),
        "submerged_mining_speed" => (0.2, 0.0, 20.0),
        "sweeping_damage_ratio" => (0.0, 0.0, 1.0),
        "tempt_range" => (10.0, 0.0, 2048.0),
        "water_movement_efficiency" => (0.0, 0.0, 1.0),
        "waypoint_transmit_range" | "waypoint_receive_range" => (0.0, 0.0, 6.0e7),
        // Newer attributes: the defaults every living entity has.
        "air_drag_modifier" | "friction_modifier" => (1.0, f64::MIN, f64::MAX),
        "bounciness" => (0.0, f64::MIN, f64::MAX),
        "below_name_distance" => (10.0, f64::MIN, f64::MAX),
        "name_tag_distance" => (64.0, f64::MIN, f64::MAX),
        "camera_distance" => (4.0, f64::MIN, f64::MAX),
        _ => return None,
    })
}

/// The attribute's default value (what a newly registered instance starts at).
pub fn default_value(name: &str) -> f64 {
    range(name).map_or(0.0, |(d, _, _)| d)
}

/// Whether the name is an attribute.
pub fn is_attribute(name: &str) -> bool {
    paper_data::registries::id("minecraft:attribute", &full(name)).is_some()
}

impl Attributes {
    /// The attributes an entity type has, at their base values (none for non-living types).
    pub fn of_type(entity_type: &str) -> Self {
        let mut out = Self::default();
        if let Some(info) = paper_data::entity_types::by_name(entity_type) {
            for (name, base) in &info.attributes {
                out.instances.insert(full(name), Instance { base: *base, modifiers: Vec::new() });
            }
        }
        out
    }

    pub fn has(&self, name: &str) -> bool {
        self.instances.contains_key(&full(name))
    }

    /// Adds an attribute the entity type does not have (Bukkit's `registerAttribute`).
    pub fn register(&mut self, name: &str) -> bool {
        let name = full(name);
        if self.instances.contains_key(&name) || !is_attribute(&name) {
            return false;
        }
        self.instances.insert(name.clone(), Instance { base: default_value(&name), modifiers: Vec::new() });
        self.dirty.insert(name);
        true
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.instances.keys().map(String::as_str)
    }

    pub fn base(&self, name: &str) -> Option<f64> {
        self.instances.get(&full(name)).map(|i| i.base)
    }

    pub fn set_base(&mut self, name: &str, base: f64) -> bool {
        let name = full(name);
        let Some(instance) = self.instances.get_mut(&name) else { return false };
        if instance.base != base {
            instance.base = base;
            self.dirty.insert(name);
        }
        true
    }

    /// `AttributeInstance.calculateValue`, clamped to the attribute's range. An attribute the
    /// entity does not have is at its default.
    pub fn value(&self, name: &str) -> f64 {
        let name = full(name);
        let Some(instance) = self.instances.get(&name) else { return default_value(&name) };
        let mut base = instance.base;
        for (m, _) in &instance.modifiers {
            if m.operation == Operation::AddValue {
                base += m.amount;
            }
        }
        let mut result = base;
        for (m, _) in &instance.modifiers {
            if m.operation == Operation::AddMultipliedBase {
                result += base * m.amount;
            }
        }
        for (m, _) in &instance.modifiers {
            if m.operation == Operation::AddMultipliedTotal {
                result *= 1.0 + m.amount;
            }
        }
        match range(&name) {
            Some((_, min, max)) => result.clamp(min, max),
            None => result,
        }
    }

    pub fn modifiers(&self, name: &str) -> Vec<Modifier> {
        self.instances.get(&full(name)).map(|i| i.modifiers.iter().map(|(m, _)| m.clone()).collect()).unwrap_or_default()
    }

    pub fn modifier(&self, name: &str, id: &str) -> Option<Modifier> {
        self.instances.get(&full(name))?.modifiers.iter().find(|(m, _)| m.id == id).map(|(m, _)| m.clone())
    }

    /// Adds a modifier, replacing one with the same id. `persistent` ones are saved with the
    /// entity. Returns false if the entity does not have the attribute.
    pub fn add_modifier(&mut self, name: &str, modifier: Modifier, persistent: bool) -> bool {
        self.add_from(name, modifier, if persistent { Source::Saved } else { Source::Transient })
    }

    /// Adds a modifier from a source, replacing one with the same id.
    pub fn add_from(&mut self, name: &str, modifier: Modifier, source: Source) -> bool {
        let name = full(name);
        let Some(instance) = self.instances.get_mut(&name) else { return false };
        match instance.modifiers.iter_mut().find(|(m, _)| m.id == modifier.id) {
            Some(slot) if *slot == (modifier.clone(), source) => return true,
            Some(slot) => *slot = (modifier, source),
            None => instance.modifiers.push((modifier, source)),
        }
        self.dirty.insert(name);
        true
    }

    /// Replaces every modifier from `source` with `modifiers` (attribute, modifier), marking
    /// only the attributes whose modifiers really changed.
    pub fn replace_source(&mut self, source: Source, modifiers: &[(String, Modifier)]) {
        let mut wanted: BTreeMap<String, Vec<Modifier>> = BTreeMap::new();
        for (name, m) in modifiers {
            let list = wanted.entry(full(name)).or_default();
            // The same id twice: the later one wins, as in vanilla's remove-then-add.
            list.retain(|old| old.id != m.id);
            list.push(m.clone());
        }
        for (name, instance) in &mut self.instances {
            let old: Vec<Modifier> = instance.modifiers.iter().filter(|(_, s)| *s == source).map(|(m, _)| m.clone()).collect();
            let new = wanted.remove(name).unwrap_or_default();
            if old == new {
                continue;
            }
            instance.modifiers.retain(|(m, s)| *s != source && !new.iter().any(|n| n.id == m.id));
            instance.modifiers.extend(new.into_iter().map(|m| (m, source)));
            self.dirty.insert(name.clone());
        }
    }

    pub fn remove_modifier(&mut self, name: &str, id: &str) -> bool {
        let name = full(name);
        let Some(instance) = self.instances.get_mut(&name) else { return false };
        let before = instance.modifiers.len();
        instance.modifiers.retain(|(m, _)| m.id != id);
        let removed = instance.modifiers.len() != before;
        if removed {
            self.dirty.insert(name);
        }
        removed
    }

    /// The attributes changed since the last call.
    pub fn take_dirty(&mut self) -> Vec<String> {
        std::mem::take(&mut self.dirty).into_iter().collect()
    }

    pub fn mark_all_dirty(&mut self) {
        self.dirty = self.instances.keys().cloned().collect();
    }

    /// Entries of `ClientboundUpdateAttributesPacket` for these attributes.
    pub fn packet_entries<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Vec<(i32, f64, Vec<(String, f64, i8)>)> {
        let mut out = Vec::new();
        for name in names {
            let name = full(name);
            let (Some(instance), Some(id)) = (self.instances.get(&name), paper_data::registries::id("minecraft:attribute", &name)) else { continue };
            out.push((id as i32, instance.base, instance.modifiers.iter().map(|(m, _)| (m.id.clone(), m.amount, m.operation.id())).collect()));
        }
        out
    }

    /// Whether every attribute is at its type's base value without modifiers.
    pub fn is_default_for(&self, entity_type: &str) -> bool {
        *self == Self { dirty: self.dirty.clone(), ..Self::of_type(entity_type) }
    }

    /// Vanilla's `attributes` list: base values and the saved modifiers.
    pub fn to_nbt(&self) -> Tag {
        let list = self
            .instances
            .iter()
            .map(|(name, instance)| {
                let modifiers: Vec<Tag> = instance
                    .modifiers
                    .iter()
                    .filter(|(_, source)| *source == Source::Saved)
                    .map(|(m, _)| Tag::Compound(Compound::new().with("id", Tag::String(m.id.clone())).with("amount", Tag::Double(m.amount)).with("operation", Tag::String(m.operation.name().to_owned()))))
                    .collect();
                let mut c = Compound::new().with("id", Tag::String(name.clone())).with("base", Tag::Double(instance.base));
                if !modifiers.is_empty() {
                    c.insert("modifiers", Tag::List(modifiers));
                }
                Tag::Compound(c)
            })
            .collect();
        Tag::List(list)
    }

    /// Applies a saved `attributes` list (unknown attributes are skipped).
    pub fn load_nbt(&mut self, list: &[Tag]) {
        for entry in list {
            let Some(c) = entry.as_compound() else { continue };
            let Some(name) = c.string("id").map(full) else { continue };
            if !is_attribute(&name) {
                continue;
            }
            let instance = self.instances.entry(name.clone()).or_insert_with(|| Instance { base: default_value(&name), modifiers: Vec::new() });
            if let Some(base) = c.f64("base") {
                instance.base = base;
            }
            for m in c.list("modifiers").unwrap_or(&[]) {
                let Some(m) = m.as_compound() else { continue };
                let (Some(id), Some(amount), Some(operation)) = (m.string("id"), m.f64("amount"), m.string("operation").and_then(Operation::from_name)) else { continue };
                instance.modifiers.retain(|(old, _)| old.id != id);
                instance.modifiers.push((Modifier::new(id, amount, operation), Source::Saved));
            }
            self.dirty.insert(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_follow_vanilla() {
        let mut a = Attributes::of_type("minecraft:player");
        assert_eq!(a.value("max_health"), 20.0);
        assert!((a.value("movement_speed") - 0.1).abs() < 1e-6);
        // Sprinting: +30% of the total.
        a.add_modifier("movement_speed", Modifier::new("minecraft:sprinting", 0.3, Operation::AddMultipliedTotal), false);
        assert!((a.value("movement_speed") - 0.13).abs() < 1e-6);
        // add_value, then add_multiplied_base of the new base, then add_multiplied_total.
        a.set_base("attack_damage", 2.0);
        a.add_modifier("attack_damage", Modifier::new("a", 2.0, Operation::AddValue), true);
        a.add_modifier("attack_damage", Modifier::new("b", 0.5, Operation::AddMultipliedBase), true);
        a.add_modifier("attack_damage", Modifier::new("c", 1.0, Operation::AddMultipliedTotal), true);
        assert_eq!(a.value("attack_damage"), (4.0 + 2.0) * 2.0);
        // Clamped to the range; an attribute the entity lacks is at its default.
        a.set_base("max_health", 5000.0);
        assert_eq!(a.value("max_health"), 1024.0);
        assert_eq!(Attributes::of_type("minecraft:pig").value("follow_range"), 16.0);
        assert_eq!(Attributes::of_type("minecraft:player").value("follow_range"), 32.0, "the default of an attribute it lacks");
        assert!(!Attributes::of_type("minecraft:item").has("max_health"));
    }

    #[test]
    fn sources_are_replaced_as_a_group() {
        let mut a = Attributes::of_type("minecraft:player");
        let chest = |amount| ("minecraft:armor".to_owned(), Modifier::new("minecraft:armor.chestplate", amount, Operation::AddValue));
        a.replace_source(Source::Equipment, &[chest(6.0)]);
        assert_eq!(a.value("armor"), 6.0);
        a.take_dirty();
        a.replace_source(Source::Equipment, &[chest(6.0)]);
        assert!(a.take_dirty().is_empty(), "unchanged equipment changes nothing");
        a.add_modifier("armor", Modifier::new("plugin:bonus", 1.0, Operation::AddValue), true);
        a.replace_source(Source::Equipment, &[]);
        assert_eq!(a.value("armor"), 1.0, "other sources stay");
    }

    #[test]
    fn saved_modifiers_round_trip() {
        let mut a = Attributes::of_type("minecraft:zombie");
        a.set_base("max_health", 40.0);
        a.add_modifier("max_health", Modifier::new("plugin:boss", 1.0, Operation::AddMultipliedBase), true);
        a.add_modifier("movement_speed", Modifier::new("minecraft:effect.speed", 0.2, Operation::AddMultipliedTotal), false);
        let Tag::List(list) = a.to_nbt() else { panic!() };
        let mut b = Attributes::of_type("minecraft:zombie");
        b.load_nbt(&list);
        assert_eq!(b.value("max_health"), 80.0);
        assert!(b.modifier("movement_speed", "minecraft:effect.speed").is_none(), "effect modifiers are not saved");
    }
}
