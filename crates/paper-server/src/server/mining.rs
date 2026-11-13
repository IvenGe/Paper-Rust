//! How long blocks take to break in survival (vanilla's `ServerPlayerGameMode` and
//! `BlockState.getDestroyProgress`): a client that reports a block broken too early has the
//! break delayed until the time is up, so modified clients cannot mine instantly. Other
//! players see the cracks grow.

use super::player::HOTBAR_START;
use super::survival::{self, Fluid};
use super::Server;
use crate::world::entity::Vec3;
use paper_data::{blocks, item_data};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::Slot;

impl Server {
    /// The share of a block broken per tick by a player (`getDestroyProgress`).
    pub fn destroy_progress(&self, id: i32, state: u32) -> f32 {
        let Some(p) = self.players.get(&id) else { return 0.0 };
        let hardness = blocks::block_of_state(state).hardness;
        if hardness < 0.0 || hardness.is_nan() {
            return 0.0;
        }
        if hardness == 0.0 {
            return 1.0;
        }
        let tool = &p.inventory[HOTBAR_START + p.held_slot as usize];
        let correct = !paper_data::physics::requires_correct_tool(state) || self.tool_is_correct(tool, state);
        let speed = self.destroy_speed(id, tool, state);
        speed / hardness / if correct { 30.0 } else { 100.0 }
    }

    /// `Player.getDestroySpeed`.
    fn destroy_speed(&self, id: i32, tool: &Slot, state: u32) -> f32 {
        let Some(p) = self.players.get(&id) else { return 1.0 };
        let mut speed = match tool.0.as_ref().and_then(|s| item_data::by_id(s.item as u32).tool.as_ref()) {
            Some(t) => t.rules.iter().find(|r| r.speed.is_some() && super::entities::block_in(&r.blocks, state)).and_then(|r| r.speed).unwrap_or(t.default_mining_speed),
            None => 1.0,
        };
        if speed > 1.0 {
            // Efficiency adds level² + 1 (the mining_efficiency attribute).
            let efficiency = super::loot::enchantment_level(Some(tool), "minecraft:efficiency");
            if efficiency > 0 {
                speed += (efficiency * efficiency + 1) as f32;
            }
        }
        let level = |name: &str| p.effects.iter().find(|e| e.name() == name).map(|e| e.amplifier);
        let haste = level("minecraft:haste").max(level("minecraft:conduit_power"));
        if let Some(h) = haste {
            speed *= 1.0 + (h + 1) as f32 * 0.2;
        }
        if let Some(f) = level("minecraft:mining_fatigue") {
            speed *= match f {
                0 => 0.3,
                1 => 0.09,
                2 => 0.0027,
                _ => 8.1e-4,
            };
        }
        let w = &self.worlds[p.world];
        let eye = Vec3::new(p.loc.x, p.loc.y + p.eye_height() - 0.11111111, p.loc.z);
        if survival::point_in_fluid(w, eye, Fluid::Water) && super::loot::enchantment_level(Some(&p.inventory[5]), "minecraft:aqua_affinity") == 0 {
            speed *= 0.2;
        }
        if !p.on_ground && !p.flying {
            speed /= 5.0;
        }
        speed
    }

    /// START_DESTROY_BLOCK in survival: instant blocks break at once (or are put back on the
    /// client if that fails), others start the clock.
    pub(super) fn start_digging(&mut self, id: i32, pos: BlockPos, state: u32) {
        let progress = self.destroy_progress(id, state);
        if progress >= 1.0 {
            if !self.break_block(id, pos) {
                self.resend_block(id, pos);
            }
            return;
        }
        let tick = self.tick;
        if let Some(p) = self.players.get_mut(&id) {
            p.digging = Some((pos, tick));
            p.delayed_break = None;
        }
    }

    /// STOP_DESTROY_BLOCK: accepted if at least 70% of the time has passed; otherwise the
    /// block breaks when the time is up. Returns `None` while waiting, or whether it broke.
    pub(super) fn finish_digging(&mut self, id: i32, pos: BlockPos, state: u32) -> Option<bool> {
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&id) else { return Some(false) };
        let Some((dig_pos, start)) = p.digging.take() else { return Some(false) };
        if dig_pos != pos {
            return Some(false);
        }
        let progress = self.destroy_progress(id, state) * (tick.saturating_sub(start) + 1) as f32;
        if progress >= 0.7 {
            self.crack(id, pos, -1);
            return Some(self.break_block(id, pos));
        }
        if let Some(p) = self.players.get_mut(&id) {
            p.delayed_break = Some((pos, start));
        }
        None
    }

    pub(super) fn abort_digging(&mut self, id: i32) {
        let pos = self.players.get_mut(&id).and_then(|p| p.digging.take().map(|(pos, _)| pos));
        if let Some(pos) = pos {
            self.crack(id, pos, -1);
        }
    }

    /// Delayed breaks and the crack overlay, each tick.
    pub(super) fn tick_digging(&mut self) {
        let tick = self.tick;
        let diggers: Vec<(i32, Option<(BlockPos, u64)>, Option<(BlockPos, u64)>)> = self.players.values().filter(|p| p.digging.is_some() || p.delayed_break.is_some()).map(|p| (p.entity_id, p.digging, p.delayed_break)).collect();
        for (id, digging, delayed) in diggers {
            let Some(world) = self.players.get(&id).map(|p| p.world) else { continue };
            if let Some((pos, start)) = delayed {
                let state = self.worlds[world].block_at(pos.x, pos.y, pos.z).unwrap_or(blocks::AIR);
                if blocks::is_air(state) {
                    if let Some(p) = self.players.get_mut(&id) {
                        p.delayed_break = None;
                    }
                    continue;
                }
                let progress = self.destroy_progress(id, state) * (tick.saturating_sub(start) + 1) as f32;
                if progress >= 1.0 {
                    if let Some(p) = self.players.get_mut(&id) {
                        p.delayed_break = None;
                    }
                    self.crack(id, pos, -1);
                    if !self.break_block(id, pos) {
                        self.resend_block(id, pos);
                    }
                } else {
                    self.crack(id, pos, (progress * 10.0) as i32);
                }
            } else if let Some((pos, start)) = digging {
                let state = self.worlds[world].block_at(pos.x, pos.y, pos.z).unwrap_or(blocks::AIR);
                let progress = self.destroy_progress(id, state) * (tick.saturating_sub(start) + 1) as f32;
                self.crack(id, pos, (progress * 10.0).min(9.0) as i32);
            }
        }
    }

    /// Shows other players the cracks of a block being broken (-1 removes them).
    fn crack(&mut self, id: i32, pos: BlockPos, stage: i32) {
        let Some(p) = self.players.get_mut(&id) else { return };
        if p.crack_stage == stage {
            return;
        }
        p.crack_stage = stage;
        let world = p.world;
        let packet = play::BlockDestruction { entity_id: VarInt(id), pos, stage: if stage < 0 { 255 } else { stage.min(9) as u8 } };
        for other in self.players.values().filter(|o| o.entity_id != id && o.world == world) {
            let (dx, dy, dz) = (other.loc.x - pos.x as f64, other.loc.y - pos.y as f64, other.loc.z - pos.z as f64);
            if dx * dx + dy * dy + dz * dz < 1024.0 {
                other.conn.send(&packet);
            }
        }
    }
}
