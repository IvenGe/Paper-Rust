//! Block state properties the generator needs, derived from paper-data and the jar's tags.

use crate::data::DataReader;
use anyhow::Result;
use paper_data::blocks;

pub const AIR: u8 = 1;
/// In the `minecraft:blocks_motion` tag (vanilla's "blocks motion" for heightmaps).
pub const BLOCKS_MOTION: u8 = 2;
/// Has a fluid state (fluids, waterlogged blocks, kelp, seagrass, ...).
pub const FLUID: u8 = 4;
pub const LEAVES: u8 = 8;
/// In `minecraft:replaceable`.
pub const REPLACEABLE: u8 = 16;

/// Per-state flags.
pub struct BlockFlags {
    flags: Vec<u8>,
}

impl BlockFlags {
    pub fn load(data: &DataReader) -> Result<Self> {
        let count = blocks::STATE_COUNT as usize;
        let motion = data.block_tag("minecraft:blocks_motion")?;
        let leaves = data.block_tag("minecraft:leaves")?;
        let replaceable = data.block_tag("minecraft:replaceable")?;
        let mut flags = vec![0u8; count];
        for (state, f) in flags.iter_mut().enumerate() {
            let state = state as u32;
            let block = blocks::block_id_of_state(state);
            let mut v = 0;
            if blocks::is_air(state) {
                v |= AIR;
            }
            if motion.contains(&block) {
                v |= BLOCKS_MOTION;
            }
            if blocks::has_fluid(state) {
                v |= FLUID;
            }
            if leaves.contains(&block) {
                v |= LEAVES;
            }
            if replaceable.contains(&block) {
                v |= REPLACEABLE;
            }
            *f = v;
        }
        Ok(Self { flags })
    }

    #[inline]
    pub fn get(&self, state: u32) -> u8 {
        self.flags.get(state as usize).copied().unwrap_or(AIR)
    }

    #[inline]
    pub fn is_air(&self, state: u32) -> bool {
        self.get(state) & AIR != 0
    }

    #[inline]
    pub fn blocks_motion(&self, state: u32) -> bool {
        self.get(state) & BLOCKS_MOTION != 0
    }

    #[inline]
    pub fn has_fluid(&self, state: u32) -> bool {
        self.get(state) & FLUID != 0
    }

    #[inline]
    pub fn is_leaves(&self, state: u32) -> bool {
        self.get(state) & LEAVES != 0
    }
}

/// Looks up a block's default state, panicking with a clear message for blocks the data
/// tables must contain.
pub fn state(name: &str) -> u32 {
    blocks::default_state(name).unwrap_or_else(|| panic!("block {name} is missing from paper-data"))
}

/// The block (registry) id of a state.
#[inline]
pub fn block_of(state: u32) -> u32 {
    blocks::block_id_of_state(state)
}
