//! Block updates on the server: the entry point game code uses to change blocks, the
//! per-tick processing of scheduled ticks and block events, player interaction with
//! redstone components and doors, placement rules, and publishing what block updates did.
//!
//! Every block change that should behave like vanilla (neighbours react, fluids flow,
//! redstone propagates) goes through [`Server::set_block_and_update`] (or one of its
//! siblings here); [`crate::world::World::set_block`] alone only stores the block.

use super::player::OFFHAND_SLOT;
use super::Server;
use crate::config::GameMode;
use crate::world::block_updates::state::kind;
use crate::world::block_updates::support::{self, PlaceContext};
use crate::world::block_updates::{self, flags, Dir, Effect, Kind, Level, UseContext};
use bytes::Bytes;
use paper_nbt::Compound;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play::{self, SoundRef};
use paper_protocol::packets::{encode_packet, Packet};

/// Players this far from a world or block event see it (vanilla's 64 blocks).
const EVENT_RANGE: f64 = 64.0;

/// Vanilla's `Direction.orderedByNearest(entity)[0]`: the direction a player looks most.
pub fn nearest_looking_direction(yaw: f32, pitch: f32) -> Dir {
    let pitch = pitch.to_radians();
    let yaw = -yaw.to_radians();
    let (pitch_sin, pitch_cos) = (pitch.sin(), pitch.cos());
    let (yaw_sin, yaw_cos) = (yaw.sin(), yaw.cos());
    let x_yaw = yaw_sin.abs();
    let y_mag = pitch_sin.abs();
    let z_yaw = yaw_cos.abs();
    let axis_x = if yaw_sin > 0.0 { Dir::East } else { Dir::West };
    let axis_y = if pitch_sin < 0.0 { Dir::Up } else { Dir::Down };
    let axis_z = if yaw_cos > 0.0 { Dir::South } else { Dir::North };
    if x_yaw > z_yaw {
        if y_mag > x_yaw * pitch_cos {
            axis_y
        } else {
            axis_x
        }
    } else if y_mag > z_yaw * pitch_cos {
        axis_y
    } else {
        axis_z
    }
}

impl Server {
    /// Changes a block the way vanilla's `Level.setBlock(pos, state, UPDATE_ALL)` does:
    /// clients are told (with the light changes), the old block reacts to its removal, the
    /// new one to its placement, neighbours get neighbour and shape updates (wires connect,
    /// torches without support break, water flows in, ...) and blocks schedule their ticks.
    /// Everything this causes is sent to clients before it returns. Returns whether the block
    /// changed.
    ///
    /// This is the entry point for game code that changes blocks.
    pub fn set_block_and_update(&mut self, world: usize, pos: BlockPos, state: u32) -> bool {
        self.set_block_with_flags(world, pos, state, flags::ALL)
    }

    /// [`Server::set_block_and_update`] with vanilla's update flags (`world::block_updates::flags`),
    /// e.g. `flags::CLIENTS | flags::KNOWN_SHAPE` to change a block without any updates.
    pub fn set_block_with_flags(&mut self, world: usize, pos: BlockPos, state: u32, flags: u32) -> bool {
        let Some(w) = self.worlds.get_mut(world) else { return false };
        let changed = Level::new(w).set_block(pos, state, flags);
        self.publish_block_updates(world);
        changed
    }

    /// Breaks a block like `Level.destroyBlock`: the break effect for everyone, items (if
    /// `drop`), and the block replaced by the fluid it held.
    pub fn destroy_block(&mut self, world: usize, pos: BlockPos, drop: bool) -> bool {
        let Some(w) = self.worlds.get_mut(world) else { return false };
        let destroyed = Level::new(w).destroy_block(pos, drop);
        self.publish_block_updates(world);
        destroyed
    }

    /// A player breaks a block (`playerWillDestroy`, then `removeBlock`): a waterlogged
    /// block leaves its water, the other half of a door or tall plant follows. Returns
    /// whether the block was removed.
    pub fn player_destroy_block(&mut self, world: usize, pos: BlockPos, player: i32) -> bool {
        let creative = self.players.get(&player).is_some_and(|p| p.game_mode == GameMode::Creative);
        let shears = self.players.get(&player).is_some_and(|p| p.held_item().0.as_ref().is_some_and(|s| paper_data::items::get(s.item as u32).is_some_and(|i| i.name == "minecraft:shears")));
        let Some(w) = self.worlds.get_mut(world) else { return false };
        let mut level = Level::new(w);
        level.actor = Some(player);
        let state = level.state(pos);
        level.player_will_destroy(state, pos, creative, shears);
        let removed = level.remove_block(pos, false);
        self.publish_block_updates(world);
        removed
    }

    /// Runs the scheduled block ticks, fluid ticks and block events of every world for this
    /// game tick. Chunks run theirs while players keep them loaded.
    pub fn tick_block_updates(&mut self) {
        for index in 0..self.worlds.len() {
            block_updates::tick(&mut self.worlds[index], &|w, p| w.has_ticket(p) && w.is_ready(p));
            block_updates::random_ticks(&mut self.worlds[index], &|w, p| w.has_ticket(p) && w.is_ready(p));
            self.publish_block_updates(index);
        }
    }

    /// A player right-clicks a block (`useWithoutItem`): levers, buttons, repeaters,
    /// comparators, wooden doors, trapdoors and fence gates, note blocks and redstone wire
    /// react instead of the held item being placed. Sneaking with an item in either hand
    /// skips this, as does the off hand. Returns whether the block used the click.
    pub fn use_block(&mut self, id: i32, use_on: &play::UseItemOn) -> bool {
        let Some(player) = self.players.get(&id) else { return false };
        if use_on.hand.0 != 0 || matches!(player.game_mode, GameMode::Spectator) {
            return false;
        }
        let holds_item = !player.held_item().is_empty() || !player.inventory[OFFHAND_SLOT].is_empty();
        if player.sneaking && holds_item {
            return false;
        }
        let world = player.world;
        let pos = use_on.pos;
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return false };
        // NoteBlock.useItemOn: a head used on the top of a note block is placed instead.
        if kind(state) == Kind::NoteBlock && use_on.face.0 == 1 {
            let head = player.held_item().0.as_ref().is_some_and(|s| paper_data::synced::tag("minecraft:item", "minecraft:noteblock_top_instruments").is_some_and(|t| t.contains(&s.item)));
            if head {
                return false;
            }
        }
        let ctx = UseContext { player: id, may_build: matches!(player.game_mode, GameMode::Creative | GameMode::Survival), facing: Dir::from_yaw(player.loc.yaw) };
        let result = Level::new(&mut self.worlds[world]).use_without_item(state, pos, ctx);
        self.publish_block_updates(world);
        result.consumes_action()
    }

    /// Vanilla's placement rules for a block a player places at `pos` (waterlogging, wire
    /// shapes, powered doors and lamps, door hinges, fence connections, ...). `None` if the
    /// block cannot survive there.
    pub fn placement_state(&mut self, world: usize, pos: BlockPos, state: u32, player: i32, use_on: &play::UseItemOn) -> Option<u32> {
        let p = self.players.get(&player)?;
        let (yaw, pitch) = (p.loc.yaw, p.loc.pitch);
        let w = self.worlds.get_mut(world)?;
        let replaced = w.block_at(pos.x, pos.y, pos.z)?;
        let click = (
            use_on.pos.x as f64 + use_on.cursor_x as f64 - pos.x as f64,
            use_on.pos.y as f64 + use_on.cursor_y as f64 - pos.y as f64,
            use_on.pos.z as f64 + use_on.cursor_z as f64 - pos.z as f64,
        );
        let ctx = PlaceContext { facing: Dir::from_yaw(yaw), looking: nearest_looking_direction(yaw, pitch), replaced, click };
        support::placement_state(&Level::new(w), state, pos, ctx)
    }

    /// What a block does right after a player placed it (`setPlacedBy`): a powered
    /// repeater or comparator schedules its first tick, a powered piston extends.
    pub fn block_placed(&mut self, world: usize, pos: BlockPos, state: u32) {
        let Some(w) = self.worlds.get_mut(world) else { return };
        support::placed(&mut Level::new(w), state, pos);
        self.publish_block_updates(world);
    }

    /// Item drops of a block destroyed by block physics (washed away by water, lost its
    /// support, the other half of a broken door, broken by a piston, ...), like vanilla's
    /// `Block.dropResources`: the block's loot table decides what (e.g. only the lower half
    /// of a door drops the door). Blocks broken by players do not come here.
    pub fn drop_block_items(&mut self, world: usize, pos: BlockPos, state: u32, block_entity: Option<Compound>) {
        // Container contents are dropped separately when the block entity goes (see
        // `Server::process_removed_block_entities`).
        let _ = block_entity;
        if self.worlds.get(world).is_some_and(|w| w.rules.bool("block_drops")) {
            self.drop_block_loot(world, pos, state, None, None);
        }
    }

    /// Sends clients what block updates did in a world (block and light changes, sounds,
    /// world events, block events) and hands block drops to [`Server::drop_block_items`].
    pub fn publish_block_updates(&mut self, world: usize) {
        let Some(w) = self.worlds.get_mut(world) else { return };
        if w.updates.effects.is_empty() {
            return;
        }
        let effects = std::mem::take(&mut w.updates.effects);
        for effect in effects {
            match effect {
                Effect::Block(change) => self.publish_block_change(world, &change),
                Effect::LevelEvent { event, pos, data } => {
                    self.send_near(world, pos, EVENT_RANGE, None, &play::LevelEvent { event, pos, data, global: false });
                }
                Effect::Sound { pos, sound, source, volume, pitch, seed, except } => {
                    let sound = match paper_data::registries::id("minecraft:sound_event", &sound) {
                        Some(id) => SoundRef::Id(id as i32),
                        None => SoundRef::Named { name: sound, fixed_range: None },
                    };
                    let range = if volume > 1.0 { 16.0 * volume as f64 } else { 16.0 };
                    let packet = play::Sound { sound, category: source, x: pos.x as f64 + 0.5, y: pos.y as f64 + 0.5, z: pos.z as f64 + 0.5, volume, pitch, seed };
                    self.send_near(world, pos, range, except, &packet);
                }
                Effect::BlockEvent { pos, block, action, param } => {
                    self.send_near(world, pos, EVENT_RANGE, None, &play::BlockEvent { pos, action, param, block: VarInt(block as i32) });
                }
                Effect::Drops { pos, state, block_entity } => self.drop_block_items(world, pos, state, block_entity),
                Effect::PrimeTnt { pos, igniter } => self.prime_tnt(world, pos, 80, igniter),
                Effect::Dispense { pos } => self.dispense(world, pos),
                Effect::CheckPlate { pos } => self.check_pressure_plate(world, pos),
                Effect::CheckTripwire { pos } => self.check_tripwire(world, pos),
                Effect::Craft { pos } => self.crafter_craft(world, pos),
                Effect::CrafterPowered { pos } => self.send_crafter_data(world, pos),
            }
        }
    }

    /// Sends a packet to the players of a world within `range` blocks of a block.
    pub(super) fn send_near<P: Packet + paper_protocol::codec::Encode>(&self, world: usize, pos: BlockPos, range: f64, except: Option<i32>, packet: &P) {
        let payload = Bytes::from(encode_packet(packet));
        let (x, y, z) = (pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5);
        for player in self.players.values() {
            if player.world != world || Some(player.entity_id) == except {
                continue;
            }
            let (dx, dy, dz) = (player.loc.x - x, player.loc.y - y, player.loc.z - z);
            if dx * dx + dy * dy + dz * dz < range * range {
                player.conn.send_payload(payload.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looking_directions() {
        assert_eq!(nearest_looking_direction(0.0, 0.0), Dir::South);
        assert_eq!(nearest_looking_direction(180.0, 10.0), Dir::North);
        assert_eq!(nearest_looking_direction(90.0, 0.0), Dir::West);
        assert_eq!(nearest_looking_direction(-90.0, 0.0), Dir::East);
        assert_eq!(nearest_looking_direction(30.0, 80.0), Dir::Down);
        assert_eq!(nearest_looking_direction(30.0, -60.0), Dir::Up);
    }
}
