//! Enchanting tables (vanilla's `EnchantmentMenu`) and anvils (`AnvilMenu`).
//!
//! Both windows keep their slots in the open menu's grid: the enchanting table its item and
//! lapis, the anvil its two inputs and the result. Offers, results and costs are worked out
//! on the server after every change and sent as window properties.

use super::containers::MenuKind;
use super::enchanting::{read_enchantments, set_component};
use super::Server;
use crate::config::GameMode;
use paper_data::{blocks, item_data, items};
use paper_protocol::codec::{BlockPos, Encode, Reader, VarInt};
use paper_protocol::packets::play;
use paper_protocol::text::Component;
use paper_protocol::types::{ItemStack, Slot};
use rand::{Rng, SeedableRng};
use std::sync::OnceLock;

/// What an open anvil has worked out.
#[derive(Clone, Debug, Default)]
pub struct AnvilState {
    /// The name typed in (`None` until the client sends one).
    pub name: Option<String>,
    /// Experience levels the result costs.
    pub cost: i32,
    /// Repair materials the result uses up from the right input (0: all of it).
    pub repair_count: i32,
}

fn component(name: &str) -> Option<i32> {
    paper_data::registries::id("minecraft:data_component_type", name).map(|i| i as i32)
}

fn varint_component(stack: &ItemStack, name: &str) -> Option<i32> {
    let ty = component(name)?;
    let (_, data) = stack.added.iter().find(|(t, _)| *t == ty)?;
    Reader::new(data).varint().ok()
}

fn set_varint_component(stack: &mut ItemStack, name: &str, value: i32) {
    let mut data = Vec::new();
    VarInt(value).encode(&mut data);
    set_component(stack, name, data);
}

fn remove_component(stack: &mut ItemStack, name: &str) {
    if let Some(ty) = component(name) {
        stack.added.retain(|(t, _)| *t != ty);
    }
}

fn is(stack: &ItemStack, name: &str) -> bool {
    items::get(stack.item as u32).is_some_and(|i| i.name == name)
}

fn enchantability(stack: &ItemStack) -> i32 {
    item_data::by_id(stack.item as u32).raw.as_ref().and_then(|r| r["enchantable"]["value"].as_i64()).unwrap_or(0) as i32
}

/// The component an item keeps its enchantments in.
fn enchantment_component(stack: &ItemStack) -> &'static str {
    if is(stack, "minecraft:enchanted_book") {
        "minecraft:stored_enchantments"
    } else {
        "minecraft:enchantments"
    }
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

/// The `custom_name` component of a stack.
pub fn custom_name_of(stack: &ItemStack) -> Option<Component> {
    custom_name(stack)
}

fn custom_name(stack: &ItemStack) -> Option<Component> {
    let ty = component("minecraft:custom_name")?;
    let (_, data) = stack.added.iter().find(|(t, _)| *t == ty)?;
    Reader::new(data).component().ok()
}

fn damageable(stack: &ItemStack) -> bool {
    item_data::by_id(stack.item as u32).max_damage.is_some_and(|m| m > 0) && component("minecraft:unbreakable").is_none_or(|u| !stack.added.iter().any(|(t, _)| *t == u))
}

fn max_damage(stack: &ItemStack) -> i32 {
    item_data::by_id(stack.item as u32).max_damage.unwrap_or(0)
}

fn damage(stack: &ItemStack) -> i32 {
    varint_component(stack, "minecraft:damage").unwrap_or(0)
}

/// Whether `material` repairs `stack` (its `repairable` component).
fn repairs(stack: &ItemStack, material: &ItemStack) -> bool {
    let Some(raw) = item_data::by_id(stack.item as u32).raw.as_ref() else { return false };
    let items_v = &raw["repairable"]["items"];
    let matches = |v: &serde_json::Value| match v.as_str() {
        Some(t) if t.starts_with('#') => paper_data::synced::tag("minecraft:item", &t[1..]).is_some_and(|ids| ids.contains(&material.item)),
        Some(name) => items::id(name).is_some_and(|i| i as i32 == material.item),
        None => false,
    };
    match items_v {
        serde_json::Value::Array(list) => list.iter().any(matches),
        v => matches(v),
    }
}

/// English names of items (vanilla's server compares anvil names against them).
fn english(key: &str, data: Option<&crate::vanilla::VanillaData>) -> Option<String> {
    static LANG: OnceLock<Option<serde_json::Value>> = OnceLock::new();
    let lang = LANG.get_or_init(|| data.and_then(|d| d.read_json("assets/minecraft/lang/en_us.json")));
    lang.as_ref()?.get(key)?.as_str().map(str::to_owned)
}

impl Server {
    /// The name an item shows (`getHoverName().getString()`, in English).
    fn hover_name(&self, stack: &ItemStack) -> String {
        if let Some(name) = custom_name(stack) {
            return name.to_plain();
        }
        let item = items::get(stack.item as u32).map_or("", |i| i.name);
        let path = item.strip_prefix("minecraft:").unwrap_or(item);
        let vanilla = self.vanilla.as_deref();
        english(&format!("item.minecraft.{path}"), vanilla).or_else(|| english(&format!("block.minecraft.{path}"), vanilla)).unwrap_or_else(|| path.to_owned())
    }

    fn take_levels(&mut self, id: i32, levels: i32) {
        if let Some(p) = self.players.get_mut(&id) {
            p.xp_level -= levels;
            if p.xp_level < 0 {
                p.xp_level = 0;
                p.xp_progress = 0.0;
                p.xp_total = 0;
            }
        }
        self.send_experience(id);
    }

    fn window_property(&self, id: i32, property: i16, value: i32) {
        if let Some(p) = self.players.get(&id) {
            if let Some(m) = &p.open_menu {
                p.conn.send(&play::ContainerSetData { window_id: m.window_id, property, value: value.clamp(i16::MIN as i32, i16::MAX as i32) as i16 });
            }
        }
    }

    // --- enchanting tables -----------------------------------------------------------------

    /// Bookshelves feeding an enchanting table (`EnchantingTableBlock.isValidBookShelf`).
    pub fn bookshelves(&self, world: usize, pos: BlockPos) -> i32 {
        let w = &self.worlds[world];
        let has = |at: BlockPos, tag: &str| w.block_at(at.x, at.y, at.z).is_some_and(|s| paper_data::synced::tag("minecraft:block", tag).is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(s) as i32))));
        let mut n = 0;
        for dy in 0..=1 {
            for dz in -2i32..=2 {
                for dx in -2i32..=2 {
                    if dx.abs() != 2 && dz.abs() != 2 {
                        continue;
                    }
                    if has(pos.offset(dx, dy, dz), "minecraft:enchantment_power_provider") && has(pos.offset(dx / 2, dy, dz / 2), "minecraft:enchantment_power_transmitter") {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    /// The enchantments offer `slot` would give for `cost` levels (`getEnchantmentList`).
    fn enchanting_list(&self, stack: &ItemStack, seed: i32, slot: i32, cost: i32) -> Vec<(i32, i32)> {
        let Some(enchantments) = self.loot.enchantments() else { return Vec::new() };
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed.wrapping_add(slot) as u32 as u64);
        let mut list = enchantments.select(stack, cost, &serde_json::json!("#minecraft:in_enchanting_table"), &mut rng);
        if is(stack, "minecraft:book") && list.len() > 1 {
            list.remove(rng.gen_range(0..list.len()));
        }
        list
    }

    /// The three offers for an item: (cost, enchantment shown, its level).
    fn enchanting_offers(&self, world: usize, pos: BlockPos, stack: &ItemStack, seed: i32) -> [(i32, i32, i32); 3] {
        let mut offers = [(0, -1, -1); 3];
        let enchanted = !read_enchantments(stack, "minecraft:enchantments").is_empty();
        if enchantability(stack) <= 0 || enchanted {
            return offers;
        }
        let power = self.bookshelves(world, pos).min(15);
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed as u32 as u64);
        for (j, offer) in offers.iter_mut().enumerate() {
            // `EnchantmentHelper.getEnchantmentCost`.
            let i = rng.gen_range(0..8) + 1 + (power >> 1) + rng.gen_range(0..=power);
            let cost = match j {
                0 => (i / 3).max(1),
                1 => i * 2 / 3 + 1,
                _ => i.max(power * 2),
            };
            offer.0 = if cost < j as i32 + 1 { 0 } else { cost };
        }
        for (j, offer) in offers.iter_mut().enumerate() {
            if offer.0 > 0 {
                let list = self.enchanting_list(stack, seed, j as i32, offer.0);
                if !list.is_empty() {
                    let (e, l) = list[rng.gen_range(0..list.len())];
                    offer.1 = e;
                    offer.2 = l;
                }
            }
        }
        offers
    }

    /// Sends the offers of an open enchanting table (`slotsChanged`).
    pub(super) fn update_enchanting_table(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else { return };
        let Some(MenuKind::Enchanting { world, pos }) = p.open_menu.as_ref().map(|m| m.kind.clone()) else { return };
        let item = p.open_menu.as_ref().and_then(|m| m.craft_grid.first().cloned()).unwrap_or(Slot::EMPTY);
        let seed = p.enchantment_seed;
        let offers = match item.0.as_ref() {
            Some(stack) => self.enchanting_offers(world, pos, stack, seed),
            None => [(0, -1, -1); 3],
        };
        for (j, (cost, clue, level)) in offers.iter().enumerate() {
            self.window_property(id, j as i16, *cost);
            self.window_property(id, 4 + j as i16, *clue);
            self.window_property(id, 7 + j as i16, *level);
        }
        self.window_property(id, 3, seed & -16);
    }

    /// A click on one of the three offers (`EnchantmentMenu.clickMenuButton`).
    pub(super) fn handle_enchant_button(&mut self, id: i32, window_id: i32, button: i32) {
        let Some(p) = self.players.get(&id) else { return };
        let Some(menu) = p.open_menu.as_ref().filter(|m| m.window_id == window_id) else { return };
        let MenuKind::Enchanting { world, pos } = menu.kind.clone() else { return };
        if !(0..3).contains(&button) {
            return;
        }
        let creative = p.game_mode == GameMode::Creative;
        let (item, lapis) = (menu.craft_grid[0].clone(), menu.craft_grid[1].clone());
        let Some(stack) = item.0.clone() else { return };
        let levels = button + 1;
        if lapis.0.as_ref().map_or(0, |l| l.count) < levels && !creative {
            return;
        }
        let (seed, xp) = (p.enchantment_seed, p.xp_level);
        let cost = self.enchanting_offers(world, pos, &stack, seed)[button as usize].0;
        if cost <= 0 || (xp < levels || xp < cost) && !creative {
            return;
        }
        let list = self.enchanting_list(&stack, seed, button, cost);
        if list.is_empty() {
            return;
        }
        // `onEnchantmentPerformed`: the levels go and the seed changes.
        self.take_levels(id, levels);
        let mut stack = stack;
        for (e, l) in &list {
            super::enchanting::enchant(&mut stack, *e, *l);
        }
        let Some(p) = self.players.get_mut(&id) else { return };
        p.enchantment_seed = rand::thread_rng().gen();
        if let Some(menu) = p.open_menu.as_mut() {
            menu.craft_grid[0] = Slot(Some(stack));
            if !creative {
                if let Some(l) = menu.craft_grid[1].0.as_mut() {
                    l.count -= levels;
                }
                if menu.craft_grid[1].0.as_ref().is_some_and(|l| l.count <= 0) {
                    menu.craft_grid[1] = Slot::EMPTY;
                }
            }
        }
        self.send_menu_contents(id);
        self.update_enchanting_table(id);
        self.award_stat(id, super::stats::CUSTOM, "minecraft:enchant_item", 1);
        let pitch = rand::thread_rng().gen::<f32>() * 0.1 + 0.9;
        self.play_sound(world, crate::world::entity::Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5), "minecraft:block.enchantment_table.use", super::survival::category::BLOCKS, 1.0, pitch, None);
    }

    // --- anvils ------------------------------------------------------------------------------

    /// Works out the anvil's result and cost (`AnvilMenu.createResult`).
    fn anvil_result(&self, left: &Slot, right: &Slot, name: Option<&str>, creative: bool) -> (Slot, i32, i32) {
        let Some(input) = left.0.as_ref() else { return (Slot::EMPTY, 0, 0) };
        let mut result = input.clone();
        let mut enchantments = read_enchantments(input, enchantment_component(input));
        let repair_cost = |s: &ItemStack| varint_component(s, "minecraft:repair_cost").unwrap_or(0) as i64;
        let mut base = repair_cost(input) + right.0.as_ref().map_or(0, repair_cost);
        let mut cost = 0i32;
        let mut repair_count = 0;
        let defs = self.loot.enchantments();
        if let Some(other) = right.0.as_ref() {
            let book = !read_enchantments(other, "minecraft:stored_enchantments").is_empty();
            if damageable(&result) && repairs(input, other) {
                let mut k = damage(&result).min(max_damage(&result) / 4);
                if k <= 0 {
                    return (Slot::EMPTY, 0, 0);
                }
                let mut m = 0;
                while k > 0 && m < other.count {
                    let d = damage(&result) - k;
                    set_varint_component(&mut result, "minecraft:damage", d);
                    cost += 1;
                    k = damage(&result).min(max_damage(&result) / 4);
                    m += 1;
                }
                repair_count = m;
            } else {
                if !book && (result.item != other.item || !damageable(&result)) {
                    return (Slot::EMPTY, 0, 0);
                }
                if damageable(&result) && !book {
                    let left_durability = max_damage(input) - damage(input);
                    let right_durability = max_damage(other) - damage(other);
                    let combined = left_durability + right_durability + max_damage(&result) * 12 / 100;
                    let new_damage = (max_damage(&result) - combined).max(0);
                    if new_damage < damage(&result) {
                        set_varint_component(&mut result, "minecraft:damage", new_damage);
                        cost += 2;
                    }
                }
                let (mut applied, mut rejected) = (false, false);
                for (e, level) in read_enchantments(other, enchantment_component(other)) {
                    let Some(def) = defs.as_ref().and_then(|d| d.by_id(e)) else { continue };
                    let current = enchantments.iter().find(|(x, _)| *x == e).map_or(0, |(_, l)| *l);
                    let mut level = if current == level { level + 1 } else { level.max(current) };
                    let mut fits = creative || is(input, "minecraft:enchanted_book") || def.supported.binary_search(&input.item).is_ok();
                    for (other_e, _) in &enchantments {
                        let compatible = *other_e == e || defs.as_ref().and_then(|d| d.by_id(*other_e)).is_some_and(|o| !o.exclusive.contains(&e) && !def.exclusive.contains(other_e));
                        if !compatible {
                            fits = false;
                            cost += 1;
                        }
                    }
                    if !fits {
                        rejected = true;
                        continue;
                    }
                    applied = true;
                    level = level.min(def.max_level);
                    match enchantments.iter_mut().find(|(x, _)| *x == e) {
                        Some((_, l)) => *l = level,
                        None => enchantments.push((e, level)),
                    }
                    let mut per = def.anvil_cost;
                    if book {
                        per = (per / 2).max(1);
                    }
                    cost += per * level;
                    if input.count > 1 {
                        cost = 40;
                    }
                }
                if rejected && !applied {
                    return (Slot::EMPTY, 0, 0);
                }
            }
        }
        // Renaming.
        let mut rename = 0;
        match name.filter(|n| !n.trim().is_empty()) {
            Some(n) => {
                if n != self.hover_name(input) {
                    rename = 1;
                    cost += 1;
                    let mut data = Vec::new();
                    paper_nbt::write_network(&mut data, &Component::text(n.to_owned()).to_nbt());
                    set_component(&mut result, "minecraft:custom_name", data);
                }
            }
            None => {
                if custom_name(input).is_some() {
                    rename = 1;
                    cost += 1;
                    remove_component(&mut result, "minecraft:custom_name");
                }
            }
        }
        if cost <= 0 {
            return (Slot::EMPTY, 0, 0);
        }
        base += cost as i64;
        let mut total = base.clamp(0, i32::MAX as i64) as i32;
        if rename == cost && total >= 40 {
            total = 39;
        }
        if total >= 40 && !creative {
            return (Slot::EMPTY, total, repair_count);
        }
        let mut penalty = repair_cost(&result).max(right.0.as_ref().map_or(0, repair_cost)) as i32;
        if rename != cost {
            penalty = penalty * 2 + 1;
        }
        set_varint_component(&mut result, "minecraft:repair_cost", penalty);
        write_enchantments(&mut result, &enchantments);
        (Slot(Some(result)), total, repair_count)
    }

    /// Updates an open anvil's result after its inputs or the name changed.
    pub(super) fn update_anvil(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else { return };
        let Some(menu) = p.open_menu.as_ref().filter(|m| matches!(m.kind, MenuKind::Anvil { .. })) else { return };
        let creative = p.game_mode == GameMode::Creative;
        let (result, cost, repair_count) = self.anvil_result(&menu.craft_grid[0], &menu.craft_grid[1], menu.anvil.name.as_deref(), creative);
        let Some(menu) = self.players.get_mut(&id).and_then(|p| p.open_menu.as_mut()) else { return };
        menu.craft_grid[2] = result.clone();
        menu.anvil.cost = cost;
        menu.anvil.repair_count = repair_count;
        let (window_id, state_id) = (menu.window_id, menu.state_id);
        if let Some(p) = self.players.get(&id) {
            p.conn.send(&play::ContainerSetSlot { window_id, state_id, slot: 2, item: result });
        }
        self.window_property(id, 0, cost);
    }

    /// The name typed into an anvil (`AnvilMenu.setItemName`).
    pub(super) fn handle_rename(&mut self, id: i32, name: String) {
        let Some(menu) = self.players.get_mut(&id).and_then(|p| p.open_menu.as_mut()) else { return };
        if !matches!(menu.kind, MenuKind::Anvil { .. }) || name.chars().count() > 50 {
            return;
        }
        menu.anvil.name = Some(name);
        self.update_anvil(id);
    }

    /// Whether the anvil's result may be taken now (`mayPickup`).
    pub(super) fn may_take_anvil_result(&self, id: i32) -> bool {
        let Some(p) = self.players.get(&id) else { return false };
        let Some(menu) = p.open_menu.as_ref() else { return false };
        menu.anvil.cost > 0 && (p.game_mode == GameMode::Creative || p.xp_level >= menu.anvil.cost)
    }

    /// The result was taken: pay, use up the inputs, and maybe damage the anvil (`onTake`).
    pub(super) fn took_anvil_result(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else { return };
        let creative = p.game_mode == GameMode::Creative;
        let Some(menu) = p.open_menu.as_ref() else { return };
        let MenuKind::Anvil { world, pos } = menu.kind.clone() else { return };
        let (cost, repair_count) = (menu.anvil.cost, menu.anvil.repair_count);
        if !creative {
            self.take_levels(id, cost);
        }
        if let Some(menu) = self.players.get_mut(&id).and_then(|p| p.open_menu.as_mut()) {
            menu.craft_grid[0] = Slot::EMPTY;
            if repair_count > 0 {
                if let Some(right) = menu.craft_grid[1].0.as_mut() {
                    right.count -= repair_count;
                }
                if menu.craft_grid[1].0.as_ref().is_some_and(|r| r.count <= 0) {
                    menu.craft_grid[1] = Slot::EMPTY;
                }
            } else {
                menu.craft_grid[1] = Slot::EMPTY;
            }
            menu.anvil.cost = 0;
        }
        // A 12% chance to damage the anvil (`AnvilBlock.damage`).
        let mut destroyed = false;
        if !creative && rand::thread_rng().gen::<f32>() < 0.12 {
            if let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) {
                let next = match blocks::block_of_state(state).name {
                    "minecraft:anvil" => Some("minecraft:chipped_anvil"),
                    "minecraft:chipped_anvil" => Some("minecraft:damaged_anvil"),
                    _ => None,
                };
                match next.and_then(blocks::default_state) {
                    Some(damaged) => {
                        let facing = blocks::property(state, "facing").unwrap_or("north");
                        let damaged = blocks::with_property(damaged, "facing", facing).unwrap_or(damaged);
                        self.set_block_and_update(world, pos, damaged);
                    }
                    None => {
                        self.set_block_and_update(world, pos, blocks::AIR);
                        destroyed = true;
                    }
                }
            }
        }
        let event = if destroyed { 1029 } else { 1030 };
        self.send_near(world, pos, 64.0, None, &play::LevelEvent { event, pos, data: 0, global: false });
        if destroyed {
            self.close_menu(id, true);
        } else {
            self.update_anvil(id);
            self.send_menu_contents(id);
        }
    }
}
