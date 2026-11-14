//! Inventory clicks, following vanilla's `AbstractContainerMenu.doClick` and each menu's
//! `quickMoveStack`.
//!
//! The engine works on a plain list of the menu's slots, the stack on the cursor and a
//! [`Layout`] describing what each slot accepts and where shift-clicks move items, so it can
//! be tested without a server. The caller copies the slots in and writes them back.

use super::entities::{max_stack, same_item};
use super::recipes::{crafting_remainder, Recipes};
use paper_protocol::types::{ItemStack, Slot};
use std::ops::Range;

/// What a slot accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    Normal,
    /// A crafting result: items can only be taken.
    CraftResult,
    /// A furnace output: items can only be taken.
    Output,
    /// Armour: only items equippable in this slot, one at a time.
    Armor(&'static str),
    /// Furnace fuel.
    Fuel,
    /// One item at a time (the enchanting table's item slot).
    Single,
    /// Lapis lazuli only (the enchanting table).
    Lapis,
    /// A brewing stand's bottle: potions and glass bottles, one at a time.
    Bottle,
    /// A brewing stand's ingredient.
    BrewIngredient,
    /// A brewing stand's fuel (blaze powder).
    BrewFuel,
}

/// The shape of a menu.
#[derive(Clone, Debug)]
pub struct Layout {
    pub kinds: Vec<SlotKind>,
    /// The player's main inventory (27 slots) and hotbar (9 slots) inside this menu.
    pub main: Range<usize>,
    pub hotbar: Range<usize>,
    /// The off hand, in the player's own inventory menu.
    pub offhand: Option<usize>,
    /// The slots that are not the player's.
    pub container: Range<usize>,
    /// Crafting: result slot, grid slots and grid width.
    pub crafting: Option<(usize, Range<usize>, usize)>,
    /// Furnace: input and fuel slots.
    pub furnace: Option<(usize, usize, &'static str)>,
    pub kind: MenuShape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuShape {
    PlayerInventory,
    Container,
    CraftingTable,
    Furnace,
    Enchanting,
    Anvil,
    Brewing,
    /// A villager's trading window: two payment slots and the result.
    Merchant,
    /// A crafter: nine slots, the player's inventory, then the result it would make.
    Crafter,
}

impl Layout {
    /// The player's own inventory window: result, 2x2 grid, armour, main, hotbar, off hand.
    pub fn player_inventory() -> Layout {
        let mut kinds = vec![SlotKind::Normal; 46];
        kinds[0] = SlotKind::CraftResult;
        for (i, slot) in ["head", "chest", "legs", "feet"].into_iter().enumerate() {
            kinds[5 + i] = SlotKind::Armor(slot);
        }
        Layout { kinds, main: 9..36, hotbar: 36..45, offhand: Some(45), container: 0..9, crafting: Some((0, 1..5, 2)), furnace: None, kind: MenuShape::PlayerInventory }
    }

    /// A chest-like container with `size` slots followed by the player's inventory.
    pub fn container(size: usize) -> Layout {
        Layout {
            kinds: vec![SlotKind::Normal; size + 36],
            main: size..size + 27,
            hotbar: size + 27..size + 36,
            offhand: None,
            container: 0..size,
            crafting: None,
            furnace: None,
            kind: MenuShape::Container,
        }
    }

    pub fn crafting_table() -> Layout {
        let mut kinds = vec![SlotKind::Normal; 46];
        kinds[0] = SlotKind::CraftResult;
        Layout { kinds, main: 10..37, hotbar: 37..46, offhand: None, container: 0..10, crafting: Some((0, 1..10, 3)), furnace: None, kind: MenuShape::CraftingTable }
    }

    /// `recipe_type` is `minecraft:smelting`, `minecraft:smoking` or `minecraft:blasting`.
    pub fn furnace(recipe_type: &'static str) -> Layout {
        let mut kinds = vec![SlotKind::Normal; 39];
        kinds[1] = SlotKind::Fuel;
        kinds[2] = SlotKind::Output;
        Layout { kinds, main: 3..30, hotbar: 30..39, offhand: None, container: 0..3, crafting: None, furnace: Some((0, 1, recipe_type)), kind: MenuShape::Furnace }
    }

    /// The enchanting table: the item, the lapis, then the player's inventory.
    pub fn enchanting() -> Layout {
        let mut kinds = vec![SlotKind::Normal; 38];
        kinds[0] = SlotKind::Single;
        kinds[1] = SlotKind::Lapis;
        Layout { kinds, main: 2..29, hotbar: 29..38, offhand: None, container: 0..2, crafting: None, furnace: None, kind: MenuShape::Enchanting }
    }

    /// The brewing stand: three bottles, the ingredient, the fuel.
    pub fn brewing() -> Layout {
        let mut kinds = vec![SlotKind::Normal; 41];
        kinds[..3].fill(SlotKind::Bottle);
        kinds[3] = SlotKind::BrewIngredient;
        kinds[4] = SlotKind::BrewFuel;
        Layout { kinds, main: 5..32, hotbar: 32..41, offhand: None, container: 0..5, crafting: None, furnace: None, kind: MenuShape::Brewing }
    }

    /// The anvil: two inputs and the result (taken only through `server::workstations`).
    pub fn anvil() -> Layout {
        let mut kinds = vec![SlotKind::Normal; 39];
        kinds[2] = SlotKind::Output;
        Layout { kinds, main: 3..30, hotbar: 30..39, offhand: None, container: 0..3, crafting: None, furnace: None, kind: MenuShape::Anvil }
    }

    /// `CrafterMenu`: the nine slots, the player's inventory, and last the result the crafter
    /// would make (only shown: the window code keeps clicks off it).
    pub fn crafter() -> Layout {
        let mut kinds = vec![SlotKind::Normal; 46];
        kinds[45] = SlotKind::CraftResult;
        Layout { kinds, main: 9..36, hotbar: 36..45, offhand: None, container: 0..9, crafting: None, furnace: None, kind: MenuShape::Crafter }
    }

    /// `MerchantMenu`: payment A, payment B, result.
    pub fn merchant() -> Layout {
        let mut kinds = vec![SlotKind::Normal; 39];
        kinds[2] = SlotKind::Output;
        Layout { kinds, main: 3..30, hotbar: 30..39, offhand: None, container: 0..3, crafting: None, furnace: None, kind: MenuShape::Merchant }
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}

/// A click from the client.
#[derive(Clone, Copy, Debug)]
pub struct Click {
    /// Clicked slot; -999 outside the window, -1 for a drag outside slots.
    pub slot: i16,
    pub button: i8,
    pub mode: i32,
}

/// What a click produced besides moving items around.
#[derive(Debug, Default)]
pub struct ClickResult {
    /// Stacks thrown out of the window.
    pub dropped: Vec<Slot>,
    /// Items crafted (result stacks taken).
    pub crafted: Vec<ItemStack>,
    /// Smelted items taken from a furnace output (for experience).
    pub smelted: i32,
}

/// Ongoing drag (quick craft) of one player.
#[derive(Clone, Debug, Default)]
pub struct Drag {
    pub active: bool,
    pub kind: i8,
    pub slots: Vec<usize>,
}

/// The engine.
pub struct Menu<'a> {
    pub layout: &'a Layout,
    pub slots: &'a mut Vec<Slot>,
    pub carried: &'a mut Slot,
    pub recipes: Option<&'a Recipes>,
    pub creative: bool,
    pub drag: &'a mut Drag,
}

fn take(slot: &mut Slot, count: i32) -> Slot {
    let Some(stack) = slot.0.as_mut() else { return Slot::EMPTY };
    let n = count.min(stack.count);
    let out = stack.with_count(n);
    stack.count -= n;
    if stack.count <= 0 {
        *slot = Slot::EMPTY;
    }
    Slot(Some(out))
}

impl Menu<'_> {
    fn may_place(&self, index: usize, item: &Slot) -> bool {
        let Some(stack) = item.0.as_ref() else { return true };
        match self.layout.kinds[index] {
            SlotKind::CraftResult | SlotKind::Output => false,
            SlotKind::Armor(slot) => paper_data::item_data::by_id(stack.item as u32).equip_slot.as_deref() == Some(slot),
            SlotKind::Fuel => self.recipes.is_none_or(|r| r.burn_time(item, false) > 0) || paper_data::items::get(stack.item as u32).is_some_and(|i| i.name == "minecraft:bucket"),
            SlotKind::Normal | SlotKind::Single => true,
            SlotKind::Lapis => paper_data::items::get(stack.item as u32).is_some_and(|i| i.name == "minecraft:lapis_lazuli"),
            SlotKind::Bottle => super::brewing::fits_bottle_slot(stack),
            SlotKind::BrewIngredient => super::brewing::is_ingredient(stack),
            SlotKind::BrewFuel => super::brewing::is_fuel(stack),
        }
    }

    fn slot_limit(&self, index: usize, stack: &ItemStack) -> i32 {
        match self.layout.kinds[index] {
            SlotKind::Armor(_) | SlotKind::Single | SlotKind::Bottle => 1,
            _ => max_stack(stack),
        }
    }

    fn is_result(&self, index: usize) -> bool {
        matches!(self.layout.kinds[index], SlotKind::CraftResult)
    }

    /// Refreshes the crafting result from the grid.
    pub fn update_result(&mut self) {
        let Some((result, grid, size)) = self.layout.crafting.clone() else { return };
        let out = self.recipes.and_then(|r| r.craft(&self.slots[grid], size)).map(|(_, s)| Slot(Some(s))).unwrap_or(Slot::EMPTY);
        self.slots[result] = out;
    }

    /// Uses up one of each grid item after the result was taken.
    fn consume_grid(&mut self, dropped: &mut Vec<Slot>) {
        let Some((_, grid, _)) = self.layout.crafting.clone() else { return };
        for i in grid {
            let Some(stack) = self.slots[i].0.clone() else { continue };
            let remainder = crafting_remainder(stack.item);
            take(&mut self.slots[i], 1);
            if let Some(rem) = remainder {
                if self.slots[i].is_empty() {
                    self.slots[i] = rem;
                } else if !self.add_to_player(&rem) {
                    dropped.push(rem);
                }
            }
        }
        self.update_result();
    }

    /// Adds a stack to the player's slots of the menu; false if it did not fit.
    fn add_to_player(&mut self, item: &Slot) -> bool {
        let mut item = item.clone();
        let (main, hotbar) = (self.layout.main.clone(), self.layout.hotbar.clone());
        self.move_into(&mut item, main.start..hotbar.end, true);
        item.is_empty()
    }

    /// vanilla's `moveItemStackTo`: merge into matching stacks, then fill empty slots.
    pub(crate) fn move_into(&mut self, item: &mut Slot, range: Range<usize>, reverse: bool) -> bool {
        let mut moved = false;
        let order: Vec<usize> = if reverse { range.clone().rev().collect() } else { range.clone().collect() };
        if let Some(stack) = item.0.clone() {
            if max_stack(&stack) > 1 {
                for &i in &order {
                    let Some(stack) = item.0.as_mut() else { break };
                    if let Some(existing) = self.slots[i].0.as_mut() {
                        if same_item(existing, stack) {
                            let limit = match self.layout.kinds[i] {
                                SlotKind::Armor(_) => 1,
                                _ => max_stack(existing),
                            };
                            let add = (limit - existing.count).min(stack.count);
                            if add > 0 && !matches!(self.layout.kinds[i], SlotKind::CraftResult | SlotKind::Output) {
                                existing.count += add;
                                stack.count -= add;
                                moved = true;
                            }
                        }
                    }
                }
                if item.0.as_ref().is_some_and(|s| s.count <= 0) {
                    *item = Slot::EMPTY;
                }
            }
        }
        for &i in &order {
            let Some(stack) = item.0.clone() else { break };
            if self.slots[i].is_empty() && self.may_place(i, item) {
                let n = stack.count.min(self.slot_limit(i, &stack));
                self.slots[i] = Slot(Some(stack.with_count(n)));
                take(item, n);
                moved = true;
                if !item.is_empty() {
                    continue;
                }
                break;
            }
        }
        moved
    }

    /// Shift-click on `index`: vanilla's per-menu `quickMoveStack`.
    fn quick_move(&mut self, index: usize, result: &mut ClickResult) {
        let l = self.layout.clone();
        let player = l.main.start..l.hotbar.end;
        if self.is_result(index) {
            // Craft as many as possible.
            let Some(first) = self.slots[index].0.clone() else { return };
            for _ in 0..64 {
                let Some(current) = self.slots[index].0.clone() else { break };
                if !same_item(&current, &first) {
                    break;
                }
                let mut moving = Slot(Some(current.clone()));
                let target = if l.kind == MenuShape::PlayerInventory { 9..45 } else { player.clone() };
                self.move_into(&mut moving, target, true);
                if !moving.is_empty() {
                    // Did not fit entirely: put back what did not move and stop.
                    let moved = current.count - moving.0.as_ref().map_or(0, |s| s.count);
                    if moved > 0 {
                        // Partial moves cannot happen for a result: undo.
                    }
                    break;
                }
                result.crafted.push(current);
                self.consume_grid(&mut result.dropped);
            }
            return;
        }
        let Some(stack) = self.slots[index].0.clone() else { return };
        let mut item = Slot(Some(stack.clone()));
        match l.kind {
            MenuShape::PlayerInventory => {
                let equip = paper_data::item_data::by_id(stack.item as u32).equip_slot.clone();
                let armor_target = equip.as_deref().and_then(|s| ["head", "chest", "legs", "feet"].iter().position(|a| *a == s)).map(|i| 5 + i);
                if (1..9).contains(&index) {
                    self.move_into(&mut item, 9..45, false);
                } else if let Some(armor) = armor_target.filter(|a| self.slots[*a].is_empty() && !(5..9).contains(&index)) {
                    self.move_into(&mut item, armor..armor + 1, false);
                } else if equip.as_deref() == Some("offhand") && self.slots[45].is_empty() && index != 45 {
                    self.move_into(&mut item, 45..46, false);
                } else if l.main.contains(&index) {
                    self.move_into(&mut item, 36..45, false);
                } else if l.hotbar.contains(&index) {
                    self.move_into(&mut item, 9..36, false);
                } else {
                    self.move_into(&mut item, 9..45, false);
                }
            }
            MenuShape::Crafter => {
                if l.container.contains(&index) {
                    self.move_into(&mut item, 9..45, true);
                } else {
                    self.move_into(&mut item, 0..9, false);
                }
            }
            MenuShape::Container => {
                if l.container.contains(&index) {
                    self.move_into(&mut item, player.clone(), true);
                } else {
                    self.move_into(&mut item, l.container.clone(), false);
                }
            }
            MenuShape::CraftingTable => {
                if player.contains(&index) {
                    if !self.move_into(&mut item, 1..10, false) {
                        if l.main.contains(&index) {
                            self.move_into(&mut item, l.hotbar.clone(), false);
                        } else {
                            self.move_into(&mut item, l.main.clone(), false);
                        }
                    }
                } else {
                    self.move_into(&mut item, player.clone(), false);
                }
            }
            MenuShape::Enchanting => {
                let lapis = paper_data::items::get(stack.item as u32).is_some_and(|i| i.name == "minecraft:lapis_lazuli");
                if l.container.contains(&index) {
                    self.move_into(&mut item, player.clone(), true);
                } else if lapis {
                    self.move_into(&mut item, 1..2, true);
                } else if self.slots[0].is_empty() {
                    // One item goes into the table.
                    let mut one = Slot(Some(stack.with_count(1)));
                    self.move_into(&mut one, 0..1, false);
                    if one.is_empty() {
                        item = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
                    }
                } else {
                    return;
                }
            }
            MenuShape::Brewing => {
                if l.container.contains(&index) {
                    self.move_into(&mut item, player.clone(), true);
                } else if super::brewing::is_fuel(&stack) {
                    // Blaze powder fills the fuel slot first, else brews (strength).
                    if !self.move_into(&mut item, 4..5, false) && super::brewing::is_ingredient(&stack) {
                        self.move_into(&mut item, 3..4, false);
                    }
                } else if super::brewing::is_ingredient(&stack) {
                    self.move_into(&mut item, 3..4, false);
                } else if super::brewing::fits_bottle_slot(&stack) {
                    self.move_into(&mut item, 0..3, false);
                } else if l.main.contains(&index) {
                    self.move_into(&mut item, l.hotbar.clone(), false);
                } else {
                    self.move_into(&mut item, l.main.clone(), false);
                }
            }
            MenuShape::Anvil => {
                if l.container.contains(&index) {
                    self.move_into(&mut item, player.clone(), index == 2);
                } else {
                    let first = if self.slots[0].is_empty() { 0 } else { 1 };
                    if !self.move_into(&mut item, first..2, false) {
                        if l.main.contains(&index) {
                            self.move_into(&mut item, l.hotbar.clone(), false);
                        } else {
                            self.move_into(&mut item, l.main.clone(), false);
                        }
                    }
                }
            }
            // `MerchantMenu.quickMoveStack`: payment goes back to the inventory, and the
            // inventory and hotbar trade places (the result slot is handled by the trading
            // code, which takes one trade at a time).
            MenuShape::Merchant => {
                if l.container.contains(&index) {
                    self.move_into(&mut item, player.clone(), index == 2);
                } else if l.main.contains(&index) {
                    self.move_into(&mut item, l.hotbar.clone(), false);
                } else {
                    self.move_into(&mut item, l.main.clone(), false);
                }
            }
            MenuShape::Furnace => {
                let (input, fuel, kind) = l.furnace.unwrap();
                if index == 2 {
                    let before = stack.count;
                    self.move_into(&mut item, player.clone(), true);
                    result.smelted += before - item.0.as_ref().map_or(0, |s| s.count);
                } else if index == input || index == fuel {
                    self.move_into(&mut item, player.clone(), false);
                } else if self.recipes.is_some_and(|r| r.can_cook(kind, &item)) {
                    if !self.move_into(&mut item, input..input + 1, false) && self.recipes.is_some_and(|r| r.burn_time(&item, false) > 0) {
                        self.move_into(&mut item, fuel..fuel + 1, false);
                    }
                } else if self.recipes.is_some_and(|r| r.burn_time(&item, false) > 0) {
                    self.move_into(&mut item, fuel..fuel + 1, false);
                } else if l.main.contains(&index) {
                    self.move_into(&mut item, l.hotbar.clone(), false);
                } else {
                    self.move_into(&mut item, l.main.clone(), false);
                }
            }
        }
        self.slots[index] = item;
        if l.crafting.as_ref().is_some_and(|(_, grid, _)| grid.contains(&index)) {
            self.update_result();
        }
    }

    /// Takes the crafting result onto the cursor (a pickup click on the result slot).
    fn take_result(&mut self, index: usize, result: &mut ClickResult) {
        let Some(out) = self.slots[index].0.clone() else { return };
        match &mut self.carried.0 {
            None => *self.carried = Slot(Some(out.clone())),
            Some(c) if same_item(c, &out) && c.count + out.count <= max_stack(c) => c.count += out.count,
            _ => return,
        }
        if self.layout.kinds[index] == SlotKind::CraftResult {
            result.crafted.push(out);
            self.consume_grid(&mut result.dropped);
        } else {
            self.slots[index] = Slot::EMPTY;
        }
    }

    pub fn click(&mut self, click: Click) -> ClickResult {
        let mut result = ClickResult::default();
        let len = self.layout.len() as i16;
        let index = (click.slot >= 0 && click.slot < len).then_some(click.slot as usize);
        // A drag in progress is cancelled by any other click.
        if click.mode != 5 && self.drag.active {
            *self.drag = Drag::default();
        }
        match click.mode {
            // Pickup.
            0 => match index {
                None if click.slot == -999 => {
                    if !self.carried.is_empty() {
                        let n = if click.button == 0 { i32::MAX } else { 1 };
                        result.dropped.push(take(self.carried, n));
                    }
                }
                None => {}
                Some(i) if matches!(self.layout.kinds[i], SlotKind::CraftResult | SlotKind::Output) => {
                    let before = self.slots[i].0.as_ref().map_or(0, |s| s.count);
                    self.take_result(i, &mut result);
                    if self.layout.kinds[i] == SlotKind::Output {
                        result.smelted += before - self.slots[i].0.as_ref().map_or(0, |s| s.count);
                    }
                }
                Some(i) => {
                    let right = click.button == 1;
                    match (self.slots[i].0.clone(), self.carried.0.clone()) {
                        (Some(s), None) => {
                            let n = if right { (s.count + 1) / 2 } else { s.count };
                            *self.carried = take(&mut self.slots[i], n);
                        }
                        (None, Some(c)) => {
                            if self.may_place(i, self.carried) {
                                let n = if right { 1 } else { c.count.min(self.slot_limit(i, &c)) };
                                self.slots[i] = take(self.carried, n);
                            }
                        }
                        (Some(s), Some(c)) => {
                            if same_item(&s, &c) {
                                let space = self.slot_limit(i, &s) - s.count;
                                let n = if right { 1.min(space) } else { space.min(c.count) };
                                if n > 0 && self.may_place(i, self.carried) {
                                    take(self.carried, n);
                                    if let Some(stack) = self.slots[i].0.as_mut() {
                                        stack.count += n;
                                    }
                                }
                            } else if self.may_place(i, self.carried) && c.count <= self.slot_limit(i, &c) {
                                std::mem::swap(&mut self.slots[i], self.carried);
                            }
                        }
                        (None, None) => {}
                    }
                    if self.layout.crafting.as_ref().is_some_and(|(_, grid, _)| grid.contains(&i)) {
                        self.update_result();
                    }
                }
            },
            // Shift-click.
            1 => {
                if let Some(i) = index {
                    self.quick_move(i, &mut result);
                }
            }
            // Number keys (button 0-8) or F (40, off hand).
            2 => {
                let Some(i) = index else { return result };
                let target = if click.button == 40 {
                    match self.layout.offhand {
                        Some(o) => o,
                        None => return result,
                    }
                } else if (0..9).contains(&click.button) {
                    self.layout.hotbar.start + click.button as usize
                } else {
                    return result;
                };
                if i == target {
                    return result;
                }
                if matches!(self.layout.kinds[i], SlotKind::CraftResult | SlotKind::Output) {
                    // Taking a result into a hotbar slot works only if it is empty.
                    if self.slots[target].is_empty() && !self.slots[i].is_empty() {
                        let out = self.slots[i].clone();
                        if self.layout.kinds[i] == SlotKind::CraftResult {
                            result.crafted.push(out.0.clone().unwrap());
                            self.slots[target] = out;
                            self.consume_grid(&mut result.dropped);
                        } else {
                            result.smelted += out.0.as_ref().map_or(0, |s| s.count);
                            self.slots[target] = std::mem::take(&mut self.slots[i]);
                        }
                    }
                    return result;
                }
                let (a, b) = (self.slots[i].clone(), self.slots[target].clone());
                if self.may_place(i, &b) && self.may_place(target, &a) {
                    self.slots[i] = b;
                    self.slots[target] = a;
                }
                if self.layout.crafting.as_ref().is_some_and(|(_, grid, _)| grid.contains(&i) || grid.contains(&target)) {
                    self.update_result();
                }
            }
            // Middle click: creative copy.
            3 => {
                if let (Some(i), true) = (index, self.creative) {
                    if self.carried.is_empty() {
                        if let Some(s) = &self.slots[i].0 {
                            *self.carried = Slot(Some(s.with_count(max_stack(s))));
                        }
                    }
                }
            }
            // Q / Ctrl+Q over a slot.
            4 => {
                if let Some(i) = index {
                    if self.carried.is_empty() && !self.slots[i].is_empty() {
                        if self.is_result(i) {
                            let out = self.slots[i].clone();
                            result.crafted.push(out.0.clone().unwrap());
                            result.dropped.push(out);
                            self.consume_grid(&mut result.dropped);
                        } else {
                            let n = if click.button == 1 { i32::MAX } else { 1 };
                            if self.layout.kinds[i] == SlotKind::Output {
                                result.smelted += self.slots[i].0.as_ref().map_or(0, |s| s.count.min(n));
                            }
                            result.dropped.push(take(&mut self.slots[i], n));
                            if self.layout.crafting.as_ref().is_some_and(|(_, grid, _)| grid.contains(&i)) {
                                self.update_result();
                            }
                        }
                    }
                }
            }
            // Drag: stage 0 start, 1 add a slot, 2 end; type 0 split evenly, 1 one each, 2 full stacks (creative).
            5 => {
                let stage = click.button & 3;
                let kind = (click.button >> 2) & 3;
                match stage {
                    0 => {
                        *self.drag = Drag { active: !self.carried.is_empty() && (kind != 2 || self.creative), kind, slots: Vec::new() };
                    }
                    1 => {
                        if let (true, Some(i)) = (self.drag.active, index) {
                            let ok = !self.is_result(i)
                                && self.may_place(i, self.carried)
                                && match (&self.slots[i].0, &self.carried.0) {
                                    (None, _) => true,
                                    (Some(s), Some(c)) => same_item(s, c) && s.count < self.slot_limit(i, s),
                                    _ => false,
                                };
                            if ok && !self.drag.slots.contains(&i) {
                                self.drag.slots.push(i);
                            }
                        }
                    }
                    _ => {
                        let drag = std::mem::take(self.drag);
                        if drag.active && !drag.slots.is_empty() {
                            let Some(carried) = self.carried.0.clone() else { return result };
                            if drag.slots.len() == 1 && drag.kind != 2 {
                                // A drag over one slot is an ordinary click.
                                let button = if drag.kind == 0 { 0 } else { 1 };
                                return self.click(Click { slot: drag.slots[0] as i16, button, mode: 0 });
                            }
                            let mut left = carried.count;
                            let per = match drag.kind {
                                0 => carried.count / drag.slots.len() as i32,
                                1 => 1,
                                _ => max_stack(&carried),
                            };
                            for &i in &drag.slots {
                                if left <= 0 && drag.kind != 2 {
                                    break;
                                }
                                let existing = self.slots[i].0.as_ref().map_or(0, |s| s.count);
                                let limit = self.slot_limit(i, &carried);
                                let add = per.min(limit - existing).max(0);
                                let add = if drag.kind == 2 { add } else { add.min(left) };
                                if add > 0 {
                                    self.slots[i] = Slot(Some(carried.with_count(existing + add)));
                                    if drag.kind != 2 {
                                        left -= add;
                                    }
                                }
                            }
                            *self.carried = if drag.kind == 2 || left > 0 { Slot(Some(carried.with_count(if drag.kind == 2 { carried.count } else { left }))) } else { Slot::EMPTY };
                            if self.layout.crafting.is_some() {
                                self.update_result();
                            }
                        }
                    }
                }
            }
            // Double click: gather matching items onto the cursor.
            6 => {
                let Some(mut c) = self.carried.0.clone() else { return result };
                let max = max_stack(&c);
                let order: Vec<usize> = if click.button == 0 { (0..self.layout.len()).collect() } else { (0..self.layout.len()).rev().collect() };
                for pass in 0..2 {
                    for &i in &order {
                        if c.count >= max {
                            break;
                        }
                        if matches!(self.layout.kinds[i], SlotKind::CraftResult) {
                            continue;
                        }
                        let Some(s) = self.slots[i].0.clone() else { continue };
                        if !same_item(&s, &c) || (pass == 0 && s.count == max_stack(&s)) {
                            continue;
                        }
                        let n = (max - c.count).min(s.count);
                        take(&mut self.slots[i], n);
                        c.count += n;
                    }
                }
                *self.carried = Slot(Some(c));
                if self.layout.crafting.is_some() {
                    self.update_result();
                }
            }
            _ => {}
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, count: i32) -> Slot {
        Slot::of(paper_data::items::id(name).unwrap() as i32, count)
    }

    struct Fixture {
        layout: Layout,
        slots: Vec<Slot>,
        carried: Slot,
        drag: Drag,
    }

    impl Fixture {
        fn new(layout: Layout) -> Self {
            let n = layout.len();
            Self { layout, slots: vec![Slot::EMPTY; n], carried: Slot::EMPTY, drag: Drag::default() }
        }

        fn click(&mut self, slot: i16, button: i8, mode: i32) -> ClickResult {
            let mut menu = Menu { layout: &self.layout, slots: &mut self.slots, carried: &mut self.carried, recipes: None, creative: false, drag: &mut self.drag };
            menu.click(Click { slot, button, mode })
        }
    }

    #[test]
    fn pickup_split_place_and_swap() {
        let mut f = Fixture::new(Layout::container(27));
        f.slots[0] = item("minecraft:stone", 10);
        f.click(0, 1, 0); // right click: take half (rounded up)
        assert_eq!((f.carried.clone(), f.slots[0].clone()), (item("minecraft:stone", 5), item("minecraft:stone", 5)));
        f.click(1, 1, 0); // right click on empty: place one
        assert_eq!(f.slots[1], item("minecraft:stone", 1));
        f.click(0, 0, 0); // left click on same item: merge
        assert_eq!((f.carried.clone(), f.slots[0].clone()), (Slot::EMPTY, item("minecraft:stone", 9)));
        f.slots[2] = item("minecraft:dirt", 3);
        f.click(2, 0, 0);
        f.click(0, 0, 0); // different item: swap
        assert_eq!((f.carried.clone(), f.slots[0].clone()), (item("minecraft:stone", 9), item("minecraft:dirt", 3)));
        let r = f.click(-999, 1, 0); // right click outside: drop one
        assert_eq!(r.dropped, vec![item("minecraft:stone", 1)]);
    }

    #[test]
    fn shift_click_moves_between_container_and_player() {
        let mut f = Fixture::new(Layout::container(27));
        f.slots[5] = item("minecraft:stone", 64);
        f.slots[27 + 27 + 8] = item("minecraft:stone", 10); // last hotbar slot
        f.click(5, 0, 1);
        // Merges into the existing stack first (from the end), then fills an empty slot.
        assert_eq!(f.slots[62], item("minecraft:stone", 64));
        assert!(f.slots[5].is_empty());
        let filled: i32 = f.slots[27..].iter().filter_map(|s| s.0.as_ref().map(|s| s.count)).sum();
        assert_eq!(filled, 74);
        f.click(62, 0, 1);
        assert_eq!(f.slots[0], item("minecraft:stone", 64), "back into the container");
    }

    #[test]
    fn drag_splits_evenly() {
        let mut f = Fixture::new(Layout::container(27));
        f.carried = item("minecraft:stone", 10);
        f.click(-999, 0, 5);
        for s in [0, 1, 2] {
            f.click(s, 1, 5);
        }
        f.click(-999, 2, 5);
        assert_eq!(f.slots[0], item("minecraft:stone", 3));
        assert_eq!(f.slots[2], item("minecraft:stone", 3));
        assert_eq!(f.carried, item("minecraft:stone", 1));
    }

    #[test]
    fn number_keys_and_double_click() {
        let mut f = Fixture::new(Layout::container(27));
        f.slots[3] = item("minecraft:dirt", 7);
        f.click(3, 4, 2); // swap with hotbar slot 4
        assert_eq!(f.slots[27 + 27 + 4], item("minecraft:dirt", 7));
        f.slots[0] = item("minecraft:dirt", 20);
        f.slots[1] = item("minecraft:dirt", 64);
        f.carried = item("minecraft:dirt", 1);
        f.click(0, 0, 6);
        assert_eq!(f.carried, item("minecraft:dirt", 64), "partial stacks first, then full ones");
    }

    #[test]
    fn armour_slots_accept_only_matching_armour() {
        let mut f = Fixture::new(Layout::player_inventory());
        f.carried = item("minecraft:iron_helmet", 1);
        f.click(6, 0, 0); // chest slot: refused
        assert!(f.slots[6].is_empty());
        f.click(5, 0, 0); // head slot
        assert_eq!(f.slots[5], item("minecraft:iron_helmet", 1));
        f.slots[20] = item("minecraft:iron_boots", 1);
        f.click(20, 0, 1); // shift-click equips
        assert_eq!(f.slots[8], item("minecraft:iron_boots", 1));
    }
}
