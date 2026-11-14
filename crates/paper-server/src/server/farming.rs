//! Items used on blocks for farming and building (vanilla's `HoeItem`, `ShovelItem`,
//! `AxeItem` and `BoneMealItem.useOn`): tilling, paths, stripping logs, bone meal. The plants
//! themselves live in `world::block_updates::plants`.

use super::player::{HOTBAR_START, OFFHAND_SLOT};
use super::survival::category;
use super::Server;
use crate::config::GameMode;
use crate::world::block_updates::{plants, Level};
use crate::world::entity::Vec3;
use paper_data::{blocks, items};
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::play;
use paper_protocol::types::Slot;

fn in_item_tag(item: i32, tag: &str) -> bool {
    paper_data::synced::tag("minecraft:item", tag).is_some_and(|ids| ids.contains(&item))
}

fn in_block_tag(state: u32, tag: &str) -> bool {
    paper_data::synced::tag("minecraft:block", tag).is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(state) as i32)))
}

/// `AxeItem.STRIPPABLES`: logs and wood lose their bark.
fn stripped(name: &str) -> Option<String> {
    let path = name.strip_prefix("minecraft:")?;
    if path.starts_with("stripped_") {
        return None;
    }
    let woody = path.ends_with("_log") || path.ends_with("_wood") || path.ends_with("_stem") || path.ends_with("_hyphae") || path == "bamboo_block";
    woody.then(|| format!("minecraft:stripped_{path}")).filter(|s| blocks::by_name(s).is_some())
}

impl Server {
    /// A hoe, shovel, axe or bone meal used on a block. Returns whether it did something.
    pub(super) fn use_tool_on_block(&mut self, id: i32, use_on: &play::UseItemOn) -> bool {
        let Some(p) = self.players.get(&id) else { return false };
        if p.game_mode == GameMode::Spectator || p.game_mode == GameMode::Adventure {
            return false;
        }
        let slot = if use_on.hand.0 == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        let (world, creative) = (p.world, p.game_mode == GameMode::Creative);
        let pos = use_on.pos;
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return false };
        let item = items::get(stack.item as u32).map_or("", |i| i.name);
        let face = use_on.face.0;
        let above_free = self.worlds[world].block_at(pos.x, pos.y + 1, pos.z).is_some_and(blocks::is_air);
        let at = Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5);
        let block = blocks::block_of_state(state).name;

        let (new_state, sound) = if in_item_tag(stack.item, "minecraft:hoes") {
            // `HoeItem.TILLABLES`.
            if face == 0 || !above_free {
                return false;
            }
            let tilled = match block {
                _ if in_block_tag(state, "minecraft:turns_into_farmland") => blocks::default_state("minecraft:farmland"),
                "minecraft:coarse_dirt" => blocks::default_state("minecraft:dirt"),
                "minecraft:rooted_dirt" => {
                    if let Some(roots) = items::id("minecraft:hanging_roots") {
                        self.pop_resource(world, pos.offset(0, -1, 0), Slot::of(roots as i32, 1));
                    }
                    blocks::default_state("minecraft:dirt")
                }
                _ => None,
            };
            (tilled, "minecraft:item.hoe.till")
        } else if in_item_tag(stack.item, "minecraft:shovels") {
            // `ShovelItem.FLATTENABLES`.
            if face == 0 || !above_free || !in_block_tag(state, "minecraft:turns_into_dirt_path") {
                return false;
            }
            (blocks::default_state("minecraft:dirt_path"), "minecraft:item.shovel.flatten")
        } else if in_item_tag(stack.item, "minecraft:axes") {
            let Some(target) = stripped(block).and_then(|n| blocks::default_state(&n)) else { return false };
            // The stripped block keeps the axis.
            let target = blocks::property(state, "axis").and_then(|a| blocks::with_property(target, "axis", a)).unwrap_or(target);
            (Some(target), "minecraft:item.axe.strip")
        } else if item == "minecraft:bone_meal" {
            let used = {
                let mut level = Level::new(&mut self.worlds[world]);
                plants::bone_meal(&mut level, state, pos)
            };
            self.publish_block_updates(world);
            if !used {
                return false;
            }
            if !creative {
                self.consume_held(id, slot);
            }
            self.send_near(world, pos, 64.0, None, &play::LevelEvent { event: 1505, pos, data: 15, global: false });
            return true;
        } else {
            return false;
        };
        let Some(new_state) = new_state else { return false };
        self.set_block_and_update(world, pos, new_state);
        self.play_sound(world, at, sound, category::BLOCKS, 1.0, 1.0, None);
        if !creative {
            let mut tool = self.players[&id].inventory[slot].clone();
            super::survival::hurt_item(&mut tool, 1, false);
            self.set_inventory_slot(id, slot, tool);
        }
        true
    }

    fn consume_held(&mut self, id: i32, slot: usize) {
        let Some(p) = self.players.get(&id) else { return };
        let mut stack = p.inventory[slot].clone();
        if let Some(s) = stack.0.as_mut() {
            s.count -= 1;
        }
        if stack.0.as_ref().is_some_and(|s| s.count <= 0) {
            stack = Slot::EMPTY;
        }
        self.set_inventory_slot(id, slot, stack);
    }

    /// A player or mob landed on `pos` after falling `distance` blocks: farmland may be
    /// trampled (`FarmBlock.fallOn`; players always may, mobs only with mob griefing).
    pub(super) fn landed_on(&mut self, world: usize, pos: BlockPos, distance: f64, player: bool) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        if blocks::block_of_state(state).name != "minecraft:farmland" {
            return;
        }
        let griefing = player || self.worlds[world].rules.bool("mob_griefing");
        {
            let mut level = Level::new(&mut self.worlds[world]);
            plants::trample(&mut level, pos, distance, griefing);
        }
        self.publish_block_updates(world);
    }
}
