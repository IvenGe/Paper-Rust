//! Brewing stands (vanilla's `BrewingStandBlockEntity`, `BrewingStandMenu` and the mixes of
//! `PotionBrewing.addVanillaMixes`): blaze powder gives 20 brews, a brew takes 400 ticks and
//! turns the three bottles with the ingredient on top.

use super::containers::MenuKind;
use super::Server;
use paper_data::{blocks, items};
use paper_nbt::Tag;
use paper_protocol::codec::{BlockPos, Encode, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::{ItemStack, Slot};

/// (input potion, ingredient, output potion).
const MIXES: &[(&str, &str, &str)] = &[
    ("water", "glowstone_dust", "thick"),
    ("water", "redstone", "mundane"),
    ("water", "nether_wart", "awkward"),
    ("awkward", "golden_carrot", "night_vision"),
    ("night_vision", "redstone", "long_night_vision"),
    ("night_vision", "fermented_spider_eye", "invisibility"),
    ("long_night_vision", "fermented_spider_eye", "long_invisibility"),
    ("invisibility", "redstone", "long_invisibility"),
    ("fire_resistance", "redstone", "long_fire_resistance"),
    ("leaping", "redstone", "long_leaping"),
    ("leaping", "glowstone_dust", "strong_leaping"),
    ("leaping", "fermented_spider_eye", "slowness"),
    ("long_leaping", "fermented_spider_eye", "long_slowness"),
    ("slowness", "redstone", "long_slowness"),
    ("slowness", "glowstone_dust", "strong_slowness"),
    ("awkward", "turtle_helmet", "turtle_master"),
    ("turtle_master", "redstone", "long_turtle_master"),
    ("turtle_master", "glowstone_dust", "strong_turtle_master"),
    ("swiftness", "fermented_spider_eye", "slowness"),
    ("long_swiftness", "fermented_spider_eye", "long_slowness"),
    ("swiftness", "redstone", "long_swiftness"),
    ("swiftness", "glowstone_dust", "strong_swiftness"),
    ("awkward", "pufferfish", "water_breathing"),
    ("water_breathing", "redstone", "long_water_breathing"),
    ("healing", "glowstone_dust", "strong_healing"),
    ("healing", "fermented_spider_eye", "harming"),
    ("strong_healing", "fermented_spider_eye", "strong_harming"),
    ("harming", "glowstone_dust", "strong_harming"),
    ("poison", "fermented_spider_eye", "harming"),
    ("long_poison", "fermented_spider_eye", "harming"),
    ("strong_poison", "fermented_spider_eye", "strong_harming"),
    ("poison", "redstone", "long_poison"),
    ("poison", "glowstone_dust", "strong_poison"),
    ("regeneration", "redstone", "long_regeneration"),
    ("regeneration", "glowstone_dust", "strong_regeneration"),
    ("strength", "redstone", "long_strength"),
    ("strength", "glowstone_dust", "strong_strength"),
    ("water", "fermented_spider_eye", "weakness"),
    ("weakness", "redstone", "long_weakness"),
    ("awkward", "phantom_membrane", "slow_falling"),
    ("slow_falling", "redstone", "long_slow_falling"),
];

/// `addStartMix`: the ingredient makes awkward potions into these, and water into mundane.
const START_MIXES: &[(&str, &str)] = &[
    ("breeze_rod", "wind_charged"),
    ("slime_block", "oozing"),
    ("stone", "infested"),
    ("cobweb", "weaving"),
    ("magma_cream", "fire_resistance"),
    ("rabbit_foot", "leaping"),
    ("sugar", "swiftness"),
    ("glistering_melon_slice", "healing"),
    ("spider_eye", "poison"),
    ("ghast_tear", "regeneration"),
    ("blaze_powder", "strength"),
];

/// (bottle, ingredient, new bottle): splash and lingering potions.
const CONTAINER_MIXES: &[(&str, &str, &str)] = &[("potion", "gunpowder", "splash_potion"), ("splash_potion", "dragon_breath", "lingering_potion")];

fn name(stack: &ItemStack) -> &'static str {
    items::get(stack.item as u32).map_or("", |i| i.name).strip_prefix("minecraft:").unwrap_or("")
}

fn is_bottle(stack: &ItemStack) -> bool {
    matches!(name(stack), "potion" | "splash_potion" | "lingering_potion")
}

/// What may go into a bottle slot (`PotionSlot.mayPlaceItem`).
pub fn fits_bottle_slot(stack: &ItemStack) -> bool {
    is_bottle(stack) || name(stack) == "glass_bottle"
}

/// Brewing fuel (`#minecraft:brewing_fuel`: blaze powder).
pub fn is_fuel(stack: &ItemStack) -> bool {
    paper_data::synced::tag("minecraft:item", "minecraft:brewing_fuel").map_or(name(stack) == "blaze_powder", |ids| ids.contains(&stack.item))
}

/// Whether something can be brewed with (`PotionBrewing.isIngredient`).
pub fn is_ingredient(stack: &ItemStack) -> bool {
    let n = name(stack);
    MIXES.iter().any(|(_, i, _)| *i == n) || START_MIXES.iter().any(|(i, _)| *i == n) || CONTAINER_MIXES.iter().any(|(_, i, _)| *i == n)
}

fn potion_contents(potion: &str) -> Option<Vec<u8>> {
    let id = paper_data::registries::id("minecraft:potion", &format!("minecraft:{potion}"))?;
    // A potion, no custom colour, no custom effects, no custom name.
    let mut data = vec![1u8];
    VarInt(id as i32).encode(&mut data);
    data.extend_from_slice(&[0, 0, 0]);
    Some(data)
}

/// The bottle after brewing `bottle` with `ingredient` (`PotionBrewing.mix`), if they mix.
pub fn mix(ingredient: &ItemStack, bottle: &ItemStack) -> Option<ItemStack> {
    if !is_bottle(bottle) {
        return None;
    }
    let ing = name(ingredient);
    let potion = super::effects::potion_of(bottle).unwrap_or_else(|| "minecraft:water".into());
    let potion = potion.strip_prefix("minecraft:").unwrap_or(&potion);
    if let Some((_, _, out)) = CONTAINER_MIXES.iter().find(|(b, i, _)| *b == name(bottle) && *i == ing) {
        let mut result = bottle.clone();
        result.item = items::id(&format!("minecraft:{out}"))? as i32;
        return Some(result);
    }
    let output = MIXES
        .iter()
        .find(|(from, i, _)| *from == potion && *i == ing)
        .map(|(_, _, out)| *out)
        .or_else(|| START_MIXES.iter().find(|(i, _)| *i == ing).and_then(|(_, out)| match potion {
            "awkward" => Some(*out),
            "water" => Some("mundane"),
            _ => None,
        }))?;
    let mut result = bottle.clone();
    super::enchanting::set_component(&mut result, "minecraft:potion_contents", potion_contents(output)?);
    Some(result)
}

/// The brewing stand's slots: three bottles, the ingredient, the fuel.
fn brewable(items: &[Slot]) -> bool {
    let Some(ingredient) = items.get(3).and_then(|s| s.0.as_ref()) else { return false };
    is_ingredient(ingredient) && items[..3].iter().filter_map(|s| s.0.as_ref()).any(|b| mix(ingredient, b).is_some())
}

impl Server {
    pub(super) fn tick_brewing_stands(&mut self) {
        for world in 0..self.worlds.len() {
            let stands: Vec<BlockPos> = self.worlds[world]
                .chunks
                .iter()
                .filter(|(p, _)| self.worlds[world].is_ready(**p))
                .flat_map(|(cpos, chunk)| chunk.block_entities.iter().filter(|(_, nbt)| nbt.string("id") == Some("minecraft:brewing_stand")).map(move |((y, z, x), _)| BlockPos::new(cpos.x * 16 + *x as i32, *y, cpos.z * 16 + *z as i32)))
                .collect();
            for pos in stands {
                self.tick_brewing_stand(world, pos);
            }
        }
    }

    fn tick_brewing_stand(&mut self, world: usize, pos: BlockPos) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        if blocks::block_of_state(state).name != "minecraft:brewing_stand" {
            return;
        }
        let nbt = self.block_entity_data(world, pos).cloned().unwrap_or_default();
        let (mut brew_time, mut fuel) = (nbt.i32("BrewTime").unwrap_or(0), nbt.i32("Fuel").unwrap_or(0));
        let mut items = self.container_items(world, pos);
        if items.len() < 5 {
            return;
        }
        let before = (brew_time, fuel, items.clone());
        if fuel <= 0 && items[4].0.as_ref().is_some_and(is_fuel) {
            fuel = 20;
            let f = items[4].0.as_mut().unwrap();
            f.count -= 1;
            if f.count <= 0 {
                items[4] = Slot::EMPTY;
            }
        }
        let can_brew = brewable(&items);
        let ingredient = items[3].0.as_ref().map(|s| s.item);
        if brew_time > 0 {
            brew_time -= 1;
            let same = ingredient.is_some() && self.brewing_ingredients.get(&(world, pos)) == ingredient.as_ref();
            if brew_time == 0 && can_brew {
                self.finish_brew(world, pos, &mut items);
            } else if !can_brew || !same {
                brew_time = 0;
            }
        } else if can_brew && fuel > 0 {
            fuel -= 1;
            brew_time = 400;
            if let Some(i) = ingredient {
                self.brewing_ingredients.insert((world, pos), i);
            }
        }
        if (brew_time, fuel, &items) != (before.0, before.1, &before.2) {
            if items != before.2 {
                self.set_container_items(world, pos, &items);
                self.refresh_brewing_viewers(world, pos);
            }
            self.set_block_entity_int(world, pos, "BrewTime", brew_time, true);
            self.set_block_entity_int(world, pos, "Fuel", fuel, false);
            self.send_brewing_data(world, pos, brew_time, fuel);
        }
        // The bottles show on the stand.
        let mut shown = state;
        for (i, slot) in items[..3].iter().enumerate() {
            if let Some(s) = blocks::with_property(shown, &format!("has_bottle_{i}"), if slot.is_empty() { "false" } else { "true" }) {
                shown = s;
            }
        }
        if shown != state {
            self.set_block_and_update(world, pos, shown);
        }
    }

    /// `doBrew`: every bottle that mixes turns; the ingredient is used up (leaving its
    /// remainder, e.g. a glass bottle from dragon's breath).
    fn finish_brew(&mut self, world: usize, pos: BlockPos, items: &mut [Slot]) {
        let Some(ingredient) = items[3].0.clone() else { return };
        for slot in items[..3].iter_mut() {
            if let Some(result) = slot.0.as_ref().and_then(|b| mix(&ingredient, b)) {
                *slot = Slot(Some(result));
            }
        }
        let remainder = super::recipes::crafting_remainder(ingredient.item);
        let left = ingredient.count - 1;
        items[3] = if left > 0 { Slot(Some(ingredient.with_count(left))) } else { Slot::EMPTY };
        if let Some(rest) = remainder {
            if items[3].is_empty() {
                items[3] = rest;
            } else {
                self.pop_resource(world, pos, rest);
            }
        }
        self.send_near(world, pos, 64.0, None, &play::LevelEvent { event: 1035, pos, data: 0, global: false });
    }

    fn set_block_entity_int(&mut self, world: usize, pos: BlockPos, key: &str, value: i32, short: bool) {
        let (chunk, k) = (crate::world::chunk::ChunkPos::of_block(pos.x, pos.z), (pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8));
        if let Some(c) = self.worlds[world].chunks.get_mut(&chunk) {
            if let Some(nbt) = c.block_entities.get_mut(&k) {
                nbt.insert(key, if short { Tag::Short(value as i16) } else { Tag::Byte(value as i8) });
                c.dirty = true;
            }
        }
    }

    /// The brew time and fuel bar of everyone looking into the stand.
    pub(super) fn send_brewing_data(&mut self, world: usize, pos: BlockPos, brew_time: i32, fuel: i32) {
        for p in self.players.values() {
            let Some(menu) = p.open_menu.as_ref() else { continue };
            if menu.kind != (MenuKind::Brewing { world, pos }) {
                continue;
            }
            p.conn.send(&play::ContainerSetData { window_id: menu.window_id, property: 0, value: brew_time as i16 });
            p.conn.send(&play::ContainerSetData { window_id: menu.window_id, property: 1, value: fuel as i16 });
            // 26.3 clients take the totals the bars are drawn against from the server.
            p.conn.send(&play::ContainerSetData { window_id: menu.window_id, property: 2, value: 400 });
            p.conn.send(&play::ContainerSetData { window_id: menu.window_id, property: 3, value: 20 });
        }
    }

    fn refresh_brewing_viewers(&mut self, world: usize, pos: BlockPos) {
        let viewers: Vec<i32> = self.players.values().filter(|p| p.open_menu.as_ref().is_some_and(|m| m.kind == (MenuKind::Brewing { world, pos }))).map(|p| p.entity_id).collect();
        for v in viewers {
            self.send_menu_contents(v);
        }
    }

    /// Sends the stand's bars to a player who just opened it.
    pub(super) fn send_brewing_state(&mut self, world: usize, pos: BlockPos) {
        let nbt = self.block_entity_data(world, pos).cloned().unwrap_or_default();
        self.send_brewing_data(world, pos, nbt.i32("BrewTime").unwrap_or(0), nbt.i32("Fuel").unwrap_or(0));
    }
}
