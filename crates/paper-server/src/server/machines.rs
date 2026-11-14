//! Redstone blocks that deal with items and entities: hoppers (vanilla's
//! `HopperBlockEntity`), droppers and dispensers (`DispenserBlock`, `DropperBlock` and the
//! common dispense behaviours) and pressure plates (`BasePressurePlateBlock`).

use super::entities::{max_stack, same_item};
use super::survival::{self, category, Fluid};
use super::Server;
use crate::config::GameMode;
use crate::world::block_updates::{flags, Level};
use crate::world::chunk::ChunkPos;
use crate::world::entity::{Aabb, EntityKind, Vec3};
use paper_data::{blocks, items};
use paper_protocol::codec::BlockPos;
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;
use std::collections::HashSet;

/// The faces of a block, as vanilla's `Direction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Face {
    pub fn parse(name: &str) -> Option<Face> {
        Some(match name {
            "down" => Face::Down,
            "up" => Face::Up,
            "north" => Face::North,
            "south" => Face::South,
            "west" => Face::West,
            "east" => Face::East,
            _ => return None,
        })
    }

    pub fn offset(self) -> (i32, i32, i32) {
        match self {
            Face::Down => (0, -1, 0),
            Face::Up => (0, 1, 0),
            Face::North => (0, 0, -1),
            Face::South => (0, 0, 1),
            Face::West => (-1, 0, 0),
            Face::East => (1, 0, 0),
        }
    }

    pub fn opposite(self) -> Face {
        match self {
            Face::Down => Face::Up,
            Face::Up => Face::Down,
            Face::North => Face::South,
            Face::South => Face::North,
            Face::West => Face::East,
            Face::East => Face::West,
        }
    }

    /// `Direction.get3DDataValue`, used by the dispenser smoke particles.
    pub fn data_value(self) -> i32 {
        self as i32
    }
}

fn at(pos: BlockPos, face: Face) -> BlockPos {
    let (dx, dy, dz) = face.offset();
    pos.offset(dx, dy, dz)
}

/// A container block as hoppers and droppers see it.
#[derive(Clone, Debug)]
struct Inventory {
    /// The blocks holding the slots (two for a double chest).
    positions: Vec<BlockPos>,
    furnace: bool,
    /// A brewing stand (`getSlotsForFace`: the ingredient from above, bottles and fuel from
    /// the sides, bottles and ingredient below).
    brewing: bool,
    /// A crafter (filled evenly, never into switched-off slots).
    crafter: bool,
}

fn is_fuel(server: &Server, stack: &ItemStack) -> bool {
    server.recipes.as_ref().is_some_and(|r| r.burn_time(&Slot(Some(stack.clone())), false) > 0)
}

fn item_name(stack: &ItemStack) -> &'static str {
    items::get(stack.item as u32).map_or("", |i| i.name)
}

impl Server {
    /// The container at a block, if it is one hoppers can use.
    fn inventory_at(&self, world: usize, pos: BlockPos) -> Option<Inventory> {
        let state = self.worlds[world].block_at(pos.x, pos.y, pos.z)?;
        use super::containers::ContainerBlock;
        match super::containers::container_block(state)? {
            ContainerBlock::Chest => {
                let mut positions = vec![pos];
                if let Some(other) = self.double_chest_partner(world, pos, state) {
                    // The RIGHT half comes first, as in the window.
                    if blocks::property(state, "type") == Some("right") {
                        positions.push(other);
                    } else {
                        positions.insert(0, other);
                    }
                }
                Some(Inventory { positions, furnace: false, brewing: false, crafter: false })
            }
            ContainerBlock::Barrel | ContainerBlock::Hopper | ContainerBlock::Dispenser => Some(Inventory { positions: vec![pos], furnace: false, brewing: false, crafter: false }),
            ContainerBlock::Furnace(_) => Some(Inventory { positions: vec![pos], furnace: true, brewing: false, crafter: false }),
            ContainerBlock::BrewingStand => Some(Inventory { positions: vec![pos], furnace: false, brewing: true, crafter: false }),
            ContainerBlock::Crafter => Some(Inventory { positions: vec![pos], furnace: false, brewing: false, crafter: true }),
            ContainerBlock::EnderChest | ContainerBlock::CraftingTable | ContainerBlock::EnchantingTable | ContainerBlock::Anvil => None,
        }
    }

    /// The inventory at a block, with any loot table rolled into it first (hoppers and
    /// droppers reaching into a loot chest fill it, as in vanilla).
    fn loaded_inventory_at(&mut self, world: usize, pos: BlockPos) -> Option<Inventory> {
        let inv = self.inventory_at(world, pos)?;
        for p in &inv.positions {
            self.unpack_loot(world, *p);
        }
        Some(inv)
    }

    fn inventory_items(&self, world: usize, inv: &Inventory) -> Vec<Slot> {
        inv.positions.iter().flat_map(|p| self.container_items(world, *p)).collect()
    }

    fn set_inventory_items(&mut self, world: usize, inv: &Inventory, items: &[Slot]) {
        let per = items.len() / inv.positions.len().max(1);
        for (i, pos) in inv.positions.iter().enumerate() {
            self.set_container_items(world, *pos, &items[i * per..(i + 1) * per]);
        }
    }

    /// Slots reachable through a face (`WorldlyContainer.getSlotsForFace`).
    fn slots_for_face(inv: &Inventory, size: usize, face: Option<Face>) -> Vec<usize> {
        if inv.brewing {
            return match face {
                Some(Face::Up) => vec![3],
                Some(Face::Down) => vec![0, 1, 2, 3],
                _ => vec![0, 1, 2, 4],
            };
        }
        if !inv.furnace {
            return (0..size).collect();
        }
        match face {
            Some(Face::Down) => vec![2, 1],
            Some(Face::Up) => vec![0],
            _ => vec![1],
        }
    }

    fn can_place(&self, inv: &Inventory, slot: usize, stack: &ItemStack) -> bool {
        if inv.brewing {
            return match slot {
                3 => super::brewing::is_ingredient(stack),
                4 => super::brewing::is_fuel(stack),
                _ => super::brewing::fits_bottle_slot(stack),
            };
        }
        if !inv.furnace {
            return true;
        }
        match slot {
            0 => true,
            1 => is_fuel(self, stack) || item_name(stack) == "minecraft:bucket",
            _ => false,
        }
    }

    fn can_take(inv: &Inventory, slot: usize, stack: &ItemStack, face: Face) -> bool {
        // Only empty buckets come out of a furnace's fuel slot, only glass bottles out of a
        // brewing stand's ingredient slot.
        !(inv.furnace && face == Face::Down && slot == 1 && !matches!(item_name(stack), "minecraft:bucket" | "minecraft:water_bucket")) && !(inv.brewing && slot == 3 && item_name(stack) != "minecraft:glass_bottle")
    }

    /// Puts as much of `stack` as fits into `inv` through `face` (`HopperBlockEntity.addItem`);
    /// returns what is left.
    fn insert_into(&mut self, world: usize, inv: &Inventory, mut stack: ItemStack, face: Option<Face>) -> ItemStack {
        let mut items = self.inventory_items(world, inv);
        let mut changed = false;
        let disabled = if inv.crafter { Some(self.crafter_disabled(world, inv.positions[0])) } else { None };
        for slot in Self::slots_for_face(inv, items.len(), face) {
            if stack.count <= 0 {
                break;
            }
            if !self.can_place(inv, slot, &stack) {
                continue;
            }
            if disabled.as_ref().is_some_and(|d| !super::crafter::accepts(&items, d, slot, &stack)) {
                continue;
            }
            match items[slot].0.as_mut() {
                None => {
                    let moved = stack.count.min(max_stack(&stack));
                    items[slot] = Slot(Some(stack.with_count(moved)));
                    stack.count -= moved;
                    changed = true;
                }
                Some(existing) if same_item(existing, &stack) => {
                    let room = max_stack(existing) - existing.count;
                    if room > 0 {
                        let moved = room.min(stack.count);
                        existing.count += moved;
                        stack.count -= moved;
                        changed = true;
                    }
                }
                Some(_) => {}
            }
        }
        if changed {
            self.set_inventory_items(world, inv, &items);
        }
        stack
    }

    /// `CrafterBlock.dispenseItem`'s container part: puts a crafted stack into the container at
    /// `target` through `face`, one at a time into another crafter (or when the stack is
    /// bigger than the container takes). Returns what did not fit (all of it without one).
    pub(super) fn crafter_output_into(&mut self, world: usize, target: BlockPos, stack: ItemStack, face: Face) -> ItemStack {
        let Some(inv) = self.loaded_inventory_at(world, target) else { return stack };
        let mut left = stack;
        if inv.crafter || left.count > max_stack(&left).min(64) {
            while left.count > 0 {
                if self.insert_into(world, &inv, left.with_count(1), Some(face)).count > 0 {
                    break;
                }
                left.count -= 1;
            }
            return left;
        }
        loop {
            let before = left.count;
            left = self.insert_into(world, &inv, left, Some(face));
            if left.count == 0 || left.count == before {
                return left;
            }
        }
    }

    // --- hoppers -------------------------------------------------------------------------------

    /// Every hopper in a loaded chunk moves an item every 8 ticks (`pushItemsTick`).
    pub(super) fn tick_hoppers(&mut self) {
        for world in 0..self.worlds.len() {
            let hoppers: Vec<BlockPos> = self.worlds[world]
                .chunks
                .iter()
                .filter(|(p, _)| self.worlds[world].is_ready(**p))
                .flat_map(|(cpos, chunk)| {
                    chunk.block_entities.iter().filter_map(move |((y, z, x), nbt)| (nbt.string("id") == Some("minecraft:hopper")).then(|| BlockPos::new(cpos.x * 16 + *x as i32, *y, cpos.z * 16 + *z as i32)))
                })
                .collect();
            for pos in hoppers {
                self.tick_hopper(world, pos);
            }
        }
    }

    fn tick_hopper(&mut self, world: usize, pos: BlockPos) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        if blocks::block_of_state(state).name != "minecraft:hopper" {
            return;
        }
        let (chunk, key) = (ChunkPos::of_block(pos.x, pos.z), (pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8));
        let cooldown = self.worlds[world].chunks.get(&chunk).and_then(|c| c.block_entities.get(&key)).and_then(|n| n.i32("TransferCooldown")).unwrap_or(0);
        if cooldown > 0 {
            self.set_hopper_cooldown(world, pos, cooldown - 1);
            return;
        }
        if blocks::property(state, "enabled") == Some("false") {
            return;
        }
        let me = Inventory { positions: vec![pos], furnace: false, brewing: false, crafter: false };
        self.unpack_loot(world, pos);
        let items = self.container_items(world, pos);
        let mut moved = false;
        if items.iter().any(|s| !s.is_empty()) {
            moved = self.hopper_eject(world, pos, state, &me);
        }
        let items = self.container_items(world, pos);
        let full = items.iter().all(|s| s.0.as_ref().is_some_and(|i| i.count >= max_stack(i)));
        if !full {
            moved |= self.hopper_suck(world, pos, &me);
        }
        if moved {
            self.set_hopper_cooldown(world, pos, 8);
        }
    }

    fn set_hopper_cooldown(&mut self, world: usize, pos: BlockPos, ticks: i32) {
        let (chunk, key) = (ChunkPos::of_block(pos.x, pos.z), (pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8));
        if let Some(nbt) = self.worlds[world].chunks.get_mut(&chunk).and_then(|c| c.block_entities.get_mut(&key)) {
            nbt.insert("TransferCooldown", paper_nbt::Tag::Int(ticks));
        }
    }

    /// One item into the container the hopper points at.
    fn hopper_eject(&mut self, world: usize, pos: BlockPos, state: u32, me: &Inventory) -> bool {
        let Some(facing) = blocks::property(state, "facing").and_then(Face::parse) else { return false };
        let target = at(pos, facing);
        let Some(dest) = self.loaded_inventory_at(world, target) else { return false };
        let mut items = self.container_items(world, pos);
        for i in 0..items.len() {
            let Some(stack) = items[i].0.clone() else { continue };
            let left = self.insert_into(world, &dest, stack.with_count(1), Some(facing.opposite()));
            if left.count == 0 {
                items[i] = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
                self.set_inventory_items(world, me, &items);
                return true;
            }
        }
        false
    }

    /// One item from the container above, or item entities lying on top.
    fn hopper_suck(&mut self, world: usize, pos: BlockPos, me: &Inventory) -> bool {
        let above = at(pos, Face::Up);
        if let Some(source) = self.loaded_inventory_at(world, above) {
            let mut items = self.inventory_items(world, &source);
            for slot in Self::slots_for_face(&source, items.len(), Some(Face::Down)) {
                let Some(stack) = items[slot].0.clone() else { continue };
                if !Self::can_take(&source, slot, &stack, Face::Down) {
                    continue;
                }
                let left = self.insert_into(world, me, stack.with_count(1), None);
                if left.count == 0 {
                    items[slot] = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
                    self.set_inventory_items(world, &source, &items);
                    return true;
                }
            }
            return false;
        }
        // A full block on top keeps items away.
        let blocked = self.worlds[world].block_at(above.x, above.y, above.z).is_some_and(|s| paper_data::physics::flags(s) & paper_data::physics::FULL_CUBE != 0);
        if blocked {
            return false;
        }
        let area = Aabb::new(Vec3::new(pos.x as f64, pos.y as f64 + 11.0 / 16.0, pos.z as f64), Vec3::new(pos.x as f64 + 1.0, pos.y as f64 + 2.0, pos.z as f64 + 1.0));
        for id in self.worlds[world].entities_in(&area) {
            let Some(e) = self.worlds[world].entities.get(&id) else { continue };
            let EntityKind::Item { stack: Slot(Some(stack)), .. } = &e.kind else { continue };
            if e.removed || !e.bounding_box().intersects(&area) {
                continue;
            }
            let stack = stack.clone();
            let left = self.insert_into(world, me, stack.clone(), None);
            if left.count == stack.count {
                continue;
            }
            if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                if left.count == 0 {
                    e.removed = true;
                } else if let EntityKind::Item { stack: s, .. } = &mut e.kind {
                    *s = Slot(Some(left));
                    e.metadata_dirty = true;
                }
            }
            return true;
        }
        false
    }

    // --- droppers and dispensers ----------------------------------------------------------------

    /// A dispenser or dropper fires: one item from a random slot.
    pub(super) fn dispense(&mut self, world: usize, pos: BlockPos) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let name = blocks::block_of_state(state).name;
        if !matches!(name, "minecraft:dispenser" | "minecraft:dropper") {
            return;
        }
        let Some(facing) = blocks::property(state, "facing").and_then(Face::parse) else { return };
        self.unpack_loot(world, pos);
        let mut items = self.container_items(world, pos);
        // A random non-empty slot (reservoir sampling, as vanilla).
        let mut rng = rand::thread_rng();
        let (mut chosen, mut seen) = (None, 0);
        for (i, s) in items.iter().enumerate() {
            if !s.is_empty() {
                seen += 1;
                if rng.gen_range(0..seen) == 0 {
                    chosen = Some(i);
                }
            }
        }
        let Some(slot) = chosen else {
            self.broadcast_level_event(world, 1001, pos, 0, false);
            return;
        };
        let stack = items[slot].0.clone().unwrap();
        let front = at(pos, facing);
        let rest = if name == "minecraft:dropper" {
            match self.loaded_inventory_at(world, front) {
                Some(dest) => {
                    let left = self.insert_into(world, &dest, stack.with_count(1), Some(facing.opposite()));
                    if left.count == 0 {
                        Some(stack.with_count(stack.count - 1))
                    } else {
                        Some(stack)
                    }
                }
                None => Some(self.drop_from(world, pos, facing, stack)),
            }
        } else {
            self.dispense_behaviour(world, pos, facing, stack)
        };
        let mut items_now = self.container_items(world, pos);
        if items_now.len() == items.len() {
            items_now[slot] = rest.filter(|s| s.count > 0).map_or(Slot::EMPTY, |s| Slot(Some(s)));
            items = items_now;
            self.set_container_items(world, pos, &items);
        }
    }

    /// `DefaultDispenseItemBehavior`: one item flies out of the front.
    fn drop_from(&mut self, world: usize, pos: BlockPos, facing: Face, stack: ItemStack) -> ItemStack {
        let (dx, dy, dz) = facing.offset();
        let center = Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5);
        let mut spawn = center.add(Vec3::new(0.7 * dx as f64, 0.7 * dy as f64, 0.7 * dz as f64));
        spawn.y -= if dy != 0 { 0.125 } else { 0.15625 };
        let mut rng = rand::thread_rng();
        let speed = rng.gen::<f64>() * 0.1 + 0.2;
        let mut triangle = |mean: f64| mean + (rng.gen::<f64>() - rng.gen::<f64>()) * 0.0172275 * 6.0;
        let vel = Vec3::new(triangle(dx as f64 * speed), triangle(0.2), triangle(dz as f64 * speed));
        self.spawn_item(world, spawn, Slot(Some(stack.with_count(1))), vel, 10);
        self.broadcast_level_event(world, 1000, pos, 0, false);
        self.broadcast_level_event(world, 2000, pos, facing.data_value(), false);
        stack.with_count(stack.count - 1)
    }

    /// What a dispenser does with an item; returns the rest of the stack.
    fn dispense_behaviour(&mut self, world: usize, pos: BlockPos, facing: Face, stack: ItemStack) -> Option<ItemStack> {
        let front = at(pos, facing);
        let name = item_name(&stack);
        let w = &self.worlds[world];
        let in_front = w.block_at(front.x, front.y, front.z).unwrap_or(blocks::AIR);
        match name {
            "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:tipped_arrow" => {
                let (dx, dy, dz) = facing.offset();
                let origin = Vec3::new(pos.x as f64 + 0.5 + 0.7 * dx as f64, pos.y as f64 + 0.6 + 0.7 * dy as f64, pos.z as f64 + 0.5 + 0.7 * dz as f64);
                self.spawn_arrow_towards(world, origin, Vec3::new(dx as f64, dy as f64, dz as f64), 1.1, 6.0, None, 2.0, false, true);
                self.dispensed(world, pos, facing, 1002);
                Some(stack.with_count(stack.count - 1))
            }
            // `ProjectileDispenseBehavior`: thrown items fly out of the front.
            "minecraft:snowball" | "minecraft:egg" | "minecraft:blue_egg" | "minecraft:brown_egg" | "minecraft:experience_bottle" | "minecraft:splash_potion" | "minecraft:lingering_potion" => {
                let Some((entity, _, _, gravity, _)) = super::throwing::throwable(name) else { return Some(stack) };
                let (dx, dy, dz) = facing.offset();
                // 0.7 in front of the centre and 0.1 higher; bottles and potions fly faster and
                // straighter (`ProjectileItem.DispenseConfig`).
                let origin = Vec3::new(pos.x as f64 + 0.5 + 0.7 * dx as f64, pos.y as f64 + 0.6 + 0.7 * dy as f64, pos.z as f64 + 0.5 + 0.7 * dz as f64);
                let (power, uncertainty) = if matches!(name, "minecraft:experience_bottle" | "minecraft:splash_potion" | "minecraft:lingering_potion") { (1.375, 3.0) } else { (1.1, 6.0) };
                self.spawn_thrown(world, entity, origin, Vec3::new(dx as f64, dy as f64, dz as f64), power, uncertainty, gravity, Slot(Some(stack.with_count(1))), None);
                self.dispensed(world, pos, facing, 1002);
                Some(stack.with_count(stack.count - 1))
            }
            // Spawn eggs hatch in front of the dispenser.
            egg if egg.ends_with("_spawn_egg") && paper_data::entity_types::by_name(egg.trim_end_matches("_spawn_egg")).is_some() => {
                let entity = egg.trim_end_matches("_spawn_egg").to_owned();
                let at = Vec3::new(front.x as f64 + 0.5, front.y as f64, front.z as f64 + 0.5);
                if self.spawn_mob(world, &entity, at, super::mobs::SpawnReason::Egg).is_none() {
                    return Some(self.drop_from(world, pos, facing, stack));
                }
                self.broadcast_level_event(world, 1000, pos, 0, false);
                Some(stack.with_count(stack.count - 1))
            }
            // `FireChargeItem`: a small fireball straight out of the front.
            "minecraft:fire_charge" => {
                let (dx, dy, dz) = facing.offset();
                let origin = Vec3::new(pos.x as f64 + 0.5 + dx as f64, pos.y as f64 + 0.5 + dy as f64, pos.z as f64 + 0.5 + dz as f64);
                let mut rng = rand::thread_rng();
                let mut wobble = |m: f64| m + (rng.gen::<f64>() - rng.gen::<f64>()) * 0.11485;
                let dir = Vec3::new(wobble(dx as f64), wobble(dy as f64), wobble(dz as f64));
                self.spawn_thrown(world, "minecraft:small_fireball", origin, dir, 1.0, 6.666_666_5, 0.0, Slot(Some(stack.with_count(1))), None);
                self.dispensed(world, pos, facing, 1018);
                Some(stack.with_count(stack.count - 1))
            }
            // Bone meal grows what is in front, or the dispenser clicks.
            "minecraft:bone_meal" => {
                let used = {
                    let mut level = Level::new(&mut self.worlds[world]);
                    crate::world::block_updates::plants::bone_meal(&mut level, in_front, front)
                };
                self.publish_block_updates(world);
                self.dispensed(world, pos, facing, if used { 1000 } else { 1001 });
                if !used {
                    return Some(stack);
                }
                self.broadcast_level_event(world, 1505, front, 15, false);
                Some(stack.with_count(stack.count - 1))
            }
            // Shears harvest a full beehive's honeycomb or shear a sheep in front.
            "minecraft:shears" => {
                let sheared = self.shear_in_front(world, front, in_front);
                self.dispensed(world, pos, facing, if sheared { 1000 } else { 1001 });
                if !sheared {
                    return Some(stack);
                }
                let mut slot = Slot(Some(stack));
                survival::hurt_item(&mut slot, 1, false);
                slot.0
            }
            // Glass bottles fill with honey from a full beehive or with water.
            "minecraft:glass_bottle" => {
                let honey = in_block_tag(in_front, "minecraft:beehives") && blocks::property(in_front, "honey_level") == Some("5");
                let water = survival::fluid_of(in_front).is_some_and(|(f, _)| f == Fluid::Water);
                let filled = if honey {
                    self.set_block_and_update(world, front, blocks::with_property(in_front, "honey_level", "0").unwrap_or(in_front));
                    ItemStack::new(items::id("minecraft:honey_bottle").unwrap_or(0) as i32, 1)
                } else if water {
                    water_bottle()
                } else {
                    return Some(self.drop_from(world, pos, facing, stack));
                };
                self.dispensed(world, pos, facing, 1000);
                Some(self.consume_with_remainder(world, pos, facing, stack, filled))
            }
            // A water bottle turns dirt in front into mud.
            "minecraft:potion" if super::effects::potion_of(&stack).as_deref() == Some("minecraft:water") && in_block_tag(in_front, "minecraft:convertible_to_mud") => {
                self.set_block_and_update(world, front, blocks::default_state("minecraft:mud").unwrap_or(in_front));
                self.play_sound(world, Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5), "minecraft:item.bottle.empty", category::BLOCKS, 1.0, 1.0, None);
                self.dispensed(world, pos, facing, 1000);
                let bottle = ItemStack::new(items::id("minecraft:glass_bottle").unwrap_or(0) as i32, 1);
                Some(self.consume_with_remainder(world, pos, facing, stack, bottle))
            }
            // Powder snow goes into an empty block in front.
            "minecraft:powder_snow_bucket" => {
                if !blocks::is_air(in_front) {
                    return Some(self.drop_from(world, pos, facing, stack));
                }
                self.set_block_and_update(world, front, blocks::default_state("minecraft:powder_snow").unwrap_or(blocks::AIR));
                self.play_sound(world, Vec3::new(front.x as f64 + 0.5, front.y as f64 + 0.5, front.z as f64 + 0.5), "minecraft:item.bucket.empty_powder_snow", category::BLOCKS, 1.0, 1.0, None);
                self.dispensed(world, pos, facing, 1000);
                Some(ItemStack::new(items::id("minecraft:bucket").unwrap_or(0) as i32, 1))
            }
            // A carved pumpkin or wither skeleton skull is only worn (building golems and
            // withers is not done here); nothing to wear it makes the dispenser click.
            "minecraft:carved_pumpkin" | "minecraft:wither_skeleton_skull" => {
                let worn = self.dispense_equipment(world, front, &stack);
                self.dispensed(world, pos, facing, if worn { 1000 } else { 1001 });
                Some(if worn { stack.with_count(stack.count - 1) } else { stack })
            }
            "minecraft:tnt" if w.rules.bool("tnt_explodes") => {
                self.prime_tnt(world, front, 80, None);
                Some(stack.with_count(stack.count - 1))
            }
            "minecraft:water_bucket" | "minecraft:lava_bucket" | "minecraft:cod_bucket" | "minecraft:salmon_bucket" | "minecraft:pufferfish_bucket" | "minecraft:tropical_fish_bucket" | "minecraft:axolotl_bucket" | "minecraft:tadpole_bucket" => {
                let fluid = if name == "minecraft:lava_bucket" { Fluid::Lava } else { Fluid::Water };
                let waterloggable = fluid == Fluid::Water && blocks::property(in_front, "waterlogged") == Some("false");
                let evaporates = fluid == Fluid::Water && paper_data::synced::dimension_type(&w.dimension).is_some_and(|d| d.water_evaporates);
                if !(waterloggable || blocks::is_replaceable(in_front) || blocks::is_air(in_front)) {
                    return Some(self.drop_from(world, pos, facing, stack));
                }
                if !evaporates {
                    let placed = if waterloggable {
                        blocks::with_property(in_front, "waterlogged", "true").unwrap_or(in_front)
                    } else {
                        blocks::default_state(if fluid == Fluid::Water { "minecraft:water" } else { "minecraft:lava" }).unwrap_or(blocks::AIR)
                    };
                    self.set_block_and_update(world, front, placed);
                }
                // A bucket of fish lets its fish out too (`MobBucketItem.checkExtraContent`).
                if let Some((mob, _)) = super::animals::mob_in_bucket(name) {
                    let at = Vec3::new(front.x as f64 + 0.5, front.y as f64, front.z as f64 + 0.5);
                    self.release_bucket_mob(world, &stack, mob, at);
                }
                self.dispensed(world, pos, facing, 1000);
                Some(ItemStack::new(items::id("minecraft:bucket").unwrap_or(0) as i32, 1))
            }
            "minecraft:bucket" => {
                let source = survival::fluid_of(in_front).filter(|(_, amount)| *amount == 8 && blocks::property(in_front, "level").is_none_or(|l| l == "0"));
                let Some((fluid, _)) = source else { return Some(self.drop_from(world, pos, facing, stack)) };
                let drained = if blocks::property(in_front, "waterlogged") == Some("true") { blocks::with_property(in_front, "waterlogged", "false").unwrap_or(in_front) } else { blocks::AIR };
                self.set_block_and_update(world, front, drained);
                let filled = ItemStack::new(items::id(if fluid == Fluid::Water { "minecraft:water_bucket" } else { "minecraft:lava_bucket" }).unwrap_or(0) as i32, 1);
                if stack.count == 1 {
                    return Some(filled);
                }
                // The filled bucket goes into the dispenser, or out of it when full.
                let me = Inventory { positions: vec![pos], furnace: false, brewing: false, crafter: false };
                let left = self.insert_into(world, &me, filled.clone(), None);
                if left.count > 0 {
                    self.drop_from(world, pos, facing, filled);
                }
                Some(stack.with_count(stack.count - 1))
            }
            "minecraft:flint_and_steel" => {
                if blocks::block_of_state(in_front).name == "minecraft:tnt" && w.rules.bool("tnt_explodes") {
                    self.set_block_and_update(world, front, blocks::AIR);
                    self.prime_tnt(world, front, 80, None);
                } else if blocks::is_air(in_front) {
                    let fire = crate::world::block_updates::fire::fire_state_for(&self.worlds[world], front);
                    self.set_block_and_update(world, front, fire);
                } else {
                    self.broadcast_level_event(world, 1001, pos, 0, false);
                    return Some(stack);
                }
                let mut slot = Slot(Some(stack));
                survival::hurt_item(&mut slot, 1, false);
                slot.0
            }
            _ => {
                // Armour, heads, elytra, saddles, horse and wolf armour and llama carpets go on
                // whoever stands in front; anything else (or with nobody there) flies out.
                if paper_data::item_data::by_id(stack.item as u32).equip_slot.is_some() && self.dispense_equipment(world, front, &stack) {
                    self.dispensed(world, pos, facing, 1000);
                    return Some(stack.with_count(stack.count - 1));
                }
                Some(self.drop_from(world, pos, facing, stack))
            }
        }
    }

    /// The dispenser's sound (1000 dispensed, 1001 failed, 1002 shot, 1018 fire charge) and its
    /// smoke (`playSound`, `playAnimation`).
    fn dispensed(&mut self, world: usize, pos: BlockPos, facing: Face, sound: i32) {
        self.broadcast_level_event(world, sound, pos, 0, false);
        self.broadcast_level_event(world, 2000, pos, facing.data_value(), false);
    }

    /// `consumeWithRemainder`: one item is used up and `filled` goes back into the dispenser,
    /// or out of it when there is no room. Returns what stays in the used slot.
    fn consume_with_remainder(&mut self, world: usize, pos: BlockPos, facing: Face, stack: ItemStack, filled: ItemStack) -> ItemStack {
        if stack.count == 1 {
            return filled;
        }
        let me = Inventory { positions: vec![pos], furnace: false, brewing: false, crafter: false };
        let left = self.insert_into(world, &me, filled.clone(), None);
        if left.count > 0 {
            self.drop_from(world, pos, facing, filled);
        }
        stack.with_count(stack.count - 1)
    }

    /// `ShearsDispenseItemBehavior`: a beehive full of honey gives three honeycomb, or a sheep
    /// in front is sheared. Returns whether anything was.
    fn shear_in_front(&mut self, world: usize, front: BlockPos, state: u32) -> bool {
        if in_block_tag(state, "minecraft:beehives") && blocks::property(state, "honey_level") == Some("5") {
            self.harvest_honeycomb(world, front, state);
            return true;
        }
        let cell = Aabb::new(Vec3::new(front.x as f64, front.y as f64, front.z as f64), Vec3::new(front.x as f64 + 1.0, front.y as f64 + 1.0, front.z as f64 + 1.0));
        let w = &self.worlds[world];
        let sheep = w.entities_in(&cell).into_iter().find(|id| w.entities.get(id).is_some_and(|e| e.info.name == "minecraft:sheep" && e.mob().is_some_and(|m| !m.living.is_dead() && !m.baby && !m.sheared)));
        match sheep {
            Some(id) => {
                self.shear_sheep(world, id);
                true
            }
            None => false,
        }
    }

    /// Takes the honeycomb of a full beehive (`BeehiveBlock.dropHoneycomb`).
    pub(super) fn harvest_honeycomb(&mut self, world: usize, pos: BlockPos, state: u32) {
        self.play_sound(world, Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5), "minecraft:block.beehive.shear", category::BLOCKS, 1.0, 1.0, None);
        if let Some(comb) = items::id("minecraft:honeycomb") {
            self.pop_resource(world, pos, Slot::of(comb as i32, 3));
        }
        self.set_block_and_update(world, pos, blocks::with_property(state, "honey_level", "0").unwrap_or(state));
    }

    /// `EquipmentDispenseItemBehavior.dispenseEquipment`: the item goes on the first player or
    /// mob in the block that may wear it there and has that slot free. Returns whether it did.
    fn dispense_equipment(&mut self, world: usize, front: BlockPos, stack: &ItemStack) -> bool {
        let data = paper_data::item_data::by_id(stack.item as u32);
        let Some(slot) = data.equip_slot.clone() else { return false };
        let equippable = data.raw.as_ref().and_then(|r| r.get("equippable"));
        if equippable.and_then(|e| e.get("dispensable")).and_then(serde_json::Value::as_bool) == Some(false) {
            return false;
        }
        let may_wear = |entity: &str| super::menu::may_wear(stack.item as u32, entity);
        let cell = Aabb::new(Vec3::new(front.x as f64, front.y as f64, front.z as f64), Vec3::new(front.x as f64 + 1.0, front.y as f64 + 1.0, front.z as f64 + 1.0));
        let one = Slot(Some(stack.with_count(1)));
        // Players wear armour, heads and elytra.
        let player_slot = match slot.as_str() {
            "head" => Some(5),
            "chest" => Some(6),
            "legs" => Some(7),
            "feet" => Some(8),
            _ => None,
        };
        if let Some(index) = player_slot.filter(|_| may_wear("minecraft:player")) {
            let player = self.players.values().find(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator && p.inventory[index].is_empty() && Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8).intersects(&cell)).map(|p| p.entity_id);
            if let Some(player) = player {
                self.set_inventory_slot(player, index, one);
                return true;
            }
        }
        // Mob equipment: main hand, off hand, feet, legs, chest, head, body, saddle.
        let Some(index) = ["", "", "feet", "legs", "chest", "head", "body", "saddle"].iter().position(|s| *s == slot) else { return false };
        let w = &self.worlds[world];
        let target = w.entities_in(&cell).into_iter().find(|id| {
            let Some(e) = w.entities.get(id) else { return false };
            let Some(m) = e.mob().filter(|m| !m.living.is_dead() && m.equipment[index].is_empty()) else { return false };
            let name = e.info.name.as_str();
            // Saddles and body armour only go on grown (and, for horses, wolves and llamas,
            // tamed) mobs.
            let grown_and_tame = !m.baby && (m.tamed || !(super::riding::is_horse(name) || name == "minecraft:wolf") || matches!(name, "minecraft:camel" | "minecraft:camel_husk" | "minecraft:skeleton_horse" | "minecraft:zombie_horse"));
            may_wear(name) && (index < 6 || grown_and_tame)
        });
        let Some(target) = target else { return false };
        if let Some(e) = self.worlds[world].entities.get_mut(&target) {
            if let Some(m) = e.mob_mut() {
                m.equipment[index] = one;
                m.persistent = true;
            }
            e.equipment_dirty = true;
            e.metadata_dirty = true;
        }
        true
    }

    // --- pressure plates -----------------------------------------------------------------------

    /// Players and entities standing on pressure plates press them; entering string presses
    /// it (`TripWireBlock.entityInside`).
    pub(super) fn tick_pressure_plates(&mut self) {
        for world in 0..self.worlds.len() {
            let mut boxes: Vec<Aabb> = self.players.values().filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator).map(|p| Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8)).collect();
            boxes.extend(self.worlds[world].entities.values().filter(|e| e.is_alive()).map(|e| e.bounding_box()));
            let mut plates: HashSet<BlockPos> = HashSet::new();
            let mut strings: HashSet<BlockPos> = HashSet::new();
            let w = &self.worlds[world];
            let string = blocks::id("minecraft:tripwire");
            for b in &boxes {
                let y = b.min.y.floor() as i32;
                for z in b.min.z.floor() as i32..=b.max.z.floor() as i32 {
                    for x in b.min.x.floor() as i32..=b.max.x.floor() as i32 {
                        let Some(state) = w.block_at(x, y, z) else { continue };
                        if blocks::block_of_state(state).name.ends_with("_pressure_plate") && plate_signal(state) == 0 {
                            plates.insert(BlockPos::new(x, y, z));
                        }
                        // String reacts to anything inside its block, wherever in it.
                        for y in y..=(b.max.y - 1.0e-5).floor() as i32 {
                            let Some(state) = w.block_at(x, y, z) else { continue };
                            if Some(blocks::block_id_of_state(state)) == string && blocks::property(state, "powered") == Some("false") {
                                strings.insert(BlockPos::new(x, y, z));
                            }
                        }
                    }
                }
            }
            for pos in plates {
                self.check_pressure_plate(world, pos);
            }
            for pos in strings {
                let Some(w) = self.worlds.get_mut(world) else { continue };
                crate::world::block_updates::tripwire::check_pressed(&mut Level::new(w), pos, true);
                self.publish_block_updates(world);
            }
        }
    }

    /// Recomputes a plate's signal from what stands on it (`checkPressed`).
    pub(super) fn check_pressure_plate(&mut self, world: usize, pos: BlockPos) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let name = blocks::block_of_state(state).name;
        if !name.ends_with("_pressure_plate") {
            return;
        }
        let touch = Aabb::new(Vec3::new(pos.x as f64 + 0.0625, pos.y as f64, pos.z as f64 + 0.0625), Vec3::new(pos.x as f64 + 0.9375, pos.y as f64 + 0.25, pos.z as f64 + 0.9375));
        // Stone plates only feel living things; wooden and weighted plates everything.
        let mobs_only = matches!(name, "minecraft:stone_pressure_plate" | "minecraft:polished_blackstone_pressure_plate");
        let mut count = self.players.values().filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator && Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8).intersects(&touch)).count();
        count += self.worlds[world].entities_in(&touch).into_iter().filter_map(|id| self.worlds[world].entities.get(&id)).filter(|e| e.is_alive() && e.bounding_box().intersects(&touch) && (!mobs_only || e.mob().is_some())).count();
        let signal = match name {
            "minecraft:light_weighted_pressure_plate" => ((count.min(15) as f32 / 15.0) * 15.0).ceil() as i32,
            "minecraft:heavy_weighted_pressure_plate" => ((count.min(150) as f32 / 150.0) * 15.0).ceil() as i32,
            _ => {
                if count > 0 {
                    15
                } else {
                    0
                }
            }
        };
        let old = plate_signal(state);
        let weighted = name.contains("weighted");
        if old != signal {
            let new_state = if weighted { blocks::with_property(state, "power", &signal.to_string()) } else { blocks::with_property(state, "powered", if signal > 0 { "true" } else { "false" }) }.unwrap_or(state);
            let block = blocks::block_id_of_state(state);
            let mut level = Level::new(&mut self.worlds[world]);
            level.set_block(pos, new_state, flags::CLIENTS);
            level.update_neighbors_at(pos, block);
            level.update_neighbors_at(pos.offset(0, -1, 0), block);
            let sound_type = if weighted {
                "metal_pressure_plate"
            } else if mobs_only {
                "stone_pressure_plate"
            } else if name.contains("crimson") || name.contains("warped") {
                "nether_wood_pressure_plate"
            } else if name.contains("bamboo") {
                "bamboo_wood_pressure_plate"
            } else if name.contains("cherry") {
                "cherry_wood_pressure_plate"
            } else {
                "wooden_pressure_plate"
            };
            let sound = format!("minecraft:block.{sound_type}.click_{}", if signal > 0 { "on" } else { "off" });
            let at = Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5);
            self.play_sound(world, at, &sound, category::BLOCKS, 1.0, 1.0, None);
        }
        if signal > 0 {
            let block = blocks::block_id_of_state(state);
            Level::new(&mut self.worlds[world]).schedule_tick(pos, block, if weighted { 10 } else { 20 });
        }
        self.publish_block_updates(world);
    }
}

impl Server {
    /// Pressed string checks whether anything is still in its outline (`checkPressed`).
    pub(super) fn check_tripwire(&mut self, world: usize, pos: BlockPos) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        if blocks::block_of_state(state).name != "minecraft:tripwire" {
            return;
        }
        let (low, high) = crate::world::block_updates::tripwire::wire_box(state);
        let inside = Aabb::new(Vec3::new(pos.x as f64, pos.y as f64 + low, pos.z as f64), Vec3::new(pos.x as f64 + 1.0, pos.y as f64 + high, pos.z as f64 + 1.0));
        let w = &self.worlds[world];
        let pressed = self.players.values().any(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator && Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8).intersects(&inside))
            || w.entities_in(&inside).into_iter().filter_map(|id| w.entities.get(&id)).any(|e| e.is_alive());
        let Some(w) = self.worlds.get_mut(world) else { return };
        crate::world::block_updates::tripwire::check_pressed(&mut Level::new(w), pos, pressed);
        self.publish_block_updates(world);
    }
}

fn in_block_tag(state: u32, tag: &str) -> bool {
    paper_data::synced::tag("minecraft:block", tag).is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(state) as i32)))
}

/// A water bottle (`PotionContents.createItemStack(POTION, WATER)`).
pub(super) fn water_bottle() -> ItemStack {
    let mut bottle = ItemStack::new(items::id("minecraft:potion").unwrap_or(0) as i32, 1);
    if let Some(water) = paper_data::registries::id("minecraft:potion", "minecraft:water") {
        super::enchanting::set_component(&mut bottle, "minecraft:potion_contents", super::loot::potion_contents(water as i32));
    }
    bottle
}

fn plate_signal(state: u32) -> i32 {
    match blocks::property(state, "power") {
        Some(p) => p.parse().unwrap_or(0),
        None => {
            if blocks::property(state, "powered") == Some("true") {
                15
            } else {
                0
            }
        }
    }
}
