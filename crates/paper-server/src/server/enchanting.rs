//! Enchantments: their definitions from the vanilla data (`data/minecraft/enchantment`) and
//! vanilla's ways of picking random ones (`EnchantRandomlyFunction` and
//! `EnchantmentHelper.selectEnchantment`), which loot tables use.

use crate::vanilla::VanillaData;
use paper_data::{item_data, items};
use paper_protocol::codec::{Encode, Reader, VarInt};
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;
use serde_json::Value;

/// One enchantment.
pub struct EnchantmentDef {
    pub name: String,
    /// Id in the synchronised `minecraft:enchantment` registry.
    pub id: i32,
    pub max_level: i32,
    pub weight: i32,
    /// Levels per enchantment level an anvil charges for adding it.
    pub anvil_cost: i32,
    /// `base` and `per_level_above_first` of the cost bounds.
    pub min_cost: (i32, i32),
    pub max_cost: (i32, i32),
    /// Items it can go on (anvils, `enchant_randomly`).
    pub supported: Vec<i32>,
    /// Items enchanting tables and `enchant_with_levels` offer it for (`supported` if unset).
    pub primary: Option<Vec<i32>>,
    /// Enchantments it cannot be combined with.
    pub exclusive: Vec<i32>,
}

impl EnchantmentDef {
    fn cost(bounds: (i32, i32), level: i32) -> i32 {
        bounds.0 + bounds.1 * (level - 1)
    }
}

/// All enchantments of the vanilla data.
pub struct Enchantments {
    defs: Vec<EnchantmentDef>,
}

fn component(name: &str) -> Option<i32> {
    paper_data::registries::id("minecraft:data_component_type", name).map(|i| i as i32)
}

/// Item ids named by a holder set: `#tag`, one id, or a list.
fn item_set(v: &Value) -> Vec<i32> {
    let mut out: Vec<i32> = match v {
        Value::String(s) if s.starts_with('#') => paper_data::synced::tag("minecraft:item", &s[1..]).map(<[i32]>::to_vec).unwrap_or_default(),
        Value::String(s) => items::id(s).map(|i| vec![i as i32]).unwrap_or_default(),
        Value::Array(list) => list.iter().flat_map(item_set).collect(),
        _ => Vec::new(),
    };
    out.sort_unstable();
    out.dedup();
    out
}

/// Enchantment ids named by a holder set; `None` when the value names none (meaning all).
fn enchantment_set(v: &Value) -> Option<Vec<i32>> {
    match v {
        Value::String(s) if s.starts_with('#') => Some(paper_data::synced::tag("minecraft:enchantment", &s[1..]).map(<[i32]>::to_vec).unwrap_or_default()),
        Value::String(s) => Some(paper_data::synced::id("minecraft:enchantment", s).map(|i| vec![i]).unwrap_or_default()),
        Value::Array(list) => Some(list.iter().filter_map(enchantment_set).flatten().collect()),
        _ => None,
    }
}

impl Enchantments {
    pub fn load(data: &VanillaData) -> Self {
        let mut defs: Vec<EnchantmentDef> = data
            .registry("enchantment")
            .into_iter()
            .filter_map(|(name, v)| {
                let id = paper_data::synced::id("minecraft:enchantment", &name)?;
                let cost = |key: &str| (v[key]["base"].as_i64().unwrap_or(1) as i32, v[key]["per_level_above_first"].as_i64().unwrap_or(0) as i32);
                Some(EnchantmentDef {
                    id,
                    max_level: v["max_level"].as_i64().unwrap_or(1) as i32,
                    weight: v["weight"].as_i64().unwrap_or(1) as i32,
                    anvil_cost: v["anvil_cost"].as_i64().unwrap_or(1) as i32,
                    min_cost: cost("min_cost"),
                    max_cost: cost("max_cost"),
                    supported: item_set(&v["supported_items"]),
                    primary: v.get("primary_items").map(item_set),
                    exclusive: enchantment_set(&v["exclusive_set"]).unwrap_or_default(),
                    name,
                })
            })
            .collect();
        defs.sort_by_key(|d| d.id);
        Self { defs }
    }

    pub fn by_id(&self, id: i32) -> Option<&EnchantmentDef> {
        self.defs.iter().find(|d| d.id == id)
    }

    /// The enchantments an `options` value allows (all of them when it names none).
    fn options(&self, options: &Value) -> Vec<&EnchantmentDef> {
        match enchantment_set(options) {
            Some(ids) => self.defs.iter().filter(|d| ids.contains(&d.id)).collect(),
            None => self.defs.iter().collect(),
        }
    }

    /// `enchant_randomly`: one random enchantment of `options` at a random level; books
    /// become enchanted books. With `only_compatible`, only enchantments the item supports.
    pub fn enchant_randomly(&self, slot: &mut Slot, options: &Value, only_compatible: bool, rng: &mut impl Rng) {
        let Some(stack) = slot.0.as_mut() else { return };
        let book = is(stack, "minecraft:book");
        let candidates: Vec<&EnchantmentDef> = self.options(options).into_iter().filter(|d| book || !only_compatible || d.supported.binary_search(&stack.item).is_ok()).collect();
        if candidates.is_empty() {
            return;
        }
        let def = candidates[rng.gen_range(0..candidates.len())];
        let level = rng.gen_range(1..=def.max_level.max(1));
        enchant(stack, def.id, level);
    }

    /// `enchant_with_levels` (vanilla's enchanting-table selection for `levels`
    /// experience levels).
    pub fn enchant_with_levels(&self, slot: &mut Slot, levels: i32, options: &Value, rng: &mut impl Rng) {
        let Some(stack) = slot.0.as_mut() else { return };
        let chosen = self.select(stack, levels, options, rng);
        for (id, level) in chosen {
            enchant(stack, id, level);
        }
    }

    /// `EnchantmentHelper.selectEnchantment`.
    pub fn select(&self, stack: &ItemStack, levels: i32, options: &Value, rng: &mut impl Rng) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        let enchantability = item_data::by_id(stack.item as u32).raw.as_ref().and_then(|r| r["enchantable"]["value"].as_i64()).unwrap_or(0) as i32;
        if enchantability <= 0 {
            return out;
        }
        let mut level = levels + 1 + rng.gen_range(0..=enchantability / 4) + rng.gen_range(0..=enchantability / 4);
        let f = (rng.gen::<f32>() + rng.gen::<f32>() - 1.0) * 0.15;
        level = ((level as f32 + level as f32 * f).round() as i32).max(1);
        let book = is(stack, "minecraft:book");
        // (enchantment, level, weight) for every enchantment the cost range admits.
        let mut available: Vec<(&EnchantmentDef, i32)> = Vec::new();
        for def in self.options(options) {
            let primary = def.primary.as_ref().unwrap_or(&def.supported);
            if !book && primary.binary_search(&stack.item).is_err() {
                continue;
            }
            if let Some(l) = (1..=def.max_level).rev().find(|l| level >= EnchantmentDef::cost(def.min_cost, *l) && level <= EnchantmentDef::cost(def.max_cost, *l)) {
                available.push((def, l));
            }
        }
        let pick = |available: &[(&EnchantmentDef, i32)], rng: &mut _| -> Option<usize> {
            let total: i32 = available.iter().map(|(d, _)| d.weight.max(0)).sum();
            if total <= 0 {
                return None;
            }
            let mut r = Rng::gen_range(rng, 0..total);
            available.iter().position(|(d, _)| {
                r -= d.weight.max(0);
                r < 0
            })
        };
        if let Some(i) = pick(&available, rng) {
            let (def, l) = available[i];
            out.push((def.id, l));
            let mut last = def;
            while rng.gen_range(0..50) <= level {
                // Drop what conflicts with the last pick.
                available.retain(|(d, _)| d.id != last.id && !d.exclusive.contains(&last.id) && !last.exclusive.contains(&d.id));
                let Some(i) = pick(&available, rng) else { break };
                let (def, l) = available[i];
                out.push((def.id, l));
                last = def;
                level /= 2;
            }
        }
        out
    }
}

fn is(stack: &ItemStack, name: &str) -> bool {
    items::id(name).is_some_and(|id| id as i32 == stack.item)
}

/// The (enchantment id, level) pairs of an `enchantments`-like component.
pub fn read_enchantments(stack: &ItemStack, name: &str) -> Vec<(i32, i32)> {
    let Some(ty) = component(name) else { return Vec::new() };
    let Some((_, data)) = stack.added.iter().find(|(t, _)| *t == ty) else { return Vec::new() };
    let mut r = Reader::new(data);
    let Ok(count) = r.varint() else { return Vec::new() };
    (0..count.max(0)).map_while(|_| Some((r.varint().ok()?, r.varint().ok()?))).collect()
}

/// Sets a component of a stack to raw network bytes.
pub fn set_component(stack: &mut ItemStack, name: &str, data: Vec<u8>) {
    let Some(ty) = component(name) else { return };
    stack.removed.retain(|t| *t != ty);
    match stack.added.iter_mut().find(|(t, _)| *t == ty) {
        Some((_, d)) => *d = data,
        None => stack.added.push((ty, data)),
    }
}

/// Adds an enchantment (keeping the higher level if it is there already). Books turn into
/// enchanted books, which store it instead.
pub fn enchant(stack: &mut ItemStack, id: i32, level: i32) {
    if is(stack, "minecraft:book") {
        if let Some(book) = items::id("minecraft:enchanted_book") {
            stack.item = book as i32;
        }
    }
    let name = if is(stack, "minecraft:enchanted_book") { "minecraft:stored_enchantments" } else { "minecraft:enchantments" };
    let mut list = read_enchantments(stack, name);
    match list.iter_mut().find(|(e, _)| *e == id) {
        Some((_, l)) => *l = (*l).max(level),
        None => list.push((id, level)),
    }
    let mut w = Vec::new();
    VarInt(list.len() as i32).encode(&mut w);
    for (e, l) in list {
        VarInt(e).encode(&mut w);
        VarInt(l).encode(&mut w);
    }
    set_component(stack, name, w);
}
