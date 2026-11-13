//! Health, damage, hunger, death and explosions.

use super::Server;
use crate::world::entity::Vec3;

impl Server {
    pub(super) fn tick_players(&mut self) {}

    pub fn explode(&mut self, _world: usize, _center: Vec3, _radius: f32, _destroy_blocks: bool, _source: Option<i32>) {}
}

impl Server {
    /// Drops, tool wear and exhaustion after a player broke a block.
    pub(super) fn after_player_broke_block(&mut self, id: i32, world: usize, pos: paper_protocol::codec::BlockPos, old: u32) {
        let Some(player) = self.players.get(&id) else { return };
        if player.game_mode == crate::config::GameMode::Creative {
            return;
        }
        let held = player.inventory[super::player::HOTBAR_START + player.held_slot as usize].clone();
        self.drop_block_loot(world, pos, old, Some(&held), None);
    }
}
