//! Crafting and cooking recipes, loaded from the vanilla data (`data/minecraft/recipe/*.json`).
//!
//! Supported: `crafting_shaped` (with mirroring), `crafting_shapeless`, `crafting_transmute`
//! (the input's components carry over), and the cooking types `smelting`, `blasting`,
//! `smoking` and `campfire_cooking`. Special recipes (dyeing armour, map copying, ...) are
//! not implemented.

use crate::vanilla::VanillaData;
use paper_data::items;
use paper_protocol::types::{ItemStack, Slot};
use serde_json::Value;
use std::collections::HashMap;

/// Item ids an ingredient accepts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ingredient(pub Vec<i32>);

impl Ingredient {
    fn parse(v: &Value) -> Option<Ingredient> {
        let mut ids = Vec::new();
        let mut add = |s: &str| {
            if let Some(tag) = s.strip_prefix('#') {
                ids.extend(paper_data::synced::tag("minecraft:item", tag).unwrap_or(&[]).iter().copied());
            } else if let Some(id) = items::id(s) {
                ids.push(id as i32);
            }
        };
        match v {
            Value::String(s) => add(s),
            Value::Array(list) => {
                for e in list {
                    match e {
                        Value::String(s) => add(s),
                        Value::Object(o) => {
                            if let Some(s) = o.get("item").and_then(Value::as_str) {
                                add(s);
                            } else if let Some(t) = o.get("tag").and_then(Value::as_str) {
                                add(&format!("#{t}"));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Value::Object(o) => {
                if let Some(s) = o.get("item").and_then(Value::as_str) {
                    add(s);
                } else if let Some(t) = o.get("tag").and_then(Value::as_str) {
                    add(&format!("#{t}"));
                }
            }
            _ => {}
        }
        (!ids.is_empty()).then_some(Ingredient(ids))
    }

    pub fn matches(&self, slot: &Slot) -> bool {
        slot.0.as_ref().is_some_and(|s| self.0.contains(&s.item))
    }
}

#[derive(Clone, Debug)]
pub enum CraftingRecipe {
    Shaped { id: String, width: usize, height: usize, cells: Vec<Option<Ingredient>>, result: ItemStack },
    Shapeless { id: String, ingredients: Vec<Ingredient>, result: ItemStack },
    /// Puts `material` with an `input` item; the result keeps the input's components.
    Transmute { id: String, input: Ingredient, material: Ingredient, result: ItemStack },
}

#[derive(Clone, Debug)]
pub struct CookingRecipe {
    pub id: String,
    pub kind: String,
    pub ingredient: Ingredient,
    pub result: ItemStack,
    pub experience: f32,
    pub cooking_time: i32,
}

/// All recipes.
#[derive(Default)]
pub struct Recipes {
    crafting: Vec<CraftingRecipe>,
    cooking: Vec<CookingRecipe>,
    /// Burn time in ticks per fuel item: (normal furnace, smoker and blast furnace).
    fuel: HashMap<i32, (i32, i32)>,
}

/// Evaluates a `context_int_provider` (constants, arithmetic, the fast-cooking condition).
fn int_provider(v: &Value, providers: &HashMap<String, Value>, fast: bool, depth: u32) -> Option<i32> {
    if depth > 16 {
        return None;
    }
    match v {
        Value::Number(n) => n.as_i64().map(|n| n as i32),
        Value::String(id) => int_provider(providers.get(id)?, providers, fast, depth + 1),
        Value::Object(o) => {
            let arg = |k: &str| int_provider(o.get(k)?, providers, fast, depth + 1);
            match o.get("type").and_then(Value::as_str).unwrap_or("") {
                "minecraft:constant" => o.get("value").and_then(Value::as_i64).map(|n| n as i32),
                "minecraft:div" => {
                    let (l, r) = (arg("left")?, arg("right")?);
                    (r != 0).then(|| l / r)
                }
                "minecraft:mul" => Some(arg("left")? * arg("right")?),
                "minecraft:add" | "minecraft:sum" => Some(arg("left")? + arg("right")?),
                "minecraft:sub" => Some(arg("left")? - arg("right")?),
                "minecraft:conditional" => {
                    let fast_condition = o.get("condition").and_then(Value::as_str).is_some_and(|c| c.ends_with("fast_cooking"));
                    if fast_condition && fast {
                        arg("on_true")
                    } else {
                        arg("on_false")
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn result_stack(v: &Value) -> Option<ItemStack> {
    let (id, count) = match v {
        Value::String(s) => (s.as_str(), 1),
        Value::Object(o) => (o.get("id").or_else(|| o.get("item")).and_then(Value::as_str)?, o.get("count").and_then(Value::as_i64).unwrap_or(1) as i32),
        _ => return None,
    };
    Some(ItemStack::new(items::id(id)? as i32, count))
}

impl Recipes {
    pub fn load(data: &VanillaData) -> Recipes {
        let mut r = Recipes::default();
        for (id, v) in data.registry("recipe") {
            let ty = v["type"].as_str().unwrap_or("");
            match ty {
                "minecraft:crafting_shaped" => {
                    let pattern: Vec<String> = v["pattern"].as_array().map(|p| p.iter().filter_map(|row| row.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                    let keys: HashMap<char, Ingredient> = v["key"].as_object().map(|k| k.iter().filter_map(|(c, ing)| Some((c.chars().next()?, Ingredient::parse(ing)?))).collect()).unwrap_or_default();
                    let (height, width) = (pattern.len(), pattern.iter().map(|row| row.chars().count()).max().unwrap_or(0));
                    let mut cells = Vec::with_capacity(width * height);
                    let mut valid = true;
                    for row in &pattern {
                        let chars: Vec<char> = row.chars().collect();
                        for x in 0..width {
                            match chars.get(x) {
                                Some(' ') | None => cells.push(None),
                                Some(c) => match keys.get(c) {
                                    Some(ing) => cells.push(Some(ing.clone())),
                                    None => valid = false,
                                },
                            }
                        }
                    }
                    if let (true, Some(result)) = (valid && width > 0, result_stack(&v["result"])) {
                        r.crafting.push(CraftingRecipe::Shaped { id, width, height, cells, result });
                    }
                }
                "minecraft:crafting_shapeless" => {
                    let ingredients: Option<Vec<Ingredient>> = v["ingredients"].as_array().map(|list| list.iter().map(Ingredient::parse).collect::<Option<Vec<_>>>()).unwrap_or(None);
                    if let (Some(ingredients), Some(result)) = (ingredients, result_stack(&v["result"])) {
                        r.crafting.push(CraftingRecipe::Shapeless { id, ingredients, result });
                    }
                }
                "minecraft:crafting_transmute" => {
                    if let (Some(input), Some(material), Some(result)) = (Ingredient::parse(&v["input"]), Ingredient::parse(&v["material"]), result_stack(&v["result"])) {
                        r.crafting.push(CraftingRecipe::Transmute { id, input, material, result });
                    }
                }
                "minecraft:smelting" | "minecraft:blasting" | "minecraft:smoking" | "minecraft:campfire_cooking" => {
                    if let (Some(ingredient), Some(result)) = (Ingredient::parse(&v["ingredient"]), result_stack(&v["result"])) {
                        let default_time = match ty {
                            "minecraft:smelting" => 200,
                            "minecraft:campfire_cooking" => 600,
                            _ => 100,
                        };
                        r.cooking.push(CookingRecipe {
                            id,
                            kind: ty.to_owned(),
                            ingredient,
                            result,
                            experience: v["experience"].as_f64().unwrap_or(0.0) as f32,
                            cooking_time: v["cookingtime"].as_i64().map_or(default_time, |t| t as i32),
                        });
                    }
                }
                _ => {}
            }
        }
        // Fuel: each item's `cooking_fuel` names a context_int_provider with the burn time.
        let providers: HashMap<String, Value> = data.registry("context_int_provider").into_iter().collect();
        for item in 0..items::count() {
            if let Some(fuel) = paper_data::item_data::by_id(item as u32).fuel.as_deref() {
                let reference = Value::String(fuel.to_owned());
                let normal = int_provider(&reference, &providers, false, 0).or_else(|| fallback_fuel(fuel));
                let fast = int_provider(&reference, &providers, true, 0).or(normal.map(|n| n / 2));
                if let (Some(normal), Some(fast)) = (normal, fast) {
                    r.fuel.insert(item as i32, (normal, fast));
                }
            }
        }
        r
    }

    pub fn crafting_count(&self) -> usize {
        self.crafting.len()
    }

    pub fn cooking_count(&self) -> usize {
        self.cooking.len()
    }

    /// The result of a crafting grid (`size` x `size`, row-major), if any recipe matches.
    pub fn craft(&self, grid: &[Slot], size: usize) -> Option<(usize, ItemStack)> {
        if grid.iter().all(Slot::is_empty) {
            return None;
        }
        for (index, recipe) in self.crafting.iter().enumerate() {
            if let Some(result) = matches(recipe, grid, size) {
                return Some((index, result));
            }
        }
        None
    }

    /// Smelting (or `kind`) result for an input.
    pub fn cook(&self, kind: &str, input: &Slot) -> Option<&CookingRecipe> {
        self.cooking.iter().find(|r| r.kind == kind && r.ingredient.matches(input))
    }

    pub fn can_cook(&self, kind: &str, input: &Slot) -> bool {
        self.cook(kind, input).is_some()
    }

    /// Ticks a fuel item burns for, in a normal furnace or (`fast`) a smoker or blast furnace.
    pub fn burn_time(&self, item: &Slot, fast: bool) -> i32 {
        item.0.as_ref().and_then(|s| self.fuel.get(&s.item)).map_or(0, |(normal, quick)| if fast { *quick } else { *normal })
    }
}

/// Burn times of the common fuel providers, used if the data cannot be read.
fn fallback_fuel(provider: &str) -> Option<i32> {
    Some(match provider.trim_start_matches("minecraft:cooking/time_") {
        "lava" | "lava_bucket" => 20000,
        "coal_block" => 16000,
        "dried_kelp_block" => 4001,
        "blaze_rod" => 2400,
        "coal" | "charcoal" => 1600,
        "boat" => 1200,
        "log" | "wood" | "planks" => 300,
        "stick" | "sapling" => 100,
        _ => return None,
    })
}

fn matches(recipe: &CraftingRecipe, grid: &[Slot], size: usize) -> Option<ItemStack> {
    match recipe {
        CraftingRecipe::Shaped { width, height, cells, result, .. } => {
            if *width > size || *height > size {
                return None;
            }
            for oy in 0..=size - height {
                for ox in 0..=size - width {
                    for mirror in [false, true] {
                        let ok = (0..size).all(|gy| {
                            (0..size).all(|gx| {
                                let slot = &grid[gy * size + gx];
                                let (rx, ry) = (gx as isize - ox as isize, gy as isize - oy as isize);
                                if rx < 0 || ry < 0 || rx >= *width as isize || ry >= *height as isize {
                                    return slot.is_empty();
                                }
                                let rx = if mirror { *width - 1 - rx as usize } else { rx as usize };
                                match &cells[ry as usize * width + rx] {
                                    Some(ing) => ing.matches(slot),
                                    None => slot.is_empty(),
                                }
                            })
                        });
                        if ok {
                            return Some(result.clone());
                        }
                    }
                }
            }
            None
        }
        CraftingRecipe::Shapeless { ingredients, result, .. } => {
            let present: Vec<&Slot> = grid.iter().filter(|s| !s.is_empty()).collect();
            if present.len() != ingredients.len() {
                return None;
            }
            // Assign each item to a distinct ingredient (backtracking; grids are tiny).
            fn assign(items: &[&Slot], ingredients: &[Ingredient], used: &mut Vec<bool>, i: usize) -> bool {
                if i == items.len() {
                    return true;
                }
                for (j, ing) in ingredients.iter().enumerate() {
                    if !used[j] && ing.matches(items[i]) {
                        used[j] = true;
                        if assign(items, ingredients, used, i + 1) {
                            return true;
                        }
                        used[j] = false;
                    }
                }
                false
            }
            assign(&present, ingredients, &mut vec![false; ingredients.len()], 0).then(|| result.clone())
        }
        CraftingRecipe::Transmute { input, material, result, .. } => {
            let present: Vec<&Slot> = grid.iter().filter(|s| !s.is_empty()).collect();
            if present.len() != 2 {
                return None;
            }
            let (a, b) = (present[0], present[1]);
            let source = if input.matches(a) && material.matches(b) {
                a
            } else if input.matches(b) && material.matches(a) {
                b
            } else {
                return None;
            };
            let mut out = result.clone();
            if let Some(src) = &source.0 {
                out.added = src.added.clone();
                out.removed = src.removed.clone();
            }
            Some(out)
        }
    }
}

/// What remains in a crafting slot after its item is used: an empty bucket for a bucket of
/// milk, and so on.
pub fn crafting_remainder(item: i32) -> Option<Slot> {
    let name = items::get(item as u32)?.name;
    let remainder = match name {
        "minecraft:water_bucket" | "minecraft:lava_bucket" | "minecraft:milk_bucket" | "minecraft:powder_snow_bucket" => "minecraft:bucket",
        "minecraft:honey_bottle" | "minecraft:dragon_breath" => "minecraft:glass_bottle",
        _ => return None,
    };
    items::id(remainder).map(|id| Slot::of(id as i32, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipes() -> Option<Recipes> {
        let jar = crate::vanilla::test_jar()?;
        let cache = std::env::temp_dir().join(format!("paper-rs-recipes-{}", std::process::id()));
        let data = VanillaData::open(&jar, &cache).ok()?;
        Some(Recipes::load(&data))
    }

    fn slot(name: &str) -> Slot {
        Slot::of(items::id(name).unwrap() as i32, 1)
    }

    fn item(s: &ItemStack) -> &'static str {
        items::get(s.item as u32).unwrap().name
    }

    #[test]
    fn crafting_and_cooking() {
        let Some(r) = recipes() else { return };
        assert!(r.crafting_count() > 1000 && r.cooking_count() > 50);
        let e = Slot::EMPTY;
        // Planks from a log, anywhere in a 2x2 grid.
        let (_, planks) = r.craft(&[e.clone(), e.clone(), e.clone(), slot("minecraft:oak_log")], 2).unwrap();
        assert_eq!((item(&planks), planks.count), ("minecraft:oak_planks", 4));
        // Sticks: two planks stacked vertically, in either column.
        let p = slot("minecraft:oak_planks");
        let (_, sticks) = r.craft(&[e.clone(), p.clone(), e.clone(), p.clone()], 2).unwrap();
        assert_eq!((item(&sticks), sticks.count), ("minecraft:stick", 4));
        assert!(r.craft(&[p.clone(), p.clone(), e.clone(), e.clone()], 2).is_none_or(|(_, s)| item(&s) != "minecraft:stick"));
        // A pickaxe needs the 3x3 grid; its shape is fixed.
        let c = slot("minecraft:cobblestone");
        let s = slot("minecraft:stick");
        let grid = [c.clone(), c.clone(), c.clone(), e.clone(), s.clone(), e.clone(), e.clone(), s.clone(), e.clone()];
        assert_eq!(item(&r.craft(&grid, 3).unwrap().1), "minecraft:stone_pickaxe");
        // Mirrored shapes match too (an axe faces either way).
        let axe_left = [c.clone(), c.clone(), e.clone(), c.clone(), s.clone(), e.clone(), e.clone(), s.clone(), e.clone()];
        let axe_right = [e.clone(), c.clone(), c.clone(), e.clone(), s.clone(), c.clone(), e.clone(), s.clone(), e.clone()];
        assert_eq!(item(&r.craft(&axe_left, 3).unwrap().1), "minecraft:stone_axe");
        assert_eq!(item(&r.craft(&axe_right, 3).unwrap().1), "minecraft:stone_axe");
        // Shapeless: any arrangement.
        let dye = r.craft(&[e.clone(), slot("minecraft:dandelion"), e.clone(), e.clone()], 2).unwrap().1;
        assert_eq!(item(&dye), "minecraft:yellow_dye");
        // Cooking and fuel.
        assert_eq!(item(&r.cook("minecraft:smelting", &slot("minecraft:iron_ore")).unwrap().result), "minecraft:iron_ingot");
        assert_eq!(r.burn_time(&slot("minecraft:coal"), false), 1600);
        assert_eq!(r.burn_time(&slot("minecraft:coal"), true), 800);
        assert_eq!(r.burn_time(&slot("minecraft:oak_planks"), false), 300);
    }
}
