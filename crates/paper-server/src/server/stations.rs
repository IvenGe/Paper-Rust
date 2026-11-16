//! Stonecutters, smithing tables, grindstones and looms (vanilla's `StonecutterMenu`,
//! `SmithingMenu`, `GrindstoneMenu` and `LoomMenu`).
//!
//! Their slots are kept in the open window's grid: the inputs, then the result. The result is
//! worked out on the server whenever an input changes, or the player picks a stonecutter
//! recipe or a loom pattern (a window button: `ContainerButtonClick`, shown back as window
//! property 0). Taking the result uses up the inputs ([`Server::take_station_result`]).

use super::containers::MenuKind;
use super::enchanting::{read_enchantments, set_component};
use super::entities::{max_stack, same_item};
use super::menu::{ClickResult, Layout};
use super::survival::category;
use super::Server;
use crate::world::entity::Vec3;
use paper_data::{item_data, items};
use paper_protocol::codec::{BlockPos, Encode, Reader, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

/// Which workstation a window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Station {
    Stonecutter,
    Smithing,
    Grindstone,
    Loom,
}

impl Station {
    /// The workstation a window is, with where it stands.
    pub fn of(kind: &MenuKind) -> Option<(Station, usize, BlockPos)> {
        match *kind {
            MenuKind::Stonecutter { world, pos } => Some((Station::Stonecutter, world, pos)),
            MenuKind::Smithing { world, pos } => Some((Station::Smithing, world, pos)),
            MenuKind::Grindstone { world, pos } => Some((Station::Grindstone, world, pos)),
            MenuKind::Loom { world, pos } => Some((Station::Loom, world, pos)),
            _ => None,
        }
    }

    /// The result slot (the inputs come before it).
    pub fn result_slot(self) -> usize {
        match self {
            Station::Stonecutter => 1,
            Station::Grindstone => 2,
            Station::Smithing | Station::Loom => 3,
        }
    }

    pub fn layout(self) -> Layout {
        match self {
            Station::Stonecutter => Layout::stonecutter(),
            Station::Smithing => Layout::smithing(),
            Station::Grindstone => Layout::grindstone(),
            Station::Loom => Layout::loom(),
        }
    }

    /// Bukkit's `InventoryType` of the window.
    pub fn inventory_type(self) -> &'static str {
        match self {
            Station::Stonecutter => "STONECUTTER",
            Station::Smithing => "SMITHING",
            Station::Grindstone => "GRINDSTONE",
            Station::Loom => "LOOM",
        }
    }
}

fn component(name: &str) -> Option<i32> {
    paper_data::registries::id("minecraft:data_component_type", name).map(|i| i as i32)
}

fn component_data<'a>(stack: &'a ItemStack, name: &str) -> Option<&'a [u8]> {
    let ty = component(name)?;
    stack.added.iter().find(|(t, _)| *t == ty).map(|(_, d)| d.as_slice())
}

fn remove_component(stack: &mut ItemStack, name: &str) {
    if let Some(ty) = component(name) {
        stack.added.retain(|(t, _)| *t != ty);
    }
}

fn item_name(item: i32) -> &'static str {
    items::get(item as u32).map_or("", |i| i.name)
}

fn damageable(stack: &ItemStack) -> bool {
    item_data::by_id(stack.item as u32).max_damage.is_some_and(|m| m > 0) && component_data(stack, "minecraft:unbreakable").is_none()
}

fn damage(stack: &ItemStack) -> i32 {
    component_data(stack, "minecraft:damage").and_then(|d| Reader::new(d).varint().ok()).unwrap_or(0)
}

/// The component an item keeps its enchantments in.
fn enchantment_component(stack: &ItemStack) -> &'static str {
    if item_name(stack.item) == "minecraft:enchanted_book" {
        "minecraft:stored_enchantments"
    } else {
        "minecraft:enchantments"
    }
}

fn enchantments(stack: &ItemStack) -> Vec<(i32, i32)> {
    read_enchantments(stack, enchantment_component(stack))
}

fn write_enchantments(stack: &mut ItemStack, list: &[(i32, i32)]) {
    let name = enchantment_component(stack);
    let mut data = Vec::new();
    VarInt(list.len() as i32).encode(&mut data);
    for (e, l) in list {
        VarInt(*e).encode(&mut data);
        VarInt(*l).encode(&mut data);
    }
    set_component(stack, name, data);
}

fn is_curse(enchantment: i32) -> bool {
    paper_data::synced::tag("minecraft:enchantment", "minecraft:curse").is_some_and(|ids| ids.contains(&enchantment))
}

/// What a grindstone takes: damageable or enchanted items.
pub fn grindable(stack: &ItemStack) -> bool {
    damageable(stack) || !enchantments(stack).is_empty()
}

/// Banners (the items, `*_banner`).
pub fn is_banner(item: i32) -> bool {
    item_name(item).ends_with("_banner") && !item_name(item).ends_with("_wall_banner")
}

/// The colour of a dye (`DyeItem`), as vanilla's `DyeColor` id.
pub fn dye_color(item: i32) -> Option<i32> {
    const COLORS: [&str; 16] = ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black"];
    let color = item_name(item).strip_prefix("minecraft:")?.strip_suffix("_dye")?;
    COLORS.iter().position(|c| *c == color).map(|i| i as i32)
}

/// A banner's pattern layers (`banner_patterns`: pattern id in `minecraft:banner_pattern`,
/// dye colour). `None` if they cannot be read (patterns written out in full).
fn banner_layers(stack: &ItemStack) -> Option<Vec<(i32, i32)>> {
    let Some(data) = component_data(stack, "minecraft:banner_patterns") else { return Some(Vec::new()) };
    let mut r = Reader::new(data);
    let n = r.varint().ok()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let holder = r.varint().ok()?;
        if holder == 0 {
            return None;
        }
        out.push((holder - 1, r.varint().ok()?));
    }
    Some(out)
}

fn write_banner_layers(stack: &mut ItemStack, layers: &[(i32, i32)]) {
    let mut data = Vec::new();
    VarInt(layers.len() as i32).encode(&mut data);
    for (pattern, color) in layers {
        VarInt(pattern + 1).encode(&mut data);
        VarInt(*color).encode(&mut data);
    }
    set_component(stack, "minecraft:banner_patterns", data);
}

/// The most layers a loom adds patterns up to (`LoomMenu`: six).
const MAX_BANNER_LAYERS: usize = 6;

/// The patterns a loom offers with a pattern item (`LoomMenu.getSelectablePatterns`): those
/// that need no item without one, else those of the item's tag, in tag order (as the client
/// lists them, from the tags it was sent).
pub fn loom_patterns(pattern_item: &Slot) -> Vec<i32> {
    let tag = match &pattern_item.0 {
        None => Some("minecraft:no_item_required"),
        Some(stack) => item_data::banner_patterns(stack.item as u32),
    };
    tag.and_then(|t| paper_data::synced::tag("minecraft:banner_pattern", t)).map(<[i32]>::to_vec).unwrap_or_default()
}

/// `ItemStack.transmuteCopy`: another item with this one's components.
fn transmute(stack: &ItemStack, item: i32, count: i32) -> ItemStack {
    let mut out = stack.clone();
    out.item = item;
    out.count = count;
    out
}

/// The experience a grindstone gives back for an item's enchantments (curses aside): the
/// enchantments' minimum enchanting costs (`GrindstoneMenu.getExperienceFromItem`).
fn grind_experience(server: &Server, slot: &Slot) -> i32 {
    let Some(stack) = slot.0.as_ref() else { return 0 };
    let Some(defs) = server.loot.enchantments() else { return 0 };
    let mut total = 0;
    for name in ["minecraft:enchantments", "minecraft:stored_enchantments"] {
        for (id, level) in read_enchantments(stack, name) {
            if !is_curse(id) {
                if let Some(def) = defs.by_id(id) {
                    total += def.min_cost.0 + def.min_cost.1 * (level - 1);
                }
            }
        }
    }
    total
}

/// `AnvilMenu.calculateIncreasedRepairCost`.
fn increased_repair_cost(cost: i32) -> i32 {
    ((cost as i64 * 2 + 1).min(i32::MAX as i64)) as i32
}

/// `GrindstoneMenu.removeNonCursesFrom`: keeps only curses, a book without any becomes a
/// plain book, and the repair cost starts over (once more per curse kept).
fn remove_non_curses(mut stack: ItemStack) -> ItemStack {
    let kept: Vec<(i32, i32)> = enchantments(&stack).into_iter().filter(|(e, _)| is_curse(*e)).collect();
    if kept.is_empty() {
        let name = enchantment_component(&stack);
        remove_component(&mut stack, name);
    } else {
        write_enchantments(&mut stack, &kept);
    }
    if item_name(stack.item) == "minecraft:enchanted_book" && kept.is_empty() {
        if let Some(book) = items::id("minecraft:book") {
            stack = transmute(&stack, book as i32, stack.count);
            remove_component(&mut stack, "minecraft:stored_enchantments");
        }
    }
    let mut cost = 0;
    for _ in &kept {
        cost = increased_repair_cost(cost);
    }
    let mut data = Vec::new();
    VarInt(cost).encode(&mut data);
    set_component(&mut stack, "minecraft:repair_cost", data);
    stack
}

/// `GrindstoneMenu.computeResult`.
fn grind(top: &Slot, bottom: &Slot) -> Slot {
    match (&top.0, &bottom.0) {
        (None, None) => Slot::EMPTY,
        (Some(a), Some(b)) if a.count > 1 || b.count > 1 => Slot::EMPTY,
        (Some(only), None) | (None, Some(only)) => {
            if only.count > 1 || enchantments(only).is_empty() {
                Slot::EMPTY
            } else {
                Slot(Some(remove_non_curses(only.clone())))
            }
        }
        (Some(a), Some(b)) => {
            // `mergeItems`: two of the same item; damageable ones are repaired with 5% extra.
            if a.item != b.item {
                return Slot::EMPTY;
            }
            let max = item_data::by_id(a.item as u32).max_damage.unwrap_or(0);
            let mut count = 1;
            if !damageable(a) {
                if max_stack(a) < 2 || !super::recipes::same_item_same_components(a, b) {
                    return Slot::EMPTY;
                }
                count = 2;
            }
            let mut out = a.with_count(count);
            if damageable(&out) {
                let durability = (max - damage(a)) + (max - damage(b)) + max * 5 / 100;
                let mut data = Vec::new();
                VarInt((max - durability).max(0)).encode(&mut data);
                set_component(&mut out, "minecraft:damage", data);
            }
            // `mergeEnchantsFrom`: the curses of the other one too.
            let mut list = enchantments(&out);
            for (e, l) in enchantments(b) {
                if is_curse(e) {
                    match list.iter_mut().find(|(have, _)| *have == e) {
                        Some((_, level)) => *level = (*level).max(l),
                        None => list.push((e, l)),
                    }
                }
            }
            if !list.is_empty() {
                write_enchantments(&mut out, &list);
            }
            Slot(Some(remove_non_curses(out)))
        }
    }
}

impl Server {
    /// The open window's workstation, grid and choice (stonecutter recipe index or loom
    /// pattern id).
    fn open_station(&self, id: i32) -> Option<(Station, usize, BlockPos, Vec<Slot>, Option<i32>)> {
        let menu = self.players.get(&id)?.open_menu.as_ref()?;
        let (station, world, pos) = Station::of(&menu.kind)?;
        Some((station, world, pos, menu.craft_grid.clone(), menu.station_choice))
    }

    /// What a workstation's inputs make (and the choice made, for a stonecutter or loom).
    fn station_result(&self, station: Station, grid: &[Slot], choice: Option<i32>) -> Slot {
        match station {
            Station::Stonecutter => {
                let Some(recipes) = self.recipes.as_ref() else { return Slot::EMPTY };
                let found = recipes.stonecutting_for(&grid[0]);
                match choice.and_then(|i| usize::try_from(i).ok()).and_then(|i| found.get(i)) {
                    Some(r) => Slot(Some(r.result.clone())),
                    None => Slot::EMPTY,
                }
            }
            Station::Smithing => {
                let Some(recipes) = self.recipes.as_ref() else { return Slot::EMPTY };
                let Some(recipe) = recipes.smithing(&grid[0], &grid[1], &grid[2]) else { return Slot::EMPTY };
                let Some(base) = grid[1].0.as_ref() else { return Slot::EMPTY };
                match recipe {
                    super::recipes::SmithingRecipe::Transform { result, .. } => Slot(Some(transmute(base, result.item, result.count))),
                    super::recipes::SmithingRecipe::Trim { pattern, .. } => {
                        // `SmithingTrimRecipe.applyTrim`: the pattern in the addition's material.
                        let material = grid[2].0.as_ref().and_then(|a| item_data::trim_material(a.item as u32)).and_then(|m| paper_data::synced::id("minecraft:trim_material", m));
                        let pattern = paper_data::synced::id("minecraft:trim_pattern", pattern);
                        let (Some(material), Some(pattern)) = (material, pattern) else { return Slot::EMPTY };
                        let mut data = Vec::new();
                        VarInt(material + 1).encode(&mut data);
                        VarInt(pattern + 1).encode(&mut data);
                        // The same trim again makes nothing.
                        if component_data(base, "minecraft:trim") == Some(data.as_slice()) {
                            return Slot::EMPTY;
                        }
                        let mut out = base.with_count(1);
                        set_component(&mut out, "minecraft:trim", data);
                        Slot(Some(out))
                    }
                }
            }
            Station::Grindstone => grind(&grid[0], &grid[1]),
            Station::Loom => {
                let (Some(banner), Some(dye)) = (grid[0].0.as_ref(), grid[1].0.as_ref()) else { return Slot::EMPTY };
                let (Some(pattern), Some(color)) = (choice, dye_color(dye.item)) else { return Slot::EMPTY };
                let Some(mut layers) = banner_layers(banner) else { return Slot::EMPTY };
                if layers.len() >= MAX_BANNER_LAYERS {
                    return Slot::EMPTY;
                }
                layers.push((pattern, color));
                let mut out = banner.with_count(1);
                write_banner_layers(&mut out, &layers);
                Slot(Some(out))
            }
        }
    }

    /// Works out a workstation's result again after its inputs changed (`slotsChanged`),
    /// letting plugins change it (`PrepareResultEvent` and its kinds), and tells the client.
    /// `input_replaced`: a different item went into the stonecutter, whose choice starts over.
    pub(super) fn update_station(&mut self, id: i32, input_replaced: bool) {
        let Some((station, _, _, grid, mut choice)) = self.open_station(id) else { return };
        match station {
            Station::Stonecutter if input_replaced || grid[0].is_empty() => choice = None,
            // `LoomMenu.slotsChanged`: with a banner and a dye, the one pattern on offer is
            // picked, else the one picked stays if still on offer.
            Station::Loom => {
                if grid[0].is_empty() || grid[1].is_empty() {
                    choice = None;
                } else {
                    let offered = loom_patterns(&grid[2]);
                    if offered.len() == 1 {
                        choice = Some(offered[0]);
                    } else if choice.is_some_and(|c| !offered.contains(&c)) {
                        choice = None;
                    }
                    let full = grid[0].0.as_ref().and_then(banner_layers).is_some_and(|l| l.len() >= MAX_BANNER_LAYERS);
                    if full {
                        choice = None;
                    }
                }
            }
            _ => {}
        }
        let mut result = self.station_result(station, &grid, choice);
        result = crate::plugins::events::on_prepare_result(self, id, station.inventory_type(), result);
        let result_slot = station.result_slot();
        let Some(menu) = self.players.get_mut(&id).and_then(|p| p.open_menu.as_mut()) else { return };
        menu.station_choice = choice;
        menu.craft_grid[result_slot] = result.clone();
        let (window_id, state_id) = (menu.window_id, menu.state_id);
        let player = &self.players[&id];
        player.conn.send(&play::ContainerSetSlot { window_id, state_id, slot: result_slot as i16, item: result });
        self.send_station_choice(id);
    }

    /// Window property 0: the stonecutter recipe or loom pattern picked, as the client
    /// numbers them (-1: none).
    pub(super) fn send_station_choice(&self, id: i32) {
        let Some((station, _, _, grid, choice)) = self.open_station(id) else { return };
        let index = match station {
            Station::Stonecutter => choice.unwrap_or(-1),
            Station::Loom => choice.and_then(|c| loom_patterns(&grid[2]).iter().position(|p| *p == c)).map_or(-1, |i| i as i32),
            _ => return,
        };
        self.window_property(id, 0, index);
    }

    /// A stonecutter recipe or loom pattern was picked (`clickMenuButton`). Returns whether
    /// the open window is one of those.
    pub(super) fn handle_station_button(&mut self, id: i32, window_id: i32, button: i32) -> bool {
        let Some((station, _, _, grid, _)) = self.open_station(id) else { return false };
        if self.players.get(&id).and_then(|p| p.open_menu.as_ref()).is_none_or(|m| m.window_id != window_id) {
            return true;
        }
        let choice = match station {
            Station::Stonecutter => {
                let count = self.recipes.as_ref().map_or(0, |r| r.stonecutting_for(&grid[0]).len());
                if button < 0 || button as usize >= count {
                    return true;
                }
                let Some(recipe) = self.recipes.as_ref().and_then(|r| r.stonecutting_for(&grid[0]).get(button as usize).map(|r| (*r).clone())) else { return true };
                // Plugins may refuse it or pick another (Paper's `PlayerStonecutterRecipeSelectEvent`).
                let info = crate::plugins::events::StationChoice { result: Slot(Some(recipe.result.clone())), inputs: recipe.ingredient.0.clone() };
                match crate::plugins::events::on_station_select(self, id, "STONECUTTER", &recipe.id, Some(info)) {
                    Some(picked) => self.recipes.as_ref().and_then(|r| r.stonecutting_for(&grid[0]).iter().position(|r| r.id == picked)).map(|i| i as i32),
                    None => None,
                }
            }
            Station::Loom => {
                let offered = loom_patterns(&grid[2]);
                let Some(pattern) = usize::try_from(button).ok().and_then(|i| offered.get(i)).copied() else { return true };
                let name = paper_data::synced::entries("minecraft:banner_pattern").and_then(|e| e.get(pattern as usize)).copied().unwrap_or("");
                // `PlayerLoomPatternSelectEvent`.
                match crate::plugins::events::on_station_select(self, id, "LOOM", name, None) {
                    Some(picked) => paper_data::synced::id("minecraft:banner_pattern", &picked).filter(|p| offered.contains(p)),
                    None => None,
                }
            }
            _ => return false,
        };
        if choice.is_none() {
            self.send_station_choice(id);
            return true;
        }
        if let Some(menu) = self.players.get_mut(&id).and_then(|p| p.open_menu.as_mut()) {
            menu.station_choice = choice;
        }
        self.update_station(id, false);
        true
    }

    /// The result was taken: uses up the inputs (`onTake`) and works out the next result.
    /// Returns false if there was nothing to take.
    pub(super) fn take_station_result(&mut self, id: i32, slots: &mut [Slot]) -> bool {
        let Some((station, world, pos, _, choice)) = self.open_station(id) else { return false };
        let result_slot = station.result_slot();
        let Some(taken) = slots[result_slot].0.clone() else { return false };
        let shrink = |slot: &mut Slot| {
            if let Some(s) = slot.0.as_mut() {
                s.count -= 1;
                if s.count <= 0 {
                    *slot = Slot::EMPTY;
                }
            }
        };
        let at = Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5);
        match station {
            Station::Stonecutter => {
                shrink(&mut slots[0]);
                self.play_sound(world, at, "minecraft:ui.stonecutter.take_result", category::BLOCKS, 1.0, 1.0, None);
            }
            Station::Smithing => {
                for slot in slots.iter_mut().take(3) {
                    shrink(slot);
                }
                // `LevelEvent.SOUND_SMITHING_TABLE_USED`.
                self.send_near(world, pos, 64.0, None, &play::LevelEvent { event: 1044, pos, data: 0, global: false });
            }
            Station::Grindstone => {
                let xp = grind_experience(self, &slots[0]) + grind_experience(self, &slots[1]);
                if xp > 0 {
                    let half = (xp + 1) / 2;
                    let amount = half + rand::thread_rng().gen_range(0..half.max(1));
                    self.award_experience(world, at, amount);
                }
                slots[0] = Slot::EMPTY;
                slots[1] = Slot::EMPTY;
                self.send_near(world, pos, 64.0, None, &play::LevelEvent { event: 1042, pos, data: 0, global: false });
            }
            Station::Loom => {
                shrink(&mut slots[0]);
                shrink(&mut slots[1]);
                self.play_sound(world, at, "minecraft:ui.loom.take_result", category::BLOCKS, 1.0, 1.0, None);
            }
        }
        if matches!(station, Station::Stonecutter | Station::Smithing) {
            if let Some(name) = items::get(taken.item as u32).map(|i| i.name) {
                self.award_stat(id, "minecraft:crafted", name, taken.count);
            }
        }
        // The next result, from what is left.
        let next_choice = match station {
            Station::Stonecutter if slots[0].is_empty() => None,
            _ => choice,
        };
        slots[result_slot] = self.station_result(station, &slots[..result_slot + 1], next_choice);
        if let Some(menu) = self.players.get_mut(&id).and_then(|p| p.open_menu.as_mut()) {
            menu.station_choice = next_choice;
            menu.craft_grid.clone_from_slice(&slots[..result_slot + 1]);
        }
        true
    }

    /// A click on a workstation's result: taken whole, one result at a time (shift-click
    /// takes as many as there are inputs for), as `ResultSlot.onTake` does.
    pub(super) fn station_result_click(&mut self, id: i32, layout: &Layout, slots: &mut Vec<Slot>, carried: &mut Slot, mode: i32, button: i8) -> ClickResult {
        let mut result = ClickResult::default();
        let Some((station, ..)) = self.open_station(id) else { return result };
        let result_slot = station.result_slot();
        let Some(first) = slots[result_slot].0.clone() else { return result };
        let player = layout.main.start..layout.hotbar.end;
        match mode {
            0 => {
                let fits = match &carried.0 {
                    None => true,
                    Some(c) => same_item(c, &first) && c.count + first.count <= max_stack(c),
                };
                if fits && self.take_station_result(id, slots) {
                    match &mut carried.0 {
                        Some(c) => c.count += first.count,
                        None => *carried = Slot(Some(first)),
                    }
                }
            }
            1 => {
                for _ in 0..64 {
                    let Some(out) = slots[result_slot].0.clone() else { break };
                    if !same_item(&out, &first) {
                        break;
                    }
                    let mut trial = slots.clone();
                    let mut moving = Slot(Some(out));
                    let (mut no_carry, mut drag) = (Slot::EMPTY, super::menu::Drag::default());
                    super::menu::Menu { layout, slots: &mut trial, carried: &mut no_carry, recipes: None, creative: false, drag: &mut drag }.move_into(&mut moving, player.clone(), true);
                    if !moving.is_empty() || !self.take_station_result(id, slots) {
                        break;
                    }
                    slots[player.clone()].clone_from_slice(&trial[player.clone()]);
                }
            }
            2 => {
                let target = layout.hotbar.start + button as usize;
                if (0..9).contains(&button) && slots[target].is_empty() && self.take_station_result(id, slots) {
                    slots[target] = Slot(Some(first));
                }
            }
            4 => {
                if self.take_station_result(id, slots) {
                    result.dropped.push(Slot(Some(first)));
                }
            }
            _ => {}
        }
        result
    }

    /// What clients need of the recipes: the stonecutter's recipes and the item sets that
    /// decide which slots take what (`ClientboundUpdateRecipesPacket`).
    pub(super) fn recipes_packet(&self) -> play::UpdateRecipes {
        let Some(recipes) = self.recipes.as_ref() else { return play::UpdateRecipes::default() };
        let mut property_sets = Vec::new();
        for (set, kind) in [
            ("minecraft:furnace_input", "minecraft:smelting"),
            ("minecraft:blast_furnace_input", "minecraft:blasting"),
            ("minecraft:smoker_input", "minecraft:smoking"),
            ("minecraft:campfire_input", "minecraft:campfire_cooking"),
        ] {
            property_sets.push((set.to_owned(), recipes.cooking_inputs(kind)));
        }
        for (slot, set) in ["minecraft:smithing_template", "minecraft:smithing_base", "minecraft:smithing_addition"].iter().enumerate() {
            property_sets.push(((*set).to_owned(), recipes.smithing_inputs(slot)));
        }
        let stonecutter = recipes.stonecutting().map(|r| (r.ingredient.0.clone(), r.result.item, r.result.count)).collect();
        play::UpdateRecipes { property_sets, stonecutter }
    }
}
