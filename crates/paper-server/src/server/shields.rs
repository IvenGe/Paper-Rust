//! Shields (vanilla's `blocks_attacks` component, with the shield's values): raised for a
//! quarter second, a shield takes all of a hit from the front, unless the damage bypasses
//! shields; it plays its sound and wears by the damage when that is at least 3. An axe that
//! hits it disables it for five seconds, unless plugins say not (`PlayerShieldDisableEvent`).

use super::player::{HOTBAR_START, OFFHAND_SLOT};
use super::survival::{category, Damage};
use super::Server;
use crate::world::entity::Vec3;
use paper_data::items;
use rand::Rng;

pub const SHIELD: &str = "minecraft:shield";
/// Ticks a shield is raised before it blocks (`block_delay_seconds` 0.25).
pub const BLOCK_DELAY: i32 = 5;
/// The longest a shield is held up (`use_duration`).
pub const USE_TICKS: i32 = 72000;

impl Server {
    /// The slot of the shield a player is blocking with (`getItemBlockingWith`): raised long
    /// enough.
    pub fn blocking_slot(&self, id: i32) -> Option<usize> {
        let p = self.players.get(&id)?;
        let (hand, remaining) = p.using_item?;
        let slot = if hand == 1 { OFFHAND_SLOT } else { HOTBAR_START + p.held_slot as usize };
        let stack = p.inventory[slot].0.as_ref()?;
        (items::get(stack.item as u32).is_some_and(|i| i.name == SHIELD) && USE_TICKS - remaining >= BLOCK_DELAY).then_some(slot)
    }

    /// Raises a shield (`ShieldItem.use`), unless it is cooling down from an axe.
    pub(super) fn raise_shield(&mut self, id: i32, hand: i32) {
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&id) else { return };
        if p.cooldowns.get(SHIELD).is_some_and(|until| *until > tick) {
            return;
        }
        p.using_item = Some((hand as u8, USE_TICKS));
        p.metadata_dirty = true;
    }

    /// How much of a hit a raised shield takes (`applyItemBlocking`): all of it from within
    /// 90 degrees of where the player looks, none from behind or of damage that bypasses
    /// shields. A blocking shield sounds, wears, and an axe in melee disables it.
    pub(super) fn shield_blocks(&mut self, id: i32, damage: &Damage, amount: f32) -> f32 {
        if amount <= 0.0 || damage.is("bypasses_shield") {
            return 0.0;
        }
        let Some(slot) = self.blocking_slot(id) else { return 0.0 };
        let p = &self.players[&id];
        let (world, at, yaw) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z), p.loc.yaw);
        let angle = match damage.position {
            Some(source) => {
                let (dx, dz) = (source.x - at.x, source.z - at.z);
                let length = (dx * dx + dz * dz).sqrt();
                if length < 1.0e-7 {
                    std::f64::consts::PI
                } else {
                    let (vx, vz) = (-(yaw.to_radians() as f64).sin(), (yaw.to_radians() as f64).cos());
                    ((dx / length) * vx + (dz / length) * vz).clamp(-1.0, 1.0).acos()
                }
            }
            None => std::f64::consts::PI,
        };
        if angle > std::f64::consts::FRAC_PI_2 + 1.0e-6 {
            return 0.0;
        }
        let blocked = amount;
        let mut rng = rand::thread_rng();
        self.play_sound(world, at, "minecraft:item.shield.block", category::PLAYERS, 1.0, 0.8 + rng.gen::<f32>() * 0.4, None);
        // `hurtBlockingItem`: from 3 damage up, a point and one per point blocked.
        if blocked >= 3.0 && self.players.get(&id).is_some_and(|p| p.game_mode != crate::config::GameMode::Creative) {
            let wear = (1.0 + blocked).floor() as i32;
            self.wear_player_item(id, slot, wear, false);
            let worn = self.players[&id].inventory[slot].clone();
            self.set_inventory_slot(id, slot, worn);
        }
        // An axe swung at it disables it (`disableBlockingForSeconds` of the attacker's weapon).
        let melee = !damage.is("is_projectile") && damage.direct.is_some() && damage.direct == damage.attacker;
        if let Some(attacker) = damage.attacker.filter(|_| melee) {
            let seconds = self.weapon_disables_blocking(world, attacker);
            if seconds > 0.0 {
                self.disable_shield(id, attacker, (seconds * 20.0).round() as i32);
            }
        }
        blocked
    }

    /// Seconds the attacker's main-hand weapon disables a shield for (its `weapon` component's
    /// `disable_blocking_for_seconds`: axes 5).
    fn weapon_disables_blocking(&self, world: usize, attacker: i32) -> f32 {
        let held = match self.players.get(&attacker) {
            Some(p) => p.held_item().0.clone(),
            None => self.worlds[world].entities.get(&attacker).and_then(|e| e.mob()).and_then(|m| m.equipment[0].0.clone()),
        };
        let Some(stack) = held else { return 0.0 };
        paper_data::item_data::by_id(stack.item as u32).raw.as_ref().and_then(|r| r["weapon"]["disable_blocking_for_seconds"].as_f64()).unwrap_or(0.0) as f32
    }

    /// `BlocksAttacks.disable`: the shield cools down and is lowered, unless plugins say not
    /// (`PlayerShieldDisableEvent`, which may change the cooldown).
    fn disable_shield(&mut self, id: i32, attacker: i32, ticks: i32) {
        let Some(ticks) = crate::plugins::events::on_shield_disable(self, id, attacker, ticks) else { return };
        self.set_cooldown(id, SHIELD, ticks);
        let Some(p) = self.players.get_mut(&id) else { return };
        p.using_item = None;
        p.metadata_dirty = true;
        let (world, at) = (p.world, Vec3::new(p.loc.x, p.loc.y, p.loc.z));
        let pitch = 0.8 + rand::thread_rng().gen::<f32>() * 0.4;
        self.play_sound(world, at, "minecraft:item.shield.break", category::PLAYERS, 1.0, pitch, None);
    }
}
