//! Item stacks and their data components as 26.3 sends them, walked value by value: a
//! client of another version gets every id inside them in its own terms, and the component
//! types it does not have are left out. The formats are ViaVersion's (`StructuredDataKey`, the
//! 26.3 types of `VersionedTypes.V26_3`); 26.2 and 26.1 encode every component the same way but
//! for the few marked below.

use crate::io::{Result, Rw};
use anyhow::bail;
use paper_protocol::codec::{Reader, WriteExt};

/// The id conversions of one direction of a translation.
pub trait Ids {
    fn item(&self, id: i32) -> i32;
    fn block_state(&self, id: i32) -> i32;
    /// A data component type (-1: the other side has none).
    fn component(&self, id: i32) -> i32;
    /// An entry of another registry (by its 26.3 name).
    fn registry(&self, registry: &str, id: i32) -> i32;
    /// Whether this turns 26.3 into an older version (else an older version into 26.3: the
    /// values read are the older version's).
    fn to_old(&self) -> bool;
    /// The protocol of the older version (whose formats are written or read).
    fn protocol(&self) -> i32 {
        crate::NATIVE_PROTOCOL
    }
}

/// The versions whose component formats differ, by protocol: a client older than one has the
/// formats of before it.
const V26_1: i32 = 775;
const V1_21_11: i32 = 774;
const V1_21_9: i32 = 773;
const V1_21_6: i32 = 771;

/// A registry value as an id (26.1 on) or, before, either an id (`true`) or a name.
fn either_id(ids: &dyn Ids, rw: &mut Rw<'_>, registry: &str) -> Result<bool> {
    if ids.protocol() >= V26_1 {
        id(ids, rw, registry)?;
        return Ok(true);
    }
    if ids.to_old() {
        rw.w.put_bool(true);
        id(ids, rw, registry)?;
        return Ok(true);
    }
    if rw.r.bool()? {
        id(ids, rw, registry)?;
        return Ok(true);
    }
    let name = rw.r.string(32767)?;
    match paper_data::synced::id(registry, &name) {
        Some(id) => {
            rw.w.put_varint(id as i32);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// A holder (26.1 on) or, before, either a holder (`true`) or a name.
fn either_holder(ids: &dyn Ids, rw: &mut Rw<'_>, registry: &str, inline: impl FnOnce(&mut Rw<'_>) -> Result<()>) -> Result<bool> {
    if ids.protocol() >= V26_1 {
        holder(ids, rw, registry, inline)?;
        return Ok(true);
    }
    if ids.to_old() {
        rw.w.put_bool(true);
        holder(ids, rw, registry, inline)?;
        return Ok(true);
    }
    if rw.r.bool()? {
        holder(ids, rw, registry, inline)?;
        return Ok(true);
    }
    let name = rw.r.string(32767)?;
    let id = paper_data::synced::id(registry, &name).or_else(|| paper_data::registries::id(registry, &name).map(|id| id as i32));
    match id {
        Some(id) => {
            rw.w.put_varint(id as i32 + 1);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// A holder set (26.1 on) or, before, a tag's name: a set of ids has no older form.
fn tag_or_holder_set(ids: &dyn Ids, rw: &mut Rw<'_>, registry: &str) -> Result<bool> {
    if ids.protocol() >= V26_1 {
        holder_set(ids, rw, registry)?;
        return Ok(true);
    }
    if ids.to_old() {
        let n = rw.r.varint()?;
        if n == 0 {
            rw.string()?;
            return Ok(true);
        }
        for _ in 0..n - 1 {
            rw.r.varint()?;
        }
        return Ok(false);
    }
    rw.w.put_varint(0);
    rw.string()?;
    Ok(true)
}

/// An entity's or block entity's data: 1.21.9 gave the type ahead of the NBT, which held it
/// as `id` before.
fn typed_nbt(ids: &dyn Ids, rw: &mut Rw<'_>, registry: &str) -> Result<()> {
    if ids.protocol() >= V1_21_9 {
        id(ids, rw, registry)?;
        return rw.nbt();
    }
    if ids.to_old() {
        let kind = rw.r.varint()?;
        let name = paper_data::registries::entries(registry).and_then(|e| e.get(kind as usize).copied()).unwrap_or("");
        let mut tag = match rw.r.nbt()? {
            Some(paper_nbt::Tag::Compound(c)) => c,
            _ => paper_nbt::Compound::default(),
        };
        tag.insert("id", paper_nbt::Tag::String(name.to_owned()));
        paper_nbt::write_network_compound(&mut rw.w, &tag);
        return Ok(());
    }
    let mut tag = match rw.r.nbt()? {
        Some(paper_nbt::Tag::Compound(c)) => c,
        _ => paper_nbt::Compound::default(),
    };
    let kind = tag.remove("id").and_then(|t| t.as_str().map(str::to_owned)).and_then(|name| paper_data::registries::id(registry, &name)).unwrap_or(0);
    rw.w.put_varint(kind as i32);
    paper_nbt::write_network_compound(&mut rw.w, &tag);
    Ok(())
}

/// The 26.3 name of a data component type, for a 26.3 id.
fn component_name(id: i32) -> &'static str {
    paper_data::registries::entries("minecraft:data_component_type").and_then(|e| e.get(id as usize).copied()).unwrap_or("")
}

/// A slot in the trusted encoding: count (0: empty), item, then the component patch.
pub fn slot(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    let count = rw.varint()?;
    if count <= 0 {
        return Ok(());
    }
    item_id(ids, rw)?;
    patch(ids, rw, false)
}

/// An item template: item, count, then the component patch.
pub fn template(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    item_id(ids, rw)?;
    rw.varint()?;
    patch(ids, rw, false)
}

fn optional_template(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    if rw.bool()? {
        template(ids, rw)?;
    }
    Ok(())
}

/// A slot in the untrusted encoding clients send (each value prefixed by its length).
pub fn untrusted_slot(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    let count = rw.varint()?;
    if count <= 0 {
        return Ok(());
    }
    item_id(ids, rw)?;
    patch(ids, rw, true)
}

fn item_id(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    rw.map_varint(|item| {
        let mapped = ids.item(item);
        // An item the other side lacks: stone, rather than a broken stack.
        if mapped < 0 {
            1
        } else {
            mapped
        }
    })?;
    Ok(())
}

/// A component patch: the added components (type and value), then the removed types.
/// `length_prefixed`: the untrusted form, where each value has its length before it.
fn patch(ids: &dyn Ids, rw: &mut Rw<'_>, length_prefixed: bool) -> Result<()> {
    let added = rw.r.len(256)?;
    let removed = rw.r.len(256)?;
    let mut kept_added = Vec::new();
    let mut count_added = 0;
    for _ in 0..added {
        let ty = rw.r.varint()?;
        let mapped = ids.component(ty);
        // The format is that of the side being read.
        let format = if ids.to_old() { ty } else { mapped };
        if length_prefixed {
            let len = rw.r.len(1 << 21)?;
            let bytes = rw.r.take(len)?;
            if mapped < 0 {
                continue;
            }
            let mut inner = Rw { r: Reader::new(bytes), w: Vec::new() };
            if component(ids, &mut inner, format)? {
                kept_added.put_varint(mapped);
                kept_added.put_varint(inner.w.len() as i32);
                kept_added.extend_from_slice(&inner.w);
                count_added += 1;
            }
            continue;
        }
        if mapped < 0 && !ids.to_old() {
            bail!("component type {ty} of the client has no 26.3 counterpart");
        }
        let mut value = Rw { r: rw.r.clone(), w: Vec::new() };
        let keep = component(ids, &mut value, format)?;
        rw.r = value.r;
        if mapped >= 0 && keep {
            kept_added.put_varint(mapped);
            kept_added.extend_from_slice(&value.w);
            count_added += 1;
        }
    }
    let mut kept_removed = Vec::new();
    let mut count_removed = 0;
    for _ in 0..removed {
        let mapped = ids.component(rw.r.varint()?);
        if mapped >= 0 {
            kept_removed.put_varint(mapped);
            count_removed += 1;
        }
    }
    rw.w.put_varint(count_added);
    rw.w.put_varint(count_removed);
    rw.w.extend_from_slice(&kept_added);
    rw.w.extend_from_slice(&kept_removed);
    Ok(())
}

fn list(rw: &mut Rw<'_>, mut each: impl FnMut(&mut Rw<'_>) -> Result<()>) -> Result<()> {
    let n = rw.varint()?;
    for _ in 0..n {
        each(rw)?;
    }
    Ok(())
}

fn optional(rw: &mut Rw<'_>, each: impl FnOnce(&mut Rw<'_>) -> Result<()>) -> Result<()> {
    if rw.bool()? {
        each(rw)?;
    }
    Ok(())
}

/// A bare registry id.
fn id(ids: &dyn Ids, rw: &mut Rw<'_>, registry: &str) -> Result<()> {
    rw.map_varint(|v| ids.registry(registry, v).max(0))?;
    Ok(())
}

/// A holder: a registry id plus one, or 0 and an inline value.
fn holder(ids: &dyn Ids, rw: &mut Rw<'_>, registry: &str, inline: impl FnOnce(&mut Rw<'_>) -> Result<()>) -> Result<()> {
    let h = rw.r.varint()?;
    if h == 0 {
        rw.w.put_varint(0);
        return inline(rw);
    }
    rw.w.put_varint(ids.registry(registry, h - 1).max(0) + 1);
    Ok(())
}

/// A holder set of `registry`: 0 and a tag name, or the count plus one and the ids.
pub fn holder_set(ids: &dyn Ids, rw: &mut Rw<'_>, registry: &str) -> Result<()> {
    let n = rw.r.varint()?;
    if n == 0 {
        rw.w.put_varint(0);
        rw.string()?;
        return Ok(());
    }
    let mut kept = Vec::new();
    for _ in 0..n - 1 {
        let id = rw.r.varint()?;
        let mapped = if registry == "minecraft:item" { ids.item(id) } else { ids.registry(registry, id) };
        if mapped >= 0 {
            kept.push(mapped);
        }
    }
    rw.w.put_varint(kept.len() as i32 + 1);
    for id in kept {
        rw.w.put_varint(id);
    }
    Ok(())
}

/// A sound event holder: an id plus one, or 0, a name and an optional fixed range.
pub fn sound(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    let h = rw.r.varint()?;
    if h == 0 {
        rw.w.put_varint(0);
        rw.string()?;
        return optional(rw, |rw| rw.f32().map(drop));
    }
    let mapped = ids.registry("minecraft:sound_event", h - 1);
    if mapped >= 0 {
        rw.w.put_varint(mapped + 1);
    } else {
        // A sound the other side lacks, by name: it plays nothing if it does not know it.
        let name = paper_data::registries::entries("minecraft:sound_event").and_then(|e| e.get((h - 1) as usize).copied()).unwrap_or("minecraft:intentionally_empty");
        rw.w.put_varint(0);
        rw.w.put_string(name);
        rw.w.put_bool(false);
    }
    Ok(())
}

fn text(rw: &mut Rw<'_>) -> Result<()> {
    rw.nbt()
}

fn effect_details(rw: &mut Rw<'_>) -> Result<()> {
    rw.varint()?;
    rw.varint()?;
    rw.bool()?;
    rw.bool()?;
    rw.bool()?;
    optional(rw, effect_details)
}

fn mob_effect_instance(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    id(ids, rw, "minecraft:mob_effect")?;
    effect_details(rw)
}

/// A consume effect. 26.3 gave `teleport_randomly` a flag after its diameter.
fn consume_effect(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    let ty = rw.varint()?;
    match ty {
        0 => {
            list(rw, |rw| mob_effect_instance(ids, rw))?;
            rw.f32()?;
        }
        1 => holder_set(ids, rw, "minecraft:mob_effect")?,
        2 => {}
        3 => {
            rw.f32()?;
            if ids.to_old() {
                rw.r.bool()?;
            } else {
                rw.w.put_bool(false);
            }
        }
        4 => sound(ids, rw)?,
        other => bail!("consume effect type {other}"),
    }
    Ok(())
}

fn firework_explosion(rw: &mut Rw<'_>) -> Result<()> {
    rw.varint()?;
    list(rw, |rw| rw.i32().map(drop))?;
    list(rw, |rw| rw.i32().map(drop))?;
    rw.bool()?;
    rw.bool()?;
    Ok(())
}

fn filterable_string(rw: &mut Rw<'_>) -> Result<()> {
    rw.string()?;
    optional(rw, |rw| rw.string().map(drop))
}

fn filterable_text(rw: &mut Rw<'_>) -> Result<()> {
    text(rw)?;
    optional(rw, text)
}

fn resolvable_int(rw: &mut Rw<'_>) -> Result<()> {
    if rw.bool()? {
        rw.i32()?;
    } else {
        rw.string()?;
    }
    Ok(())
}

fn resolvable_float(rw: &mut Rw<'_>) -> Result<()> {
    if rw.bool()? {
        rw.f32()?;
    } else {
        rw.string()?;
    }
    Ok(())
}

/// A resolvable profile (a player head's owner, a mannequin's look).
pub fn resolvable_profile(rw: &mut Rw<'_>) -> Result<()> {
    let property = |rw: &mut Rw<'_>| -> Result<()> {
        rw.string()?;
        rw.string()?;
        optional(rw, |rw| rw.string().map(drop))
    };
    if rw.bool()? {
        rw.uuid()?;
        rw.string()?;
        list(rw, property)?;
    } else {
        optional(rw, |rw| rw.string().map(drop))?;
        optional(rw, |rw| rw.uuid().map(drop))?;
        list(rw, property)?;
    }
    for _ in 0..3 {
        optional(rw, |rw| rw.string().map(drop))?;
    }
    optional(rw, |rw| rw.bool().map(drop))
}

/// A player head's profile: resolvable from 1.21.9 (a whole or partial profile, then skin
/// overrides), before a plain game profile (optional name, optional id, properties).
fn profile(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    if ids.protocol() >= V1_21_9 {
        return resolvable_profile(rw);
    }
    let property = |rw: &mut Rw<'_>| -> Result<()> {
        rw.string()?;
        rw.string()?;
        optional(rw, |rw| rw.string().map(drop))
    };
    if !ids.to_old() {
        // An older client's game profile is a partial one, without overrides.
        rw.w.put_bool(false);
        optional(rw, |rw| rw.string().map(drop))?;
        optional(rw, |rw| rw.uuid().map(drop))?;
        list(rw, property)?;
        for _ in 0..4 {
            rw.w.put_bool(false);
        }
        return Ok(());
    }
    if rw.r.bool()? {
        let uuid = rw.r.uuid()?;
        let name = rw.r.string(32767)?;
        rw.w.put_bool(true);
        rw.w.put_string(&name);
        rw.w.put_bool(true);
        rw.w.put_uuid(uuid);
        list(rw, property)?;
    } else {
        optional(rw, |rw| rw.string().map(drop))?;
        optional(rw, |rw| rw.uuid().map(drop))?;
        list(rw, property)?;
    }
    // The overrides have no older form.
    let mut skip = Rw { r: rw.r.clone(), w: Vec::new() };
    for _ in 0..3 {
        optional(&mut skip, |rw| rw.string().map(drop))?;
    }
    optional(&mut skip, |rw| rw.bool().map(drop))?;
    rw.r = skip.r;
    Ok(())
}

/// A painting variant holder (entity data of paintings).
pub fn painting_variant(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    holder(ids, rw, "minecraft:painting_variant", |rw| {
        rw.varint()?;
        rw.varint()?;
        rw.string()?;
        optional(rw, text)?;
        optional(rw, text)
    })
}

/// A component inside an exact matcher: its type and value.
fn typed_component(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<bool> {
    let ty = rw.r.varint()?;
    let mapped = ids.component(ty);
    let format = if ids.to_old() { ty } else { mapped };
    let mut value = Rw { r: rw.r.clone(), w: Vec::new() };
    let keep = component(ids, &mut value, format)?;
    rw.r = value.r;
    if mapped >= 0 && keep {
        rw.w.put_varint(mapped);
        rw.w.extend_from_slice(&value.w);
        return Ok(true);
    }
    Ok(false)
}

/// A block predicate of `can_place_on`/`can_break`.
fn block_predicate(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    optional(rw, |rw| holder_set(ids, rw, "minecraft:block"))?;
    optional(rw, |rw| {
        list(rw, |rw| {
            rw.string()?;
            if rw.bool()? {
                rw.string()?;
            } else {
                optional(rw, |rw| rw.string().map(drop))?;
                optional(rw, |rw| rw.string().map(drop))?;
            }
            Ok(())
        })
    })?;
    optional(rw, |rw| rw.nbt())?;
    // Exact components: only those the other side has.
    let n = rw.r.len(256)?;
    let mut kept = Vec::new();
    let mut count = 0;
    for _ in 0..n {
        let mut entry = Rw { r: rw.r.clone(), w: Vec::new() };
        if typed_component(ids, &mut entry)? {
            kept.extend_from_slice(&entry.w);
            count += 1;
        }
        rw.r = entry.r;
    }
    rw.w.put_varint(count);
    rw.w.extend_from_slice(&kept);
    // Partial predicates: a predicate type or a component type, then NBT.
    list(rw, |rw| {
        let predicate = rw.bool()?;
        if predicate {
            rw.varint()?;
        } else {
            rw.map_varint(|ty| ids.component(ty).max(0))?;
        }
        rw.nbt()
    })
}

/// One component value. `ty` is the id of its type on the side being read (26.3's when
/// translating to an older version, else the older version's mapped to 26.3). Returns
/// whether the value is kept (else it was read and is left out).
fn component(ids: &dyn Ids, rw: &mut Rw<'_>, ty: i32) -> Result<bool> {
    let name = component_name(ty);
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "custom_data" | "map_decorations" | "debug_stick_state" | "bucket_entity_data" | "lock" | "container_loot" | "recipes" | "intangible_projectile" => rw.nbt()?,
        "max_stack_size" | "max_damage" | "damage" | "repair_cost" | "rarity" | "enchantable" | "map_id" | "map_post_processing" | "ominous_bottle_amplifier" | "additional_trade_cost" | "villager_food" | "dye" | "base_color" | "wolf/collar"
        | "fox/variant" | "salmon/size" | "parrot/variant" | "tropical_fish/pattern" | "tropical_fish/base_color" | "tropical_fish/pattern_color" | "mooshroom/variant" | "rabbit/variant" | "horse/variant" | "llama/variant"
        | "axolotl/variant" | "cat/collar" | "sheep/color" | "shulker/color" | "cushion/color" => {
            rw.varint()?;
        }
        "unbreakable" | "creative_slot_lock" | "glider" | "waxed" => {}
        "use_effects" => {
            rw.bool()?;
            rw.bool()?;
            rw.f32()?;
        }
        "custom_name" | "item_name" => text(rw)?,
        "minimum_attack_charge" | "potion_duration_scale" => {
            rw.f32()?;
        }
        "damage_type" => return either_id(ids, rw, "minecraft:damage_type"),
        "item_model" | "tooltip_style" | "note_block_sound" => {
            rw.string()?;
        }
        "lore" => list(rw, text)?,
        "enchantments" | "stored_enchantments" => list(rw, |rw| {
            id(ids, rw, "minecraft:enchantment")?;
            rw.varint().map(drop)
        })?,
        "can_place_on" | "can_break" => list(rw, |rw| block_predicate(ids, rw))?,
        "attribute_modifiers" => {
            // 1.21.6 gave each modifier how it is displayed.
            let display = ids.protocol() >= V1_21_6;
            let n = rw.r.len(1024)?;
            let mut kept = Vec::new();
            let mut count = 0;
            for _ in 0..n {
                let attribute = rw.r.varint()?;
                let mut entry = Rw { r: rw.r.clone(), w: Vec::new() };
                entry.string()?;
                entry.f64()?;
                entry.varint()?;
                entry.varint()?;
                if display {
                    if entry.varint()? == 2 {
                        text(&mut entry)?;
                    }
                } else if ids.to_old() {
                    let mark = entry.w.len();
                    if entry.varint()? == 2 {
                        text(&mut entry)?;
                    }
                    entry.w.truncate(mark);
                } else {
                    entry.w.put_varint(0);
                }
                rw.r = entry.r;
                let mapped = ids.registry("minecraft:attribute", attribute);
                if mapped >= 0 {
                    kept.put_varint(mapped);
                    kept.extend_from_slice(&entry.w);
                    count += 1;
                }
            }
            rw.w.put_varint(count);
            rw.w.extend_from_slice(&kept);
        }
        "custom_model_data" => {
            list(rw, |rw| rw.f32().map(drop))?;
            list(rw, |rw| rw.bool().map(drop))?;
            list(rw, |rw| rw.string().map(drop))?;
            list(rw, |rw| rw.i32().map(drop))?;
        }
        "tooltip_display" => {
            rw.bool()?;
            let hidden = rw.r.list(256, |r| r.varint())?;
            let hidden: Vec<i32> = hidden.into_iter().map(|ty| ids.component(ty)).filter(|ty| *ty >= 0).collect();
            rw.w.put_varint(hidden.len() as i32);
            for ty in hidden {
                rw.w.put_varint(ty);
            }
        }
        "enchantment_glint_override" => {
            rw.bool()?;
        }
        "food" => {
            rw.varint()?;
            rw.f32()?;
            rw.bool()?;
        }
        "consumable" => {
            rw.f32()?;
            // 1.21.11 added the spear's animation (11).
            rw.map_varint(|animation| if animation == 11 && ids.to_old() && ids.protocol() < V1_21_11 { 0 } else { animation })?;
            sound(ids, rw)?;
            rw.bool()?;
            list(rw, |rw| consume_effect(ids, rw))?;
        }
        "death_protection" => list(rw, |rw| consume_effect(ids, rw))?,
        "use_remainder" | "sulfur_cube_content" => template(ids, rw)?,
        "use_cooldown" => {
            rw.f32()?;
            optional(rw, |rw| rw.string().map(drop))?;
        }
        "damage_resistant" => return tag_or_holder_set(ids, rw, "minecraft:damage_type"),
        "tool" => {
            list(rw, |rw| {
                holder_set(ids, rw, "minecraft:block")?;
                optional(rw, |rw| rw.f32().map(drop))?;
                optional(rw, |rw| rw.bool().map(drop))
            })?;
            rw.f32()?;
            rw.varint()?;
            rw.bool()?;
        }
        "weapon" => {
            rw.varint()?;
            rw.f32()?;
        }
        "attack_range" => rw.bytes(24)?,
        "equippable" => {
            rw.varint()?;
            sound(ids, rw)?;
            optional(rw, |rw| rw.string().map(drop))?;
            optional(rw, |rw| rw.string().map(drop))?;
            optional(rw, |rw| holder_set(ids, rw, "minecraft:entity_type"))?;
            rw.bytes(4)?;
            // 1.21.6 added whether it can be sheared off, and the sound of it.
            if ids.protocol() >= V1_21_6 {
                rw.bytes(1)?;
                sound(ids, rw)?;
            } else if ids.to_old() {
                rw.r.bool()?;
                let mut skip = Rw { r: rw.r.clone(), w: Vec::new() };
                sound(ids, &mut skip)?;
                rw.r = skip.r;
            } else {
                rw.w.put_bool(false);
                let snip = paper_data::registries::id("minecraft:sound_event", "minecraft:item.shears.snip").map_or(0, |id| id as i32 + 1);
                rw.w.put_varint(snip);
                if snip == 0 {
                    rw.w.put_string("minecraft:item.shears.snip");
                    rw.w.put_bool(false);
                }
            }
        }
        "repairable" => holder_set(ids, rw, "minecraft:item")?,
        "blocks_attacks" => {
            rw.f32()?;
            rw.f32()?;
            list(rw, |rw| {
                rw.f32()?;
                optional(rw, |rw| holder_set(ids, rw, "minecraft:damage_type"))?;
                rw.f32()?;
                rw.f32().map(drop)
            })?;
            rw.bytes(12)?;
            // What gets past it: a tag's name before 26.1, a holder set from it.
            if ids.protocol() >= V26_1 || !ids.to_old() {
                if ids.protocol() >= V26_1 {
                    optional(rw, |rw| holder_set(ids, rw, "minecraft:damage_type"))?;
                } else if rw.r.bool()? {
                    rw.w.put_bool(true);
                    rw.w.put_varint(0);
                    rw.string()?;
                } else {
                    rw.w.put_bool(false);
                }
            } else if rw.r.bool()? {
                let mut set = Rw { r: rw.r.clone(), w: Vec::new() };
                let n = set.r.varint()?;
                if n == 0 {
                    let tag = set.r.string(32767)?;
                    rw.w.put_bool(true);
                    rw.w.put_string(&tag);
                } else {
                    for _ in 0..n - 1 {
                        set.r.varint()?;
                    }
                    rw.w.put_bool(false);
                }
                rw.r = set.r;
            } else {
                rw.w.put_bool(false);
            }
            optional(rw, |rw| sound(ids, rw))?;
            optional(rw, |rw| sound(ids, rw))?;
        }
        "piercing_weapon" => {
            rw.bool()?;
            rw.bool()?;
            optional(rw, |rw| sound(ids, rw))?;
            optional(rw, |rw| sound(ids, rw))?;
        }
        "kinetic_weapon" => {
            rw.varint()?;
            rw.varint()?;
            for _ in 0..3 {
                optional(rw, |rw| {
                    rw.varint()?;
                    rw.f32()?;
                    rw.f32().map(drop)
                })?;
            }
            rw.f32()?;
            rw.f32()?;
            optional(rw, |rw| sound(ids, rw))?;
            optional(rw, |rw| sound(ids, rw))?;
        }
        "attack_animation" | "interact_animation" | "swing_animation" => {
            rw.varint()?;
            rw.varint()?;
        }
        "block_transformer" => id(ids, rw, "minecraft:block_transformer")?,
        "provides_pottery_pattern" => id(ids, rw, "minecraft:decorated_pot_pattern")?,
        "dyed_color" => {
            rw.i32()?;
        }
        "charged_projectiles" | "bundle_contents" => list(rw, |rw| template(ids, rw))?,
        "potion_contents" => {
            optional(rw, |rw| id(ids, rw, "minecraft:potion"))?;
            optional(rw, |rw| rw.i32().map(drop))?;
            list(rw, |rw| mob_effect_instance(ids, rw))?;
            optional(rw, |rw| rw.string().map(drop))?;
        }
        "suspicious_stew_effects" => list(rw, |rw| {
            id(ids, rw, "minecraft:mob_effect")?;
            rw.varint().map(drop)
        })?,
        "writable_book_content" => list(rw, filterable_string)?,
        "written_book_content" => {
            filterable_string(rw)?;
            rw.string()?;
            rw.varint()?;
            list(rw, filterable_text)?;
            rw.bool()?;
        }
        "trim" | "provides_trim_material" => {
            // The inline form of a material changed in 26.3 (a palette id where 26.2 has an
            // asset name and overrides): items with one lose their trim on older clients.
            // Before 26.1 the material an item provides may also be named.
            let mut inline = false;
            let material = |rw: &mut Rw<'_>| -> Result<()> {
                inline = true;
                rw.string()?;
                text(rw)
            };
            if name.ends_with("provides_trim_material") {
                if !either_holder(ids, rw, "minecraft:trim_material", material)? {
                    return Ok(false);
                }
            } else {
                holder(ids, rw, "minecraft:trim_material", material)?;
            }
            if name.ends_with("trim") {
                holder(ids, rw, "minecraft:trim_pattern", |rw| {
                    rw.string()?;
                    text(rw)?;
                    rw.bool().map(drop)
                })?;
            }
            if inline {
                return Ok(false);
            }
        }
        "entity_data" => typed_nbt(ids, rw, "minecraft:entity_type")?,
        "block_entity_data" => typed_nbt(ids, rw, "minecraft:block_entity_type")?,
        "instrument" => {
            // 26.3 gave an inline instrument a durability damage before its description.
            return either_holder(ids, rw, "minecraft:instrument", |rw| {
                sound(ids, rw)?;
                rw.f32()?;
                rw.f32()?;
                if ids.to_old() {
                    rw.r.varint()?;
                } else {
                    rw.w.put_varint(1);
                }
                text(rw)
            });
        }
        "jukebox_playable" => {
            return either_holder(ids, rw, "minecraft:jukebox_song", |rw| {
                sound(ids, rw)?;
                text(rw)?;
                rw.f32()?;
                rw.varint().map(drop)
            })
        }
        "provides_banner_patterns" => return tag_or_holder_set(ids, rw, "minecraft:banner_pattern"),
        "lodestone_tracker" => {
            optional(rw, |rw| {
                rw.string()?;
                rw.i64().map(drop)
            })?;
            rw.bool()?;
        }
        "firework_explosion" => firework_explosion(rw)?,
        "fireworks" => {
            rw.varint()?;
            list(rw, firework_explosion)?;
        }
        "profile" => profile(ids, rw)?,
        "banner_patterns" => list(rw, |rw| {
            holder(ids, rw, "minecraft:banner_pattern", |rw| {
                rw.string()?;
                rw.string().map(drop)
            })?;
            rw.varint().map(drop)
        })?,
        "pot_decorations" => pot_decorations(ids, rw)?,
        "container" => list(rw, |rw| optional_template(ids, rw))?,
        "block_state" => list(rw, |rw| {
            rw.string()?;
            rw.string().map(drop)
        })?,
        "bees" => list(rw, |rw| {
            typed_nbt(ids, rw, "minecraft:entity_type")?;
            rw.varint()?;
            rw.varint().map(drop)
        })?,
        "break_sound" => sound(ids, rw)?,
        "compostable" => resolvable_int(rw)?,
        "cooking_fuel" | "brewing_fuel" => {
            resolvable_int(rw)?;
            resolvable_float(rw)?;
        }
        "mob_visibility" => {
            holder_set(ids, rw, "minecraft:entity_type")?;
            rw.f32()?;
        }
        "villager/variant" => id(ids, rw, "minecraft:villager_type")?,
        "wolf/variant" => id(ids, rw, "minecraft:wolf_variant")?,
        "wolf/sound_variant" => id(ids, rw, "minecraft:wolf_sound_variant")?,
        "pig/variant" => id(ids, rw, "minecraft:pig_variant")?,
        "pig/sound_variant" => id(ids, rw, "minecraft:pig_sound_variant")?,
        "cow/variant" => id(ids, rw, "minecraft:cow_variant")?,
        "cow/sound_variant" => id(ids, rw, "minecraft:cow_sound_variant")?,
        "chicken/variant" => return either_id(ids, rw, "minecraft:chicken_variant"),
        "chicken/sound_variant" => id(ids, rw, "minecraft:chicken_sound_variant")?,
        "zombie_nautilus/variant" => return either_id(ids, rw, "minecraft:zombie_nautilus_variant"),
        "frog/variant" => id(ids, rw, "minecraft:frog_variant")?,
        "cat/variant" => id(ids, rw, "minecraft:cat_variant")?,
        "cat/sound_variant" => id(ids, rw, "minecraft:cat_sound_variant")?,
        "painting/variant" => holder(ids, rw, "minecraft:painting_variant", |rw| {
            rw.varint()?;
            rw.varint()?;
            rw.string()?;
            optional(rw, text)?;
            optional(rw, text)
        })?,
        "sign_text_front" | "sign_text_back" => {
            for _ in 0..4 {
                text(rw)?;
            }
            optional(rw, |rw| (0..4).try_for_each(|_| text(rw)))?;
            rw.varint()?;
            rw.bool()?;
        }
        "" => bail!("data component type {ty} is unknown"),
        other => bail!("the {other} component is not translated"),
    }
    Ok(true)
}

/// A decorated pot's sherds. 26.3: four optional item templates (back, left, right, front);
/// before: a list of item ids (bricks for plain sides).
fn pot_decorations(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    let brick = paper_data::items::id("minecraft:brick").unwrap_or(0) as i32;
    if ids.to_old() {
        let mut items = Vec::with_capacity(4);
        for _ in 0..4 {
            let item = if rw.r.bool()? {
                let item = rw.r.varint()?;
                rw.r.varint()?;
                let mut rest = Rw { r: rw.r.clone(), w: Vec::new() };
                patch(ids, &mut rest, false)?;
                rw.r = rest.r;
                item
            } else {
                brick
            };
            items.push(ids.item(item).max(0));
        }
        rw.w.put_varint(4);
        for item in items {
            rw.w.put_varint(item);
        }
    } else {
        let items = rw.r.list(4, |r| r.varint())?;
        for side in 0..4 {
            match items.get(side) {
                Some(item) => {
                    rw.w.put_bool(true);
                    rw.w.put_varint(ids.item(*item).max(0));
                    rw.w.put_varint(1);
                    rw.w.put_varint(0);
                    rw.w.put_varint(0);
                }
                None => rw.w.put_bool(false),
            }
        }
    }
    Ok(())
}

/// A trade cost: item, count, and component values (type then value) the payment needs.
pub fn item_cost(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    item_id(ids, rw)?;
    rw.varint()?;
    let n = rw.r.len(256)?;
    let mut kept = Vec::new();
    let mut count = 0;
    for _ in 0..n {
        let mut entry = Rw { r: rw.r.clone(), w: Vec::new() };
        if typed_component(ids, &mut entry)? {
            kept.extend_from_slice(&entry.w);
            count += 1;
        }
        rw.r = entry.r;
    }
    rw.w.put_varint(count);
    rw.w.extend_from_slice(&kept);
    Ok(())
}

/// A slot display (`minecraft:slot_display` type, then its data).
pub fn slot_display(ids: &dyn Ids, rw: &mut Rw<'_>) -> Result<()> {
    let ty = rw.r.varint()?;
    let name = paper_data::registries::entries("minecraft:slot_display").and_then(|e| e.get(ty as usize).copied()).unwrap_or("");
    rw.w.put_varint(ids.registry("minecraft:slot_display", ty).max(0));
    match name {
        "minecraft:empty" | "minecraft:any_fuel" => {}
        "minecraft:item" => item_id(ids, rw)?,
        "minecraft:item_stack" => template(ids, rw)?,
        "minecraft:tag" => {
            rw.string()?;
        }
        "minecraft:smithing_trim" => {
            slot_display(ids, rw)?;
            slot_display(ids, rw)?;
            holder(ids, rw, "minecraft:trim_pattern", |rw| {
                rw.string()?;
                text(rw)?;
                rw.bool().map(drop)
            })?;
        }
        "minecraft:with_remainder" => {
            slot_display(ids, rw)?;
            slot_display(ids, rw)?;
        }
        "minecraft:composite" => list(rw, |rw| slot_display(ids, rw))?,
        other => bail!("slot display {other} is not translated"),
    }
    Ok(())
}

/// The identity: reads 26.3 values as they are (to find where they end).
pub struct Same;

impl Ids for Same {
    fn item(&self, id: i32) -> i32 {
        id
    }
    fn block_state(&self, id: i32) -> i32 {
        id
    }
    fn component(&self, id: i32) -> i32 {
        id
    }
    fn registry(&self, _registry: &str, id: i32) -> i32 {
        id
    }
    fn to_old(&self) -> bool {
        true
    }
}

/// Reads a 26.3 slot (trusted encoding) into an item stack whose component values are their
/// bytes, as the server holds them.
pub fn read_slot(r: &mut Reader<'_>) -> Result<Option<paper_protocol::types::ItemStack>> {
    let count = r.varint()?;
    if count <= 0 {
        return Ok(None);
    }
    let item = r.varint()?;
    let added = r.len(256)?;
    let removed = r.len(256)?;
    let mut stack = paper_protocol::types::ItemStack::new(item, count);
    for _ in 0..added {
        let ty = r.varint()?;
        let mut value = Rw { r: r.clone(), w: Vec::new() };
        component(&Same, &mut value, ty)?;
        *r = value.r;
        stack.added.push((ty, value.w));
    }
    for _ in 0..removed {
        stack.removed.push(r.varint()?);
    }
    Ok(Some(stack))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(name: &str) -> i32 {
        paper_data::registries::id("minecraft:data_component_type", name).unwrap() as i32
    }

    /// A slot with an enchanted, named, damaged and potion-filled item reads back whole.
    #[test]
    fn slots_with_components_read_whole() {
        let mut w = Vec::new();
        w.put_varint(3);
        w.put_varint(42);
        w.put_varint(5);
        w.put_varint(1);
        w.put_varint(ty("minecraft:damage"));
        w.put_varint(7);
        w.put_varint(ty("minecraft:custom_name"));
        paper_nbt::write_network(&mut w, &paper_nbt::Tag::String("Sword".into()));
        w.put_varint(ty("minecraft:enchantments"));
        w.put_varint(1);
        w.put_varint(3);
        w.put_varint(2);
        w.put_varint(ty("minecraft:potion_contents"));
        w.put_bool(true);
        w.put_varint(4);
        w.put_bool(false);
        w.put_varint(1);
        w.put_varint(5);
        for v in [1, 200] {
            w.put_varint(v);
        }
        w.put_bool(false);
        w.put_bool(true);
        w.put_bool(true);
        w.put_bool(false);
        w.put_bool(false);
        w.put_varint(ty("minecraft:intangible_projectile"));
        w.put_slice(&[0x0A, 0x00]);
        w.put_varint(ty("minecraft:rarity"));
        w.put_u8(0xAA);
        let mut r = Reader::new(&w);
        let stack = read_slot(&mut r).unwrap().unwrap();
        assert_eq!(stack.added.len(), 5);
        assert_eq!(stack.removed, vec![ty("minecraft:rarity")]);
        assert_eq!(r.remaining(), 1, "the byte after the slot is left");
    }
}
