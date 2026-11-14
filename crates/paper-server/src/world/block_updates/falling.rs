//! Blocks that fall (`FallingBlock` and the other `Fallable` blocks): sand, gravel, concrete
//! powder, anvils, dragon eggs and suspicious sand and gravel check two ticks after they are
//! placed or a neighbour changes, and fall when nothing holds them up (the server turns them
//! into falling block entities, see `Effect::Fall`). Concrete powder hardens into concrete
//! next to water.

use super::fluid::{fluid_state, Fluid};
use super::state::{block_id, block_name, default_state, in_tag, is_air};
use super::{flags, Dir, Effect, Kind, Level, PosExt};
use paper_data::{blocks, physics};
use paper_protocol::codec::BlockPos;

/// `getDelayAfterPlace`.
const DELAY: i32 = 2;

/// Whether a block falls when nothing holds it.
pub fn is_falling(name: &str) -> bool {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    matches!(n, "sand" | "red_sand" | "gravel" | "suspicious_sand" | "suspicious_gravel" | "anvil" | "chipped_anvil" | "damaged_anvil" | "dragon_egg") || n.ends_with("_concrete_powder")
}

/// `FallingBlock.isFree`: what a falling block falls through (air, fire, fluids and blocks
/// that can be replaced).
pub fn is_free(state: u32) -> bool {
    is_air(state) || in_tag!(state, "fire") || matches!(super::kind(state), Kind::Water | Kind::Lava) || blocks::is_replaceable(state)
}

/// The concrete a concrete powder hardens into.
pub fn concrete_of(state: u32) -> Option<u32> {
    let name = block_name(state).strip_suffix("_powder")?;
    if !name.ends_with("_concrete") {
        return None;
    }
    blocks::default_state(name)
}

fn is_water(state: u32) -> bool {
    fluid_state(state).fluid == Fluid::Water
}

/// `ConcretePowderBlock.touchesLiquid`: water beside or above it, unless the water's side
/// facing it is a full face (a waterlogged block's solid side).
pub fn touches_water(level: &Level, pos: BlockPos) -> bool {
    Dir::ALL.iter().filter(|d| **d != Dir::Down).any(|d| {
        let neighbor = level.state(pos.rel(*d));
        is_water(neighbor) && !physics::is_face_sturdy(neighbor, d.opposite().index())
    })
}

/// `shouldSolidify`: concrete powder placed into water or next to it hardens.
pub fn hardens_at(level: &Level, pos: BlockPos, replaced: u32) -> bool {
    is_water(replaced) || touches_water(level, pos)
}

/// `onPlace`: check two ticks later.
pub fn on_place(level: &mut Level, state: u32, pos: BlockPos) {
    level.schedule_tick(pos, block_id(state), DELAY);
}

/// `updateShape`: check again soon; concrete powder next to water hardens at once.
pub fn update_shape(level: &mut Level, state: u32, pos: BlockPos) -> u32 {
    if let Some(concrete) = concrete_of(state) {
        if touches_water(level, pos) {
            return concrete;
        }
    }
    level.schedule_tick(pos, block_id(state), DELAY);
    state
}

/// `tick`: with nothing below to hold it, the block leaves its fluid (or air) behind and
/// falls (`FallingBlockEntity.fall`).
pub fn tick(level: &mut Level, state: u32, pos: BlockPos) {
    if pos.y < level.world.min_y || !is_free(level.state(pos.below())) {
        return;
    }
    let left = if is_water(state) { default_state!("water") } else { blocks::AIR };
    level.set_block(pos, left, flags::ALL);
    level.world.updates.effects.push(Effect::Fall { pos, state });
}
