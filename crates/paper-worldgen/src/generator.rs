//! The world generator: construction from data and the per-chunk stages.
//!
//! The stage drivers are derived from `pumpkin-world/src/generation/{proto_chunk,
//! generator/mod}.rs` of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::aquifer::{Aquifer, AquiferFunctions, FluidPicker};
use crate::blocks::BlockFlags;
use crate::chunk::ProtoChunk;
use crate::data::{parse_block_state, DataReader, DataSource, JsonExt};
use crate::density::{build_stack, prepare_all, SeededNoises, SeededStack};
use crate::noise::DensityVolume;
use crate::random::{LegacyRandom, PositionalRandom, Xoroshiro};
use crate::router::{ChunkRouter, SurfaceEstimator};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;

/// The three vanilla dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
}

impl Dimension {
    /// The dimension's id in world presets.
    pub fn id(self) -> &'static str {
        match self {
            Dimension::Overworld => "minecraft:overworld",
            Dimension::Nether => "minecraft:the_nether",
            Dimension::End => "minecraft:the_end",
        }
    }
}

/// The terrain stages, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    Biomes,
    Noise,
    Surface,
    Carvers,
}

/// Stack indices of the terrain functions.
#[derive(Clone, Copy, Debug)]
struct TerrainFunctions {
    final_density: usize,
    aquifer: AquiferFunctions,
}

/// Generates chunks of one dimension for one seed. Immutable after construction and shared
/// by worker threads.
pub struct WorldGenerator {
    pub(crate) dimension: Dimension,
    pub(crate) seed: i64,
    pub(crate) min_y: i32,
    pub(crate) height: i32,
    pub(crate) gen_min_y: i32,
    pub(crate) gen_height: i32,
    pub(crate) sea_level: i32,
    pub(crate) default_block: u32,
    pub(crate) default_fluid: u32,
    pub(crate) aquifers_enabled: bool,
    pub(crate) legacy_random: bool,
    pub(crate) flags: BlockFlags,
    terrain: SeededStack,
    terrain_fns: TerrainFunctions,
    surface_stack: SeededStack,
    pub(crate) climate: SeededStack,
    pub(crate) base_random: PositionalRandom,
    pub(crate) aquifer_random: PositionalRandom,
    pub(crate) ore_random: PositionalRandom,
    water: u32,
    lava: u32,
}

impl WorldGenerator {
    /// Builds the generator for `dimension` from the vanilla data in `data`.
    pub fn new(data: &dyn DataSource, dimension: Dimension, seed: i64) -> Result<Self> {
        let reader = DataReader::new(data);
        Self::build(&reader, dimension, seed).with_context(|| format!("building the {} generator", dimension.id()))
    }

    fn build(data: &DataReader, dimension: Dimension, seed: i64) -> Result<Self> {
        let preset = data.worldgen("world_preset", "minecraft:normal")?;
        let dim = preset
            .get("dimensions")
            .and_then(|d| d.get(dimension.id()))
            .ok_or_else(|| anyhow!("world preset has no {}", dimension.id()))?;
        let dim_type = data.registry("dimension_type", dim.str_field("type")?)?;
        let generator = dim.field("generator")?;
        let settings_ref = generator.field("settings")?;
        let settings = match settings_ref {
            Value::String(s) => (*data.worldgen("noise_settings", s)?).clone(),
            other => other.clone(),
        };

        let min_y = dim_type.i64_or("min_y", 0) as i32;
        let height = dim_type.i64_or("height", 256) as i32;
        let noise = settings.field("noise")?;
        let noise_min_y = noise.i64_or("min_y", min_y as i64) as i32;
        let noise_height = noise.i64_or("height", height as i64) as i32;
        let gen_min_y = noise_min_y.max(min_y);
        let gen_height = (noise_min_y + noise_height).min(min_y + height) - gen_min_y;

        let legacy_random = settings.bool_or("legacy_random_source", false);
        let sea_level = settings.i64_or("sea_level", 63) as i32;
        let default_block = parse_block_state(settings.field("default_block")?)?;
        let default_fluid = parse_block_state(settings.field("default_fluid")?)?;
        let aquifers = settings.get("aquifers");

        let base_random = if legacy_random { LegacyRandom::new(seed).fork_positional() } else { Xoroshiro::new(seed).fork_positional() };
        let aquifer_random = base_random.from_hash_of("minecraft:aquifer").fork_positional();
        let ore_random = base_random.from_hash_of("minecraft:ore").fork_positional();

        let router = settings.field("noise_router")?;
        let aq = |k: &str| aquifers.and_then(|a| a.get(k));
        let terrain_defs = [router.get("final_density"), aq("barrier"), aq("fluid_level_floodedness"), aq("fluid_level_spread"), aq("lava"), aq("exclusion")];
        let surface_def = aq("surface_level").or_else(|| router.get("preliminary_surface_level")).or_else(|| router.get("chunk_surface_level"));
        let climate_defs = [router.get("temperature"), router.get("vegetation"), router.get("continents"), router.get("erosion"), router.get("depth"), router.get("ridges")];

        let mut noises = SeededNoises::new(data, seed, legacy_random, base_random);
        let terrain = build_stack(&prepare_all(data, &terrain_defs)?, &mut noises)?;
        let surface_stack = build_stack(&prepare_all(data, &[surface_def])?, &mut noises)?;
        let climate = build_stack(&prepare_all(data, &climate_defs)?, &mut noises)?;
        let terrain_fns = TerrainFunctions {
            final_density: terrain.root(0),
            aquifer: AquiferFunctions { barrier: terrain.root(1), floodedness: terrain.root(2), spread: terrain.root(3), lava: terrain.root(4), exclusion: terrain.root(5) },
        };

        Ok(Self {
            dimension,
            seed,
            min_y,
            height,
            gen_min_y,
            gen_height,
            sea_level,
            default_block,
            default_fluid,
            aquifers_enabled: aquifers.is_some(),
            legacy_random,
            flags: BlockFlags::load(data)?,
            terrain,
            terrain_fns,
            surface_stack,
            climate,
            base_random,
            aquifer_random,
            ore_random,
            water: crate::blocks::state("minecraft:water"),
            lava: crate::blocks::state("minecraft:lava"),
        })
    }

    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// Lowest block y of the dimension.
    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    /// Height of the dimension in blocks.
    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    pub(crate) fn fluid_picker(&self) -> FluidPicker {
        FluidPicker::new(self.sea_level, self.default_fluid, self.lava)
    }

    pub(crate) fn surface_estimator(&self) -> SurfaceEstimator<'_> {
        SurfaceEstimator::new(&self.surface_stack, self.gen_min_y)
    }

    /// Runs the terrain stages up to and including `last`. [`WorldGenerator::generate_terrain`]
    /// runs all of them; stopping early is mainly useful for tests.
    pub fn generate_until(&self, chunk_x: i32, chunk_z: i32, last: Stage) -> ProtoChunk {
        let mut chunk = ProtoChunk::new(chunk_x, chunk_z, self.min_y, self.height);
        let mut estimator = self.surface_estimator();
        if last >= Stage::Noise {
            self.generate_noise(&mut chunk, &mut estimator);
        }
        chunk
    }

    /// The noise stage: terrain shape, aquifers and the default fluid.
    pub fn generate_noise(&self, chunk: &mut ProtoChunk, estimator: &mut SurfaceEstimator) {
        let volume = DensityVolume::blocks(16, self.gen_height as usize, 16, chunk.start_x(), self.gen_min_y, chunk.start_z());
        let mut router = ChunkRouter::terrain(&self.terrain);
        let mut density = vec![0.0f32; volume.size()];
        router.sample_volume(self.terrain_fns.final_density, &mut density, &volume);
        let picker = self.fluid_picker();
        let mut aquifer = self.aquifers_enabled.then(|| {
            Aquifer::new(chunk.chunk_x, chunk.chunk_z, &self.aquifer_random, self.gen_min_y, self.gen_height, picker, self.terrain_fns.aquifer, self.water, self.lava)
        });
        for z in 0..volume.size_z {
            let bz = volume.block_z(z);
            for x in 0..volume.size_x {
                let bx = volume.block_x(x);
                for y in (0..volume.size_y).rev() {
                    let by = volume.block_y(y);
                    let d = density[volume.index(x, y, z)];
                    let state = match &mut aquifer {
                        Some(a) => a.compute(&mut router, estimator, bx, by, bz, d).0,
                        None => (d <= 0.0).then(|| picker.level(by).at(by)),
                    };
                    chunk.set(bx, by, bz, state.unwrap_or(self.default_block), &self.flags);
                }
            }
        }
    }
}
