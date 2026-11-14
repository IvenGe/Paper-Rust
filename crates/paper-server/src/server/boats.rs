//! Boats and rafts (`AbstractBoat`): placed from their items on water or on a block, boarded
//! with a click (two seats, one in chest boats), steered by the player in front (the client
//! moves the boat and reports where it went, as with horses), floating when nobody steers
//! them, and broken into their item by hits. Chest boats carry a chest (`AbstractChestBoat`)
//! that spills when they break.

use super::containers::MenuKind;
use super::player::{HOTBAR_START, OFFHAND_SLOT};
use super::survival::{fluid_of, Damage, Fluid};
use super::Server;
use crate::config::GameMode;
use crate::world::entity::{collides, friction_below, Aabb, Entity, EntityChest, EntityKind, Vec3};
use crate::world::World;
use paper_data::{entity_types, items};
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

/// Whether an entity (or its item) is a boat or raft, chest ones included.
pub fn is_boat(name: &str) -> bool {
    name.ends_with("_boat") || name.ends_with("_raft")
}

/// How many can ride: two, or one beside a chest.
fn seats(name: &str) -> usize {
    if name.contains("_chest_") {
        1
    } else {
        2
    }
}

/// A new boat of type `name`: undamaged, with an empty chest if it is a chest boat.
pub fn boat_kind(name: &str) -> EntityKind {
    let chest = name.contains("_chest_").then(|| Box::new(EntityChest { items: vec![Slot::EMPTY; 27], ..Default::default() }));
    EntityKind::Boat { damage: 0.0, hurt_time: 0, hurt_dir: 1, paddles: (false, false), chest }
}

/// Where riders' feet are, above the boat's base: its seat (`getPassengerAttachmentPoint`)
/// less a player's own attachment point.
pub fn seat_height(name: &str, height: f64) -> f64 {
    let seat = if name.contains("raft") { height * 0.888_888_9 } else { height / 3.0 };
    seat - 0.6
}

/// The top of the water around a boat's base, if there is any.
fn water_level(world: &World, b: &Aabb) -> Option<f64> {
    let mut level: Option<f64> = None;
    for x in b.min.x.floor() as i32..b.max.x.ceil() as i32 {
        for z in b.min.z.floor() as i32..b.max.z.ceil() as i32 {
            for y in b.min.y.floor() as i32 - 1..b.max.y.ceil() as i32 + 1 {
                let Some((Fluid::Water, amount)) = world.block_at(x, y, z).and_then(fluid_of) else { continue };
                let top = y as f64 + amount as f64 / 9.0;
                level = Some(level.map_or(top, |l: f64| l.max(top)));
            }
        }
    }
    level.filter(|top| *top >= b.min.y - 0.001)
}

/// A boat's tick (`AbstractBoat.tick`): its wobble and damage wear off, and unless someone
/// steers it, it falls, floats a little below the surface of water (`floatBoat`) and slows
/// down (fast on the ground, slowly on ice).
pub fn tick_boat(world: &World, e: &mut Entity, steered: bool) {
    let mut changed = false;
    if let EntityKind::Boat { damage, hurt_time, .. } = &mut e.kind {
        if *hurt_time > 0 {
            *hurt_time -= 1;
            changed = true;
        }
        if *damage > 0.0 {
            *damage = (*damage - 1.0).max(0.0);
            changed = true;
        }
    }
    if changed {
        e.metadata_dirty = true;
    }
    if steered {
        return;
    }
    let gravity = if e.no_gravity { 0.0 } else { -0.04 };
    let mut vel = e.vel;
    match water_level(world, &e.bounding_box()) {
        Some(top) => {
            vel = Vec3::new(vel.x * 0.9, vel.y + gravity, vel.z * 0.9);
            let sunk = (top - e.pos.y) / e.info.height as f64;
            if sunk > 0.0 {
                vel.y = (vel.y + sunk * 0.061_538_46) * 0.75;
            }
        }
        None => {
            let friction = if e.on_ground { friction_below(world, e.pos) } else { 0.9 };
            vel = Vec3::new(vel.x * friction, vel.y + gravity, vel.z * friction);
        }
    }
    e.vel = vel;
    if vel.length_squared() > 1.0e-10 || !e.on_ground {
        e.move_by(world, vel, 0.0);
    }
}

impl Server {
    /// A boat item used (`BoatItem.use`): a boat where the player looks, on the water's
    /// surface or on a block, turned their way, unless it would be stuck in blocks. Returns
    /// whether the item was used.
    pub(super) fn use_boat(&mut self, id: i32, hand: i32, yaw: f32, pitch: f32) -> bool {
        let Some(p) = self.players.get(&id) else { return false };
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let Some(stack) = p.inventory[slot].0.clone() else { return false };
        let Some(name) = items::get(stack.item as u32).map(|i| i.name).filter(|n| is_boat(n)) else { return false };
        let Some(info) = entity_types::by_name(name) else { return false };
        if !matches!(p.game_mode, GameMode::Survival | GameMode::Creative) {
            return false;
        }
        let (world, creative) = (p.world, p.game_mode == GameMode::Creative);
        let eye = Vec3::new(p.loc.x, p.loc.y + p.eye_height(), p.loc.z);
        let Some((hit, face, fluid)) = super::portals::raycast(&self.worlds[world], eye, Vec3::from_rotation(yaw, pitch), 5.0, true) else { return false };
        let at = match (fluid, face) {
            (Some(_), _) => Vec3::new(hit.x as f64 + 0.5, hit.y as f64 + 8.0 / 9.0, hit.z as f64 + 0.5),
            (None, (0, 1, 0)) => Vec3::new(hit.x as f64 + 0.5, hit.y as f64 + 1.0, hit.z as f64 + 0.5),
            (None, (dx, dy, dz)) => Vec3::new((hit.x + dx) as f64 + 0.5, (hit.y + dy) as f64, (hit.z + dz) as f64 + 0.5),
        };
        let mut e = Entity::new(self.next_entity_id(), info, at, boat_kind(name));
        e.yaw = yaw;
        // `createDefaultStackConfig`: a named boat item makes a named boat.
        e.custom_name = super::workstations::custom_name_of(&stack);
        if collides(&self.worlds[world], &e.bounding_box()) {
            return false;
        }
        self.spawn_entity(world, e);
        if !creative {
            let rest = if stack.count > 1 { Slot(Some(stack.with_count(stack.count - 1))) } else { Slot::EMPTY };
            self.set_inventory_slot(id, slot, rest);
        }
        self.award_stat(id, "minecraft:used", name, 1);
        true
    }

    /// A click on a boat (`AbstractBoat.interact`): the player gets in if there is room and
    /// they are not sneaking. Otherwise a chest boat opens its chest
    /// (`AbstractChestBoat.interact`).
    pub(super) fn board_boat(&mut self, id: i32, world: usize, boat: i32) {
        let Some(e) = self.worlds[world].entities.get(&boat) else { return };
        let room = e.passengers.len() < seats(&e.info.name);
        let chest = e.chest().is_some();
        if room && self.players.get(&id).is_some_and(|p| !p.sneaking) {
            self.mount(world, id, boat);
        } else if chest {
            self.open_boat_chest(id, world, boat);
        }
    }

    /// Opens a chest boat's chest for a player (`interactWithContainerVehicle`, or the
    /// rider's inventory key), rolling its loot table in first. Spectators cannot open one
    /// whose loot is not rolled yet (`createMenu`). Returns whether it opened.
    pub(super) fn open_boat_chest(&mut self, player: i32, world: usize, boat: i32) -> bool {
        let spectator = self.players.get(&player).is_some_and(|p| p.game_mode == GameMode::Spectator);
        let Some(chest) = self.worlds[world].entities.get(&boat).and_then(|e| e.chest()) else { return false };
        if spectator && chest.loot_table.is_some() {
            return false;
        }
        self.unpack_entity_loot(world, boat);
        let Some(title) = self.entity_display_name(world, boat) else { return false };
        self.open_menu(player, MenuKind::EntityChest { world, id: boat }, "minecraft:generic_9x3", title)
    }

    /// Rolls a chest boat's loot table into its chest (`unpackChestVehicleLootTable`).
    pub(crate) fn unpack_entity_loot(&mut self, world: usize, id: i32) {
        let Some(chest) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.chest_mut()) else { return };
        let Some(table) = chest.loot_table.take() else { return };
        let seed = std::mem::take(&mut chest.loot_seed);
        let mut items = std::mem::take(&mut chest.items);
        super::containers::fill_with_loot(&self.loot, &table, seed, &mut items);
        if let Some(chest) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.chest_mut()) {
            chest.items = items;
        }
    }

    /// Changes a chest boat's slots (hoppers and droppers); players looking into it see it.
    pub(super) fn set_entity_chest_items(&mut self, world: usize, id: i32, items: &[Slot]) {
        let Some(chest) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.chest_mut()) else { return };
        let n = chest.items.len().min(items.len());
        chest.items[..n].clone_from_slice(&items[..n]);
        let inv = crate::plugins::inventory::InvRef::Entity { world, id };
        for viewer in inv.viewers(self) {
            self.send_menu_contents(viewer);
        }
    }

    /// A chest boat's slots (with its loot, if not rolled yet) spill where it is, as when it
    /// breaks or is killed (`Containers.dropContents` from `remove`).
    pub(crate) fn spill_entity_chest(&mut self, world: usize, id: i32) {
        self.unpack_entity_loot(world, id);
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let pos = e.pos;
        let Some(chest) = e.chest_mut() else { return };
        let empty = vec![Slot::EMPTY; chest.items.len()];
        for stack in std::mem::replace(&mut chest.items, empty) {
            if let Some(stack) = stack.0 {
                self.drop_item_stack(world, pos, stack);
            }
        }
    }

    /// `Containers.dropItemStack`: a stack spills from a random spot in the block at `pos`,
    /// in bits of 10 to 30 that can be picked up at once.
    pub(crate) fn drop_item_stack(&mut self, world: usize, pos: Vec3, mut stack: ItemStack) {
        let mut rng = rand::thread_rng();
        // An item entity is 0.25 wide.
        let spot = Vec3::new(pos.x.floor() + rng.gen::<f64>() * 0.75 + 0.125, pos.y.floor() + rng.gen::<f64>() * 0.75, pos.z.floor() + rng.gen::<f64>() * 0.75 + 0.125);
        while stack.count > 0 {
            let n = rng.gen_range(10..=30).min(stack.count);
            let mut triangle = |mean: f64| mean + 0.114_850_001_711_398_36 * (rng.gen::<f64>() - rng.gen::<f64>());
            let vel = Vec3::new(triangle(0.0), triangle(0.2), triangle(0.0));
            self.spawn_item(world, spot, Slot(Some(stack.with_count(n))), vel, 0);
            stack.count -= n;
        }
    }

    /// A boat is hit (`AbstractBoat.hurtServer`): it wobbles and takes ten times the damage,
    /// breaking into its item past 40 (at once, and without the item, for a player in
    /// creative). Its riders get off.
    pub(super) fn hurt_boat(&mut self, world: usize, id: i32, damage: &Damage, amount: f32) -> bool {
        let creative = damage.attacker.and_then(|a| self.players.get(&a)).is_some_and(|p| p.game_mode == GameMode::Creative);
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return false };
        if e.removed {
            return false;
        }
        let EntityKind::Boat { damage: taken, hurt_time, hurt_dir, .. } = &mut e.kind else { return false };
        *hurt_dir = -*hurt_dir;
        *hurt_time = 10;
        *taken += amount * 10.0;
        let broken = creative || *taken > 40.0;
        e.metadata_dirty = true;
        if !broken {
            return true;
        }
        e.removed = true;
        let (name, pos, riders, label) = (e.info.name.clone(), e.pos, e.passengers.clone(), e.custom_name.clone());
        for rider in riders {
            self.dismount(rider);
        }
        // `destroy`: the boat's item (named as the boat was), as `spawnAtLocation` drops it.
        if !creative && self.worlds[world].rules.bool("entity_drops") {
            if let Some(item) = items::id(&name) {
                let mut stack = ItemStack::new(item as i32, 1);
                if let Some(label) = label {
                    let mut bytes = Vec::new();
                    paper_nbt::write_network(&mut bytes, &label.to_nbt());
                    super::enchanting::set_component(&mut stack, "minecraft:custom_name", bytes);
                }
                let mut rng = rand::thread_rng();
                let vel = Vec3::new(rng.gen::<f64>() * 0.2 - 0.1, 0.2, rng.gen::<f64>() * 0.2 - 0.1);
                self.spawn_item(world, pos, Slot(Some(stack)), vel, 10);
            }
        }
        // A chest boat's chest spills, also when a creative player breaks it.
        self.spill_entity_chest(world, id);
        true
    }

    /// `ServerboundPaddleBoatPacket`: the steering player's paddles, which others see move.
    pub(super) fn handle_paddle_boat(&mut self, id: i32, left: bool, right: bool) {
        let Some((world, boat)) = self.vehicle_of(id) else { return };
        if self.controller_of(world, boat) != Some(id) {
            return;
        }
        if let Some(e) = self.worlds[world].entities.get_mut(&boat) {
            if let EntityKind::Boat { paddles, .. } = &mut e.kind {
                *paddles = (left, right);
            }
            e.metadata_dirty = true;
        }
    }
}
