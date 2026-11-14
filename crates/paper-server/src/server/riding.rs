//! Riding (`startRiding`, `stopRiding`): passengers and vehicles, vehicles the rider steers
//! (the client moves them and reports with `ServerboundMoveVehiclePacket`), getting off with
//! the sneak key, saddles and horse armour put on by right-click, horses that throw their
//! rider off until their temper lets them be tamed, horse food, and the boosts of carrots
//! and warped fungi on sticks.
//!
//! What is not here: boats and minecarts (their entities exist but do nothing), the horse
//! inventory window, and chest-carrying donkeys, mules and llamas.

use super::containers::MenuKind;
use super::menu::SlotKind;
use super::player::Location;
use super::survival::category;
use super::Server;
use crate::world::entity::{Aabb, Vec3};
use paper_data::items;
use paper_protocol::packets::play;
use paper_protocol::types::{ItemStack, Slot};
use rand::Rng;

/// The horse family (`AbstractHorse`): ridden after taming, steered with a saddle.
pub fn is_horse(name: &str) -> bool {
    matches!(
        name,
        "minecraft:horse" | "minecraft:donkey" | "minecraft:mule" | "minecraft:skeleton_horse" | "minecraft:zombie_horse" | "minecraft:llama" | "minecraft:trader_llama" | "minecraft:camel" | "minecraft:camel_husk"
    )
}

/// How many columns of chest a donkey, mule or llama carries (`getInventoryColumns`): five,
/// or a llama's strength; none for the others.
pub fn chest_columns(name: &str, strength: i32) -> i32 {
    match name {
        "minecraft:donkey" | "minecraft:mule" => 5,
        "minecraft:llama" | "minecraft:trader_llama" => strength.clamp(1, 5),
        _ => 0,
    }
}

/// Whether a mob can wear a saddle (`canUseSlot(SADDLE)`); horses must be tamed first.
fn takes_saddle(name: &str) -> bool {
    matches!(name, "minecraft:pig" | "minecraft:strider" | "minecraft:horse" | "minecraft:donkey" | "minecraft:mule" | "minecraft:skeleton_horse" | "minecraft:zombie_horse" | "minecraft:camel" | "minecraft:camel_husk")
}

/// Camels need no taming.
fn always_tame(name: &str) -> bool {
    matches!(name, "minecraft:camel" | "minecraft:camel_husk" | "minecraft:skeleton_horse" | "minecraft:zombie_horse")
}

/// Where a rider sits above the vehicle's feet (`EntityType.passengerAttachments`).
pub fn rider_height(name: &str, height: f64) -> f64 {
    match name {
        "minecraft:horse" => 1.44375,
        "minecraft:donkey" => 1.1125,
        "minecraft:mule" => 1.2125,
        "minecraft:skeleton_horse" => 1.31875,
        "minecraft:zombie_horse" => 1.35,
        "minecraft:llama" | "minecraft:trader_llama" => 1.2375,
        "minecraft:camel" | "minecraft:camel_husk" => 1.975,
        "minecraft:pig" => 0.86875,
        "minecraft:strider" => 1.1875,
        n if super::boats::is_boat(n) => super::boats::seat_height(n, height),
        _ => height,
    }
}

const SADDLE_SLOT: usize = 7;
const BODY_SLOT: usize = 6;

fn item_name(stack: &ItemStack) -> &'static str {
    items::get(stack.item as u32).map_or("", |i| i.name)
}

impl Server {
    /// The world and vehicle of a rider (a player or an entity).
    pub fn vehicle_of(&self, rider: i32) -> Option<(usize, i32)> {
        if let Some(p) = self.players.get(&rider) {
            return p.vehicle.map(|v| (p.world, v));
        }
        self.worlds.iter().enumerate().find_map(|(w, world)| world.entities.get(&rider).and_then(|e| e.vehicle).map(|v| (w, v)))
    }

    /// Who rides an entity, the one steering first.
    pub fn passengers_of(&self, world: usize, vehicle: i32) -> Vec<i32> {
        self.worlds.get(world).and_then(|w| w.entities.get(&vehicle)).map(|e| e.passengers.clone()).unwrap_or_default()
    }

    /// `startRiding`: puts a player or entity on an entity. Returns whether it got on.
    pub fn mount(&mut self, world: usize, rider: i32, vehicle: i32) -> bool {
        if rider == vehicle || self.worlds[world].entities.get(&vehicle).is_none_or(|v| !v.is_alive()) {
            return false;
        }
        // An entity cannot ride something that rides it.
        let mut up = Some(vehicle);
        while let Some(v) = up {
            if v == rider {
                return false;
            }
            up = self.worlds[world].entities.get(&v).and_then(|e| e.vehicle);
        }
        if !crate::plugins::events::on_mount(self, world, rider, vehicle) {
            return false;
        }
        if self.vehicle_of(rider).is_some() {
            self.dismount_quietly(rider);
        }
        if let Some(p) = self.players.get_mut(&rider) {
            if p.world != world {
                return false;
            }
            p.vehicle = Some(vehicle);
            p.fall_distance = 0.0;
        } else if let Some(e) = self.worlds[world].entities.get_mut(&rider) {
            e.vehicle = Some(vehicle);
            if let Some(m) = e.mob_mut() {
                m.brain.path.clear();
            }
        } else {
            return false;
        }
        if let Some(v) = self.worlds[world].entities.get_mut(&vehicle) {
            v.passengers.push(rider);
            if let Some(m) = v.mob_mut() {
                m.brain.path.clear();
            }
        }
        self.send_passengers(world, vehicle);
        self.position_riders(world, vehicle, false);
        true
    }

    /// `stopRiding` without moving the rider (for a new mount, a teleport, a death).
    pub(super) fn dismount_quietly(&mut self, rider: i32) -> Option<(usize, i32)> {
        let (world, vehicle) = self.vehicle_of(rider)?;
        if let Some(p) = self.players.get_mut(&rider) {
            p.vehicle = None;
        } else if let Some(e) = self.worlds[world].entities.get_mut(&rider) {
            e.vehicle = None;
        }
        if let Some(v) = self.worlds[world].entities.get_mut(&vehicle) {
            v.passengers.retain(|p| *p != rider);
        }
        self.send_passengers(world, vehicle);
        Some((world, vehicle))
    }

    /// `stopRiding`: gets off and steps down next to the vehicle
    /// (`getDismountLocationForPassenger`).
    pub fn dismount(&mut self, rider: i32) {
        if let Some((world, vehicle)) = self.vehicle_of(rider) {
            if !crate::plugins::events::on_dismount(self, world, rider, vehicle) {
                return;
            }
        }
        let Some((world, vehicle)) = self.dismount_quietly(rider) else { return };
        let Some(v) = self.worlds[world].entities.get(&vehicle) else { return };
        let (pos, yaw, width, height) = (v.pos, v.yaw, v.info.width as f64, v.info.height as f64);
        let spot = self.dismount_spot(world, pos, yaw, width).unwrap_or(Vec3::new(pos.x, pos.y + height, pos.z));
        if let Some(p) = self.players.get(&rider) {
            let loc = Location { x: spot.x, y: spot.y, z: spot.z, yaw: p.loc.yaw, pitch: p.loc.pitch };
            self.teleport(rider, loc);
            if let Some(p) = self.players.get_mut(&rider) {
                p.fall_distance = 0.0;
            }
        } else if let Some(e) = self.worlds[world].entities.get_mut(&rider) {
            e.pos = spot;
            e.force_sync = true;
            self.worlds[world].reindex_entity(rider);
        }
    }

    /// Room for a player beside a vehicle: left, right, then around it.
    fn dismount_spot(&self, world: usize, pos: Vec3, yaw: f32, width: f64) -> Option<Vec3> {
        let w = &self.worlds[world];
        let (sin, cos) = ((yaw as f64).to_radians().sin(), (yaw as f64).to_radians().cos());
        let side = width / 2.0 + 0.6;
        let offsets = [(cos * side, sin * side), (-cos * side, -sin * side), (sin * side, -cos * side), (-sin * side, cos * side)];
        for (dx, dz) in offsets.into_iter().chain([(side, side), (-side, side), (side, -side), (-side, -side)]) {
            for dy in [0, 1, -1] {
                let (x, z) = (pos.x + dx, pos.z + dz);
                let y = pos.y.floor() as i32 + dy;
                if super::mobs::walkable(w, x.floor() as i32, y, z.floor() as i32, 2) {
                    return Some(Vec3::new(x, y as f64, z));
                }
            }
        }
        None
    }

    /// Tells the vehicle's viewers (and its riders) who rides it.
    fn send_passengers(&self, world: usize, vehicle: i32) {
        let passengers = self.passengers_of(world, vehicle);
        let packet = play::SetPassengers { vehicle, passengers: passengers.clone() };
        let payload = bytes::Bytes::from(paper_protocol::packets::encode_packet(&packet));
        self.broadcast_entity(world, vehicle, payload.clone());
        for p in passengers {
            if let Some(player) = self.players.get(&p) {
                if !self.worlds[world].entities.get(&vehicle).is_some_and(|v| v.viewers.contains(&p)) {
                    player.conn.send_payload(payload.clone());
                }
            }
        }
    }

    /// Who steers a vehicle (`getControllingPassenger`): its first rider if that is a player
    /// and the vehicle lets itself be steered: a saddled horse or camel, a saddled pig with a
    /// carrot on a stick in hand, a saddled strider with a warped fungus on a stick.
    pub fn controller_of(&self, world: usize, vehicle: i32) -> Option<i32> {
        let v = self.worlds.get(world)?.entities.get(&vehicle)?;
        let first = *v.passengers.first()?;
        let p = self.players.get(&first)?;
        // Whoever sits in front steers a boat.
        if super::boats::is_boat(&v.info.name) {
            return Some(first);
        }
        let mob = v.mob()?;
        let saddled = !mob.equipment[SADDLE_SLOT].is_empty();
        let holds = |name: &str| [&p.inventory[super::player::HOTBAR_START + p.held_slot as usize], &p.inventory[super::player::OFFHAND_SLOT]].iter().any(|s| s.0.as_ref().is_some_and(|st| item_name(st) == name));
        let steers = match v.info.name.as_str() {
            "minecraft:pig" => saddled && holds("minecraft:carrot_on_a_stick"),
            "minecraft:strider" => saddled && holds("minecraft:warped_fungus_on_a_stick"),
            name if is_horse(name) => saddled,
            _ => false,
        };
        steers.then_some(first)
    }

    /// `ServerboundMoveVehiclePacket`: the player steering a vehicle moved it.
    pub(super) fn handle_move_vehicle(&mut self, id: i32, packet: play::MoveVehicle) {
        let Some((world, vehicle)) = self.vehicle_of(id) else { return };
        if self.controller_of(world, vehicle) != Some(id) {
            return;
        }
        let Some(v) = self.worlds[world].entities.get(&vehicle) else { return };
        let to = Vec3::new(packet.x, packet.y, packet.z);
        let finite = [packet.x, packet.y, packet.z].iter().all(|c| c.is_finite()) && packet.yaw.is_finite() && packet.pitch.is_finite();
        // "moved too quickly": more than ten blocks in a tick goes back.
        if !finite || to.distance_squared(v.pos) > 100.0 {
            let back = play::ClientboundMoveVehicle { x: v.pos.x, y: v.pos.y, z: v.pos.z, yaw: v.yaw, pitch: v.pitch };
            self.send(id, &back);
            return;
        }
        let old_chunk = v.pos.chunk();
        let e = self.worlds[world].entities.get_mut(&vehicle).unwrap();
        e.pos = to;
        e.yaw = packet.yaw;
        e.head_yaw = packet.yaw;
        e.pitch = packet.pitch;
        e.on_ground = packet.on_ground;
        e.vel = Vec3::ZERO;
        if let Some(m) = e.mob_mut() {
            m.living.fall_distance = 0.0;
        }
        if to.chunk() != old_chunk {
            self.worlds[world].reindex_entity(vehicle);
        }
        self.position_riders(world, vehicle, true);
    }

    /// Riders sit on their vehicles (`positionRider`); players riding along count how far
    /// (`checkRidingStatistics`), except when just getting on.
    fn position_riders(&mut self, world: usize, vehicle: i32, count: bool) {
        let Some(v) = self.worlds[world].entities.get(&vehicle) else { return };
        let seat = Vec3::new(v.pos.x, v.pos.y + rider_height(&v.info.name, v.info.height as f64), v.pos.z);
        let kind = v.info.name.clone();
        let mut ridden = Vec::new();
        for rider in v.passengers.clone() {
            if let Some(p) = self.players.get_mut(&rider) {
                if count {
                    ridden.push((rider, seat.x - p.loc.x, seat.y - p.loc.y, seat.z - p.loc.z));
                }
                let old_chunk = p.loc.chunk();
                p.loc.x = seat.x;
                p.loc.y = seat.y;
                p.loc.z = seat.z;
                p.fall_distance = 0.0;
                if p.loc.chunk() != old_chunk {
                    self.refresh_chunk_view(rider);
                }
            } else if let Some(e) = self.worlds[world].entities.get_mut(&rider) {
                let moved_chunk = e.pos.chunk() != seat.chunk();
                e.pos = seat;
                e.vel = Vec3::ZERO;
                if moved_chunk {
                    self.worlds[world].reindex_entity(rider);
                }
            }
        }
        for (rider, dx, dy, dz) in ridden {
            self.riding_stats(rider, &kind, dx, dy, dz);
        }
    }

    /// Every tick: riders get off what died or went away, and sit on what they ride; pigs
    /// and striders count down their boosts; untamed horses buck.
    pub(super) fn tick_riding(&mut self) {
        self.check_mount_menus();
        // Players whose vehicle is gone, or who left its world.
        let stale: Vec<i32> = self.players.values().filter(|p| p.vehicle.is_some_and(|v| self.worlds[p.world].entities.get(&v).is_none_or(|e| !e.is_alive() || !e.passengers.contains(&p.entity_id)))).map(|p| p.entity_id).collect();
        for id in stale {
            if let Some(p) = self.players.get_mut(&id) {
                p.vehicle = None;
            }
        }
        for world in 0..self.worlds.len() {
            let vehicles: Vec<i32> = self.worlds[world].entities.iter().filter(|(_, e)| !e.passengers.is_empty()).map(|(id, _)| *id).collect();
            for vehicle in vehicles {
                let Some(v) = self.worlds[world].entities.get(&vehicle) else { continue };
                let alive = v.is_alive();
                let gone: Vec<i32> = v
                    .passengers
                    .iter()
                    .copied()
                    .filter(|p| !alive || match self.players.get(p) {
                        Some(player) => player.dead || player.world != world || player.vehicle != Some(vehicle),
                        None => self.worlds[world].entities.get(p).is_none_or(|e| !e.is_alive() || e.vehicle != Some(vehicle)),
                    })
                    .collect();
                // Riders of what died get off where they are; the dead, the departed and those
                // gone elsewhere are simply no longer riders.
                for p in gone {
                    if let Some(v) = self.worlds[world].entities.get_mut(&vehicle) {
                        v.passengers.retain(|x| *x != p);
                    }
                    if let Some(e) = self.worlds[world].entities.get_mut(&p).filter(|e| e.vehicle == Some(vehicle)) {
                        e.vehicle = None;
                    }
                    if let Some(pl) = self.players.get_mut(&p).filter(|pl| pl.vehicle == Some(vehicle)) {
                        pl.vehicle = None;
                    }
                    self.send_passengers(world, vehicle);
                }
                self.position_riders(world, vehicle, true);
                self.buck(world, vehicle);
            }
            // Boosts run out (`ItemBasedSteering`).
            for e in self.worlds[world].entities.values_mut() {
                if let Some(m) = e.mob_mut().filter(|m| m.boost > 0) {
                    m.boost -= 1;
                }
            }
        }
    }

    /// `RunAroundLikeCrazyGoal`: an untamed horse with a player on it runs about, and now and
    /// then either gives in (more likely the higher its temper) or throws the player off and
    /// gets angrier.
    fn buck(&mut self, world: usize, vehicle: i32) {
        let Some(v) = self.worlds[world].entities.get(&vehicle) else { return };
        let Some(mob) = v.mob() else { return };
        if !is_horse(&v.info.name) || mob.tamed || always_tame(&v.info.name) {
            return;
        }
        let Some(&rider) = v.passengers.first() else { return };
        let (pos, idle, temper) = (v.pos, mob.brain.path.is_empty(), mob.temper);
        let mut rng = rand::thread_rng();
        if idle {
            let d = Vec3::new(rng.gen_range(-5.0..5.0), 0.0, rng.gen_range(-5.0..5.0));
            if let Some(m) = self.worlds[world].entities.get_mut(&vehicle).and_then(|e| e.mob_mut()) {
                m.brain.walk_target = Some(pos.add(d));
            }
        }
        if rng.gen_range(0..50) != 0 {
            return;
        }
        if self.players.contains_key(&rider) {
            if rng.gen_range(0..100) < temper && crate::plugins::events::on_tame(self, world, vehicle, rider) {
                self.tame(world, vehicle, rider);
                return;
            }
            if let Some(m) = self.worlds[world].entities.get_mut(&vehicle).and_then(|e| e.mob_mut()) {
                m.temper = (m.temper + 5).min(100);
            }
        }
        self.dismount(rider);
        self.play_sound(world, pos, "minecraft:entity.horse.angry", category::NEUTRAL, 1.0, 1.0, None);
        self.entity_event(world, vehicle, 6);
    }

    /// `TamableAnimal.tame` / `AbstractHorse.tameWithName`: the player owns it now (hearts).
    pub fn tame(&mut self, world: usize, id: i32, player: i32) {
        let Some(uuid) = self.players.get(&player).map(|p| p.uuid()) else { return };
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let name = e.info.name.clone();
        if let Some(m) = e.mob_mut() {
            m.tamed = true;
            m.owner = Some(uuid);
            m.brain.target = None;
            m.brain.path.clear();
            // `applyTamingSideEffects`: a tamed wolf is tougher.
            if name == "minecraft:wolf" {
                m.attributes.set_base("max_health", 40.0);
                m.living.max_health = m.attributes.value("max_health") as f32;
                m.living.health = m.living.max_health;
            }
        }
        e.metadata_dirty = true;
        self.entity_event(world, id, 7);
    }

    /// Right-click on a mob for riding (`AbstractHorse`, `Pig`, `Strider.mobInteract`): put
    /// on a saddle or horse armour, feed a horse, or get on. Returns whether the click was
    /// used.
    pub(super) fn ride_interact(&mut self, player: i32, world: usize, target: i32, hand: i32, held: Option<&ItemStack>) -> bool {
        let Some(e) = self.worlds[world].entities.get(&target) else { return false };
        let name = e.info.name.clone();
        let Some(mob) = e.mob() else { return false };
        let (baby, tamed, saddled, ridden) = (mob.baby, mob.tamed || always_tame(&name), !mob.equipment[SADDLE_SLOT].is_empty(), !e.passengers.is_empty());
        let chested = mob.chest.is_some();
        let sneaking = self.players.get(&player).is_some_and(|p| p.sneaking);
        let pos = e.pos;
        let held_name = held.map(item_name).unwrap_or("");
        let equip_slot = held.and_then(|s| paper_data::item_data::by_id(s.item as u32).equip_slot.clone());
        match name.as_str() {
            "minecraft:pig" | "minecraft:strider" => {
                let food = held.is_some_and(|s| super::animals::is_food(&name, s.item));
                if !food && saddled && !ridden && !sneaking {
                    return self.mount(world, player, target);
                }
                if equip_slot.as_deref() == Some("saddle") && !saddled && !baby {
                    self.equip(world, target, player, hand, SADDLE_SLOT, "minecraft:entity.pig.saddle");
                    return true;
                }
                false
            }
            n if is_horse(n) => {
                if ridden || baby {
                    return false;
                }
                // Sneaking opens a tamed one's inventory (`openCustomInventoryScreen`).
                if sneaking && tamed {
                    return self.open_mount_inventory(player, world, target);
                }
                if let Some(stack) = held {
                    if !n.contains("llama") && self.horse_food(world, target, stack) {
                        self.use_held(player, hand);
                        return true;
                    }
                    // A chest for a tamed donkey, mule or llama.
                    if held_name == "minecraft:chest" && chest_columns(n, 1) > 0 && tamed && !chested {
                        self.equip_chest(world, target, player, hand);
                        return true;
                    }
                    if equip_slot.as_deref() == Some("saddle") && takes_saddle(n) {
                        if tamed && !saddled {
                            self.equip(world, target, player, hand, SADDLE_SLOT, "minecraft:entity.horse.saddle");
                            return true;
                        }
                    } else if equip_slot.as_deref() == Some("body") && tamed && super::menu::may_wear(stack.item as u32, n) {
                        // Horse armour, or a carpet on a llama.
                        if mob_slot_empty(self, world, target, BODY_SLOT) {
                            let sound = if n.contains("llama") { "minecraft:entity.llama.swag" } else { "minecraft:entity.horse.armor" };
                            self.equip(world, target, player, hand, BODY_SLOT, sound);
                            return true;
                        }
                    } else if !tamed {
                        // Something it does not like: it rears (`makeMad`).
                        self.play_sound(world, pos, "minecraft:entity.horse.angry", category::NEUTRAL, 1.0, 1.0, None);
                        return true;
                    }
                }
                self.mount(world, player, target)
            }
            _ => false,
        }
    }

    /// A tamed horse's (donkey's, llama's, camel's) inventory window: saddle, body armour and
    /// chest. Someone else riding it keeps it to themselves. Returns whether it opened.
    pub(super) fn open_mount_inventory(&mut self, player: i32, world: usize, mount: i32) -> bool {
        let Some(e) = self.worlds[world].entities.get(&mount) else { return false };
        let Some(m) = e.mob() else { return false };
        let name: &'static str = e.info.name.as_str();
        if !is_horse(name) || !(m.tamed || always_tame(name)) || m.baby || e.passengers.iter().any(|p| *p != player) {
            return false;
        }
        let type_id = e.info.id;
        let in_tag = |tag: &str| paper_data::synced::tag("minecraft:entity_type", tag).is_some_and(|ids| ids.contains(&type_id));
        let worn = |slot: &'static str, active: bool| if active { SlotKind::Worn(slot, name) } else { SlotKind::Locked };
        let saddle = worn("saddle", in_tag("minecraft:can_equip_saddle"));
        let body = worn("body", in_tag("minecraft:can_wear_horse_armor") || name.contains("llama"));
        self.open_menu(player, MenuKind::Mount { world, id: mount, saddle, body }, "", paper_protocol::text::Component::text(String::new()))
    }

    /// The vehicle's inventory, asked for from the saddle (`OPEN_INVENTORY`).
    pub(super) fn open_vehicle_inventory(&mut self, player: i32) {
        if let Some((world, vehicle)) = self.vehicle_of(player) {
            self.open_mount_inventory(player, world, vehicle);
        }
    }

    /// Mount windows close once the mount is gone, dead or out of reach (`stillValid`).
    fn check_mount_menus(&mut self) {
        let stale: Vec<i32> = self
            .players
            .values()
            .filter(|p| match p.open_menu.as_ref().map(|m| &m.kind) {
                Some(MenuKind::Mount { world, id, .. }) => *world != p.world || self.worlds[*world].entities.get(id).is_none_or(|e| !e.is_alive() || e.pos.distance_squared(Vec3::new(p.loc.x, p.loc.y, p.loc.z)) > 64.0),
                _ => false,
            })
            .map(|p| p.entity_id)
            .collect();
        for id in stale {
            self.close_menu(id, true);
        }
    }

    /// Puts a chest on a donkey, mule or llama (`AbstractChestedHorse.equipChest`).
    fn equip_chest(&mut self, world: usize, target: i32, player: i32, hand: i32) {
        let Some(e) = self.worlds[world].entities.get_mut(&target) else { return };
        let (name, pos) = (e.info.name.as_str(), e.pos);
        let Some(m) = e.mob_mut() else { return };
        m.chest = Some(vec![Slot::EMPTY; chest_columns(name, m.strength) as usize * 3]);
        m.persistent = true;
        e.metadata_dirty = true;
        self.use_held(player, hand);
        let sound = match name {
            "minecraft:donkey" => "minecraft:entity.donkey.chest",
            "minecraft:mule" => "minecraft:entity.mule.chest",
            _ => "minecraft:entity.llama.chest",
        };
        let pitch = (rand::thread_rng().gen::<f32>() - rand::thread_rng().gen::<f32>()) * 0.2 + 1.0;
        self.play_sound(world, pos, sound, category::NEUTRAL, 1.0, pitch, None);
    }

    /// Puts one of the held item on a mob's equipment slot.
    fn equip(&mut self, world: usize, target: i32, player: i32, hand: i32, slot: usize, sound: &str) {
        let Some(stack) = self.players.get(&player).map(|p| p.inventory[if hand == 1 { super::player::OFFHAND_SLOT } else { super::player::HOTBAR_START + p.held_slot as usize }].clone()) else { return };
        let Some(one) = stack.0.as_ref().map(|s| s.with_count(1)) else { return };
        let Some(e) = self.worlds[world].entities.get_mut(&target) else { return };
        let pos = e.pos;
        if let Some(m) = e.mob_mut() {
            m.equipment[slot] = Slot(Some(one));
            m.persistent = true;
        }
        e.equipment_dirty = true;
        e.metadata_dirty = true;
        self.use_held(player, hand);
        self.play_sound(world, pos, sound, category::NEUTRAL, 0.5, 1.0, None);
    }

    /// `AbstractHorse.handleEating`: food heals, grows foals, raises temper (helping to tame
    /// it) and golden food puts tamed horses in love. Returns whether it ate.
    fn horse_food(&mut self, world: usize, id: i32, stack: &ItemStack) -> bool {
        let (heal, age_up, temper, love) = match item_name(stack) {
            "minecraft:wheat" => (2.0, 20, 3, false),
            "minecraft:sugar" => (1.0, 30, 3, false),
            "minecraft:hay_block" => (20.0, 180, 0, false),
            "minecraft:apple" | "minecraft:carrot" => (3.0, 60, 3, false),
            "minecraft:golden_carrot" => (4.0, 60, 5, true),
            "minecraft:golden_apple" | "minecraft:enchanted_golden_apple" => (10.0, 240, 10, true),
            _ => return false,
        };
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return false };
        let Some(m) = e.mob_mut() else { return false };
        let mut ate = false;
        if love && m.tamed && m.age == 0 && m.in_love <= 0 {
            m.in_love = 600;
            ate = true;
        }
        if m.living.health < m.living.max_health && heal > 0.0 {
            m.living.health = (m.living.health + heal).min(m.living.max_health);
            ate = true;
        }
        if m.baby && age_up > 0 {
            m.age = (m.age + age_up).min(0);
            ate = true;
        }
        if temper > 0 && (ate || !m.tamed) && m.temper < 100 {
            m.temper = (m.temper + temper).min(100);
            ate = true;
        }
        if ate {
            e.metadata_dirty = true;
            let pos = e.pos;
            self.play_sound(world, pos, "minecraft:entity.horse.eat", category::NEUTRAL, 1.0, 1.0, None);
        }
        ate
    }

    /// The carrot or warped fungus on a stick of a player riding a pig or strider: a boost
    /// (`FoodOnAStickItem.use`). Returns whether it boosted.
    pub(super) fn boost_ridden(&mut self, player: i32, hand: i32) -> bool {
        let Some((world, vehicle)) = self.vehicle_of(player) else { return false };
        let slot = if hand == 1 { super::player::OFFHAND_SLOT } else { super::player::HOTBAR_START + self.players[&player].held_slot as usize };
        let Some(stack) = self.players[&player].inventory[slot].0.clone() else { return false };
        let (needed, wear) = match item_name(&stack) {
            "minecraft:carrot_on_a_stick" => ("minecraft:pig", 7),
            "minecraft:warped_fungus_on_a_stick" => ("minecraft:strider", 1),
            _ => return false,
        };
        let Some(e) = self.worlds[world].entities.get_mut(&vehicle) else { return false };
        if e.info.name != needed {
            return false;
        }
        let Some(m) = e.mob_mut() else { return false };
        if m.boost > 0 {
            return false;
        }
        let total = rand::thread_rng().gen_range(140..=980);
        m.boost = total;
        // `DATA_BOOST_TIME` starts a boost on the client each time it arrives, so it is sent
        // once, here, rather than with the rest of the entity's data.
        if let Some(index) = e.info.data_index("DATA_BOOST_TIME") {
            let data = paper_protocol::metadata::Metadata::new().varint(index, total).finish();
            let packet = play::SetEntityData { entity_id: paper_protocol::codec::VarInt(vehicle), metadata: paper_protocol::codec::RemainingBytes(data) };
            let payload = bytes::Bytes::from(paper_protocol::packets::encode_packet(&packet));
            self.broadcast_entity(world, vehicle, payload.clone());
            if let Some(p) = self.players.get(&player) {
                p.conn.send_payload(payload);
            }
        }
        // The stick wears; a broken one leaves a fishing rod.
        let mut worn = Slot(Some(stack));
        let broke = super::survival::hurt_item(&mut worn, wear, false);
        if broke {
            worn = Slot::of(items::id("minecraft:fishing_rod").unwrap_or(0) as i32, 1);
        }
        self.set_inventory_slot(player, slot, worn);
        true
    }

    /// Mobs steered by a player do not think for themselves.
    pub fn steered(&self, world: usize, id: i32) -> bool {
        self.controller_of(world, id).is_some()
    }

    /// The box a player rides in, for collisions with what it rides.
    pub fn rider_box(&self, world: usize, vehicle: i32) -> Option<Aabb> {
        let v = self.worlds.get(world)?.entities.get(&vehicle)?;
        Some(Aabb::of_entity(Vec3::new(v.pos.x, v.pos.y + rider_height(&v.info.name, v.info.height as f64), v.pos.z), 0.6, 1.8))
    }
}

fn mob_slot_empty(server: &Server, world: usize, id: i32, slot: usize) -> bool {
    server.worlds[world].entities.get(&id).and_then(|e| e.mob()).is_some_and(|m| m.equipment[slot].is_empty())
}

/// Vanilla's random horse at spawn (`randomizeAttributes`, a random coat): its health,
/// speed and jump strength, and for horses colour and markings.
pub fn randomize_horse(name: &str, mob: &mut crate::world::entity::Mob, rng: &mut impl Rng) {
    let health = |rng: &mut dyn rand::RngCore| 15.0 + rng.gen_range(0..8) as f64 + rng.gen_range(0..9) as f64;
    match name {
        "minecraft:horse" => {
            let h = health(rng);
            mob.attributes.set_base("max_health", h);
            let speed = (0.45 + rng.gen::<f64>() * 0.3 + rng.gen::<f64>() * 0.3 + rng.gen::<f64>() * 0.3) * 0.25;
            mob.attributes.set_base("movement_speed", speed);
            let jump = 0.4 + rng.gen::<f64>() * 0.2 + rng.gen::<f64>() * 0.2 + rng.gen::<f64>() * 0.2;
            mob.attributes.set_base("jump_strength", jump);
            mob.variant = rng.gen_range(0..7) | (rng.gen_range(0..5) << 8);
        }
        "minecraft:donkey" | "minecraft:mule" | "minecraft:llama" | "minecraft:trader_llama" => {
            let h = health(rng);
            mob.attributes.set_base("max_health", h);
            // `Llama.setRandomStrength`: 1 to 3, now and then up to 5.
            if name.contains("llama") {
                let most = if rng.gen::<f32>() < 0.04 { 5 } else { 3 };
                mob.strength = 1 + rng.gen_range(0..most);
            }
        }
        _ => return,
    }
    mob.living.max_health = mob.attributes.value("max_health") as f32;
    mob.living.health = mob.living.max_health;
}

