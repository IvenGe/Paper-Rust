//! Items and windows for 1.8: items as id, count, damage and NBT (name and lore with `§`
//! codes, enchantments by number, dye colour); windows by name, with the slots of those
//! whose layout changed renumbered.

use super::ids::cb;
use super::tables::{self, Tables};
use super::{text, Legacy18};
use crate::io::Result;
use crate::Outcome;
use paper_data::packets::play::clientbound as p;
use paper_nbt::{Compound, Tag};
use paper_protocol::codec::{Reader, WriteExt};
use paper_protocol::types::ItemStack;

fn packet(id: i32) -> Vec<u8> {
    let mut w = Vec::new();
    w.put_varint(id);
    w
}

fn component(name: &str) -> i32 {
    paper_data::registries::id("minecraft:data_component_type", name).map_or(-1, |i| i as i32)
}

/// The value of a component an item stack has.
fn value<'a>(stack: &'a ItemStack, name: &str) -> Option<&'a [u8]> {
    let ty = component(name);
    stack.added.iter().find(|(t, _)| *t == ty).map(|(_, v)| v.as_slice())
}

/// 1.8's potion damage values (drinkable; splash potions add 16384 and drop 8192).
fn potion_damage(potion: &str) -> i16 {
    match potion.strip_prefix("minecraft:").unwrap_or(potion) {
        "awkward" => 16,
        "thick" => 32,
        "mundane" => 64,
        "regeneration" | "long_regeneration" | "strong_regeneration" => 8193,
        "swiftness" | "long_swiftness" | "strong_swiftness" => 8194,
        "fire_resistance" | "long_fire_resistance" => 8227,
        "poison" | "long_poison" | "strong_poison" => 8196,
        "healing" | "strong_healing" => 8261,
        "night_vision" | "long_night_vision" => 8230,
        "weakness" | "long_weakness" => 8232,
        "strength" | "long_strength" | "strong_strength" => 8201,
        "slowness" | "long_slowness" | "strong_slowness" => 8234,
        "leaping" | "long_leaping" | "strong_leaping" => 8203,
        "harming" | "strong_harming" => 8268,
        "water_breathing" | "long_water_breathing" => 8237,
        "invisibility" | "long_invisibility" => 8238,
        _ => 0,
    }
}

/// A 26.3 item stack as a 1.8 slot: id (-1 for none), count, damage, NBT (0 for none).
pub fn item(tables: &Tables, stack: Option<&ItemStack>) -> Vec<u8> {
    let mut w = Vec::new();
    let Some(stack) = stack.filter(|s| s.count > 0) else {
        w.put_i16(-1);
        return w;
    };
    let name = paper_data::items::get(stack.item as u32).map_or("", |i| i.name);
    let (id, mut damage) = tables.item(stack.item);
    let mut nbt = Compound::default();
    let mut display = Compound::default();
    if name.ends_with("_spawn_egg") {
        if let Some(tables::EntityKind::Mob(mob)) = tables::entity(&name.replace("_spawn_egg", "")) {
            damage = mob as i16;
        }
    }
    if let Some(v) = value(stack, "minecraft:damage") {
        damage = Reader::new(v).varint().unwrap_or(0) as i16;
    }
    for key in ["minecraft:custom_name", "minecraft:item_name"] {
        if display.get("Name").is_none() {
            if let Some(tag) = value(stack, key).and_then(|v| Reader::new(v).nbt().ok().flatten()) {
                display.insert("Name", Tag::String(text::legacy_string(&paper_protocol::text::Component::from_nbt(&tag).0)));
            }
        }
    }
    if let Some(v) = value(stack, "minecraft:lore") {
        let mut r = Reader::new(v);
        if let Ok(lines) = r.list(256, |r| r.nbt()) {
            let lines = lines.into_iter().flatten().map(|t| Tag::String(text::legacy_string(&paper_protocol::text::Component::from_nbt(&t).0))).collect();
            display.insert("Lore", Tag::List(lines));
        }
    }
    if let Some(v) = value(stack, "minecraft:dyed_color") {
        if v.len() == 4 {
            display.insert("color", Tag::Int(i32::from_be_bytes([v[0], v[1], v[2], v[3]])));
        }
    }
    for (key, nbt_key) in [("minecraft:enchantments", "ench"), ("minecraft:stored_enchantments", "StoredEnchantments")] {
        let Some(v) = value(stack, key) else { continue };
        let Ok(list) = Reader::new(v).list(256, |r| Ok((r.varint()?, r.varint()?))) else { continue };
        let entries: Vec<Tag> = list
            .into_iter()
            .filter_map(|(e, level)| {
                let name = paper_data::synced::entries("minecraft:enchantment")?.get(e as usize)?;
                let id = tables::enchantment(name)?;
                let mut c = Compound::default();
                c.insert("id", Tag::Short(id));
                c.insert("lvl", Tag::Short(level as i16));
                Some(Tag::Compound(c))
            })
            .collect();
        if !entries.is_empty() {
            nbt.insert(nbt_key, Tag::List(entries));
        }
    }
    if let Some(v) = value(stack, "minecraft:potion_contents") {
        let mut r = Reader::new(v);
        if let Ok(Some(potion)) = r.option(|r| r.varint()) {
            let potion = paper_data::registries::entries("minecraft:potion").and_then(|e| e.get(potion as usize).copied()).unwrap_or("");
            let base = potion_damage(potion);
            damage = if name == "minecraft:splash_potion" || name == "minecraft:lingering_potion" { (base & !8192) | 16384 } else { base };
        }
    }
    if value(stack, "minecraft:unbreakable").is_some() {
        nbt.insert("Unbreakable", Tag::Byte(1));
    }
    if !display.0.is_empty() {
        nbt.insert("display", Tag::Compound(display));
    }
    w.put_i16(id);
    w.put_i8(stack.count.clamp(1, 127) as i8);
    w.put_i16(damage);
    if nbt.0.is_empty() {
        w.put_u8(0);
    } else {
        // A named root compound, with an empty name.
        let mut net = Vec::new();
        paper_nbt::write_network_compound(&mut net, &nbt);
        w.put_u8(10);
        w.put_u16(0);
        w.put_slice(&net[1..]);
    }
    w
}

/// How a 26.3 window shows on 1.8: its type, its slot count and how its slots move.
pub(super) struct Window {
    pub kind: &'static str,
    pub slots: u8,
    /// A 26.3 slot that 1.8 does not have (brewing stands' fuel); the ones after it move
    /// down by one.
    pub dropped: Option<i16>,
}

pub(super) fn window(menu: i32) -> Option<Window> {
    let name = paper_data::registries::entries("minecraft:menu").and_then(|e| e.get(menu as usize).copied()).unwrap_or("");
    let w = |kind, slots| Some(Window { kind, slots, dropped: None });
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "generic_9x1" => w("minecraft:chest", 9),
        "generic_9x2" => w("minecraft:chest", 18),
        "generic_9x3" | "shulker_box" => w("minecraft:chest", 27),
        "generic_9x4" => w("minecraft:chest", 36),
        "generic_9x5" => w("minecraft:chest", 45),
        "generic_9x6" => w("minecraft:chest", 54),
        "generic_3x3" => w("minecraft:dispenser", 9),
        "anvil" => w("minecraft:anvil", 0),
        "beacon" => w("minecraft:beacon", 1),
        "furnace" | "blast_furnace" | "smoker" => w("minecraft:furnace", 3),
        "brewing_stand" => Some(Window { kind: "minecraft:brewing_stand", slots: 4, dropped: Some(4) }),
        "crafting" => w("minecraft:crafting_table", 0),
        "enchantment" => w("minecraft:enchanting_table", 0),
        "hopper" => w("minecraft:hopper", 5),
        "merchant" => w("minecraft:villager", 3),
        _ => None,
    }
}

impl Legacy18 {
    /// A 26.3 window slot as 1.8's, if 1.8 has it.
    pub(super) fn slot_to_1_8(&self, window: i32, slot: i16) -> Option<i16> {
        if window == 0 {
            return (slot < 45).then_some(slot);
        }
        let dropped = self.window.filter(|(id, _)| *id == window).and_then(|(_, menu)| window_of(menu)).and_then(|w| w.dropped);
        match dropped {
            Some(d) if slot == d => None,
            Some(d) if slot > d => Some(slot - 1),
            _ => Some(slot),
        }
    }

    /// A 1.8 window slot as 26.3's.
    pub(super) fn slot_from_1_8(&self, window: i32, slot: i16) -> i16 {
        let dropped = self.window.filter(|(id, _)| *id == window && window != 0).and_then(|(_, menu)| window_of(menu)).and_then(|w| w.dropped);
        match dropped {
            Some(d) if slot >= d => slot + 1,
            _ => slot,
        }
    }

    pub(super) fn inventory_clientbound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        let mut r = Reader::new(body);
        let mut w;
        match id {
            p::OPEN_SCREEN => {
                let window_id = r.varint()?;
                let menu = r.varint()?;
                let title = r.component()?;
                let Some(window) = window(menu) else {
                    // A window 1.8 has no layout for: closed again at once.
                    let mut close = packet(paper_data::packets::play::serverbound::CONTAINER_CLOSE);
                    close.put_varint(window_id);
                    out.to_server.push(close);
                    return Ok(());
                };
                self.window = Some((window_id, menu));
                w = packet(cb::OPEN_WINDOW);
                w.put_u8(window_id as u8);
                w.put_string(window.kind);
                w.put_string(&text::json_1_8(&title.0));
                w.put_u8(window.slots);
            }
            p::CONTAINER_SET_CONTENT => {
                let window_id = r.varint()?;
                self.state_id = r.varint()?;
                let count = r.varint()?;
                let mut slots = Vec::new();
                for index in 0..count {
                    let stack = crate::components::read_slot(&mut r)?;
                    if self.slot_to_1_8(window_id, index as i16).is_some() {
                        slots.push(item(self.tables, stack.as_ref()));
                    }
                }
                let carried = crate::components::read_slot(&mut r)?;
                w = packet(cb::WINDOW_ITEMS);
                w.put_u8(window_id as u8);
                w.put_i16(slots.len() as i16);
                for slot in slots {
                    w.put_slice(&slot);
                }
                out.to_client.push(w);
                w = packet(cb::SET_SLOT);
                w.put_i8(-1);
                w.put_i16(-1);
                w.put_slice(&item(self.tables, carried.as_ref()));
            }
            p::CONTAINER_SET_SLOT => {
                let window_id = r.varint()?;
                self.state_id = r.varint()?;
                let slot = r.i16()?;
                let stack = crate::components::read_slot(&mut r)?;
                let Some(slot) = self.slot_to_1_8(window_id, slot) else { return Ok(()) };
                w = packet(cb::SET_SLOT);
                w.put_i8(window_id as i8);
                w.put_i16(slot);
                w.put_slice(&item(self.tables, stack.as_ref()));
            }
            p::SET_CURSOR_ITEM => {
                let stack = crate::components::read_slot(&mut r)?;
                w = packet(cb::SET_SLOT);
                w.put_i8(-1);
                w.put_i16(-1);
                w.put_slice(&item(self.tables, stack.as_ref()));
            }
            p::SET_PLAYER_INVENTORY => {
                let index = r.varint()?;
                let stack = crate::components::read_slot(&mut r)?;
                let slot = match index {
                    0..=8 => index + 36,
                    9..=35 => index,
                    36..=39 => 44 - index,
                    _ => return Ok(()),
                };
                w = packet(cb::SET_SLOT);
                w.put_i8(0);
                w.put_i16(slot as i16);
                w.put_slice(&item(self.tables, stack.as_ref()));
            }
            p::CONTAINER_SET_DATA => {
                let window_id = r.varint()?;
                let property = r.i16()?;
                let value = r.i16()?;
                let brewing = self.window.filter(|(id, _)| *id == window_id).and_then(|(_, m)| window_of(m)).is_some_and(|w| w.dropped.is_some());
                if brewing && property != 0 {
                    return Ok(());
                }
                w = packet(cb::CRAFT_PROGRESS_BAR);
                w.put_u8(window_id as u8);
                w.put_i16(property);
                w.put_i16(value);
            }
            p::CONTAINER_CLOSE => {
                let window_id = r.varint()?;
                self.window = None;
                w = packet(cb::CLOSE_WINDOW);
                w.put_u8(window_id as u8);
            }
            _ => return Ok(()),
        }
        out.to_client.push(w);
        Ok(())
    }
}

fn window_of(menu: i32) -> Option<Window> {
    window(menu)
}

/// A 1.8 slot (as a client sends it, in creative mode) as a 26.3 untrusted slot.
pub fn item_from_1_8(tables: &Tables, r: &mut Reader<'_>, w: &mut Vec<u8>) -> Result<()> {
    let id = r.i16()?;
    if id < 0 {
        w.put_varint(0);
        return Ok(());
    }
    let count = r.i8()?;
    let damage = r.i16()?;
    let nbt = read_named_nbt(r)?;
    let item = tables.item_new(id, damage);
    let mut components: Vec<(i32, Vec<u8>)> = Vec::new();
    let damageable = paper_data::item_data::by_id(item as u32).max_damage.unwrap_or(0) > 0;
    if damageable && damage > 0 {
        let mut v = Vec::new();
        v.put_varint(damage as i32);
        components.push((component("minecraft:damage"), v));
    }
    if let Some(Tag::Compound(root)) = &nbt {
        if let Some(Tag::Compound(display)) = root.get("display") {
            if let Some(Tag::String(name)) = display.get("Name") {
                let mut v = Vec::new();
                paper_nbt::write_network(&mut v, &Tag::String(name.clone()));
                components.push((component("minecraft:custom_name"), v));
            }
        }
        if let Some(Tag::List(list)) = root.get("ench") {
            let mut v = Vec::new();
            let entries: Vec<(i32, i32)> = list
                .iter()
                .filter_map(|e| {
                    let c = e.as_compound()?;
                    let legacy = match c.get("id")? {
                        Tag::Short(s) => *s,
                        Tag::Int(i) => *i as i16,
                        _ => return None,
                    };
                    let name = paper_data::synced::entries("minecraft:enchantment")?.iter().find(|n| tables::enchantment(n) == Some(legacy))?;
                    let id = paper_data::synced::id("minecraft:enchantment", name)?;
                    let level = match c.get("lvl")? {
                        Tag::Short(s) => *s as i32,
                        Tag::Int(i) => *i,
                        _ => 1,
                    };
                    Some((id, level))
                })
                .collect();
            v.put_varint(entries.len() as i32);
            for (id, level) in entries {
                v.put_varint(id);
                v.put_varint(level);
            }
            components.push((component("minecraft:enchantments"), v));
        }
    }
    w.put_varint(count.max(1) as i32);
    w.put_varint(item);
    w.put_varint(components.len() as i32);
    w.put_varint(0);
    for (ty, value) in components {
        w.put_varint(ty);
        w.put_varint(value.len() as i32);
        w.put_slice(&value);
    }
    Ok(())
}

/// A named root NBT compound as 1.8 sends it (0 for none).
fn read_named_nbt(r: &mut Reader<'_>) -> Result<Option<Tag>> {
    let ty = r.u8()?;
    if ty == 0 {
        return Ok(None);
    }
    let name_len = r.u16()? as usize;
    r.take(name_len)?;
    // The rest is a network compound's payload: put the type back in front.
    let rest = r.peek();
    let mut buf = Vec::with_capacity(rest.len() + 1);
    buf.push(ty);
    buf.extend_from_slice(rest);
    let mut slice: &[u8] = &buf;
    let tag = paper_nbt::read_network(&mut slice, 1 << 21)?;
    let used = buf.len() - slice.len() - 1;
    r.take(used)?;
    Ok(tag)
}
