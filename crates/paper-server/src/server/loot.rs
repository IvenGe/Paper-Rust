//! Loot tables: what blocks drop when broken and mobs drop when killed.
//!
//! Tables are vanilla's JSON (`data/minecraft/loot_table/**` in the server jar), evaluated
//! with the parts of vanilla's model that drops depend on: pools with rolls, weighted
//! entries (`item`, `tag`, `alternatives`, `group`, `sequence`, `loot_table`, `empty`),
//! conditions (`survives_explosion`, `match_tool` with item and enchantment predicates,
//! `table_bonus`, `random_chance`, `random_chance_with_enchanted_bonus`, `killed_by_player`,
//! `block_state_property`, `entity_properties` on fire, `inverted`, `any_of`, `all_of`) and
//! functions (`set_count`, `apply_bonus`, `explosion_decay`, `limit_count`,
//! `enchanted_count_increase`, `furnace_smelt`, `set_damage`).

use crate::vanilla::VanillaData;
use paper_data::{blocks, items};
use paper_protocol::types::{ItemStack, Slot};
use parking_lot::RwLock;
use rand::Rng;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// What a loot table is evaluated for.
#[derive(Default)]
pub struct LootContext<'a> {
    /// The tool or weapon used.
    pub tool: Option<&'a Slot>,
    /// The broken block's state.
    pub block_state: Option<u32>,
    /// Radius of the explosion that caused the drop.
    pub explosion_radius: Option<f32>,
    pub killed_by_player: bool,
    /// The killed entity was burning (cooked drops).
    pub on_fire: bool,
}

/// Cached, parsed loot tables.
pub struct LootTables {
    data: Option<Arc<VanillaData>>,
    cache: RwLock<HashMap<String, Option<Arc<Value>>>>,
    predicates: RwLock<HashMap<String, Option<Arc<Value>>>>,
    enchantments: std::sync::OnceLock<Option<Arc<super::enchanting::Enchantments>>>,
}

/// Conditions and modifiers can be one value or a list (26.x), or a legacy list.
fn one_or_many<'a>(v: &'a Value, legacy: &'a Value) -> Vec<&'a Value> {
    let mut out: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    };
    if let Value::Array(a) = legacy {
        out.extend(a.iter());
    }
    out
}

fn kind(v: &Value) -> &str {
    v["type"].as_str().or_else(|| v["condition"].as_str()).or_else(|| v["function"].as_str()).unwrap_or("")
}

impl LootTables {
    pub fn new(data: Option<Arc<VanillaData>>) -> Self {
        Self { data, cache: RwLock::new(HashMap::new()), predicates: RwLock::new(HashMap::new()), enchantments: std::sync::OnceLock::new() }
    }

    /// A condition, resolving references to predicate files (`minecraft:tool/can_silk_touch`).
    fn predicate(&self, v: &Value) -> Option<Arc<Value>> {
        let Value::String(id) = v else { return Some(Arc::new(v.clone())) };
        if let Some(p) = self.predicates.read().get(id) {
            return p.clone();
        }
        let path = id.strip_prefix("minecraft:").unwrap_or(id);
        let p = self.data.as_ref().and_then(|d| d.read_json(&format!("data/minecraft/predicate/{path}.json"))).map(Arc::new);
        self.predicates.write().insert(id.clone(), p.clone());
        p
    }

    fn holds(&self, owner: &Value, ctx: &LootContext, rng: &mut impl Rng) -> bool {
        one_or_many(&owner["condition"], &owner["conditions"]).into_iter().all(|c| self.condition(c, ctx, rng))
    }

    fn condition(&self, c: &Value, ctx: &LootContext, rng: &mut impl Rng) -> bool {
        let Some(c) = self.predicate(c) else { return false };
        let c = c.as_ref();
        match kind(c) {
            "minecraft:inverted" => !self.condition(&c["term"], ctx, rng),
            "minecraft:any_of" | "minecraft:alternative" => c["terms"].as_array().is_some_and(|t| t.iter().any(|t| self.condition(t, ctx, rng))),
            "minecraft:all_of" => c["terms"].as_array().is_none_or(|t| t.iter().all(|t| self.condition(t, ctx, rng))),
            "minecraft:reference" => c["name"].as_str().is_some_and(|n| self.condition(&Value::String(n.to_owned()), ctx, rng)),
            _ => condition_holds(c, ctx, rng),
        }
    }

    /// The enchantment definitions of the vanilla data (read on first use).
    pub fn enchantments(&self) -> Option<Arc<super::enchanting::Enchantments>> {
        self.enchantments.get_or_init(|| self.data.as_ref().map(|d| Arc::new(super::enchanting::Enchantments::load(d)))).clone()
    }

    fn modify(&self, owner: &Value, ctx: &LootContext, rng: &mut impl Rng, stacks: &mut [Slot]) {
        for f in one_or_many(&owner["modifier"], &owner["functions"]) {
            if self.holds(f, ctx, rng) {
                self.apply(f, ctx, rng, stacks);
            }
        }
        for slot in stacks.iter_mut() {
            if slot.0.as_ref().is_some_and(|st| st.count <= 0) {
                *slot = Slot::EMPTY;
            }
        }
    }

    pub fn available(&self) -> bool {
        self.data.is_some()
    }

    fn table(&self, id: &str) -> Option<Arc<Value>> {
        if let Some(t) = self.cache.read().get(id) {
            return t.clone();
        }
        let path = id.strip_prefix("minecraft:").unwrap_or(id);
        let table = self.data.as_ref().and_then(|d| d.read_json(&format!("data/minecraft/loot_table/{path}.json"))).map(Arc::new);
        self.cache.write().insert(id.to_owned(), table.clone());
        table
    }

    /// Rolls a table. Unknown tables drop nothing.
    pub fn roll(&self, id: &str, ctx: &LootContext) -> Vec<Slot> {
        self.roll_with(id, ctx, &mut rand::thread_rng())
    }

    /// Rolls a table with the given random source (seeded for container loot).
    pub fn roll_with(&self, id: &str, ctx: &LootContext, rng: &mut impl Rng) -> Vec<Slot> {
        let Some(table) = self.table(id) else { return Vec::new() };
        let mut out = Vec::new();
        self.roll_table(&table, ctx, rng, &mut out, 0);
        // Split stacks above the maximum stack size.
        let mut split = Vec::new();
        for slot in out {
            let Some(stack) = slot.0 else { continue };
            let max = items::get(stack.item as u32).map_or(64, |i| i.max_stack_size as i32).max(1);
            let mut left = stack.count;
            while left > 0 {
                let n = left.min(max);
                split.push(Slot(Some(stack.with_count(n))));
                left -= n;
            }
        }
        split
    }

    fn roll_table(&self, table: &Value, ctx: &LootContext, rng: &mut impl Rng, out: &mut Vec<Slot>, depth: u32) {
        if depth > 8 {
            return;
        }
        let start = out.len();
        for pool in table["pools"].as_array().into_iter().flatten() {
            if !self.holds(pool, ctx, rng) {
                continue;
            }
            let rolls = number(&pool["rolls"], rng).round().max(0.0) as i32;
            let pool_start = out.len();
            for _ in 0..rolls {
                self.roll_entries(pool["entries"].as_array().map_or(&[][..], Vec::as_slice), ctx, rng, out, depth);
            }
            self.modify(pool, ctx, rng, &mut out[pool_start..]);
        }
        self.modify(table, ctx, rng, &mut out[start..]);
    }

    /// Picks one entry by weight among those whose conditions hold, as a pool roll does.
    fn roll_entries(&self, entries: &[Value], ctx: &LootContext, rng: &mut impl Rng, out: &mut Vec<Slot>, depth: u32) {
        let mut candidates: Vec<(&Value, i32)> = Vec::new();
        for entry in entries {
            self.expand(entry, ctx, rng, &mut candidates);
        }
        let total: i32 = candidates.iter().map(|(_, w)| *w).sum();
        if total <= 0 {
            return;
        }
        let mut pick = rng.gen_range(0..total);
        for (entry, weight) in candidates {
            if pick < weight {
                self.produce(entry, ctx, rng, out, depth);
                return;
            }
            pick -= weight;
        }
    }

    /// Composite entries resolve to the leaf entries that can be chosen.
    fn expand<'v>(&self, entry: &'v Value, ctx: &LootContext, rng: &mut impl Rng, out: &mut Vec<(&'v Value, i32)>) {
        if !self.holds(entry, ctx, rng) {
            return;
        }
        let children = || entry["children"].as_array().map_or(&[][..], Vec::as_slice);
        match kind(entry) {
            "minecraft:alternatives" => {
                for child in children() {
                    let before = out.len();
                    self.expand(child, ctx, rng, out);
                    if out.len() > before {
                        return;
                    }
                }
            }
            "minecraft:group" => {
                for child in children() {
                    self.expand(child, ctx, rng, out);
                }
            }
            "minecraft:sequence" => {
                for child in children() {
                    let before = out.len();
                    self.expand(child, ctx, rng, out);
                    if out.len() == before {
                        return;
                    }
                }
            }
            _ => out.push((entry, entry["weight"].as_i64().unwrap_or(1) as i32)),
        }
    }

    fn produce(&self, entry: &Value, ctx: &LootContext, rng: &mut impl Rng, out: &mut Vec<Slot>, depth: u32) {
        let start = out.len();
        match kind(entry) {
            "minecraft:item" => {
                if let Some(id) = entry["name"].as_str().and_then(items::id) {
                    out.push(Slot::of(id as i32, 1));
                }
            }
            "minecraft:tag" => {
                let tag = entry["name"].as_str().unwrap_or("");
                let ids = paper_data::synced::tag("minecraft:item", tag).unwrap_or(&[]);
                if entry["expand"].as_bool().unwrap_or(false) {
                    if !ids.is_empty() {
                        out.push(Slot::of(ids[rng.gen_range(0..ids.len())], 1));
                    }
                } else {
                    out.extend(ids.iter().map(|id| Slot::of(*id, 1)));
                }
            }
            "minecraft:loot_table" => {
                let nested = match &entry["value"] {
                    Value::String(id) => self.table(id),
                    v @ Value::Object(_) => Some(Arc::new(v.clone())),
                    _ => entry["name"].as_str().and_then(|id| self.table(id)),
                };
                if let Some(nested) = nested {
                    self.roll_table(&nested, ctx, rng, out, depth + 1);
                }
            }
            _ => {}
        }
        self.modify(entry, ctx, rng, &mut out[start..]);
    }
}

impl LootTables {
    /// One item modifier (or a list of them) on the stacks it applies to.
    pub(crate) fn apply(&self, f: &Value, ctx: &LootContext, rng: &mut impl Rng, stacks: &mut [Slot]) {
        if let Value::Array(list) = f {
            for f in list {
                self.apply(f, ctx, rng, stacks);
            }
            return;
        }
        let additional_cost = f["include_additional_cost_component"].as_bool().unwrap_or(false);
        match kind(f) {
            "minecraft:enchant_randomly" => {
                if let Some(enchantments) = self.enchantments() {
                    for slot in stacks.iter_mut() {
                        let chosen = enchantments.enchant_randomly(slot, &f["options"], f["only_compatible"].as_bool().unwrap_or(true), rng);
                        // What a trade adds to its price: vanilla's enchanted book price.
                        if let (true, Some((_, level)), Some(stack)) = (additional_cost, chosen, slot.0.as_mut()) {
                            add_trade_cost(stack, 2 + rng.gen_range(0..5 + level * 10) + 3 * level);
                        }
                    }
                }
            }
            "minecraft:enchant_with_levels" => {
                if let Some(enchantments) = self.enchantments() {
                    for slot in stacks.iter_mut() {
                        let levels = number(&f["levels"], rng).floor() as i32;
                        enchantments.enchant_with_levels(slot, levels, &f["options"], rng);
                        if let (true, Some(stack)) = (additional_cost, slot.0.as_mut()) {
                            add_trade_cost(stack, levels);
                        }
                    }
                }
            }
            "minecraft:set_random_potion" => {
                let ids: Vec<i32> = match f["options"].as_str() {
                    Some(tag) if tag.starts_with('#') => paper_data::synced::tag("minecraft:potion", &tag[1..]).map(<[i32]>::to_vec).unwrap_or_default(),
                    Some(name) => paper_data::registries::id("minecraft:potion", name).map(|i| vec![i as i32]).unwrap_or_default(),
                    None => paper_data::registries::entries("minecraft:potion").map(|e| (0..e.len() as i32).collect()).unwrap_or_default(),
                };
                for stack in stacks.iter_mut().filter_map(|s| s.0.as_mut()) {
                    if !ids.is_empty() {
                        set_potion(stack, ids[rng.gen_range(0..ids.len())]);
                    }
                }
            }
            "minecraft:set_stew_effect" => {
                let Some(effects) = f["effects"].as_array().filter(|e| !e.is_empty()) else { return };
                for stack in stacks.iter_mut().filter_map(|s| s.0.as_mut()) {
                    let entry = &effects[rng.gen_range(0..effects.len())];
                    let Some(info) = entry["type"].as_str().and_then(paper_data::effects::by_name) else { continue };
                    let mut duration = number(&entry["duration"], rng) as i32;
                    if !info.instantaneous {
                        duration *= 20;
                    }
                    let mut data = Vec::new();
                    paper_protocol::codec::Encode::encode(&paper_protocol::codec::VarInt(1), &mut data);
                    paper_protocol::codec::Encode::encode(&paper_protocol::codec::VarInt(info.id), &mut data);
                    paper_protocol::codec::Encode::encode(&paper_protocol::codec::VarInt(duration), &mut data);
                    super::enchanting::set_component(stack, "minecraft:suspicious_stew_effects", data);
                }
            }
            "minecraft:set_random_dyes" => {
                for stack in stacks.iter_mut().filter_map(|s| s.0.as_mut()) {
                    let n = number(&f["number_of_dyes"], rng) as i32;
                    let dyes: Vec<u32> = (0..n.max(0)).map(|_| DYE_COLORS[rng.gen_range(0..DYE_COLORS.len())]).collect();
                    if !dyes.is_empty() && is_dyeable(stack.item) {
                        let current = dyed_color(stack);
                        super::enchanting::set_component(stack, "minecraft:dyed_color", mix_dyes(current, &dyes).to_be_bytes().to_vec());
                    }
                }
            }
            "minecraft:filtered" => {
                for slot in stacks.iter_mut() {
                    let pass = slot.0.as_ref().is_some_and(|s| item_filter(&f["item_filter"], s));
                    let branch = if pass { &f["on_pass"] } else { &f["on_fail"] };
                    if !branch.is_null() {
                        self.apply(branch, ctx, rng, std::slice::from_mut(slot));
                    }
                }
            }
            "minecraft:discard" => {
                for slot in stacks.iter_mut() {
                    *slot = Slot::EMPTY;
                }
            }
            // Explorer maps need maps, which this server does not keep yet: the stack stays
            // a blank explorer map (and trades that require a located map drop out).
            "minecraft:exploration_map" => {}
            "minecraft:set_damage" => {
                for stack in stacks.iter_mut().filter_map(|s| s.0.as_mut()) {
                    let Some(max) = paper_data::item_data::by_id(stack.item as u32).max_damage.filter(|m| *m > 0) else { continue };
                    let current = if f["add"].as_bool().unwrap_or(false) { 1.0 - damage_of(stack) as f64 / max as f64 } else { 0.0 };
                    let fraction = (number(&f["damage"], rng) + current).clamp(0.0, 1.0);
                    let damage = ((1.0 - fraction) * max as f64).floor() as i32;
                    let mut data = Vec::new();
                    paper_protocol::codec::Encode::encode(&paper_protocol::codec::VarInt(damage), &mut data);
                    super::enchanting::set_component(stack, "minecraft:damage", data);
                }
            }
            "minecraft:set_potion" => {
                let Some(potion) = f["id"].as_str().and_then(|p| paper_data::registries::id("minecraft:potion", p)) else { return };
                for stack in stacks.iter_mut().filter_map(|s| s.0.as_mut()) {
                    set_potion(stack, potion as i32);
                }
            }
            _ => apply_function(f, ctx, rng, stacks),
        }
    }
}

/// The `damage` component of a stack (0 when undamaged).
fn damage_of(stack: &paper_protocol::types::ItemStack) -> i32 {
    let Some(ty) = paper_data::registries::id("minecraft:data_component_type", "minecraft:damage") else { return 0 };
    stack.added.iter().find(|(t, _)| *t as u32 == ty).and_then(|(_, d)| paper_protocol::codec::Reader::new(d).varint().ok()).unwrap_or(0)
}

/// The `potion_contents` of a plain potion: the potion, no custom colour, no custom
/// effects, no custom name.
pub fn potion_contents(potion: i32) -> Vec<u8> {
    let mut data = vec![1u8];
    paper_protocol::codec::Encode::encode(&paper_protocol::codec::VarInt(potion), &mut data);
    data.extend_from_slice(&[0, 0, 0]);
    data
}

fn set_potion(stack: &mut ItemStack, potion: i32) {
    super::enchanting::set_component(stack, "minecraft:potion_contents", potion_contents(potion));
}

/// Adds to a stack's `additional_trade_cost` (what a trade adds to its price for it).
fn add_trade_cost(stack: &mut ItemStack, cost: i32) {
    let Some(ty) = paper_data::registries::id("minecraft:data_component_type", "minecraft:additional_trade_cost") else { return };
    let current = stack.added.iter().find(|(t, _)| *t as u32 == ty).and_then(|(_, d)| paper_protocol::codec::Reader::new(d).varint().ok()).unwrap_or(0);
    let mut data = Vec::new();
    paper_protocol::codec::Encode::encode(&paper_protocol::codec::VarInt(current + cost), &mut data);
    super::enchanting::set_component(stack, "minecraft:additional_trade_cost", data);
}

/// Takes a stack's `additional_trade_cost` off it, returning the cost.
pub fn take_trade_cost(stack: &mut ItemStack) -> i32 {
    let Some(ty) = paper_data::registries::id("minecraft:data_component_type", "minecraft:additional_trade_cost") else { return 0 };
    let Some(i) = stack.added.iter().position(|(t, _)| *t as u32 == ty) else { return 0 };
    let (_, data) = stack.added.remove(i);
    paper_protocol::codec::Reader::new(&data).varint().unwrap_or(0)
}

/// Texture colours of the sixteen dyes (`DyeColor.getTextureDiffuseColor`), in dye order.
pub const DYE_COLORS: [u32; 16] = [
    0xF9FFFE, 0xF9801D, 0xC74EBD, 0x3AB3DA, 0xFED83D, 0x80C71F, 0xF38BAA, 0x474F52, 0x9D9D97, 0x169C9C, 0x8932B8, 0x3C44AA, 0x835432, 0x5E7C16, 0xB02E26, 0x1D1D21,
];

fn is_dyeable(item: i32) -> bool {
    paper_data::synced::tag("minecraft:item", "minecraft:cauldron_can_remove_dye").is_some_and(|ids| ids.contains(&item))
}

fn dyed_color(stack: &ItemStack) -> Option<u32> {
    let ty = paper_data::registries::id("minecraft:data_component_type", "minecraft:dyed_color")?;
    let (_, data) = stack.added.iter().find(|(t, _)| *t as u32 == ty)?;
    Some(u32::from_be_bytes(data.get(..4)?.try_into().ok()?))
}

/// `DyedItemColor.applyDyes`: the average of the colours, kept as bright as they were.
pub fn mix_dyes(current: Option<u32>, dyes: &[u32]) -> u32 {
    let (mut r, mut g, mut b, mut intensity, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
    for color in current.into_iter().chain(dyes.iter().copied()) {
        let (cr, cg, cb) = ((color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF);
        intensity += cr.max(cg).max(cb);
        r += cr;
        g += cg;
        b += cb;
        n += 1;
    }
    if n == 0 {
        return 0;
    }
    let (r, g, b) = (r / n, g / n, b / n);
    let average = intensity as f32 / n as f32;
    let max = r.max(g).max(b).max(1) as f32;
    let scale = |c: u32| ((c as f32 * average / max) as u32).min(255);
    (scale(r) << 16) | (scale(g) << 8) | scale(b)
}

/// An `ItemPredicate` as `filtered` uses it: items (an id, `#tag` or list) and component
/// predicates, which this checks for presence (and enchantments for being non-empty).
pub fn item_filter(filter: &Value, stack: &ItemStack) -> bool {
    let items_ok = match &filter["items"] {
        Value::Null => true,
        Value::String(s) if s.starts_with('#') => paper_data::synced::tag("minecraft:item", &s[1..]).is_some_and(|ids| ids.contains(&stack.item)),
        Value::String(s) => paper_data::items::id(s).is_some_and(|i| i as i32 == stack.item),
        Value::Array(list) => list.iter().filter_map(Value::as_str).any(|s| paper_data::items::id(s).is_some_and(|i| i as i32 == stack.item)),
        _ => false,
    };
    if !items_ok {
        return false;
    }
    let components = filter["predicates"].as_object().into_iter().flatten().map(|(k, _)| k).chain(filter["components"].as_object().into_iter().flatten().map(|(k, _)| k));
    for name in components {
        match name.as_str() {
            "minecraft:enchantments" | "minecraft:stored_enchantments" => {
                if super::enchanting::read_enchantments(stack, name).is_empty() {
                    return false;
                }
            }
            other => {
                let Some(ty) = paper_data::registries::id("minecraft:data_component_type", other) else { return false };
                if !stack.added.iter().any(|(t, _)| *t as u32 == ty) {
                    return false;
                }
            }
        }
    }
    true
}

/// Evaluates a number provider: a constant, `uniform`, `binomial`, `constant` or `add`.
pub(crate) fn number(v: &Value, rng: &mut impl Rng) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::Object(o) => match o.get("type").and_then(Value::as_str).unwrap_or("minecraft:uniform") {
            "minecraft:constant" => o.get("value").and_then(Value::as_f64).unwrap_or(0.0),
            "minecraft:add" => o.get("inputs").and_then(Value::as_array).map_or(0.0, |inputs| inputs.iter().map(|i| number(i, rng)).sum()),
            "minecraft:binomial" => {
                let n = number(o.get("n").unwrap_or(&Value::Null), rng).round() as i32;
                let p = number(o.get("p").unwrap_or(&Value::Null), rng);
                (0..n.max(0)).filter(|_| rng.gen::<f64>() < p).count() as f64
            }
            _ => {
                let min = number(o.get("min").unwrap_or(&Value::Null), rng);
                let max = number(o.get("max").unwrap_or(&Value::Null), rng);
                if max > min {
                    rng.gen_range(min..=max)
                } else {
                    min
                }
            }
        },
        _ => 0.0,
    }
}

/// Level of an enchantment on an item (read from its `enchantments` component).
pub fn enchantment_level(slot: Option<&Slot>, enchantment: &str) -> i32 {
    let Some(stack) = slot.and_then(|s| s.0.as_ref()) else { return 0 };
    let Some(component) = paper_data::registries::id("minecraft:data_component_type", "minecraft:enchantments") else { return 0 };
    let Some(wanted) = paper_data::synced::id("minecraft:enchantment", enchantment) else { return 0 };
    for (ty, data) in &stack.added {
        if *ty as u32 != component {
            continue;
        }
        let mut r = paper_protocol::codec::Reader::new(data);
        let Ok(count) = r.varint() else { return 0 };
        for _ in 0..count.max(0) {
            let (Ok(id), Ok(level)) = (r.varint(), r.varint()) else { return 0 };
            if id == wanted {
                return level;
            }
        }
    }
    0
}

fn tool_matches(predicate: &Value, tool: Option<&Slot>) -> bool {
    let stack = tool.and_then(|t| t.0.as_ref());
    if let Some(items_v) = predicate.get("items") {
        let Some(stack) = stack else { return false };
        let ok = match items_v {
            Value::String(s) if s.starts_with('#') => paper_data::synced::tag("minecraft:item", &s[1..]).is_some_and(|ids| ids.contains(&stack.item)),
            Value::String(s) => items::id(s) == Some(stack.item as u32),
            Value::Array(list) => list.iter().filter_map(Value::as_str).any(|s| items::id(s) == Some(stack.item as u32)),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    let enchantments = predicate.get("predicates").and_then(|p| p.get("minecraft:enchantments")).or_else(|| predicate.get("enchantments"));
    if let Some(list) = enchantments.and_then(Value::as_array) {
        for e in list {
            let names: Vec<&str> = match &e["enchantments"] {
                Value::String(s) => vec![s.as_str()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            let min = e["levels"]["min"].as_i64().or_else(|| e["levels"].as_i64()).unwrap_or(1) as i32;
            if !names.iter().any(|n| enchantment_level(tool, n) >= min) {
                return false;
            }
        }
    }
    true
}

fn block_matches(blocks_v: &Value, state: u32) -> bool {
    let name = blocks::block_of_state(state).name;
    match blocks_v {
        Value::String(s) if s.starts_with('#') => paper_data::synced::tag("minecraft:block", &s[1..]).is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(state) as i32))),
        Value::String(s) => s == name,
        Value::Array(a) => a.iter().any(|b| block_matches(b, state)),
        _ => true,
    }
}

fn condition_holds(c: &Value, ctx: &LootContext, rng: &mut impl Rng) -> bool {
    match kind(c) {
        "minecraft:survives_explosion" => ctx.explosion_radius.is_none_or(|r| rng.gen::<f32>() <= 1.0 / r),
        "minecraft:match_tool" => tool_matches(&c["predicate"], ctx.tool),
        "minecraft:match_block" => {
            let Some(state) = ctx.block_state else { return false };
            (c["blocks"].is_null() || block_matches(&c["blocks"], state))
                && c["state"].as_object().is_none_or(|props| props.iter().all(|(k, v)| blocks::property(state, k) == v.as_str()))
        }
        "minecraft:random_chance" => rng.gen::<f64>() < number(&c["chance"], rng),
        "minecraft:random_chance_with_enchanted_bonus" => {
            let level = enchantment_level(ctx.tool, c["enchantment"].as_str().unwrap_or(""));
            let chance = if level > 0 {
                let b = &c["enchanted_chance"];
                match b["type"].as_str() {
                    Some("minecraft:linear") => b["base"].as_f64().unwrap_or(0.0) + b["per_level_above_first"].as_f64().unwrap_or(0.0) * (level - 1) as f64,
                    _ => number(b, rng),
                }
            } else {
                c["unenchanted_chance"].as_f64().unwrap_or(0.0)
            };
            rng.gen::<f64>() < chance
        }
        "minecraft:table_bonus" => {
            let level = enchantment_level(ctx.tool, c["enchantment"].as_str().unwrap_or("")) as usize;
            let chances: Vec<f64> = c["chances"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            chances.get(level.min(chances.len().saturating_sub(1))).is_some_and(|p| rng.gen::<f64>() < *p)
        }
        "minecraft:killed_by_player" => ctx.killed_by_player,
        "minecraft:block_state_property" => {
            let Some(state) = ctx.block_state else { return false };
            if let Some(block) = c["block"].as_str() {
                if blocks::block_of_state(state).name != block.trim_start_matches('#') && !block.starts_with('#') {
                    return false;
                }
            }
            c["properties"].as_object().is_none_or(|props| props.iter().all(|(k, v)| blocks::property(state, k) == v.as_str()))
        }
        "minecraft:entity_properties" => {
            let predicate = &c["predicate"];
            if c["entity"].as_str().is_some_and(|e| e != "this") {
                return false;
            }
            let Some(fields) = predicate.as_object() else { return true };
            fields.iter().all(|(k, v)| match k.as_str() {
                "minecraft:flags" | "flags" => v.get("is_on_fire").and_then(Value::as_bool).is_none_or(|f| f == ctx.on_fire),
                // Vehicles, equipment, locations, ...: not modelled, so not matched.
                _ => false,
            })
        }
        // Unknown conditions (weather, location, ...) are treated as met.
        _ => true,
    }
}

fn apply_function(f: &Value, ctx: &LootContext, rng: &mut impl Rng, stacks: &mut [Slot]) {
    {
        for slot in stacks.iter_mut() {
            let Some(stack) = slot.0.as_mut() else { continue };
            match kind(f) {
                "minecraft:set_count" => {
                    let n = number(&f["count"], rng).floor() as i32;
                    stack.count = if f["add"].as_bool().unwrap_or(false) { stack.count + n } else { n };
                }
                "minecraft:apply_bonus" => {
                    let level = enchantment_level(ctx.tool, f["enchantment"].as_str().unwrap_or(""));
                    match f["formula"].as_str().unwrap_or("") {
                        "minecraft:ore_drops" => {
                            if level > 0 {
                                let bonus = rng.gen_range(0..level + 2) - 1;
                                stack.count *= bonus.max(0) + 1;
                            }
                        }
                        "minecraft:uniform_bonus_count" => {
                            let multiplier = f["parameters"]["bonusMultiplier"].as_i64().unwrap_or(1) as i32;
                            stack.count += rng.gen_range(0..=level * multiplier);
                        }
                        "minecraft:binomial_with_bonus_count" => {
                            let p = f["parameters"]["probability"].as_f64().unwrap_or(0.5);
                            let extra = f["parameters"]["extra"].as_i64().unwrap_or(0) as i32;
                            stack.count += (0..level + extra).filter(|_| rng.gen::<f64>() < p).count() as i32;
                        }
                        _ => {}
                    }
                }
                "minecraft:explosion_decay" => {
                    if let Some(r) = ctx.explosion_radius {
                        let keep = 1.0 / r;
                        stack.count = (0..stack.count).filter(|_| rng.gen::<f32>() <= keep).count() as i32;
                    }
                }
                "minecraft:limit_count" => {
                    let limit = &f["limit"];
                    if let Some(min) = limit.get("min") {
                        stack.count = stack.count.max(number(min, rng) as i32);
                    }
                    if let Some(max) = limit.get("max") {
                        stack.count = stack.count.min(number(max, rng) as i32);
                    }
                }
                "minecraft:enchanted_count_increase" => {
                    let level = enchantment_level(ctx.tool, f["enchantment"].as_str().unwrap_or("minecraft:looting"));
                    if level > 0 {
                        let per = number(&f["count"], rng);
                        stack.count += (per * level as f64).round() as i32;
                        if let Some(limit) = f["limit"].as_i64().filter(|l| *l > 0) {
                            stack.count = stack.count.min(limit as i32);
                        }
                    }
                }
                "minecraft:furnace_smelt" => {
                    if ctx.on_fire {
                        if let Some(cooked) = cooked_version(stack.item) {
                            stack.item = cooked;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// The cooked form of raw meat dropped by a burning animal.
fn cooked_version(item: i32) -> Option<i32> {
    let name = items::get(item as u32)?.name;
    let cooked = match name {
        "minecraft:beef" => "minecraft:cooked_beef",
        "minecraft:porkchop" => "minecraft:cooked_porkchop",
        "minecraft:chicken" => "minecraft:cooked_chicken",
        "minecraft:mutton" => "minecraft:cooked_mutton",
        "minecraft:rabbit" => "minecraft:cooked_rabbit",
        "minecraft:cod" => "minecraft:cooked_cod",
        "minecraft:salmon" => "minecraft:cooked_salmon",
        _ => return None,
    };
    items::id(cooked).map(|i| i as i32)
}

/// Drops of a block when no loot tables are available: the block's own item.
pub fn fallback_block_drop(state: u32) -> Vec<Slot> {
    blocks::block_of_state(state).item.and_then(items::id).map(|id| vec![Slot::of(id as i32, 1)]).unwrap_or_default()
}

#[allow(dead_code)]
fn _uses(_: ItemStack) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> Option<LootTables> {
        let jar = crate::vanilla::test_jar()?;
        let cache = std::env::temp_dir().join(format!("paper-rs-loot-{}", std::process::id()));
        Some(LootTables::new(Some(VanillaData::open(&jar, &cache).ok()?)))
    }

    fn item(name: &str) -> i32 {
        items::id(name).unwrap() as i32
    }

    #[test]
    fn block_drops() {
        let Some(tables) = tables() else { return };
        let stone = blocks::default_state("minecraft:stone").unwrap();
        let pick = Slot::of(item("minecraft:iron_pickaxe"), 1);
        let drops = tables.roll("minecraft:blocks/stone", &LootContext { tool: Some(&pick), block_state: Some(stone), ..Default::default() });
        assert_eq!(drops, vec![Slot::of(item("minecraft:cobblestone"), 1)]);
        let dirt = tables.roll("minecraft:blocks/dirt", &LootContext::default());
        assert_eq!(dirt, vec![Slot::of(item("minecraft:dirt"), 1)]);
        // Grass needs silk touch to drop itself; otherwise dirt.
        let grass = tables.roll("minecraft:blocks/grass_block", &LootContext::default());
        assert_eq!(grass, vec![Slot::of(item("minecraft:dirt"), 1)]);
        // Glass drops nothing without silk touch.
        assert!(tables.roll("minecraft:blocks/glass", &LootContext::default()).is_empty());
        // Coal ore drops 1 coal without fortune.
        let coal = tables.roll("minecraft:blocks/coal_ore", &LootContext { tool: Some(&pick), ..Default::default() });
        assert_eq!(coal, vec![Slot::of(item("minecraft:coal"), 1)]);
        // Doors drop once, from the lower half.
        let lower = blocks::parse_state("minecraft:oak_door[half=lower]").unwrap();
        let upper = blocks::parse_state("minecraft:oak_door[half=upper]").unwrap();
        assert_eq!(tables.roll("minecraft:blocks/oak_door", &LootContext { block_state: Some(lower), ..Default::default() }).len(), 1);
        assert!(tables.roll("minecraft:blocks/oak_door", &LootContext { block_state: Some(upper), ..Default::default() }).is_empty());
    }

    #[test]
    fn mob_drops() {
        let Some(tables) = tables() else { return };
        for _ in 0..20 {
            let drops = tables.roll("minecraft:entities/zombie", &LootContext { killed_by_player: true, ..Default::default() });
            for d in &drops {
                let id = d.0.as_ref().unwrap().item;
                assert!(
                    [item("minecraft:rotten_flesh"), item("minecraft:iron_ingot"), item("minecraft:carrot"), item("minecraft:potato")].contains(&id),
                    "unexpected zombie drop {:?}",
                    items::get(id as u32).map(|i| i.name)
                );
            }
        }
        let beef: Vec<Slot> = (0..30).flat_map(|_| tables.roll("minecraft:entities/cow", &LootContext { on_fire: true, ..Default::default() })).collect();
        assert!(beef.iter().any(|s| s.0.as_ref().unwrap().item == item("minecraft:cooked_beef")), "burning cows drop cooked beef");
    }
}
