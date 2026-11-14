//! Taming (`TamableAnimal`): wolves (bones), cats (raw cod and salmon) and parrots (seeds)
//! become a player's pets. Pets sit when their owner tells them to, follow their owner
//! (teleporting when far behind), heal from their food, wear dyed collars (wolves and cats)
//! and wolf armour, and wolves defend their owner and attack what the owner attacks.
//!
//! What is not here: cats lying on beds and bringing gifts, parrots on shoulders and
//! dancing, and wild wolves hunting.

use super::survival::category;
use super::Server;
use crate::config::GameMode;
use crate::world::entity::Vec3;
use paper_data::items;
use paper_protocol::types::ItemStack;
use rand::Rng;

/// Animals players tame by feeding (`TamableAnimal`).
pub fn is_tameable(name: &str) -> bool {
    matches!(name, "minecraft:wolf" | "minecraft:cat" | "minecraft:parrot")
}

fn item_name(stack: &ItemStack) -> &'static str {
    items::get(stack.item as u32).map_or("", |i| i.name)
}

fn in_item_tag(tag: &str, item: i32) -> bool {
    paper_data::synced::tag("minecraft:item", tag).is_some_and(|ids| ids.contains(&item))
}

/// What tames an animal, and the chance of each try.
fn taming(name: &str, stack: &ItemStack) -> Option<u32> {
    match name {
        "minecraft:wolf" if item_name(stack) == "minecraft:bone" => Some(3),
        "minecraft:cat" if in_item_tag("minecraft:cat_food", stack.item) => Some(3),
        "minecraft:parrot" if in_item_tag("minecraft:parrot_food", stack.item) => Some(10),
        _ => None,
    }
}

/// `FollowOwnerGoal`: how far away an owner must be for a pet to follow, and how close it
/// gets.
fn follow_distances(name: &str) -> (f64, f64) {
    match name {
        "minecraft:cat" => (10.0, 5.0),
        "minecraft:parrot" => (5.0, 1.0),
        _ => (10.0, 2.0),
    }
}

impl Server {
    /// Right-click on a wolf, cat or parrot (`Wolf`, `Cat`, `Parrot.mobInteract`): tame it,
    /// heal it, dye its collar, give a wolf armour, or tell it to sit or stand. Returns
    /// whether the click was used; feeding a pet to breed it is left to the breeding code.
    pub(super) fn pet_interact(&mut self, player: i32, world: usize, target: i32, hand: i32, held: Option<&ItemStack>) -> bool {
        let Some(e) = self.worlds[world].entities.get(&target) else { return false };
        let name = e.info.name.clone();
        let Some(mob) = e.mob() else { return false };
        let Some(p) = self.players.get(&player) else { return false };
        let owned = mob.owner == Some(p.uuid());
        let (tamed, baby, health, max_health, angry) = (mob.tamed, mob.baby, mob.living.health, mob.living.max_health, mob.brain.target.is_some());
        let pos = e.pos;
        if !tamed {
            let Some(chance) = held.and_then(|s| taming(&name, s)) else { return false };
            if name == "minecraft:wolf" && angry {
                return false;
            }
            self.use_held(player, hand);
            if name == "minecraft:parrot" {
                self.play_sound(world, pos, "minecraft:entity.parrot.eat", category::NEUTRAL, 1.0, 1.0, None);
            }
            let success = rand::thread_rng().gen_range(0..chance) == 0;
            if success && crate::plugins::events::on_tame(self, world, target, player) {
                self.tame(world, target, player);
                self.set_sitting(world, target, true);
            } else {
                self.entity_event(world, target, 6);
            }
            return true;
        }
        if let Some(stack) = held {
            let food_tag = match name.as_str() {
                "minecraft:wolf" => Some("minecraft:wolf_food"),
                "minecraft:cat" => Some("minecraft:cat_food"),
                _ => None,
            };
            // Food heals a hurt pet: a wolf twice the food's nutrition, a cat once.
            if food_tag.is_some_and(|t| in_item_tag(t, stack.item)) && health < max_health {
                let nutrition = paper_data::item_data::by_id(stack.item as u32).food.as_ref().map_or(1, |f| f.nutrition) as f32;
                let heal = if name == "minecraft:wolf" { 2.0 * nutrition } else { nutrition };
                if let Some(m) = self.worlds[world].entities.get_mut(&target).and_then(|e| e.mob_mut()) {
                    m.living.health = (m.living.health + heal).min(m.living.max_health);
                }
                if let Some(e) = self.worlds[world].entities.get_mut(&target) {
                    e.metadata_dirty = true;
                }
                self.use_held(player, hand);
                return true;
            }
            // A dye colours the collar (`DyeItem`).
            let held_name = item_name(stack);
            if owned && matches!(name.as_str(), "minecraft:wolf" | "minecraft:cat") && held_name.ends_with("_dye") {
                let color = super::animals::COLORS.iter().position(|c| held_name == format!("minecraft:{c}_dye")).map(|c| c as i32);
                if let Some(color) = color.filter(|c| *c != self.worlds[world].entities[&target].mob().map_or(-1, |m| m.collar)) {
                    if let Some(e) = self.worlds[world].entities.get_mut(&target) {
                        if let Some(m) = e.mob_mut() {
                            m.collar = color;
                        }
                        e.metadata_dirty = true;
                    }
                    self.use_held(player, hand);
                    return true;
                }
            }
            // Wolf armour.
            if owned && name == "minecraft:wolf" && !baby && held_name == "minecraft:wolf_armor" && self.worlds[world].entities[&target].mob().is_some_and(|m| m.equipment[6].is_empty()) {
                if let Some(e) = self.worlds[world].entities.get_mut(&target) {
                    if let Some(m) = e.mob_mut() {
                        m.equipment[6] = paper_protocol::types::Slot(Some(stack.with_count(1)));
                    }
                    e.equipment_dirty = true;
                }
                self.use_held(player, hand);
                self.play_sound(world, pos, "minecraft:item.armor.equip_wolf", category::NEUTRAL, 1.0, 1.0, None);
                return true;
            }
            // Breeding food goes to the breeding code.
            if super::animals::is_food(&name, stack.item) {
                return false;
            }
        }
        if owned {
            let sitting = self.worlds[world].entities[&target].mob().is_some_and(|m| m.sitting);
            self.set_sitting(world, target, !sitting);
            if let Some(m) = self.worlds[world].entities.get_mut(&target).and_then(|e| e.mob_mut()) {
                m.brain.target = None;
            }
            return true;
        }
        false
    }

    /// `setOrderedToSit` (and the sitting pose that goes with it).
    pub fn set_sitting(&mut self, world: usize, id: i32, sitting: bool) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        if let Some(m) = e.mob_mut() {
            m.sitting = sitting;
            m.brain.path.clear();
            m.brain.walk_target = None;
        }
        e.metadata_dirty = true;
    }

    /// A pet's own tick, before its common AI: sitting pets stay put; the others follow
    /// their owner and (wolves) join the owner's fights.
    pub(super) fn tick_pet(&mut self, world: usize, id: i32) {
        let tick = self.tick;
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let name = e.info.name.clone();
        let (pos, width) = (e.pos, e.info.width as f64);
        let Some(mob) = e.mob() else { return };
        if !mob.tamed || mob.living.is_dead() {
            return;
        }
        let Some(owner) = mob.owner.and_then(|u| self.by_uuid.get(&u).copied()) else { return };
        let Some(p) = self.players.get(&owner) else { return };
        if mob.sitting {
            if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                m.brain.path.clear();
                m.brain.walk_target = None;
            }
            return;
        }
        if p.world != world || p.dead || p.game_mode == GameMode::Spectator {
            return;
        }
        let at = Vec3::new(p.loc.x, p.loc.y, p.loc.z);
        // `OwnerHurtByTargetGoal` and `OwnerHurtTargetGoal`: wolves take on whoever hurt
        // their owner, or whatever the owner hit (not other pets of the owner, nor creepers).
        if name == "minecraft:wolf" && mob.brain.target.is_none() {
            let recent = |e: Option<(i32, u64)>| e.filter(|(_, t)| tick.saturating_sub(*t) < 100).map(|(e, _)| e);
            let foe = recent(p.last_attacker).or_else(|| recent(p.last_hurt_mob)).filter(|f| *f != id && *f != owner);
            if let Some(foe) = foe.filter(|f| self.wolf_may_attack(world, *f, p.uuid())) {
                if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                    m.brain.target = Some(foe);
                    m.brain.path.clear();
                }
                return;
            }
        }
        let Some(mob) = self.worlds[world].entities.get(&id).and_then(|e| e.mob()) else { return };
        if mob.brain.target.is_some() {
            return;
        }
        // `FollowOwnerGoal`: far behind, it teleports close; else it walks over.
        let (start, stop) = follow_distances(&name);
        let d2 = at.distance_squared(pos);
        if d2 >= 144.0 {
            self.teleport_to_owner(world, id, at, width);
        } else if d2 >= start * start {
            if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                m.brain.walk_target = Some(at);
            }
        } else if d2 <= stop * stop {
            if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                if m.brain.walk_target.is_some() {
                    m.brain.walk_target = None;
                    m.brain.path.clear();
                }
            }
        }
    }

    /// `Wolf.wantsToAttack`: not its owner's other pets, tamed horses or creepers.
    fn wolf_may_attack(&self, world: usize, target: i32, owner: uuid::Uuid) -> bool {
        if self.players.contains_key(&target) {
            return true;
        }
        let Some(e) = self.worlds[world].entities.get(&target) else { return false };
        let Some(m) = e.mob() else { return false };
        if matches!(e.info.name.as_str(), "minecraft:creeper" | "minecraft:ghast" | "minecraft:happy_ghast") {
            return false;
        }
        !(m.tamed && (m.owner == Some(owner) || super::riding::is_horse(&e.info.name)))
    }

    /// `tryToTeleportToOwner`: ten tries at a spot near the owner, not right next to them.
    fn teleport_to_owner(&mut self, world: usize, id: i32, owner: Vec3, width: f64) {
        let mut rng = rand::thread_rng();
        let w = &self.worlds[world];
        for _ in 0..10 {
            let (dx, dy, dz) = (rng.gen_range(-3..=3), rng.gen_range(-1..=1), rng.gen_range(-3..=3));
            if dx * dx < 4 && dz * dz < 4 {
                continue;
            }
            let (x, y, z) = (owner.x.floor() as i32 + dx, owner.y.floor() as i32 + dy, owner.z.floor() as i32 + dz);
            if !super::mobs::walkable(w, x, y, z, 1) {
                continue;
            }
            let to = Vec3::new(x as f64 + 0.5, y as f64, z as f64 + 0.5);
            if crate::world::entity::collides(w, &crate::world::entity::Aabb::of_entity(to, width, 0.85)) {
                continue;
            }
            let e = self.worlds[world].entities.get_mut(&id).unwrap();
            e.pos = to;
            e.vel = Vec3::ZERO;
            e.force_sync = true;
            if let Some(m) = e.mob_mut() {
                m.brain.path.clear();
                m.brain.walk_target = None;
            }
            self.worlds[world].reindex_entity(id);
            return;
        }
    }
}
