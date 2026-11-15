//! Gliding with an elytra (vanilla's fall flying): a player in the air starts gliding when
//! their client asks, as long as they wear something with the `glider` component that will not
//! break at its next wear; gliding wears it every second, and stops when they land, ride,
//! levitate or lose it. Plugins hear of each start and stop (`EntityToggleGlideEvent`).

use super::Server;
use crate::plugins::events;
use paper_protocol::types::Slot;

/// The chest armour slot of the player inventory window.
pub const CHEST: usize = 6;

/// Whether an item lets its wearer glide (`canGlideUsing`): it has the `glider` component
/// and its next wear would not break it.
pub fn glides_with(slot: &Slot) -> bool {
    let Some(stack) = slot.0.as_ref() else { return false };
    let Some(glider) = paper_data::registries::id("minecraft:data_component_type", "minecraft:glider").map(|t| t as i32) else { return false };
    if stack.removed.contains(&glider) {
        return false;
    }
    let by_default = paper_data::item_data::by_id(stack.item as u32).raw.as_ref().is_some_and(|r| r.get("glider").is_some());
    if !by_default && !stack.added.iter().any(|(t, _)| *t == glider) {
        return false;
    }
    let max = super::survival::max_damage(stack);
    max <= 0 || super::survival::item_damage(stack) < max - 1
}

impl Server {
    /// Whether a player could glide now (`canGlide`): in the air, riding nothing, without
    /// levitation, wearing a glider.
    fn can_glide(&self, id: i32) -> bool {
        self.players.get(&id).is_some_and(|p| {
            !p.on_ground && p.vehicle.is_none() && !p.dead && !p.effects.iter().any(|e| e.name() == "minecraft:levitation") && glides_with(&p.inventory[CHEST])
        })
    }

    /// The client asks to glide (`tryToStartFallFlying`): only out of water, when the player
    /// can, and if plugins let them; a refused client is told it is not gliding.
    pub(super) fn start_gliding(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else { return };
        if p.gliding {
            return;
        }
        let in_water = {
            let w = &self.worlds[p.world];
            let area = crate::world::entity::Aabb::of_entity(crate::world::entity::Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8);
            super::survival::box_in_fluid(w, &area, super::survival::Fluid::Water)
        };
        let allowed = !in_water && self.can_glide(id) && events::on_player_toggle(self, id, "EntityToggleGlideEvent", "event.toggle_glide", paper_bridge::bmap! { "gliding" => true });
        let Some(p) = self.players.get_mut(&id) else { return };
        if allowed {
            p.gliding = true;
            p.glide_ticks = 0;
        }
        // Either way the client (and those who see the player) learn what is so.
        p.metadata_dirty = true;
    }

    /// A gliding player's tick (`updateFallFlying`): the glider wears a point every 20 ticks;
    /// a player who can no longer glide stops, unless plugins keep them gliding.
    pub(super) fn tick_gliding(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else { return };
        if !p.gliding {
            return;
        }
        if !self.can_glide(id) {
            if events::on_player_toggle(self, id, "EntityToggleGlideEvent", "event.toggle_glide", paper_bridge::bmap! { "gliding" => false }) {
                self.stop_gliding(id);
            }
            return;
        }
        let Some(p) = self.players.get_mut(&id) else { return };
        p.glide_ticks += 1;
        let creative = p.game_mode == crate::config::GameMode::Creative;
        if p.glide_ticks % 20 == 0 && !creative {
            self.wear_player_item(id, CHEST, 1, true);
            let worn = self.players[&id].inventory[CHEST].clone();
            self.set_inventory_slot(id, CHEST, worn);
        }
    }

    pub fn stop_gliding(&mut self, id: i32) {
        if let Some(p) = self.players.get_mut(&id) {
            if p.gliding {
                p.gliding = false;
                p.glide_ticks = 0;
                p.metadata_dirty = true;
            }
        }
    }
}
