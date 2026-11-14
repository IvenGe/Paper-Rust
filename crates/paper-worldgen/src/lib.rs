//! Vanilla Minecraft 26.3 world generation for the overworld, the nether and the end.
//!
//! The generator is built at runtime from the worldgen data files of the vanilla server jar
//! (`data/minecraft/worldgen/**`), read through a [`DataSource`]. Nothing from the jar is
//! compiled into this crate except the multi-noise biome parameter trees, which vanilla
//! builds in code (see `data/26.3/README.md`).
//!
//! Generation of a chunk happens in two parts:
//!
//! * [`WorldGenerator::generate_terrain`] runs the stages that only depend on the chunk
//!   itself: biomes, noise (terrain shape, aquifers), surface (material rules, bedrock,
//!   ore veins) and carvers. It returns a [`ProtoChunk`].
//! * [`WorldGenerator::decorate`] places features. Features may write into the eight
//!   neighbouring chunks, so it works on a [`RegionAccess`] covering the 3x3 chunks around
//!   the decorated one, all of which must have their terrain generated.
//!
//! [`WorldGenerator::find_spawn`] picks the initial world spawn the way vanilla does for a
//! new world.
//!
//! The algorithms are ported from Pumpkin (https://github.com/Pumpkin-MC/Pumpkin, GPL-3.0),
//! which reproduces vanilla's output; see the crate README. Structures are generated, and
//! their starts are saved with chunks and read back
//! ([`WorldGenerator::save_structures`], [`WorldGenerator::load_structures`]).

// File headers credit Pumpkin as "Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0".
#![allow(rustdoc::bare_urls)]

pub mod aquifer;
pub mod biome;
pub mod blocks;
pub mod carver;
pub mod chunk;
pub mod data;
pub mod density;
pub mod feature;
mod generator;
pub mod math;
pub mod noise;
pub mod providers;
pub mod random;
pub mod router;
mod spawn;
pub mod structure;
pub mod surface;

pub use chunk::{Heightmap, ProtoChunk};
pub use data::DataSource;
pub use feature::{BlockEntity, Entity, Region, RegionAccess};
pub use generator::{Dimension, Stage, WorldGenerator};
