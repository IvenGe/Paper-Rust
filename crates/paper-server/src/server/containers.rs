//! Container blocks and windows: chests (single and double), trapped chests, barrels, ender
//! chests, crafting tables and furnaces (smoker and blast furnace too).
//!
//! Container contents live in the chunk's block entity data (`Items`, as vanilla saves
//! them), so they are saved with the chunk. While a window is open, clicks are run through
//! the click engine in [`super::menu`] on a copy of the slots, which is then written back and
//! sent to everyone looking at the same container.

use super::menu::{Click, Layout, Menu};
use super::playerdata::{item_from_nbt, item_to_nbt};
use super::Server;
use crate::world::chunk::ChunkPos;
use paper_data::blocks;
use paper_nbt::{Compound, Tag};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::text::Component;
use paper_protocol::types::{HashedSlot, Slot};

/// The window a player has open.
#[derive(Clone, Debug)]
pub struct OpenMenu {
    pub window_id: i32,
    pub kind: MenuKind,
    pub state_id: i32,
    /// Crafting table grid (result first); the enchanting table's item and lapis; the
    /// anvil's inputs and result.
    pub craft_grid: Vec<Slot>,
    /// What an open anvil has worked out.
    pub anvil: super::workstations::AnvilState,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuKind {
    /// Chest, barrel, trapped chest; two positions for a double chest (top half first).
    Container { world: usize, positions: Vec<BlockPos> },
    EnderChest,
    CraftingTable,
    Furnace { world: usize, pos: BlockPos, recipe_type: &'static str },
    Enchanting { world: usize, pos: BlockPos },
    Anvil { world: usize, pos: BlockPos },
    /// A custom inventory made by a plugin (`Bukkit.createInventory`); its slots are kept in
    /// `Server::plugin_data` (see `plugins::inventory`) and clicks go to the plugin host.
    Custom { id: u64 },
}

/// Vanilla's `LootTable.fill`: the loot goes into random empty slots, with stacks split up
/// to spread over the free space. A non-zero `seed` makes the result repeatable.
pub fn fill_with_loot(loot: &super::loot::LootTables, table: &str, seed: i64, items: &mut [Slot]) {
    use rand::seq::SliceRandom;
    use rand::{Rng, SeedableRng};
    let mut rng = if seed == 0 { rand::rngs::StdRng::from_entropy() } else { rand::rngs::StdRng::seed_from_u64(seed as u64) };
    let mut stacks: Vec<Slot> = loot.roll_with(table, &super::loot::LootContext::default(), &mut rng);
    let mut free: Vec<usize> = (0..items.len()).filter(|i| items[*i].is_empty()).collect();
    free.shuffle(&mut rng);
    // `shuffleAndSplitItems`.
    let mut splittable: Vec<Slot> = Vec::new();
    stacks.retain(|s| match &s.0 {
        None => false,
        Some(st) if st.count > 1 => {
            splittable.push(s.clone());
            false
        }
        Some(_) => true,
    });
    while free.len() as i64 - stacks.len() as i64 - splittable.len() as i64 > 0 && !splittable.is_empty() {
        let mut a = splittable.remove(rng.gen_range(0..splittable.len())).0.unwrap();
        let n = rng.gen_range(1..=(a.count / 2).max(1));
        let b = a.with_count(n);
        a.count -= n;
        for part in [a, b] {
            if part.count > 1 && rng.gen::<bool>() {
                splittable.push(Slot(Some(part)));
            } else {
                stacks.push(Slot(Some(part)));
            }
        }
    }
    stacks.extend(splittable);
    stacks.shuffle(&mut rng);
    for stack in stacks {
        let Some(slot) = free.pop() else {
            log::warn!("Loot table {table} has more items than the container has room for");
            return;
        };
        items[slot] = stack;
    }
}

/// Which container a block is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerBlock {
    Chest,
    Barrel,
    EnderChest,
    CraftingTable,
    Furnace(&'static str),
    Hopper,
    /// Dispensers and droppers (nine slots).
    Dispenser,
    EnchantingTable,
    Anvil,
}

pub fn container_block(state: u32) -> Option<ContainerBlock> {
    Some(match blocks::block_of_state(state).name {
        "minecraft:chest" | "minecraft:trapped_chest" => ContainerBlock::Chest,
        "minecraft:barrel" => ContainerBlock::Barrel,
        "minecraft:ender_chest" => ContainerBlock::EnderChest,
        "minecraft:crafting_table" => ContainerBlock::CraftingTable,
        "minecraft:furnace" => ContainerBlock::Furnace("minecraft:smelting"),
        "minecraft:smoker" => ContainerBlock::Furnace("minecraft:smoking"),
        "minecraft:blast_furnace" => ContainerBlock::Furnace("minecraft:blasting"),
        "minecraft:hopper" => ContainerBlock::Hopper,
        "minecraft:dispenser" | "minecraft:dropper" => ContainerBlock::Dispenser,
        "minecraft:enchanting_table" => ContainerBlock::EnchantingTable,
        "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil" => ContainerBlock::Anvil,
        _ => return None,
    })
}

/// Number of item slots a block entity keeps.
pub fn storage_size(state: u32) -> usize {
    match container_block(state) {
        Some(ContainerBlock::Chest | ContainerBlock::Barrel) => 27,
        Some(ContainerBlock::Furnace(_)) => 3,
        Some(ContainerBlock::Hopper) => 5,
        Some(ContainerBlock::Dispenser) => 9,
        _ => 0,
    }
}

fn menu_type(name: &str) -> i32 {
    paper_data::registries::id("minecraft:menu", name).unwrap_or(0) as i32
}

fn clockwise(facing: &str) -> Option<(&'static str, i32, i32)> {
    Some(match facing {
        "north" => ("east", 1, 0),
        "east" => ("south", 0, 1),
        "south" => ("west", -1, 0),
        "west" => ("north", 0, -1),
        _ => return None,
    })
}

fn counter_clockwise(facing: &str) -> Option<(&'static str, i32, i32)> {
    Some(match facing {
        "north" => ("west", -1, 0),
        "west" => ("south", 0, 1),
        "south" => ("east", 1, 0),
        "east" => ("north", 0, -1),
        _ => return None,
    })
}

impl Server {
    // --- block entity storage ----------------------------------------------------------

    fn block_entity_key(pos: BlockPos) -> (ChunkPos, (i32, u8, u8)) {
        (ChunkPos::of_block(pos.x, pos.z), (pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8))
    }

    /// The items stored in a container block (empty slots included).
    pub fn container_items(&self, world: usize, pos: BlockPos) -> Vec<Slot> {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return Vec::new() };
        let size = storage_size(state);
        let mut items = vec![Slot::EMPTY; size];
        let (chunk, key) = Self::block_entity_key(pos);
        if let Some(nbt) = self.worlds[world].chunks.get(&chunk).and_then(|c| c.block_entities.get(&key)) {
            for entry in nbt.list("Items").unwrap_or(&[]) {
                let Some(c) = entry.as_compound() else { continue };
                if let Some(slot) = c.i32("Slot").filter(|s| (0..size as i32).contains(s)) {
                    items[slot as usize] = item_from_nbt(c);
                }
            }
        }
        items
    }

    /// Stores a container's items in its block entity and marks the chunk for saving.
    pub fn set_container_items(&mut self, world: usize, pos: BlockPos, items: &[Slot]) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let (chunk, key) = Self::block_entity_key(pos);
        let Some(c) = self.worlds[world].chunks.get_mut(&chunk) else { return };
        let nbt = c.block_entities.entry(key).or_insert_with(|| Compound::new().with("id", Tag::String(block_entity_id(state))));
        let list: Vec<Tag> = items
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let mut e = item_to_nbt(s)?;
                e.insert("Slot", Tag::Byte(i as i8));
                Some(Tag::Compound(e))
            })
            .collect();
        nbt.insert("Items", Tag::List(list));
        c.dirty = true;
        // Comparators reading the container re-evaluate (`setChanged`).
        let block = blocks::block_id_of_state(state);
        crate::world::block_updates::Level::new(&mut self.worlds[world]).update_neighbour_for_output_signal(pos, block);
        self.publish_block_updates(world);
    }

    /// Rolls a container's loot table (generated dungeon chests, ...) into its slots, as
    /// vanilla does the first time it is opened, reached by a hopper or broken.
    pub fn unpack_loot(&mut self, world: usize, pos: BlockPos) {
        let (chunk, key) = Self::block_entity_key(pos);
        let Some(nbt) = self.worlds[world].chunks.get_mut(&chunk).and_then(|c| c.block_entities.get_mut(&key)) else { return };
        let Some(table) = nbt.string("LootTable").map(str::to_owned) else { return };
        let seed = nbt.i64("LootTableSeed").unwrap_or(0);
        nbt.remove("LootTable");
        nbt.remove("LootTableSeed");
        let mut items = self.container_items(world, pos);
        fill_with_loot(&self.loot, &table, seed, &mut items);
        self.set_container_items(world, pos, &items);
    }

    pub fn block_entity_data(&self, world: usize, pos: BlockPos) -> Option<&Compound> {
        let (chunk, key) = Self::block_entity_key(pos);
        self.worlds[world].chunks.get(&chunk)?.block_entities.get(&key)
    }

    fn set_block_entity_value(&mut self, world: usize, pos: BlockPos, field: &str, value: Tag) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let (chunk, key) = Self::block_entity_key(pos);
        let Some(c) = self.worlds[world].chunks.get_mut(&chunk) else { return };
        c.block_entities.entry(key).or_insert_with(|| Compound::new().with("id", Tag::String(block_entity_id(state)))).insert(field, value);
        c.dirty = true;
    }

    /// Handles block entities that block changes removed: containers drop their contents,
    /// viewers are thrown out, and the other half of a double chest becomes single.
    pub fn process_removed_block_entities(&mut self, world: usize) {
        while !self.worlds[world].removed_block_entities.is_empty() {
            let removed = std::mem::take(&mut self.worlds[world].removed_block_entities);
            for (pos, old, nbt) in removed {
                self.on_container_removed(world, pos, old, &nbt);
            }
        }
    }

    fn on_container_removed(&mut self, world: usize, pos: BlockPos, old: u32, nbt: &Compound) {
        let viewers: Vec<i32> = self.players.values().filter(|p| self.menu_shows(p.open_menu.as_ref(), world, pos)).map(|p| p.entity_id).collect();
        for v in viewers {
            self.close_menu(v, true);
        }
        for entry in nbt.list("Items").unwrap_or(&[]) {
            if let Some(c) = entry.as_compound() {
                let stack = item_from_nbt(c);
                if !stack.is_empty() {
                    self.pop_resource(world, pos, stack);
                }
            }
        }
        // A loot chest broken before anyone opened it drops its loot.
        if let Some(table) = nbt.string("LootTable") {
            let mut items = vec![Slot::EMPTY; storage_size(old).max(1)];
            fill_with_loot(&self.loot, table, nbt.i64("LootTableSeed").unwrap_or(0), &mut items);
            for stack in items.into_iter().filter(|s| !s.is_empty()) {
                self.pop_resource(world, pos, stack);
            }
        }
        // The other half of a double chest becomes a single chest.
        if container_block(old) == Some(ContainerBlock::Chest) && blocks::property(old, "type").is_some_and(|t| t != "single") {
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let other = pos.offset(dx, 0, dz);
                let Some(s) = self.worlds[world].block_at(other.x, other.y, other.z) else { continue };
                if blocks::block_of_state(s).name != blocks::block_of_state(old).name || blocks::property(s, "type").is_none_or(|t| t == "single") {
                    continue;
                }
                if self.double_chest_partner(world, other, s).is_none() {
                    if let Some(single) = blocks::with_property(s, "type", "single") {
                        self.set_block_and_update(world, other, single);
                    }
                }
            }
        }
    }

    /// The other half of a double chest, if it is still there.
    pub(super) fn double_chest_partner(&self, world: usize, pos: BlockPos, state: u32) -> Option<BlockPos> {
        let ty = blocks::property(state, "type")?;
        let facing = blocks::property(state, "facing")?;
        // LEFT pairs with the chest on its clockwise side, RIGHT with the counter-clockwise one.
        let (_, dx, dz) = if ty == "left" { clockwise(facing)? } else if ty == "right" { counter_clockwise(facing)? } else { return None };
        let other = pos.offset(dx, 0, dz);
        let s = self.worlds[world].block_at(other.x, other.y, other.z)?;
        let expected = if ty == "left" { "right" } else { "left" };
        (blocks::block_of_state(s).name == blocks::block_of_state(state).name && blocks::property(s, "type") == Some(expected) && blocks::property(s, "facing") == Some(facing)).then_some(other)
    }

    /// Chest placement joins a single chest beside it facing the same way into a double chest
    /// (as vanilla's `ChestBlock.getStateForPlacement`). Returns the state to place and the
    /// neighbour's new state.
    pub fn chest_placement(&self, world: usize, pos: BlockPos, state: u32) -> Option<(u32, (BlockPos, u32))> {
        if container_block(state) != Some(ContainerBlock::Chest) {
            return None;
        }
        let facing = blocks::property(state, "facing")?;
        let name = blocks::block_of_state(state).name;
        for (side, new_type, partner_type) in [(clockwise(facing), "left", "right"), (counter_clockwise(facing), "right", "left")] {
            let Some((_, dx, dz)) = side else { continue };
            let other = pos.offset(dx, 0, dz);
            let Some(s) = self.worlds[world].block_at(other.x, other.y, other.z) else { continue };
            if blocks::block_of_state(s).name == name && blocks::property(s, "type") == Some("single") && blocks::property(s, "facing") == Some(facing) {
                let partner = blocks::with_property(s, "type", partner_type)?;
                return Some((blocks::with_property(state, "type", new_type)?, (other, partner)));
            }
        }
        None
    }

    // --- opening and closing -----------------------------------------------------------

    fn menu_shows(&self, menu: Option<&OpenMenu>, world: usize, pos: BlockPos) -> bool {
        match menu.map(|m| &m.kind) {
            Some(MenuKind::Container { world: w, positions }) => *w == world && positions.contains(&pos),
            Some(MenuKind::Furnace { world: w, pos: p, .. }) => *w == world && *p == pos,
            _ => false,
        }
    }

    /// The container a block belongs to and the positions its window shows (both halves of a
    /// double chest, top first), for the plugin API.
    pub fn container_positions(&self, world: usize, pos: BlockPos) -> Option<(ContainerBlock, Vec<BlockPos>)> {
        let state = self.worlds[world].block_at(pos.x, pos.y, pos.z)?;
        let kind = container_block(state)?;
        if kind == ContainerBlock::Chest {
            if let Some(other) = self.double_chest_partner(world, pos, state) {
                let top_first = blocks::property(state, "type") == Some("right");
                return Some((kind, if top_first { vec![pos, other] } else { vec![other, pos] }));
            }
        }
        Some((kind, vec![pos]))
    }

    /// Opens the window of a container block. Returns false if it is not one.
    pub fn try_open_container(&mut self, id: i32, world: usize, pos: BlockPos) -> bool {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return false };
        let Some(kind) = container_block(state) else { return false };
        // A chest cannot be opened with a solid block on top of it.
        if matches!(kind, ContainerBlock::Chest | ContainerBlock::EnderChest) {
            if let Some(above) = self.worlds[world].block_at(pos.x, pos.y + 1, pos.z) {
                if blocks::flags(above) & blocks::FLAG_FULL_CUBE != 0 {
                    return true;
                }
            }
        }
        let custom_name = self.block_entity_data(world, pos).and_then(|n| n.get("CustomName")).and_then(|t| match t {
            Tag::String(s) => Component::from_json_str(s).ok().or_else(|| Some(Component::text(s.clone()))),
            Tag::Compound(_) => Some(Component::from_nbt(t)),
            _ => None,
        });
        let (menu_kind, menu_name, title) = match kind {
            ContainerBlock::Chest => {
                let partner = self.double_chest_partner(world, pos, state);
                match partner {
                    Some(other) => {
                        // The RIGHT half is the top of the window.
                        let top_first = blocks::property(state, "type") == Some("right");
                        let positions = if top_first { vec![pos, other] } else { vec![other, pos] };
                        (MenuKind::Container { world, positions }, "minecraft:generic_9x6", "container.chestDouble")
                    }
                    None => (MenuKind::Container { world, positions: vec![pos] }, "minecraft:generic_9x3", "container.chest"),
                }
            }
            ContainerBlock::Barrel => (MenuKind::Container { world, positions: vec![pos] }, "minecraft:generic_9x3", "container.barrel"),
            ContainerBlock::Hopper => (MenuKind::Container { world, positions: vec![pos] }, "minecraft:hopper", "container.hopper"),
            ContainerBlock::Dispenser => {
                let title = if blocks::block_of_state(state).name == "minecraft:dropper" { "container.dropper" } else { "container.dispenser" };
                (MenuKind::Container { world, positions: vec![pos] }, "minecraft:generic_3x3", title)
            }
            ContainerBlock::EnderChest => (MenuKind::EnderChest, "minecraft:generic_9x3", "container.enderchest"),
            ContainerBlock::CraftingTable => (MenuKind::CraftingTable, "minecraft:crafting", "container.crafting"),
            ContainerBlock::EnchantingTable => (MenuKind::Enchanting { world, pos }, "minecraft:enchantment", "container.enchant"),
            ContainerBlock::Anvil => (MenuKind::Anvil { world, pos }, "minecraft:anvil", "container.repair"),
            ContainerBlock::Furnace(recipe_type) => {
                let (menu, title) = match recipe_type {
                    "minecraft:smoking" => ("minecraft:smoker", "container.smoker"),
                    "minecraft:blasting" => ("minecraft:blast_furnace", "container.blast_furnace"),
                    _ => ("minecraft:furnace", "container.furnace"),
                };
                (MenuKind::Furnace { world, pos, recipe_type }, menu, title)
            }
        };
        let title = custom_name.unwrap_or_else(|| Component::translatable(title, vec![]));
        if let MenuKind::Container { positions, .. } = &menu_kind {
            for p in positions.clone() {
                self.unpack_loot(world, p);
            }
        }
        if !self.open_menu(id, menu_kind, menu_name, title) {
            return true;
        }
        if matches!(kind, ContainerBlock::EnderChest) {
            self.block_event_and_sound(world, pos, 1, 1, "minecraft:block.ender_chest.open");
        }
        true
    }

    /// Opens a window for a player: fires `InventoryOpenEvent`, closes the window they have
    /// open, and sends the new one with its contents. Used for container blocks and for
    /// windows plugins open. Returns false if a plugin cancelled the event.
    pub fn open_menu(&mut self, id: i32, menu_kind: MenuKind, menu_name: &str, title: Component) -> bool {
        if !crate::plugins::events::on_inventory_open(self, id, &menu_kind) {
            return false;
        }
        self.close_menu_with_reason(id, false, "OPEN_NEW");
        let Some(player) = self.players.get_mut(&id) else { return false };
        let window_id = player.next_window_id;
        player.next_window_id = player.next_window_id % 100 + 1;
        let craft_grid = match menu_kind {
            MenuKind::CraftingTable => vec![Slot::EMPTY; 10],
            MenuKind::Enchanting { .. } => vec![Slot::EMPTY; 2],
            MenuKind::Anvil { .. } => vec![Slot::EMPTY; 3],
            _ => Vec::new(),
        };
        player.open_menu = Some(OpenMenu { window_id, kind: menu_kind.clone(), state_id: 1, craft_grid, anvil: Default::default() });
        player.conn.send(&play::OpenScreen { window_id: VarInt(window_id), menu_type: VarInt(menu_type(menu_name)), title });
        self.send_menu_contents(id);
        // Lid animation and sound.
        if let MenuKind::Container { world, positions } = &menu_kind {
            self.container_viewers_changed(*world, positions.clone(), true);
        }
        match menu_kind {
            MenuKind::Furnace { world, pos, .. } => self.send_furnace_data(world, pos),
            MenuKind::Enchanting { .. } => self.update_enchanting_table(id),
            _ => {}
        }
        true
    }

    /// Closes the open window (the client closed it, or `notify` the server did).
    pub fn close_menu(&mut self, id: i32, notify: bool) {
        self.close_menu_with_reason(id, notify, if notify { "UNKNOWN" } else { "PLAYER" });
    }

    /// Closes the open window, firing `InventoryCloseEvent` with a Bukkit close `reason`
    /// (`OPEN_NEW` only fires it if another window was open).
    pub fn close_menu_with_reason(&mut self, id: i32, notify: bool, reason: &str) {
        let window = |s: &Self| s.players.get(&id).and_then(|p| p.open_menu.as_ref()).map(|m| m.window_id);
        let before = window(self);
        if reason != "OPEN_NEW" || before.is_some() {
            crate::plugins::events::on_inventory_close(self, id, reason);
            // A listener opened another window instead (the one closing is gone already).
            if before.is_some() && window(self) != before {
                return;
            }
        }
        let Some(player) = self.players.get_mut(&id) else { return };
        let Some(menu) = player.open_menu.take() else {
            // Closing the inventory returns the 2x2 grid and the cursor to the inventory.
            self.return_grid_and_cursor(id, Vec::new());
            return;
        };
        if notify {
            player.conn.send(&play::ContainerClose { window_id: VarInt(menu.window_id) });
        }
        // What the player put in goes back (not results).
        let inputs = match menu.kind {
            MenuKind::Enchanting { .. } | MenuKind::Anvil { .. } => &menu.craft_grid[..2],
            _ => menu.craft_grid.get(1..).unwrap_or(&[]),
        };
        let leftovers: Vec<Slot> = inputs.iter().filter(|s| !s.is_empty()).cloned().collect();
        self.return_grid_and_cursor(id, leftovers);
        match &menu.kind {
            MenuKind::Container { world, positions } => self.container_viewers_changed(*world, positions.clone(), false),
            MenuKind::EnderChest => {
                // The ender chest lid stays open while anyone looks; a close sound is enough.
                if let Some(p) = self.players.get(&id) {
                    let pos = BlockPos::new(p.loc.x.floor() as i32, p.loc.y.floor() as i32, p.loc.z.floor() as i32);
                    let world = p.world;
                    self.play_sound_at(world, pos, "minecraft:block.ender_chest.close", 0.5);
                }
            }
            // A plugin inventory nobody refers to any more can go now.
            MenuKind::Custom { .. } => crate::plugins::inventory::collect_released(self),
            _ => {}
        }
    }

    /// Puts crafting grid items and the cursor stack back into the inventory, dropping what
    /// does not fit.
    fn return_grid_and_cursor(&mut self, id: i32, mut items: Vec<Slot>) {
        let Some(player) = self.players.get_mut(&id) else { return };
        for i in 1..5 {
            let s = std::mem::take(&mut player.inventory[i]);
            if !s.is_empty() {
                items.push(s);
            }
        }
        player.inventory[0] = Slot::EMPTY;
        let carried = std::mem::take(&mut player.carried);
        if !carried.is_empty() {
            items.push(carried);
        }
        for item in items {
            let Some(stack) = item.0.clone() else { continue };
            let left = self.add_to_inventory(id, &stack);
            if left > 0 {
                self.player_drop(id, Slot(Some(stack.with_count(left))), false);
            }
        }
    }

    /// Updates the lid animation after viewers came or went, and plays the sound.
    fn container_viewers_changed(&mut self, world: usize, positions: Vec<BlockPos>, opened: bool) {
        let viewers = self.players.values().filter(|p| positions.iter().any(|pos| self.menu_shows(p.open_menu.as_ref(), world, *pos))).count();
        for pos in &positions {
            let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { continue };
            match container_block(state) {
                Some(ContainerBlock::Chest) => {
                    let block = blocks::block_id_of_state(state) as i32;
                    self.broadcast_to_chunk_viewers(world, ChunkPos::of_block(pos.x, pos.z), &play::BlockEvent { pos: *pos, action: 1, param: viewers.min(255) as u8, block: VarInt(block) });
                }
                Some(ContainerBlock::Barrel) => {
                    let open = viewers > 0;
                    if blocks::property(state, "open") != Some(if open { "true" } else { "false" }) {
                        if let Some(s) = blocks::with_property(state, "open", if open { "true" } else { "false" }) {
                            self.set_block_and_update(world, *pos, s);
                        }
                    }
                }
                _ => {}
            }
        }
        let first = positions[0];
        let state = self.worlds[world].block_at(first.x, first.y, first.z).unwrap_or(0);
        let sound = match (container_block(state), opened) {
            (Some(ContainerBlock::Barrel), true) => "minecraft:block.barrel.open",
            (Some(ContainerBlock::Barrel), false) => "minecraft:block.barrel.close",
            (_, true) => "minecraft:block.chest.open",
            (_, false) => "minecraft:block.chest.close",
        };
        let last_viewer_left = !opened && viewers == 0;
        if opened && viewers == 1 || last_viewer_left {
            self.play_sound_at(world, first, sound, 0.5);
        }
    }

    fn block_event_and_sound(&mut self, world: usize, pos: BlockPos, action: u8, param: u8, sound: &str) {
        if let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) {
            let block = blocks::block_id_of_state(state) as i32;
            self.broadcast_to_chunk_viewers(world, ChunkPos::of_block(pos.x, pos.z), &play::BlockEvent { pos, action, param, block: VarInt(block) });
        }
        self.play_sound_at(world, pos, sound, 0.5);
    }

    /// Plays a block sound for players nearby.
    pub fn play_sound_at(&mut self, world: usize, pos: BlockPos, sound: &str, volume: f32) {
        let sound_ref = match paper_data::registries::id("minecraft:sound_event", sound) {
            Some(i) => play::SoundRef::Id(i as i32),
            None => play::SoundRef::Named { name: sound.to_owned(), fixed_range: None },
        };
        let packet = play::Sound { sound: sound_ref, category: 4, x: pos.x as f64 + 0.5, y: pos.y as f64 + 0.5, z: pos.z as f64 + 0.5, volume, pitch: 1.0, seed: rand::random() };
        let payload = bytes::Bytes::from(paper_protocol::packets::encode_packet(&packet));
        self.broadcast_near(world, crate::world::entity::Vec3::new(pos.x as f64, pos.y as f64, pos.z as f64), 16.0 * volume.max(1.0) as f64, payload);
    }

    // --- slots of a window ---------------------------------------------------------------

    fn menu_layout(kind: Option<&MenuKind>, size: usize) -> Layout {
        match kind {
            None => Layout::player_inventory(),
            Some(MenuKind::Container { .. } | MenuKind::EnderChest | MenuKind::Custom { .. }) => Layout::container(size),
            Some(MenuKind::CraftingTable) => Layout::crafting_table(),
            Some(MenuKind::Enchanting { .. }) => Layout::enchanting(),
            Some(MenuKind::Anvil { .. }) => Layout::anvil(),
            Some(MenuKind::Furnace { recipe_type, .. }) => Layout::furnace(recipe_type),
        }
    }

    /// The slots of the player's open window (or inventory), as the client numbers them.
    fn menu_slots(&self, id: i32) -> Option<(Option<MenuKind>, Vec<Slot>)> {
        let player = self.players.get(&id)?;
        let Some(menu) = &player.open_menu else { return Some((None, player.inventory.clone())) };
        let mut slots = match &menu.kind {
            MenuKind::Container { world, positions } => positions.iter().flat_map(|p| self.container_items(*world, *p)).collect(),
            MenuKind::EnderChest => player.ender_items.clone(),
            MenuKind::CraftingTable | MenuKind::Enchanting { .. } | MenuKind::Anvil { .. } => menu.craft_grid.clone(),
            MenuKind::Furnace { world, pos, .. } => self.container_items(*world, *pos),
            MenuKind::Custom { id } => self.plugin_data.inventories.get(id).map(|i| i.items.clone()).unwrap_or_default(),
        };
        slots.extend(player.inventory[9..45].iter().cloned());
        Some((Some(menu.kind.clone()), slots))
    }

    /// Writes a window's slots back where they belong.
    fn store_menu_slots(&mut self, id: i32, kind: Option<&MenuKind>, slots: &[Slot]) {
        let Some(player) = self.players.get_mut(&id) else { return };
        let Some(kind) = kind else {
            player.inventory.clone_from_slice(slots);
            player.equipment_dirty = true;
            return;
        };
        let container_len = slots.len() - 36;
        player.inventory[9..45].clone_from_slice(&slots[container_len..]);
        player.equipment_dirty = true;
        match kind {
            MenuKind::Container { world, positions } => {
                // Each block holds an equal share (a double chest is two 27-slot halves).
                let per = container_len / positions.len().max(1);
                for (i, pos) in positions.iter().enumerate() {
                    self.set_container_items(*world, *pos, &slots[i * per..(i + 1) * per]);
                }
            }
            MenuKind::EnderChest => player.ender_items.clone_from_slice(&slots[..27]),
            MenuKind::CraftingTable | MenuKind::Enchanting { .. } | MenuKind::Anvil { .. } => {
                if let Some(menu) = player.open_menu.as_mut() {
                    let n = menu.craft_grid.len();
                    menu.craft_grid.clone_from_slice(&slots[..n]);
                }
            }
            MenuKind::Furnace { world, pos, .. } => self.set_container_items(*world, *pos, &slots[..3]),
            MenuKind::Custom { id } => {
                if let Some(custom) = self.plugin_data.inventories.get_mut(id) {
                    custom.items.clone_from_slice(&slots[..container_len]);
                }
            }
        }
    }

    /// Sends the whole content of the open window (or the inventory).
    pub fn send_menu_contents(&mut self, id: i32) {
        let Some((kind, slots)) = self.menu_slots(id) else { return };
        let Some(player) = self.players.get_mut(&id) else { return };
        let (window_id, state_id) = match player.open_menu.as_mut() {
            Some(m) => {
                m.state_id = (m.state_id + 1) & 0x7FFF;
                (m.window_id, m.state_id)
            }
            None => {
                player.inventory_state = (player.inventory_state + 1) & 0x7FFF;
                (0, player.inventory_state)
            }
        };
        let _ = kind;
        player.conn.send(&play::ContainerSetContent { window_id, state_id, slots, carried: player.carried.clone() });
    }

    /// Handles a click in a window.
    pub fn handle_container_click(&mut self, id: i32, click: play::ContainerClick) {
        let Some(player) = self.players.get(&id) else { return };
        let open_window = player.open_menu.as_ref().map_or(0, |m| m.window_id);
        if click.window_id != open_window {
            return;
        }
        if player.game_mode == crate::config::GameMode::Spectator {
            self.send_menu_contents(id);
            return;
        }
        let creative = player.game_mode == crate::config::GameMode::Creative;
        let Some((kind, mut slots)) = self.menu_slots(id) else { return };
        let layout = Self::menu_layout(kind.as_ref(), slots.len() - 36);
        if click.slot >= 0 && click.slot as usize >= layout.len() {
            return;
        }
        // The anvil's result is only for those who can pay for it.
        let anvil = matches!(kind, Some(MenuKind::Anvil { .. }));
        if anvil && click.slot == 2 && !slots[2].is_empty() && !self.may_take_anvil_result(id) {
            self.send_menu_contents(id);
            return;
        }
        // Clicks on a crafting result are CraftItemEvents; the result is only taken if no
        // plugin cancels it. (The result is worked out as the click will see it.)
        if layout.crafting.is_some() {
            let recipes = self.recipes.clone();
            let (mut carried, mut drag) = (Slot::EMPTY, super::menu::Drag::default());
            let before = slots[0].clone();
            Menu { layout: &layout, slots: &mut slots, carried: &mut carried, recipes: recipes.as_deref(), creative, drag: &mut drag }.update_result();
            if slots[0] != before {
                // Listeners read the result slot (CraftItemEvent#getCurrentItem).
                self.store_menu_slots(id, kind.as_ref(), &slots);
            }
        }
        let crafting = layout.crafting.as_ref().is_some_and(|(result, _, _)| click.slot >= 0 && click.slot as usize == *result) && !slots[0].is_empty() && click.mode != 5;
        if !crate::plugins::events::on_inventory_click(self, id, &click, kind.as_ref(), crafting.then(|| slots[0].clone())) {
            self.send_menu_contents(id);
            return;
        }
        // Listeners may have changed the window (or closed it, or opened another one).
        let open_window = self.players.get(&id).and_then(|p| p.open_menu.as_ref()).map_or(0, |m| m.window_id);
        if open_window != click.window_id {
            return;
        }
        let Some((kind, mut slots)) = self.menu_slots(id) else { return };
        if slots.len() != layout.len() {
            self.send_menu_contents(id);
            return;
        }
        let before = slots.clone();
        let Some(player) = self.players.get_mut(&id) else { return };
        let mut carried = std::mem::take(&mut player.carried);
        let mut drag = std::mem::take(&mut player.drag);
        let recipes = self.recipes.clone();
        let result = {
            let mut menu = Menu { layout: &layout, slots: &mut slots, carried: &mut carried, recipes: recipes.as_deref(), creative, drag: &mut drag };
            if kind.is_none() || matches!(kind, Some(MenuKind::CraftingTable)) {
                menu.update_result();
            }
            menu.click(Click { slot: click.slot, button: click.button, mode: click.mode })
        };
        let player = self.players.get_mut(&id).unwrap();
        player.carried = carried;
        player.drag = drag;
        self.store_menu_slots(id, kind.as_ref(), &slots);
        for stack in result.dropped {
            self.player_drop(id, stack, true);
        }

        // Tell this client what changed (and correct what it guessed differently).
        let player = &self.players[&id];
        let (window_id, state_id) = match &player.open_menu {
            Some(m) => (m.window_id, m.state_id),
            None => (0, player.inventory_state),
        };
        let claimed: Vec<usize> = click.changed_slots.iter().map(|(s, _)| *s as usize).collect();
        for (i, slot) in slots.iter().enumerate() {
            if before[i] != *slot || claimed.contains(&i) || (i == 0 && layout.crafting.is_some()) {
                player.conn.send(&play::ContainerSetSlot { window_id, state_id, slot: i as i16, item: slot.clone() });
            }
        }
        if !hashed_matches(&click.carried, &player.carried) {
            player.conn.send(&play::SetCursorItem { item: player.carried.clone() });
        }
        // Enchanting tables and anvils work out their offers and results again.
        match &kind {
            Some(MenuKind::Enchanting { .. }) if before[..2] != slots[..2] => self.update_enchanting_table(id),
            Some(MenuKind::Anvil { .. }) if !before[2].is_empty() && slots[2].is_empty() => self.took_anvil_result(id),
            Some(MenuKind::Anvil { .. }) if before[..2] != slots[..2] => self.update_anvil(id),
            _ => {}
        }
        // Others looking into the same container see its slots change.
        if let Some(MenuKind::Container { world, positions }) = &kind {
            let size = slots.len() - 36;
            let changed: Vec<usize> = (0..size).filter(|i| before[*i] != slots[*i]).collect();
            if !changed.is_empty() {
                self.broadcast_container_slots(id, *world, positions, &slots[..size], &changed);
            }
        }
        if let Some(MenuKind::Furnace { world, pos, .. }) = &kind {
            let changed: Vec<usize> = (0..3).filter(|i| before[*i] != slots[*i]).collect();
            if !changed.is_empty() {
                self.broadcast_container_slots(id, *world, &[*pos], &slots[..3], &changed);
            }
        }
        if let Some(MenuKind::Custom { id: custom }) = &kind {
            let size = slots.len() - 36;
            if (0..size).any(|i| before[i] != slots[i]) {
                let inv = crate::plugins::inventory::InvRef::Custom(*custom);
                for viewer in inv.viewers(self).into_iter().filter(|v| *v != id) {
                    self.send_menu_contents(viewer);
                }
            }
        }
    }

    /// Sends container slot changes to every viewer except `except`.
    fn broadcast_container_slots(&mut self, except: i32, world: usize, positions: &[BlockPos], items: &[Slot], changed: &[usize]) {
        let viewers: Vec<i32> = self.players.values().filter(|p| p.entity_id != except && positions.iter().any(|pos| self.menu_shows(p.open_menu.as_ref(), world, *pos))).map(|p| p.entity_id).collect();
        for v in viewers {
            let Some(p) = self.players.get_mut(&v) else { continue };
            let Some(menu) = p.open_menu.as_mut() else { continue };
            // A viewer of one half of a double chest sees it at a different offset.
            let offset = match &menu.kind {
                MenuKind::Container { positions: theirs, .. } => theirs.iter().position(|p| *p == positions[0]).map(|i| i * 27),
                MenuKind::Furnace { .. } => Some(0),
                _ => None,
            };
            let Some(offset) = offset else { continue };
            menu.state_id = (menu.state_id + 1) & 0x7FFF;
            let (window_id, state_id) = (menu.window_id, menu.state_id);
            for &i in changed {
                if i < items.len() {
                    p.conn.send(&play::ContainerSetSlot { window_id, state_id, slot: (i + offset) as i16, item: items[i].clone() });
                }
            }
        }
    }

    // --- furnaces ------------------------------------------------------------------------

    /// Smelting progress of every furnace in loaded chunks with something to do.
    pub(super) fn tick_furnaces(&mut self) {
        let Some(recipes) = self.recipes.clone() else { return };
        for world in 0..self.worlds.len() {
            let furnaces: Vec<BlockPos> = self.worlds[world]
                .chunks
                .iter()
                .flat_map(|(cpos, chunk)| {
                    chunk.block_entities.iter().filter_map(move |((y, z, x), nbt)| {
                        let id = nbt.string("id")?;
                        matches!(id, "minecraft:furnace" | "minecraft:smoker" | "minecraft:blast_furnace").then(|| BlockPos::new(cpos.x * 16 + *x as i32, *y, cpos.z * 16 + *z as i32))
                    })
                })
                .collect();
            for pos in furnaces {
                self.tick_furnace(world, pos, &recipes);
            }
        }
    }

    fn tick_furnace(&mut self, world: usize, pos: BlockPos, recipes: &super::recipes::Recipes) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let Some(ContainerBlock::Furnace(kind)) = container_block(state) else { return };
        let fast = kind != "minecraft:smelting";
        let nbt = self.block_entity_data(world, pos).cloned().unwrap_or_default();
        let (mut lit, mut lit_total, mut progress) = (nbt.i32("lit_time_remaining").unwrap_or(0), nbt.i32("lit_total_time").unwrap_or(0), nbt.i32("cooking_time_spent").unwrap_or(0));
        let mut items = self.container_items(world, pos);
        if lit <= 0 && progress == 0 && (items[0].is_empty() || items[1].is_empty()) {
            return;
        }
        let was_lit = lit > 0;
        let before = items.clone();
        if lit > 0 {
            lit -= 1;
        }
        let recipe = recipes.cook(kind, &items[0]).cloned();
        let can_burn = recipe.as_ref().is_some_and(|r| match &items[2].0 {
            None => true,
            Some(out) => super::entities::same_item(out, &r.result) && out.count + r.result.count <= super::entities::max_stack(out),
        });
        let total = recipe.as_ref().map_or(200, |r| r.cooking_time);
        if lit <= 0 && can_burn && !items[1].is_empty() {
            let burn = recipes.burn_time(&items[1], fast);
            if burn > 0 {
                lit = burn;
                lit_total = burn;
                let fuel = items[1].0.clone().unwrap();
                let is_lava = paper_data::items::get(fuel.item as u32).is_some_and(|i| i.name == "minecraft:lava_bucket");
                if is_lava {
                    items[1] = Slot::of(paper_data::items::id("minecraft:bucket").unwrap_or(0) as i32, 1);
                } else if fuel.count > 1 {
                    items[1] = Slot(Some(fuel.with_count(fuel.count - 1)));
                } else {
                    items[1] = Slot::EMPTY;
                }
            }
        }
        if lit > 0 && can_burn {
            progress += 1;
            if progress >= total {
                progress = 0;
                let r = recipe.unwrap();
                match items[2].0.as_mut() {
                    Some(out) => out.count += r.result.count,
                    None => items[2] = Slot(Some(r.result.clone())),
                }
                let input = items[0].0.clone().unwrap();
                items[0] = if input.count > 1 { Slot(Some(input.with_count(input.count - 1))) } else { Slot::EMPTY };
            }
        } else if progress > 0 {
            progress = if lit > 0 { 0 } else { (progress - 2).max(0) };
        }
        self.set_block_entity_value(world, pos, "lit_time_remaining", Tag::Short(lit as i16));
        self.set_block_entity_value(world, pos, "lit_total_time", Tag::Short(lit_total as i16));
        self.set_block_entity_value(world, pos, "cooking_time_spent", Tag::Short(progress as i16));
        self.set_block_entity_value(world, pos, "cooking_total_time", Tag::Short(total as i16));
        if items != before {
            self.set_container_items(world, pos, &items);
            let changed: Vec<usize> = (0..3).filter(|i| before[*i] != items[*i]).collect();
            self.broadcast_container_slots(-1, world, &[pos], &items, &changed);
        }
        if was_lit != (lit > 0) {
            if let Some(s) = blocks::with_property(state, "lit", if lit > 0 { "true" } else { "false" }) {
                self.set_block_and_update(world, pos, s);
            }
        }
        self.send_furnace_data(world, pos);
    }

    fn send_furnace_data(&mut self, world: usize, pos: BlockPos) {
        let nbt = self.block_entity_data(world, pos).cloned().unwrap_or_default();
        let values = [
            nbt.i32("lit_time_remaining").unwrap_or(0),
            nbt.i32("lit_total_time").unwrap_or(0),
            nbt.i32("cooking_time_spent").unwrap_or(0),
            nbt.i32("cooking_total_time").unwrap_or(200),
        ];
        for p in self.players.values() {
            if !self.menu_shows(p.open_menu.as_ref(), world, pos) {
                continue;
            }
            let window_id = p.open_menu.as_ref().unwrap().window_id;
            for (property, value) in values.iter().enumerate() {
                p.conn.send(&play::ContainerSetData { window_id, property: property as i16, value: (*value).clamp(0, i16::MAX as i32) as i16 });
            }
        }
    }
}

/// The block entity type a container block uses.
fn block_entity_id(state: u32) -> String {
    match blocks::block_of_state(state).name {
        "minecraft:trapped_chest" => "minecraft:trapped_chest".into(),
        name => name.to_owned(),
    }
}

/// Whether the client's hashed view of a slot equals the server's slot (as far as can be
/// told without the component hashes: same item, count and component types).
pub fn hashed_matches(hashed: &HashedSlot, slot: &Slot) -> bool {
    match (&hashed.0, &slot.0) {
        (None, None) => true,
        (Some(h), Some(s)) => h.item == s.item && h.count == s.count && h.added.len() == s.added.len() && h.removed == s.removed && s.added.is_empty(),
        _ => false,
    }
}
