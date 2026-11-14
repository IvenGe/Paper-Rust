//! Crafters (vanilla's `CrafterBlock`, `CrafterBlockEntity` and `CrafterMenu`): nine slots
//! that hoppers fill evenly, slots players switch off by clicking them, and one craft per
//! redstone pulse. The result and the ingredients' leftovers come out of the front, into a
//! container there if there is one.

use super::containers::MenuKind;
use super::Server;
use crate::config::GameMode;
use crate::world::block_updates::{flags, Dir, Level};
use crate::world::entity::Vec3;
use paper_data::blocks;
use paper_nbt::Tag;
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::play;
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

/// The window's result slot (`CrafterMenu` adds it after the player's inventory).
pub const RESULT_SLOT: usize = 45;

/// Ticks a crafter shows that it crafted (`MAX_CRAFTING_TICKS`).
pub const CRAFTING_TICKS: i32 = 6;

/// Where a crafter's front points (the first half of its `orientation`).
pub fn front_of(state: u32) -> Dir {
    blocks::property(state, "orientation").and_then(|o| o.split('_').next()).and_then(Dir::from_name).unwrap_or(Dir::North)
}

/// `CrafterBlockEntity.canPlaceItem`: hoppers fill a crafter evenly, never a switched-off
/// slot, and a slot only once no later slot has room with fewer of the same item.
pub fn accepts(items: &[Slot], disabled: &[bool; 9], slot: usize, stack: &ItemStack) -> bool {
    if slot >= 9 || disabled[slot] {
        return false;
    }
    let Some(existing) = items[slot].0.as_ref() else { return true };
    let count = existing.count;
    if count >= super::entities::max_stack(existing) || !super::entities::same_item(existing, stack) {
        return false;
    }
    !(slot + 1..9).any(|i| !disabled[i] && items[i].0.as_ref().is_none_or(|s| s.count < count && super::entities::same_item(s, existing)))
}

impl Server {
    /// Which of a crafter's slots are switched off (`disabled_slots`).
    pub fn crafter_disabled(&self, world: usize, pos: BlockPos) -> [bool; 9] {
        let mut out = [false; 9];
        if let Some(Tag::IntArray(list)) = self.block_entity_data(world, pos).and_then(|n| n.get("disabled_slots")) {
            for i in list.iter().filter(|i| (0..9).contains(*i)) {
                out[*i as usize] = true;
            }
        }
        out
    }

    fn set_crafter_disabled(&mut self, world: usize, pos: BlockPos, disabled: [bool; 9]) {
        let list: Vec<i32> = (0..9).filter(|i| disabled[*i]).map(|i| i as i32).collect();
        self.set_block_entity_value(world, pos, "disabled_slots", Tag::IntArray(list));
        // Comparators count switched-off slots too.
        if let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) {
            Level::new(&mut self.worlds[world]).update_neighbour_for_output_signal(pos, blocks::block_id_of_state(state));
            self.publish_block_updates(world);
        }
    }

    /// What the crafter would make from its slots now (`CrafterMenu.refreshRecipeResult`).
    pub fn crafter_result(&self, world: usize, pos: BlockPos) -> Slot {
        let items = self.container_items(world, pos);
        if items.len() != 9 {
            return Slot::EMPTY;
        }
        self.recipes.as_ref().and_then(|r| r.craft(&items, 3)).map_or(Slot::EMPTY, |(_, s)| Slot(Some(s)))
    }

    /// The crafter's window data (`ContainerData`): each slot's switch (1 off), then whether
    /// the crafter is powered.
    pub(super) fn send_crafter_data(&mut self, world: usize, pos: BlockPos) {
        let disabled = self.crafter_disabled(world, pos);
        let powered = self.worlds[world].block_at(pos.x, pos.y, pos.z).and_then(|s| blocks::property(s, "triggered")) == Some("true");
        for p in self.players.values() {
            let Some(menu) = p.open_menu.as_ref().filter(|m| m.kind == MenuKind::Crafter { world, pos }) else { continue };
            for (i, off) in disabled.iter().enumerate() {
                p.conn.send(&play::ContainerSetData { window_id: menu.window_id, property: i as i16, value: *off as i16 });
            }
            p.conn.send(&play::ContainerSetData { window_id: menu.window_id, property: 9, value: powered as i16 });
        }
    }

    /// Sends everyone looking into a crafter its slots and result again.
    pub(super) fn refresh_crafter_viewers(&mut self, world: usize, pos: BlockPos, except: Option<i32>) {
        let viewers: Vec<i32> = self.players.values().filter(|p| Some(p.entity_id) != except && p.open_menu.as_ref().is_some_and(|m| m.kind == MenuKind::Crafter { world, pos })).map(|p| p.entity_id).collect();
        for v in viewers {
            self.send_menu_contents(v);
        }
    }

    /// A player switches a crafter slot on or off (`ServerboundContainerSlotStateChangedPacket`;
    /// only empty slots switch).
    pub(super) fn handle_slot_state_changed(&mut self, id: i32, slot: i32, window_id: i32, enabled: bool) {
        let Some(p) = self.players.get(&id) else { return };
        if p.game_mode == GameMode::Spectator {
            return;
        }
        let Some(MenuKind::Crafter { world, pos }) = p.open_menu.as_ref().filter(|m| m.window_id == window_id).map(|m| m.kind.clone()) else { return };
        if !(0..9).contains(&slot) || !self.container_items(world, pos)[slot as usize].is_empty() {
            return;
        }
        let mut disabled = self.crafter_disabled(world, pos);
        disabled[slot as usize] = !enabled;
        self.set_crafter_disabled(world, pos, disabled);
        self.send_crafter_data(world, pos);
    }

    /// A powered crafter crafts once (`CrafterBlock.dispenseFrom`): the result and the
    /// leftovers come out of the front and every slot gives up one item; without a recipe
    /// it only clicks.
    pub(super) fn crafter_craft(&mut self, world: usize, pos: BlockPos) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        if blocks::block_of_state(state).name != "minecraft:crafter" {
            return;
        }
        let mut items = self.container_items(world, pos);
        let crafted = self.recipes.as_ref().and_then(|r| r.craft(&items, 3)).map(|(_, s)| s).filter(|s| s.count > 0);
        let Some(result) = crafted else {
            self.broadcast_level_event(world, 1050, pos, 0, false);
            return;
        };
        {
            let mut level = Level::new(&mut self.worlds[world]);
            level.set_block(pos, blocks::with_property(state, "crafting", "true").unwrap_or(state), flags::CLIENTS);
            let end = level.game_time() + CRAFTING_TICKS as i64;
            level.world.updates.crafting.push((pos, end));
        }
        self.publish_block_updates(world);
        self.set_block_entity_value(world, pos, "crafting_ticks_remaining", Tag::Int(CRAFTING_TICKS));
        let leftovers: Vec<ItemStack> = items.iter().filter_map(|s| s.0.as_ref()).filter_map(|s| super::recipes::crafting_remainder(s.item)).filter_map(|s| s.0).collect();
        for s in items.iter_mut() {
            if let Some(stack) = s.0.as_mut() {
                stack.count -= 1;
                if stack.count <= 0 {
                    *s = Slot::EMPTY;
                }
            }
        }
        // Vanilla takes the ingredients after the items are out; nothing can see the
        // difference since the crafter is not a container the outputs could reach.
        let front = front_of(state);
        self.crafter_eject(world, pos, front, result);
        for leftover in leftovers {
            self.crafter_eject(world, pos, front, leftover);
        }
        self.set_container_items(world, pos, &items);
        self.refresh_crafter_viewers(world, pos, None);
    }

    /// `CrafterBlock.dispenseItem`: into the container in front (one at a time into another
    /// crafter or when the stack is bigger than the container takes), the rest flies out.
    fn crafter_eject(&mut self, world: usize, pos: BlockPos, front: Dir, stack: ItemStack) {
        let (dx, dy, dz) = front.offset();
        let target = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
        let face = super::machines::Face::parse(front.opposite().name()).unwrap_or(super::machines::Face::North);
        let left = self.crafter_output_into(world, target, stack, face);
        if left.count <= 0 {
            return;
        }
        // `DefaultDispenseItemBehavior.spawnItem` with speed 6, from 0.7 in front.
        let mut at = Vec3::new(pos.x as f64 + 0.5 + 0.7 * dx as f64, pos.y as f64 + 0.5 + 0.7 * dy as f64, pos.z as f64 + 0.5 + 0.7 * dz as f64);
        at.y -= if dy != 0 { 0.125 } else { 0.15625 };
        let mut rng = rand::thread_rng();
        let speed = rng.gen::<f64>() * 0.1 + 0.2;
        let mut triangle = |mean: f64| mean + (rng.gen::<f64>() - rng.gen::<f64>()) * 0.0172275 * 6.0;
        let vel = Vec3::new(triangle(dx as f64 * speed), triangle(0.2), triangle(dz as f64 * speed));
        self.spawn_item(world, at, Slot(Some(left)), vel, 10);
        self.broadcast_level_event(world, 1049, pos, 0, false);
        self.broadcast_level_event(world, 2010, pos, front.index() as i32, false);
    }
}
