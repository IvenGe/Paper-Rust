//! Boats and rafts (`AbstractBoat`): placed from their items on water or on a block, boarded
//! with a click (two seats, one in chest boats), steered by the player in front (the client
//! moves the boat and reports where it went, as with horses), floating when nobody steers
//! them, and broken into their item by hits.

use super::player::{HOTBAR_START, OFFHAND_SLOT};
use super::survival::{fluid_of, Damage, Fluid};
use super::Server;
use crate::config::GameMode;
use crate::world::entity::{collides, friction_below, Aabb, Entity, EntityKind, Vec3};
use crate::world::World;
use paper_data::{entity_types, items};
use paper_protocol::types::{ItemStack, Slot};

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
        let mut e = Entity::new(self.next_entity_id(), info, at, EntityKind::Boat { damage: 0.0, hurt_time: 0, hurt_dir: 1, paddles: (false, false) });
        e.yaw = yaw;
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
    /// they are not sneaking.
    pub(super) fn board_boat(&mut self, id: i32, world: usize, boat: i32) {
        let Some(e) = self.worlds[world].entities.get(&boat) else { return };
        let room = e.passengers.len() < seats(&e.info.name);
        if room && self.players.get(&id).is_some_and(|p| !p.sneaking) {
            self.mount(world, id, boat);
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
        let (name, pos, riders) = (e.info.name.clone(), e.pos, e.passengers.clone());
        for rider in riders {
            self.dismount(rider);
        }
        if !creative && self.worlds[world].rules.bool("entity_drops") {
            if let Some(item) = items::id(&name) {
                self.spawn_item(world, Vec3::new(pos.x, pos.y + 0.25, pos.z), Slot(Some(ItemStack::new(item as i32, 1))), Vec3::new(0.0, 0.2, 0.0), 10);
            }
        }
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
