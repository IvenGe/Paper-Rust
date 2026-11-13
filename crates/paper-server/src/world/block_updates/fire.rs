//! Fire and Nether portal blocks.
//!
//! Fire follows vanilla's `BaseFireBlock`/`FireBlock` in outline: it lights a Nether portal
//! when placed inside an empty frame (in the overworld and the Nether), needs a sturdy block
//! below or a flammable neighbour, ages on scheduled ticks, burns out unless it sits on an
//! `infiniburn` block, and burns and spreads to flammable blocks. Burn and spread chances are
//! simplified: vanilla's per-block flammability table is not ported, every block with the
//! burnable flag counts alike.
//!
//! Nether portal blocks vanish when a block above, below or beside them along the portal
//! breaks the frame (`NetherPortalBlock.updateShape`).

use super::state::{self, Dir, Kind};
use super::{flags, Effect, Level};
use crate::world::portal::{Axis, PortalShape};
use paper_data::blocks;
use paper_protocol::codec::BlockPos;

fn at(pos: BlockPos, dir: Dir) -> BlockPos {
    let (dx, dy, dz) = dir.offset();
    pos.offset(dx, dy, dz)
}

fn burnable(state: u32) -> bool {
    paper_data::physics::flags(state) & paper_data::physics::BURNABLE != 0
}

fn in_tag(state: u32, tag: &str) -> bool {
    paper_data::synced::tag("minecraft:block", tag).is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(state) as i32)))
}

/// Portals can be lit in the overworld and the Nether.
fn in_portal_dimension(level: &Level) -> bool {
    matches!(level.world.dimension.as_str(), "minecraft:overworld" | "minecraft:the_nether")
}

/// A neighbour fire could burn (`isValidFireLocation`).
fn next_to_fuel(level: &Level, pos: BlockPos) -> bool {
    Dir::ALL.iter().any(|d| burnable(level.state(at(pos, *d))))
}

/// `FireBlock.canSurvive` / `SoulFireBlock.canSurvive`.
pub fn can_survive(level: &Level, state: u32, pos: BlockPos) -> bool {
    let below = level.state(at(pos, Dir::Down));
    if blocks::block_of_state(state).name == "minecraft:soul_fire" {
        return in_tag(below, "minecraft:soul_fire_base_blocks");
    }
    paper_data::physics::is_face_sturdy(below, Dir::Up.index()) || next_to_fuel(level, pos)
}

/// The fire a flint and steel or fire charge would make at `pos` (`BaseFireBlock.getState`).
pub fn fire_state_for(world: &crate::world::World, pos: BlockPos) -> u32 {
    let below = world.block_at(pos.x, pos.y - 1, pos.z).unwrap_or(blocks::AIR);
    if in_tag(below, "minecraft:soul_fire_base_blocks") {
        blocks::default_state("minecraft:soul_fire").unwrap_or(blocks::AIR)
    } else {
        blocks::default_state("minecraft:fire").unwrap_or(blocks::AIR)
    }
}

fn tick_delay(level: &mut Level) -> i32 {
    30 + (level.random() % 10) as i32
}

/// `BaseFireBlock.onPlace`: an empty portal frame lights up; otherwise the fire starts
/// ticking (or goes out at once where it cannot burn).
pub fn on_place(level: &mut Level, state: u32, pos: BlockPos, old: u32) {
    if state::kind(old) == Kind::Fire {
        return;
    }
    if in_portal_dimension(level) {
        if let Some(shape) = PortalShape::find_empty(level.world, pos, Axis::X) {
            create_portal(level, &shape);
            return;
        }
    }
    if !can_survive(level, state, pos) {
        level.remove_block(pos, false);
        return;
    }
    let delay = tick_delay(level);
    level.schedule_tick(pos, state::block_id(state), delay);
}

/// Fills a portal frame with portal blocks (without neighbour updates, as vanilla).
pub fn create_portal(level: &mut Level, shape: &PortalShape) {
    let Some(portal) = blocks::default_state("minecraft:nether_portal").and_then(|s| blocks::with_property(s, "axis", shape.axis.name())) else { return };
    for pos in shape.inside() {
        level.set_block(pos, portal, flags::CLIENTS | flags::KNOWN_SHAPE);
    }
}

pub fn update_shape(level: &mut Level, state: u32, pos: BlockPos) -> u32 {
    if can_survive(level, state, pos) {
        state
    } else {
        blocks::AIR
    }
}

/// Scheduled fire tick: age, burn out, burn and spread.
pub fn tick(level: &mut Level, state: u32, pos: BlockPos) {
    let delay = tick_delay(level);
    level.schedule_tick(pos, state::block_id(state), delay);
    if !level.settings().fire_spreads {
        return;
    }
    if !can_survive(level, state, pos) {
        level.remove_block(pos, false);
        return;
    }
    let below = level.state(at(pos, Dir::Down));
    let infiniburn = in_tag(below, level.world_infiniburn());
    let age: i32 = blocks::property(state, "age").and_then(|a| a.parse().ok()).unwrap_or(0);
    let grown = (age + (level.random() % 3) as i32 / 2).min(15);
    if grown != age {
        if let Some(s) = blocks::with_property(state, "age", &grown.to_string()) {
            level.set_block(pos, s, flags::INVISIBLE);
        }
    }
    if !infiniburn {
        if !next_to_fuel(level, pos) {
            if !paper_data::physics::is_face_sturdy(below, Dir::Up.index()) || grown > 3 {
                level.remove_block(pos, false);
            }
            return;
        }
        if grown == 15 && level.random() % 4 == 0 && !burnable(below) {
            level.remove_block(pos, false);
            return;
        }
    }
    // Neighbours burn away, catching fire themselves now and then.
    for dir in Dir::ALL {
        let p = at(pos, dir);
        let neighbour = level.state(p);
        if !burnable(neighbour) || level.random() % 12 != 0 {
            continue;
        }
        if state::kind(neighbour) == Kind::Tnt && level.settings().tnt_explodes {
            let igniter = level.actor;
            level.world.updates.effects.push(Effect::PrimeTnt { pos: p, igniter });
            level.remove_block(p, false);
        } else if level.random() % (grown as u64 + 10) < 5 {
            let fire = fire_state_for(level.world, p);
            let aged = blocks::with_property(fire, "age", &(grown + (level.random() % 5) as i32 / 4).min(15).to_string()).unwrap_or(fire);
            level.set_block(p, aged, flags::ALL);
        } else {
            level.remove_block(p, false);
        }
    }
    // Spreading into the air around flammable blocks.
    if level.random() % 6 == 0 {
        let r = level.random();
        let p = pos.offset((r % 3) as i32 - 1, ((r >> 8) % 6) as i32 - 1, ((r >> 16) % 3) as i32 - 1);
        if p != pos && blocks::is_air(level.state(p)) && next_to_fuel(level, p) {
            let fire = fire_state_for(level.world, p);
            level.set_block(p, fire, flags::ALL);
        }
    }
}

/// `NetherPortalBlock.updateShape`: a portal block goes when the frame it belongs to is no
/// longer complete (checked for neighbours above, below and along the portal).
pub fn portal_update_shape(level: &mut Level, state: u32, pos: BlockPos, dir: Dir, neighbor_state: u32) -> u32 {
    let axis = Axis::of_state(state);
    let along = match dir {
        Dir::Up | Dir::Down => true,
        Dir::East | Dir::West => axis == Axis::X,
        Dir::North | Dir::South => axis == Axis::Z,
    };
    if along && state::kind(neighbor_state) != Kind::NetherPortal && !PortalShape::find_any(level.world, pos, axis).is_complete() {
        blocks::AIR
    } else {
        state
    }
}
