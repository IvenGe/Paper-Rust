//! Attributes in the game: the modifiers of equipment, effects and sprinting applied to
//! players' and mobs' attribute maps, and their values sent to the clients
//! (`ClientboundUpdateAttributesPacket`).

use super::Server;
use crate::world::attributes::{Attributes, Modifier, Operation, Source};
use crate::world::entity::EntityKind;
use paper_data::{effects, item_data};
use paper_protocol::codec::Reader;
use paper_protocol::packets::play;
use paper_protocol::types::{ItemStack, Slot};

/// `EquipmentSlotGroup`s by protocol id.
const SLOT_GROUPS: &[&str] = &["any", "mainhand", "offhand", "hand", "feet", "legs", "chest", "head", "armor", "body", "saddle"];

/// Whether a modifier for `group` applies to an item in `slot`.
fn in_group(slot: &str, group: &str) -> bool {
    match group {
        "any" => true,
        "hand" => slot == "mainhand" || slot == "offhand",
        "armor" => matches!(slot, "feet" | "legs" | "chest" | "head" | "body"),
        _ => group == slot,
    }
}

/// An item's attribute modifiers: (attribute, modifier, slot group). The stack's own
/// `attribute_modifiers` component replaces the item's defaults.
pub fn item_modifiers(stack: &ItemStack) -> Vec<(String, Modifier, String)> {
    let Some(component) = paper_data::registries::id("minecraft:data_component_type", "minecraft:attribute_modifiers").map(|i| i as i32) else { return Vec::new() };
    if let Some((_, data)) = stack.added.iter().find(|(t, _)| *t == component) {
        return decode_modifiers(data).unwrap_or_default();
    }
    if stack.removed.contains(&component) {
        return Vec::new();
    }
    item_data::by_id(stack.item as u32)
        .modifiers
        .iter()
        .filter_map(|m| Some((m.attribute.clone(), Modifier::new(m.id.clone(), m.amount, Operation::from_name(&m.operation)?), m.slot.clone())))
        .collect()
}

/// The network encoding of `minecraft:attribute_modifiers`: per entry the attribute, the
/// modifier (id, amount, operation), the slot group and how the tooltip shows it.
fn decode_modifiers(data: &[u8]) -> anyhow::Result<Vec<(String, Modifier, String)>> {
    let mut r = Reader::new(data);
    let entries = r.list(1024, |r| {
        let attribute = r.varint()?;
        let id = r.string(32767)?;
        let amount = r.f64()?;
        let operation = r.varint()?;
        let slot = r.varint()?;
        // Display: 0 default, 1 hidden, 2 a text component overriding the line.
        if r.varint()? == 2 {
            r.component()?;
        }
        Ok((attribute, id, amount, operation, slot))
    })?;
    Ok(entries
        .into_iter()
        .filter_map(|(attribute, id, amount, operation, slot)| {
            let name = paper_data::registries::name("minecraft:attribute", attribute as u32)?;
            let operation = Operation::from_id(operation as i64)?;
            let group = SLOT_GROUPS.get(slot as usize)?;
            Some((name.to_owned(), Modifier::new(id, amount, operation), (*group).to_owned()))
        })
        .collect())
}

/// The modifiers of the items worn and held, as `LivingEntity.collectEquipmentChanges`
/// applies them: each item counts for the slot it is in.
pub fn equipment_modifiers(slots: &[(&str, &Slot)]) -> Vec<(String, Modifier)> {
    let mut out = Vec::new();
    for (slot, item) in slots {
        let Some(stack) = item.0.as_ref().filter(|s| s.count > 0) else { continue };
        // Broken elytra and the like have no modifiers; everything else does.
        for (attribute, modifier, group) in item_modifiers(stack) {
            if in_group(slot, &group) {
                out.push((attribute, modifier));
            }
        }
    }
    out
}

/// The modifiers of active effects (`MobEffect.addAttributeModifiers`): each template's
/// amount times the amplifier plus one.
pub fn effect_modifiers(active: impl Iterator<Item = (i32, i32)>) -> Vec<(String, Modifier)> {
    let mut out = Vec::new();
    for (effect, amplifier) in active {
        let Some(info) = effects::by_id(effect) else { continue };
        for (attribute, operation, amount, id) in &info.modifiers {
            let Some(operation) = Operation::from_name(operation) else { continue };
            out.push((attribute.clone(), Modifier::new(id.clone(), amount * (amplifier + 1) as f64, operation)));
        }
    }
    out
}

const SPRINTING: &str = "minecraft:sprinting";

/// Mob equipment slots, in the order `Mob::equipment` keeps them.
const MOB_SLOTS: [&str; 6] = ["mainhand", "offhand", "feet", "legs", "chest", "head"];

impl Server {
    /// Applies a player's equipment, effects and sprinting to its attributes (cheap when
    /// nothing changed: only real changes mark attributes for sending).
    pub fn refresh_player_attributes(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        let held = super::player::HOTBAR_START + p.held_slot as usize;
        let slots = [
            ("mainhand", &p.inventory[held]),
            ("offhand", &p.inventory[super::player::OFFHAND_SLOT]),
            ("head", &p.inventory[5]),
            ("chest", &p.inventory[6]),
            ("legs", &p.inventory[7]),
            ("feet", &p.inventory[8]),
        ];
        let equipment = equipment_modifiers(&slots);
        let effect_mods = effect_modifiers(p.effects.iter().map(|e| (e.id, e.amplifier)));
        p.attributes.replace_source(Source::Equipment, &equipment);
        p.attributes.replace_source(Source::Effect, &effect_mods);
        if p.sprinting {
            p.attributes.add_from("movement_speed", Modifier::new(SPRINTING, 0.3, Operation::AddMultipliedTotal), Source::Transient);
        } else {
            p.attributes.remove_modifier("movement_speed", SPRINTING);
        }
        // `onAttributeUpdated`: a lower max health caps the health at once.
        let max = p.max_health();
        if p.health > max {
            p.health = max;
        }
    }

    /// The same for a mob.
    pub fn refresh_mob_attributes(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds.get_mut(world).and_then(|w| w.entities.get_mut(&id)) else { return };
        let EntityKind::Mob(mob) = &mut e.kind else { return };
        let slots: Vec<(&str, &Slot)> = MOB_SLOTS.iter().copied().zip(mob.equipment.iter()).collect();
        let equipment = equipment_modifiers(&slots);
        let effect_mods = effect_modifiers(mob.living.effects.iter().map(|(effect, amplifier, _)| (*effect, *amplifier)));
        mob.attributes.replace_source(Source::Equipment, &equipment);
        mob.attributes.replace_source(Source::Effect, &effect_mods);
        if mob.attributes.has("max_health") {
            mob.living.max_health = mob.attributes.value("max_health") as f32;
            mob.living.health = mob.living.health.min(mob.living.max_health);
        }
    }

    /// Each tick: refreshes every player's and mob's attribute sources and sends the
    /// attributes that changed to the entity's viewers (and a player to itself). A lower
    /// maximum health caps the health.
    pub(super) fn tick_attributes(&mut self) {
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for id in ids {
            self.refresh_player_attributes(id);
            let Some(p) = self.players.get_mut(&id) else { continue };
            let dirty = p.attributes.take_dirty();
            if dirty.is_empty() {
                continue;
            }
            if dirty.iter().any(|d| d == "minecraft:max_health") {
                let max = p.attributes.value("max_health") as f32;
                if p.health > max {
                    p.health = max;
                }
            }
            let packet = play::UpdateAttributes { entity_id: id, attributes: p.attributes.packet_entries(dirty.iter().map(String::as_str)) };
            p.conn.send(&packet);
            self.broadcast_to_viewers(id, &packet);
        }
        for world in 0..self.worlds.len() {
            let mobs: Vec<i32> = self.worlds[world].entities.iter().filter(|(_, e)| matches!(e.kind, EntityKind::Mob(_))).map(|(id, _)| *id).collect();
            for id in mobs {
                self.refresh_mob_attributes(world, id);
                let Some(e) = self.worlds[world].entities.get_mut(&id) else { continue };
                let EntityKind::Mob(mob) = &mut e.kind else { continue };
                let dirty = mob.attributes.take_dirty();
                if dirty.is_empty() {
                    continue;
                }
                if dirty.iter().any(|d| d == "minecraft:max_health") {
                    mob.living.max_health = mob.attributes.value("max_health") as f32;
                    mob.living.health = mob.living.health.min(mob.living.max_health);
                }
                let packet = play::UpdateAttributes { entity_id: id, attributes: mob.attributes.packet_entries(dirty.iter().map(String::as_str)) };
                let viewers: Vec<i32> = e.viewers.iter().copied().collect();
                for viewer in viewers {
                    self.send(viewer, &packet);
                }
            }
        }
    }

    /// The attributes of a mob a player starts to see, unless they are its type's defaults.
    pub(super) fn mob_attributes_for_new_viewer(attributes: &Attributes, entity_type: &str, id: i32) -> Option<play::UpdateAttributes> {
        if attributes.is_default_for(entity_type) {
            return None;
        }
        let names: Vec<String> = attributes.names().map(str::to_owned).collect();
        Some(play::UpdateAttributes { entity_id: id, attributes: attributes.packet_entries(names.iter().map(String::as_str)) })
    }
}

/// The attribute map of a player or mob, for the plugin API.
pub fn attributes_of(server: &Server, id: i32) -> Option<&Attributes> {
    if let Some(p) = server.players.get(&id) {
        return Some(&p.attributes);
    }
    server.worlds.iter().find_map(|w| match &w.entities.get(&id)?.kind {
        EntityKind::Mob(mob) => Some(&mob.attributes),
        _ => None,
    })
}

pub fn attributes_of_mut(server: &mut Server, id: i32) -> Option<&mut Attributes> {
    if server.players.contains_key(&id) {
        return server.players.get_mut(&id).map(|p| &mut p.attributes);
    }
    server.worlds.iter_mut().find_map(|w| match &mut w.entities.get_mut(&id)?.kind {
        EntityKind::Mob(mob) => Some(&mut mob.attributes),
        _ => None,
    })
}
