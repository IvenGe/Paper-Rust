//! Item frames, glow item frames and paintings in play: players hang them on block faces,
//! they break when hit, blown up or left without support (dropping themselves), and frames
//! take an item that players turn and knock out again. Plugins hear of each step
//! (`HangingPlaceEvent`, `HangingBreakEvent`, Paper's `PlayerItemFrameChangeEvent`).

use super::survival::{category, Damage};
use super::Server;
use crate::config::GameMode;
use crate::plugins::events;
use crate::world::entity::{EntityKind, Vec3};
use crate::world::hanging::{self, Hanging, PAINTING, STEP};
use paper_data::{blocks, items};
use paper_protocol::packets::play::UseItemOn;
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

/// A sound of a frame or painting: `minecraft:entity.item_frame.place`, ...
fn sound(name: &str, what: &str) -> String {
    format!("minecraft:entity.{}.{what}", name.strip_prefix("minecraft:").unwrap_or(name))
}

/// One of a stack: what a frame holds (`ItemFrame.setItem` copies one).
fn one_of(slot: &Slot) -> Slot {
    match &slot.0 {
        Some(stack) => Slot(Some(stack.with_count(1))),
        None => Slot::EMPTY,
    }
}

impl Server {
    /// An item frame, glow item frame or painting used on a block (`HangingEntityItem.useOn`):
    /// it hangs on the clicked face, paintings only on walls, a painting taking a random one
    /// of the largest placeable variants that fit. Returns whether the click was used.
    pub(super) fn use_hanging_item(&mut self, player: i32, use_on: &UseItemOn) -> bool {
        let Some(p) = self.players.get(&player) else { return false };
        let hand = use_on.hand.0;
        let slot = self.hand_slot(player, hand);
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        let Some(name) = items::get(stack.item as u32).map(|i| i.name).filter(|n| hanging::is_hanging(n)) else { return false };
        let face = use_on.face.0;
        if !(0..6).contains(&face) || !matches!(p.game_mode, GameMode::Survival | GameMode::Creative) || name == PAINTING && face < 2 {
            return false;
        }
        let world = p.world;
        let (dx, dy, dz) = STEP[face as usize];
        let block = use_on.pos.offset(dx, dy, dz);
        let id = self.next_entity_id();
        let mut h = Hanging::new(block, face as u8);
        if name == PAINTING {
            let fitting = hanging::fitting_variants(&self.worlds[world], block, face as u8, id);
            if fitting.is_empty() {
                return true;
            }
            h.variant = fitting[rand::thread_rng().gen_range(0..fitting.len())].to_owned();
        }
        if !h.survives(&self.worlds[world], name, id) {
            return true;
        }
        let Some(mut e) = hanging::new_entity(id, name, h) else { return false };
        // A name given at an anvil names it.
        e.custom_name = super::workstations::custom_name_of(&stack);
        self.spawn_entity(world, e);
        // Plugins may stop it (`HangingPlaceEvent`), and hear of it as of any new entity
        // (`EntitySpawnEvent`).
        if !events::on_hanging_place(self, world, id, player, use_on.pos, face as usize, hand, &stack) {
            self.worlds[world].remove_entity(id);
            self.resync_inventory(player);
            return true;
        }
        if !self.keep_spawned(world, id) {
            self.resync_inventory(player);
            return true;
        }
        let pos = self.worlds[world].entities[&id].pos;
        self.play_sound(world, pos, &sound(name, "place"), category::NEUTRAL, 1.0, 1.0, None);
        self.use_held(player, hand);
        self.award_stat(player, "minecraft:used", name, 1);
        true
    }

    /// A hanging entity's tick (`BlockAttachedEntity.tick`): every 100 ticks it checks that it
    /// can stay, and breaks if not, unless plugins keep it (`HangingBreakEvent`, with cause
    /// `OBSTRUCTION` when something took its place, else `PHYSICS`).
    pub(super) fn tick_hanging(&mut self, world: usize, id: i32) {
        let min_y = self.worlds[world].min_y;
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        if e.pos.y < (min_y - 64) as f64 {
            e.removed = true;
            return;
        }
        let EntityKind::Hanging(h) = &mut e.kind else { return };
        let due = h.check >= hanging::CHECK_INTERVAL;
        h.check += 1;
        if !due || e.removed {
            return;
        }
        h.check = 0;
        let (name, h, at) = (e.info.name.clone(), (**h).clone(), e.pos);
        if h.survives(&self.worlds[world], &name, id) {
            return;
        }
        let taken = self.worlds[world].block_at(at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32).is_some_and(|s| !blocks::is_air(s));
        if !events::on_hanging_break(self, world, id, if taken { "OBSTRUCTION" } else { "PHYSICS" }, None) {
            return;
        }
        self.break_hanging(world, id, None);
    }

    /// A hanging entity hurt (`ItemFrame.hurtServer`, `BlockAttachedEntity.hurtServer`): a
    /// frame's item is knocked out first, unless by an explosion; otherwise it breaks, unless
    /// plugins keep it (`HangingBreakEvent`, `HangingBreakByEntityEvent`). A fixed frame only
    /// breaks for creative players and `/kill`. Returns whether the hit landed.
    pub(super) fn hurt_hanging(&mut self, world: usize, id: i32, damage: &Damage, amount: f32) -> bool {
        let Some(e) = self.worlds[world].entities.get(&id) else { return false };
        let EntityKind::Hanging(h) = &e.kind else { return false };
        if e.removed {
            return false;
        }
        let frame = hanging::is_frame(&e.info.name);
        let by_creative = damage.attacker.and_then(|a| self.players.get(&a)).is_some_and(|p| p.game_mode == GameMode::Creative);
        if frame && h.fixed && !(damage.is("bypasses_invulnerability") || by_creative) {
            return false;
        }
        if frame && !h.fixed && !damage.is("is_explosion") && !h.item.is_empty() {
            let item = h.item.clone();
            let name = e.info.name.clone();
            // Plugins may stop the hit (`EntityDamageEvent`), and keep the item in when a
            // player knocks it out, or change what comes out (`PlayerItemFrameChangeEvent`).
            let mut amount = amount;
            if !events::on_entity_damage(self, id, damage, &mut amount) {
                return true;
            }
            if let Some(player) = damage.attacker.filter(|a| self.players.contains_key(a)) {
                let Some(left) = events::on_item_frame_change(self, world, id, player, &item, "REMOVE") else { return true };
                self.set_frame_item(world, id, left, false);
            }
            self.drop_frame_item(world, id, damage.attacker, false);
            if let Some(pos) = self.worlds[world].entities.get(&id).map(|e| e.pos) {
                self.play_sound(world, pos, &sound(&name, "remove_item"), category::NEUTRAL, 1.0, 1.0, None);
            }
            return true;
        }
        // Mobs only break them while mobs may grief.
        let by_mob = damage.attacker.is_some_and(|a| self.worlds[world].entities.get(&a).is_some_and(|m| matches!(m.kind, EntityKind::Mob(_))));
        if by_mob && !self.worlds[world].rules.bool("mob_griefing") {
            return false;
        }
        let cause = if damage.is("is_explosion") {
            "EXPLOSION"
        } else if damage.direct.is_some() {
            "ENTITY"
        } else {
            "DEFAULT"
        };
        if !events::on_hanging_break(self, world, id, cause, Some(damage)) {
            return true;
        }
        self.break_hanging(world, id, damage.attacker);
        true
    }

    /// Takes a hanging entity away with its sound, dropping itself and a frame's item
    /// (`Painting.dropItem`, `ItemFrame.dropItem`): nothing for a creative player, nor with
    /// `entity_drops` off, and nothing at all from a fixed frame.
    pub fn break_hanging(&mut self, world: usize, id: i32, breaker: Option<i32>) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        e.removed = true;
        let (name, pos) = (e.info.name.clone(), e.pos);
        let drops = self.worlds[world].rules.bool("entity_drops");
        self.play_sound(world, pos, &sound(&name, "break"), category::NEUTRAL, 1.0, 1.0, None);
        if name == PAINTING {
            let by_creative = breaker.and_then(|b| self.players.get(&b)).is_some_and(|p| p.game_mode == GameMode::Creative);
            if drops && !by_creative {
                if let Some(item) = items::id(PAINTING) {
                    self.drop_at(world, pos, Slot::of(item as i32, 1));
                }
            }
        } else {
            self.drop_frame_item(world, id, breaker, true);
        }
    }

    /// `ItemFrame.dropItem(level, entity, dropSelf)`: empties the frame, dropping its item
    /// (by its drop chance) and, with `drop_self`, the frame with its name.
    fn drop_frame_item(&mut self, world: usize, id: i32, breaker: Option<i32>, drop_self: bool) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let EntityKind::Hanging(h) = &mut e.kind else { return };
        if h.fixed {
            return;
        }
        let item = std::mem::replace(&mut h.item, Slot::EMPTY);
        let chance = h.drop_chance;
        e.metadata_dirty = true;
        let (name, pos, label) = (e.info.name.clone(), e.pos, e.custom_name.clone());
        let by_creative = breaker.and_then(|b| self.players.get(&b)).is_some_and(|p| p.game_mode == GameMode::Creative);
        if !self.worlds[world].rules.bool("entity_drops") || by_creative {
            return;
        }
        if drop_self {
            if let Some(frame) = items::id(&name) {
                let mut stack = ItemStack::new(frame as i32, 1);
                if let Some(label) = label {
                    stack = super::workstations::with_custom_name(stack, label);
                }
                self.drop_at(world, pos, Slot(Some(stack)));
            }
        }
        if !item.is_empty() && rand::thread_rng().gen::<f32>() < chance {
            self.drop_at(world, pos, item);
        }
    }

    /// `Entity.spawnAtLocation`: an item where the entity is, with the usual pickup delay.
    fn drop_at(&mut self, world: usize, pos: Vec3, stack: Slot) {
        let mut rng = rand::thread_rng();
        let vel = Vec3::new(rng.gen::<f64>() * 0.2 - 0.1, 0.2, rng.gen::<f64>() * 0.2 - 0.1);
        self.spawn_item(world, pos, stack, vel, 10);
    }

    /// Puts an item in a frame (one of it), with the sound of it unless `quiet`.
    pub fn set_frame_item(&mut self, world: usize, id: i32, item: Slot, sound_it: bool) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let EntityKind::Hanging(h) = &mut e.kind else { return };
        h.item = one_of(&item);
        e.metadata_dirty = true;
        let (name, pos) = (e.info.name.clone(), e.pos);
        if sound_it && !item.is_empty() {
            self.play_sound(world, pos, &sound(&name, "add_item"), category::NEUTRAL, 1.0, 1.0, None);
        }
    }

    /// A player using an item frame (`ItemFrame.interact`): an empty one takes one of the
    /// held item, a full one turns its item an eighth; plugins may stop either or change the
    /// item (`PlayerItemFrameChangeEvent`). Fixed frames do nothing.
    pub(super) fn use_item_frame(&mut self, player: i32, world: usize, id: i32, hand: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let EntityKind::Hanging(h) = &e.kind else { return };
        if h.fixed || e.removed {
            return;
        }
        let (name, current) = (e.info.name.clone(), h.item.clone());
        let slot = self.hand_slot(player, hand);
        let held = self.players[&player].inventory[slot].clone();
        if current.is_empty() {
            if held.is_empty() {
                return;
            }
            let Some(item) = events::on_item_frame_change(self, world, id, player, &held, "PLACE") else {
                self.resync_inventory(player);
                return;
            };
            self.set_frame_item(world, id, item, true);
            self.use_held(player, hand);
            return;
        }
        let Some(item) = events::on_item_frame_change(self, world, id, player, &current, "ROTATE") else { return };
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let EntityKind::Hanging(h) = &mut e.kind else { return };
        h.item = one_of(&item);
        h.rotation = (h.rotation + 1) % 8;
        e.metadata_dirty = true;
        let pos = e.pos;
        self.play_sound(world, pos, &sound(&name, "rotate_item"), category::NEUTRAL, 1.0, 1.0, None);
    }

    /// A hanging entity a plugin spawns (`CraftRegionAccessor.createEntity`): on the first
    /// side of the block at `pos` with a solid block behind and no other hanging entity in
    /// the way, else facing south in mid-air; a painting there takes a variant that fits.
    pub fn spawn_hanging(&mut self, world: usize, name: &str, pos: Vec3) -> Option<i32> {
        let block = paper_protocol::codec::BlockPos::new(pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32);
        let id = self.next_entity_id();
        // The sides to try (Bukkit faces east, north, west, south, then up and down for frames)
        // and the ways the entity then faces, away from them.
        let sides: &[(u8, u8)] = if name == PAINTING { &[(5, 4), (2, 3), (4, 5), (3, 2)] } else { &[(5, 4), (2, 3), (4, 5), (3, 2), (1, 0), (0, 1)] };
        let w = &self.worlds[world];
        let facing = sides
            .iter()
            .find(|(side, facing)| {
                let (dx, dy, dz) = STEP[*side as usize];
                let support = w.block_at(block.x + dx, block.y + dy, block.z + dz).is_some_and(|s| paper_data::physics::is_solid(s) || matches!(blocks::block_of_state(s).name, "minecraft:repeater" | "minecraft:comparator"));
                let mut h = Hanging::new(block, *facing);
                h.variant = "minecraft:kebab".into();
                let bb = h.bounding_box(name);
                support && !w.entities_in(&bb).into_iter().any(|o| w.entities.get(&o).is_some_and(|o| matches!(o.kind, EntityKind::Hanging(_))))
            })
            .map(|(_, facing)| *facing);
        let mut h = Hanging::new(block, facing.unwrap_or(3));
        if name == PAINTING {
            let fitting = if facing.is_some() { hanging::fitting_variants(w, block, h.facing, id) } else { Vec::new() };
            h.variant = match fitting.len() {
                0 => "minecraft:kebab".to_owned(),
                n => fitting[rand::thread_rng().gen_range(0..n)].to_owned(),
            };
        }
        let e = hanging::new_entity(id, name, h)?;
        Some(self.spawn_entity(world, e))
    }
}
