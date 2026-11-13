//! Player data files (`<world>/playerdata/<uuid>.dat`), in vanilla's layout where the
//! server models the data: position, rotation, dimension, game mode, health and food, the
//! inventory and equipment, the selected slot and flight.
//!
//! Item components are kept in this server's wire form under `paper-rs:components`, because
//! translating every component to its NBT form needs codecs this server does not have.

use super::player::{Location, Player, HOTBAR_START, INVENTORY_SLOTS, OFFHAND_SLOT};
use super::Server;
use crate::config::GameMode;
use paper_data::items;
use paper_nbt::{Compound, Tag};
use paper_protocol::types::{ItemStack, Slot};

/// Equipment keys and the inventory window slots they occupy.
const EQUIPMENT: [(&str, usize); 5] = [("head", 5), ("chest", 6), ("legs", 7), ("feet", 8), ("offhand", OFFHAND_SLOT)];

/// Player data read back from disk.
pub struct SavedPlayer {
    pub world: usize,
    pub loc: Location,
    pub game_mode: Option<GameMode>,
    pub previous_game_mode: Option<GameMode>,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub held_slot: u8,
    pub inventory: Vec<Slot>,
    pub flying: bool,
}

fn component_name(id: i32) -> Option<&'static str> {
    paper_data::registries::name("minecraft:data_component_type", id as u32)
}

fn component_id(name: &str) -> Option<i32> {
    paper_data::registries::id("minecraft:data_component_type", name).map(|i| i as i32)
}

pub fn item_to_nbt(slot: &Slot) -> Option<Compound> {
    let stack = slot.0.as_ref().filter(|s| s.count > 0)?;
    let info = items::get(stack.item as u32)?;
    let mut c = Compound::new().with("id", Tag::String(info.name.to_owned())).with("count", Tag::Int(stack.count));
    if !stack.added.is_empty() || !stack.removed.is_empty() {
        let added = stack
            .added
            .iter()
            .filter_map(|(ty, data)| Some(Tag::Compound(Compound::new().with("type", Tag::String(component_name(*ty)?.to_owned())).with("data", Tag::ByteArray(data.clone())))))
            .collect();
        let removed = stack.removed.iter().filter_map(|ty| Some(Tag::String(component_name(*ty)?.to_owned()))).collect();
        c.insert("paper-rs:components", Tag::Compound(Compound::new().with("added", Tag::List(added)).with("removed", Tag::List(removed))));
    }
    Some(c)
}

pub fn item_from_nbt(c: &Compound) -> Slot {
    let Some(item) = c.string("id").and_then(items::id) else { return Slot::EMPTY };
    let count = c.i32("count").or_else(|| c.i32("Count")).unwrap_or(1);
    if count <= 0 || item == 0 {
        return Slot::EMPTY;
    }
    let mut stack = ItemStack::new(item as i32, count.min(99));
    if let Some(components) = c.compound("paper-rs:components") {
        for added in components.list("added").unwrap_or(&[]) {
            let Some(added) = added.as_compound() else { continue };
            if let (Some(ty), Some(data)) = (added.string("type").and_then(component_id), added.byte_array("data")) {
                stack.added.push((ty, data.to_vec()));
            }
        }
        for removed in components.list("removed").unwrap_or(&[]) {
            if let Some(ty) = removed.as_str().and_then(component_id) {
                stack.removed.push(ty);
            }
        }
    }
    Slot(Some(stack))
}

/// Window slot of a vanilla `Inventory` entry's `Slot` byte.
fn window_slot(vanilla: i8) -> Option<usize> {
    match vanilla {
        0..=8 => Some(HOTBAR_START + vanilla as usize),
        9..=35 => Some(vanilla as usize),
        // Before 1.21.5 armour and the off hand were inventory entries too.
        100..=103 => Some(8 - (vanilla - 100) as usize),
        -106 => Some(OFFHAND_SLOT),
        _ => None,
    }
}

fn vanilla_slot(window: usize) -> Option<i8> {
    match window {
        HOTBAR_START..=44 => Some((window - HOTBAR_START) as i8),
        9..=35 => Some(window as i8),
        _ => None,
    }
}

impl Server {
    pub fn player_to_nbt(&self, player: &Player) -> Compound {
        let world = &self.worlds[player.world];
        let inventory = player
            .inventory
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                let vanilla = vanilla_slot(i)?;
                let mut c = item_to_nbt(slot)?;
                c.insert("Slot", Tag::Byte(vanilla));
                Some(Tag::Compound(c))
            })
            .collect();
        let mut equipment = Compound::new();
        for (key, slot) in EQUIPMENT {
            if let Some(c) = item_to_nbt(&player.inventory[slot]) {
                equipment.insert(key, Tag::Compound(c));
            }
        }
        let mut abilities = Compound::new().with("flying", Tag::Byte(player.flying as i8)).with("mayfly", Tag::Byte(player.allow_flight as i8));
        abilities.insert("instabuild", Tag::Byte((player.game_mode == GameMode::Creative) as i8));
        abilities.insert("invulnerable", Tag::Byte(matches!(player.game_mode, GameMode::Creative | GameMode::Spectator) as i8));
        let mut c = Compound::new()
            .with("DataVersion", Tag::Int(crate::world::storage::data_version()))
            .with("Pos", Tag::List(vec![Tag::Double(player.loc.x), Tag::Double(player.loc.y), Tag::Double(player.loc.z)]))
            .with("Rotation", Tag::List(vec![Tag::Float(player.loc.yaw), Tag::Float(player.loc.pitch)]))
            .with("Motion", Tag::List(vec![Tag::Double(0.0), Tag::Double(0.0), Tag::Double(0.0)]))
            .with("OnGround", Tag::Byte(player.on_ground as i8))
            .with("Dimension", Tag::String(world.dimension.clone()))
            .with("paper-rs:world", Tag::String(world.name.clone()))
            .with("playerGameType", Tag::Int(player.game_mode.id()))
            .with("Health", Tag::Float(player.health))
            .with("foodLevel", Tag::Int(player.food))
            .with("foodSaturationLevel", Tag::Float(player.saturation))
            .with("SelectedItemSlot", Tag::Int(player.held_slot as i32))
            .with("Inventory", Tag::List(inventory))
            .with("equipment", Tag::Compound(equipment))
            .with("abilities", Tag::Compound(abilities));
        if let Some(previous) = player.previous_game_mode {
            c.insert("previousPlayerGameType", Tag::Int(previous.id()));
        }
        c
    }

    pub fn player_from_nbt(&self, c: &Compound) -> Option<SavedPlayer> {
        let pos = c.list("Pos")?;
        let (x, y, z) = (pos.first()?.as_f64()?, pos.get(1)?.as_f64()?, pos.get(2)?.as_f64()?);
        if !(x.is_finite() && y.is_finite() && z.is_finite()) {
            return None;
        }
        let rotation = c.list("Rotation").unwrap_or(&[]);
        let yaw = rotation.first().and_then(Tag::as_f64).unwrap_or(0.0) as f32;
        let pitch = rotation.get(1).and_then(Tag::as_f64).unwrap_or(0.0) as f32;
        let world = c
            .string("paper-rs:world")
            .and_then(|name| self.worlds.iter().position(|w| w.name == name))
            .or_else(|| c.string("Dimension").and_then(|d| self.worlds.iter().position(|w| w.dimension == d)))
            .unwrap_or(0);
        let mut inventory = vec![Slot::EMPTY; INVENTORY_SLOTS];
        for entry in c.list("Inventory").unwrap_or(&[]) {
            let Some(entry) = entry.as_compound() else { continue };
            if let Some(slot) = entry.i32("Slot").and_then(|s| window_slot(s as i8)) {
                inventory[slot] = item_from_nbt(entry);
            }
        }
        if let Some(equipment) = c.compound("equipment") {
            for (key, slot) in EQUIPMENT {
                if let Some(item) = equipment.compound(key) {
                    inventory[slot] = item_from_nbt(item);
                }
            }
        }
        Some(SavedPlayer {
            world,
            loc: Location { x, y, z, yaw, pitch: pitch.clamp(-90.0, 90.0) },
            game_mode: c.i32("playerGameType").and_then(GameMode::from_id),
            previous_game_mode: c.i32("previousPlayerGameType").and_then(GameMode::from_id),
            health: c.f64("Health").map_or(20.0, |h| h as f32).clamp(0.0, 20.0),
            food: c.i32("foodLevel").unwrap_or(20).clamp(0, 20),
            saturation: c.f64("foodSaturationLevel").map_or(5.0, |s| s as f32),
            held_slot: c.i32("SelectedItemSlot").unwrap_or(0).clamp(0, 8) as u8,
            inventory,
            flying: c.compound("abilities").and_then(|a| a.bool("flying")).unwrap_or(false),
        })
    }

    /// Reads a player's saved data, if the server saves worlds and the player was here before.
    pub fn load_player_data(&self, uuid: uuid::Uuid) -> Option<SavedPlayer> {
        let storage = self.worlds.first()?.storage.as_ref()?;
        match storage.read_player(uuid) {
            Ok(Some(nbt)) => self.player_from_nbt(&nbt),
            Ok(None) => None,
            Err(e) => {
                log::error!("Could not read the player data of {uuid}: {e:#}");
                None
            }
        }
    }

    /// Queues a player's data for saving.
    pub fn save_player_data(&self, player: &Player) {
        let Some(storage) = self.worlds.first().and_then(|w| w.storage.as_ref()) else { return };
        storage.save_player(player.uuid(), self.player_to_nbt(player));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_round_trip() {
        let stone = items::id("minecraft:stone").unwrap() as i32;
        let mut stack = ItemStack::new(stone, 12);
        let name = component_id("minecraft:custom_name").unwrap();
        stack.added.push((name, vec![1, 2, 3]));
        stack.removed.push(component_id("minecraft:lore").unwrap());
        let slot = Slot(Some(stack));
        let nbt = item_to_nbt(&slot).unwrap();
        assert_eq!(nbt.string("id"), Some("minecraft:stone"));
        assert_eq!(item_from_nbt(&nbt), slot);
        assert!(item_to_nbt(&Slot::EMPTY).is_none());
    }

    #[test]
    fn slot_numbering() {
        for window in (9..=44).chain([HOTBAR_START]) {
            assert_eq!(window_slot(vanilla_slot(window).unwrap()), Some(window));
        }
        assert_eq!(window_slot(103), Some(5), "old-style helmet slot");
        assert_eq!(window_slot(-106), Some(OFFHAND_SLOT));
    }
}
