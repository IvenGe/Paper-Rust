//! Crossbows (vanilla's `CrossbowItem`): held, one charges for 25 ticks (a quarter second
//! less per Quick Charge level); released charged, it loads what it shoots (a firework rocket
//! or an arrow in hand, else an arrow from the inventory; three with Multishot, only one used
//! up) unless plugins stop it (`EntityLoadCrossbowEvent`), and shows as loaded. The next use
//! fires it all: arrows always critical, rockets flying straight, Multishot's three spread 10
//! degrees apart, each the shooter's `EntityShootBowEvent`; the crossbow wears a point per
//! arrow and three per rocket.

use super::player::{HOTBAR_START, OFFHAND_SLOT};
use super::survival::category;
use super::Server;
use crate::config::GameMode;
use crate::plugins::events;
use crate::world::entity::{EntityKind, Vec3};
use paper_data::items;
use paper_protocol::codec::Reader;
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

pub const CROSSBOW: &str = "minecraft:crossbow";

fn component(name: &str) -> Option<i32> {
    paper_data::registries::id("minecraft:data_component_type", name).map(|t| t as i32)
}

fn name_of(stack: &ItemStack) -> &'static str {
    items::get(stack.item as u32).map_or("", |i| i.name)
}

/// What a crossbow is loaded with (its `charged_projectiles` component). Component values
/// in it carry no lengths, so only those arrows and rockets carry can be read; anything
/// else loaded counts as a plain arrow.
pub fn charged_projectiles(stack: &ItemStack) -> Vec<Slot> {
    let Some(ty) = component("minecraft:charged_projectiles") else { return Vec::new() };
    let Some((_, data)) = stack.added.iter().find(|(t, _)| *t == ty) else { return Vec::new() };
    let mut r = Reader::new(data);
    let n = r.varint().unwrap_or(0).clamp(0, 64) as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        match decode_projectile(&mut r) {
            Some(slot) => out.push(slot),
            None => break,
        }
    }
    let plain = items::id("minecraft:arrow").unwrap_or(0) as i32;
    while out.len() < n {
        let mut arrow = ItemStack::new(plain, 1);
        make_intangible(&mut arrow);
        out.push(Slot(Some(arrow)));
    }
    out
}

/// One item of a component list (`ItemStack.STREAM_CODEC`).
fn decode_projectile(r: &mut Reader<'_>) -> Option<Slot> {
    let count = r.varint().ok()?;
    if count <= 0 {
        return Some(Slot::EMPTY);
    }
    let item = r.varint().ok()?;
    let (n_add, n_remove) = (r.varint().ok()?, r.varint().ok()?);
    let mut stack = ItemStack::new(item, count);
    for _ in 0..n_add.clamp(0, 256) {
        let ty = r.varint().ok()?;
        let name = paper_data::registries::name("minecraft:data_component_type", ty as u32)?;
        let start = r.peek();
        skip_component(name, r)?;
        let used = start.len() - r.remaining();
        stack.added.push((ty, start[..used].to_vec()));
    }
    for _ in 0..n_remove.clamp(0, 256) {
        stack.removed.push(r.varint().ok()?);
    }
    Some(Slot(Some(stack)))
}

/// Reads past a component value of a kind arrows and rockets carry.
fn skip_component(name: &str, r: &mut Reader<'_>) -> Option<()> {
    let varints = |r: &mut Reader<'_>, n: usize| (0..n).try_for_each(|_| r.varint().ok().map(|_| ()));
    match name {
        "minecraft:intangible_projectile" => Some(()),
        "minecraft:custom_name" | "minecraft:item_name" | "minecraft:custom_data" => r.nbt().ok().map(|_| ()),
        "minecraft:lore" => {
            let n = r.varint().ok()?.clamp(0, 256);
            (0..n).try_for_each(|_| r.nbt().ok().map(|_| ()))
        }
        "minecraft:damage" | "minecraft:max_damage" | "minecraft:repair_cost" | "minecraft:max_stack_size" | "minecraft:rarity" => varints(r, 1),
        "minecraft:enchantment_glint_override" => r.bool().ok().map(|_| ()),
        "minecraft:enchantments" | "minecraft:stored_enchantments" => {
            let n = r.varint().ok()?.clamp(0, 256) as usize;
            varints(r, 2 * n)
        }
        // `Fireworks`: flight, then explosions (shape, colours, fade colours, trail, twinkle).
        "minecraft:fireworks" => {
            r.varint().ok()?;
            let n = r.varint().ok()?.clamp(0, 256);
            (0..n).try_for_each(|_| {
                r.varint().ok()?;
                for _ in 0..2 {
                    let colors = r.varint().ok()?.clamp(0, 256);
                    (0..colors).try_for_each(|_| r.i32().ok().map(|_| ()))?;
                }
                r.bool().ok()?;
                r.bool().ok().map(|_| ())
            })
        }
        // `PotionContents`: potion, colour, custom effects, custom name.
        "minecraft:potion_contents" => {
            if r.bool().ok()? {
                r.varint().ok()?;
            }
            if r.bool().ok()? {
                r.i32().ok()?;
            }
            let n = r.varint().ok()?.clamp(0, 256);
            (0..n).try_for_each(|_| {
                r.varint().ok()?;
                skip_effect_details(r)
            })?;
            if r.bool().ok()? {
                r.string(32767).ok()?;
            }
            Some(())
        }
        _ => None,
    }
}

/// `MobEffectInstance.Details`: amplifier, duration, flags, and a hidden effect's details.
fn skip_effect_details(r: &mut Reader<'_>) -> Option<()> {
    r.varint().ok()?;
    r.varint().ok()?;
    for _ in 0..3 {
        r.bool().ok()?;
    }
    if r.bool().ok()? {
        skip_effect_details(r)?;
    }
    Some(())
}

/// Loads (or with nothing, empties) a crossbow.
fn set_charged_projectiles(stack: &mut ItemStack, projectiles: &[Slot]) {
    let Some(ty) = component("minecraft:charged_projectiles") else { return };
    stack.added.retain(|(t, _)| *t != ty);
    if projectiles.is_empty() {
        return;
    }
    let mut data = Vec::new();
    paper_protocol::codec::Encode::encode(&paper_protocol::codec::VarInt(projectiles.len() as i32), &mut data);
    for p in projectiles {
        p.encode_trusted(&mut data);
    }
    stack.added.push((ty, data));
}

/// Whether a loaded projectile was a free copy (Multishot's extras, or a creative player's):
/// its arrow can then only be picked up in creative (`intangible_projectile`).
fn intangible(slot: &Slot) -> bool {
    component("minecraft:intangible_projectile").is_some_and(|ty| slot.0.as_ref().is_some_and(|s| s.added.iter().any(|(t, _)| *t == ty)))
}

fn make_intangible(stack: &mut ItemStack) {
    if let Some(ty) = component("minecraft:intangible_projectile") {
        if !stack.added.iter().any(|(t, _)| *t == ty) {
            stack.added.push((ty, Vec::new()));
        }
    }
}

/// Ticks a crossbow takes to charge (`getChargeDuration`).
pub fn charge_ticks(crossbow: &Slot) -> i32 {
    (25 - 5 * super::loot::enchantment_level(Some(crossbow), "minecraft:quick_charge")).max(0)
}

impl Server {
    /// The ammunition a crossbow would load (`Player.getProjectile`): a rocket or arrow in
    /// either hand (off hand first), else the first arrow in the hotbar or inventory; the
    /// slot it is in, or none for a creative player's free arrow.
    fn crossbow_ammo(&self, player: i32) -> Option<(Option<usize>, ItemStack)> {
        let p = self.players.get(&player)?;
        let held = |slot: usize| p.inventory[slot].0.as_ref().filter(|s| super::survival::is_arrow(s.item) || name_of(s) == super::fireworks::ROCKET).map(|s| (Some(slot), s.clone()));
        let main = HOTBAR_START + p.held_slot as usize;
        if let Some(found) = held(OFFHAND_SLOT).or_else(|| held(main)) {
            return Some(found);
        }
        for slot in (HOTBAR_START..HOTBAR_START + 9).chain(9..HOTBAR_START) {
            if let Some(s) = p.inventory[slot].0.as_ref().filter(|s| super::survival::is_arrow(s.item)) {
                return Some((Some(slot), s.clone()));
            }
        }
        (p.game_mode == GameMode::Creative).then(|| (None, ItemStack::new(items::id("minecraft:arrow").unwrap_or(0) as i32, 1)))
    }

    /// A crossbow used (`CrossbowItem.use`): a loaded one fires; an empty one starts charging
    /// when there is something to load. Returns whether it was used.
    pub(super) fn use_crossbow(&mut self, player: i32, hand: i32) -> bool {
        let slot = self.hand_slot(player, hand);
        let Some(crossbow) = self.players.get(&player).and_then(|p| p.inventory[slot].0.clone()) else { return false };
        if !charged_projectiles(&crossbow).is_empty() {
            self.fire_crossbow(player, hand, slot);
            return true;
        }
        if self.crossbow_ammo(player).is_none() {
            return false;
        }
        let Some(p) = self.players.get_mut(&player) else { return false };
        p.using_item = Some((hand as u8, 72000));
        p.metadata_dirty = true;
        true
    }

    /// The charging sounds (`onUseTick`): a fifth of the way and halfway.
    pub(super) fn crossbow_charging(&mut self, player: i32, slot: usize, used: i32) {
        let Some(p) = self.players.get(&player) else { return };
        let crossbow = p.inventory[slot].clone();
        let charge = charge_ticks(&crossbow).max(1);
        let (world, at) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z));
        let quick = super::loot::enchantment_level(Some(&crossbow), "minecraft:quick_charge");
        let sound = if used == (charge as f32 * 0.2).ceil() as i32 {
            match quick {
                0 => "minecraft:item.crossbow.loading_start".to_owned(),
                n => format!("minecraft:item.crossbow.quick_charge_{}", n.min(3)),
            }
        } else if used == (charge as f32 * 0.5).ceil() as i32 && quick == 0 {
            "minecraft:item.crossbow.loading_middle".to_owned()
        } else {
            return;
        };
        self.play_sound(world, at, &sound, category::PLAYERS, 0.5, 1.0, None);
    }

    /// Letting go of a charging crossbow (`releaseUsing`): fully charged, it loads, unless
    /// plugins stop it (`EntityLoadCrossbowEvent`, which may also keep the ammunition).
    pub(super) fn release_crossbow(&mut self, player: i32, slot: usize, used: i32) {
        let Some(p) = self.players.get(&player) else { return };
        let Some(mut crossbow) = p.inventory[slot].0.clone() else { return };
        let (world, at, creative) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z), p.game_mode == GameMode::Creative);
        if used < charge_ticks(&Slot(Some(crossbow.clone()))) || !charged_projectiles(&crossbow).is_empty() {
            return;
        }
        let Some((ammo_slot, ammo)) = self.crossbow_ammo(player) else { return };
        let hand = if slot == OFFHAND_SLOT { 1 } else { 0 };
        let Some(consume) = events::on_load_crossbow(self, player, &crossbow, hand) else {
            self.resync_inventory(player);
            return;
        };
        // `draw`: one used up (none by a creative player or when plugins say so), the rest copies.
        let count = if super::loot::enchantment_level(Some(&Slot(Some(crossbow.clone()))), "minecraft:multishot") > 0 { 3 } else { 1 };
        let mut loaded = Vec::new();
        for i in 0..count {
            let mut one = ammo.with_count(1);
            if i > 0 || creative || !consume || ammo_slot.is_none() {
                make_intangible(&mut one);
            }
            loaded.push(Slot(Some(one)));
        }
        if !creative && consume {
            if let Some(s) = ammo_slot {
                let rest = if ammo.count > 1 { Slot(Some(ammo.with_count(ammo.count - 1))) } else { Slot::EMPTY };
                self.set_inventory_slot(player, s, rest);
            }
        }
        set_charged_projectiles(&mut crossbow, &loaded);
        self.set_inventory_slot(player, slot, Slot(Some(crossbow)));
        let pitch = 1.0 / (rand::thread_rng().gen::<f32>() * 0.5 + 1.0) + 0.2;
        self.play_sound(world, at, "minecraft:item.crossbow.loading_end", category::PLAYERS, 1.0, pitch, None);
    }

    /// `performShooting`: everything loaded flies (arrows at 3.15, rockets at 1.6) and the
    /// crossbow is emptied.
    fn fire_crossbow(&mut self, player: i32, hand: i32, slot: usize) {
        let Some(p) = self.players.get(&player) else { return };
        let Some(mut crossbow) = p.inventory[slot].0.clone() else { return };
        let loaded = charged_projectiles(&crossbow);
        set_charged_projectiles(&mut crossbow, &[]);
        let (world, loc, eye, creative) = (p.world, p.loc, p.eye_height(), p.game_mode == GameMode::Creative);
        self.set_inventory_slot(player, slot, Slot(Some(crossbow.clone())));
        let rocket = loaded.iter().any(|s| s.0.as_ref().is_some_and(|s| name_of(s) == super::fireworks::ROCKET));
        let power = if rocket { 1.6 } else { 3.15 };
        let multishot = super::loot::enchantment_level(Some(&Slot(Some(crossbow.clone()))), "minecraft:multishot") > 0;
        let spread = if multishot { 10.0f64 } else { 0.0 };
        let n = loaded.len();
        let step = if n <= 1 { 0.0 } else { 2.0 * spread / (n - 1) as f64 };
        let offset = ((n.max(1) - 1) % 2) as f64 * step / 2.0;
        let look = Vec3::from_rotation(loc.yaw, loc.pitch);
        let up = Vec3::from_rotation(loc.yaw, loc.pitch - 90.0);
        let mut direction = 1.0;
        let mut rng = rand::thread_rng();
        for (i, projectile) in loaded.iter().enumerate() {
            let Some(item) = projectile.0.as_ref() else { continue };
            let angle = offset + direction * ((i + 1) / 2) as f64 * step;
            direction = -direction;
            // The look rotated about the shooter's up by the spread (`getProjectileShotVector`).
            let (sin, cos) = angle.to_radians().sin_cos();
            let cross = Vec3::new(up.y * look.z - up.z * look.y, up.z * look.x - up.x * look.z, up.x * look.y - up.y * look.x);
            let dot = up.x * look.x + up.y * look.y + up.z * look.z;
            let shot = Vec3::new(look.x * cos + cross.x * sin + up.x * dot * (1.0 - cos), look.y * cos + cross.y * sin + up.y * dot * (1.0 - cos), look.z * cos + cross.z * sin + up.z * dot * (1.0 - cos));
            let id = if name_of(item) == super::fireworks::ROCKET {
                let origin = Vec3::new(loc.x, loc.y + eye - 0.15, loc.z);
                let Some(mut e) = self.new_firework(origin, Slot(Some(item.with_count(1))), Some(player)) else { continue };
                if let EntityKind::Firework(f) = &mut e.kind {
                    f.shot_at_angle = true;
                }
                let triangle = |rng: &mut rand::rngs::ThreadRng| (rng.gen::<f64>() - rng.gen::<f64>()) * 0.0172275;
                let d = shot.normalize();
                e.vel = Vec3::new(d.x + triangle(&mut rng), d.y + triangle(&mut rng), d.z + triangle(&mut rng)).scale(power);
                self.spawn_entity(world, e)
            } else {
                let origin = Vec3::new(loc.x, loc.y + eye - 0.1, loc.z);
                self.spawn_arrow_unlaunched(world, origin, shot, power, 1.0, Some(player), 2.0, true, !intangible(projectile))
            };
            // Plugins may stop each shot (`EntityShootBowEvent`), then its launch.
            if events::on_shoot_bow(self, world, player, &Slot(Some(crossbow.clone())), projectile, id, hand, 1.0, true).is_none() {
                self.worlds[world].remove_entity(id);
                continue;
            }
            if !self.launch(world, id) {
                continue;
            }
            let pitch = if i == 0 { 1.0 } else { 1.0 / (rng.gen::<f32>() * 0.5 + 1.8) + if i % 2 == 1 { 0.63 } else { 0.43 } };
            self.play_sound(world, Vec3::new(loc.x, loc.y, loc.z), "minecraft:item.crossbow.shoot", category::PLAYERS, 1.0, pitch, None);
            if !creative {
                let wear = if name_of(item) == super::fireworks::ROCKET { 3 } else { 1 };
                if self.wear_player_item(player, slot, wear, false) {
                    break;
                }
            }
        }
        let worn = self.players[&player].inventory[slot].clone();
        self.set_inventory_slot(player, slot, worn);
        self.award_stat(player, "minecraft:used", CROSSBOW, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_protocol::codec::{Encode, VarInt};

    #[test]
    fn loaded_rockets_and_arrows_read_back_whole() {
        let mut rocket = ItemStack::new(items::id(super::super::fireworks::ROCKET).unwrap() as i32, 1);
        let mut fireworks = Vec::new();
        VarInt(2).encode(&mut fireworks);
        VarInt(1).encode(&mut fireworks);
        VarInt(0).encode(&mut fireworks);
        VarInt(2).encode(&mut fireworks);
        fireworks.extend_from_slice(&0xFF0000i32.to_be_bytes());
        fireworks.extend_from_slice(&0x00FF00i32.to_be_bytes());
        VarInt(0).encode(&mut fireworks);
        fireworks.extend_from_slice(&[0, 1]);
        rocket.added.push((component("minecraft:fireworks").unwrap(), fireworks));
        let mut name = Vec::new();
        paper_nbt::write_network(&mut name, &paper_protocol::text::Component::text("Big one").to_nbt());
        rocket.added.push((component("minecraft:custom_name").unwrap(), name));
        let mut arrow = ItemStack::new(items::id("minecraft:arrow").unwrap() as i32, 1);
        make_intangible(&mut arrow);
        let mut crossbow = ItemStack::new(items::id(CROSSBOW).unwrap() as i32, 1);
        let loaded = vec![Slot(Some(rocket)), Slot(Some(arrow))];
        set_charged_projectiles(&mut crossbow, &loaded);
        assert_eq!(charged_projectiles(&crossbow), loaded);
        assert_eq!(super::super::fireworks::fireworks_of(loaded[0].0.as_ref().unwrap()), (2, 1));
        assert!(intangible(&loaded[1]) && !intangible(&loaded[0]));
        set_charged_projectiles(&mut crossbow, &[]);
        assert!(charged_projectiles(&crossbow).is_empty());
    }
}
