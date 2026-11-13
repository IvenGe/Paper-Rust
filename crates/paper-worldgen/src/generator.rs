//! The world generator: construction from data and the per-chunk stages.
//!
//! The stage drivers are derived from `pumpkin-world/src/generation/{proto_chunk,
//! generator/mod}.rs` of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::aquifer::{Aquifer, AquiferFunctions, FluidPicker};
use crate::biome::{obfuscate_seed, BiomeSource, Biomes};
use crate::blocks::BlockFlags;
use crate::chunk::ProtoChunk;
use crate::data::{parse_block_state, DataReader, DataSource, JsonExt};
use crate::density::{build_stack, prepare_all, SeededNoises, SeededStack};
use crate::noise::DensityVolume;
use crate::random::{LegacyRandom, PositionalRandom, Xoroshiro};
use crate::router::{ChunkRouter, ClimateSampler, SurfaceEstimator};
use crate::surface::terrain::{TerrainBuilder, TerrainNoises};
use crate::surface::{RuleParser, SurfaceSystem};
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
    pub(crate) terrain: SeededStack,
    terrain_fns: TerrainFunctions,
    surface_stack: SeededStack,
    pub(crate) climate: SeededStack,
    pub(crate) base_random: PositionalRandom,
    pub(crate) aquifer_random: PositionalRandom,
    pub(crate) ore_random: PositionalRandom,
    water: u32,
    lava: u32,
    pub(crate) biomes: Biomes,
    pub(crate) biome_source: BiomeSource,
    /// `BiomeManager`'s obfuscated seed for the fuzzy biome lookup.
    pub(crate) biome_zoom_seed: i64,
    pub(crate) surface: SurfaceSystem,
    pub(crate) carvers: crate::carver::Carvers,
    pub(crate) decoration: crate::feature::Decoration,
    pub(crate) spawn_targets: Vec<crate::spawn::SpawnTarget>,
    pub(crate) has_ceiling: bool,
}

// The generator is shared by worker threads.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<WorldGenerator>();
};

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

        let biomes = Biomes::load(data)?;
        let biome_source = BiomeSource::from_json(generator.field("biome_source")?, data, &biomes)?;

        let possible = biome_source.possible_biomes();
        let mut rules = RuleParser::new(data, &biomes, base_random);
        let rule_json = settings.get("material_rule").or_else(|| settings.get("surface_rule")).ok_or_else(|| anyhow!("noise settings without material_rule"))?;
        let rule = rules.rule(rule_json)?;
        let rule_noises = rules.noises.iter().map(|n| crate::density::stack::NoiseFactory::noise(&mut noises, n)).collect::<Result<Vec<_>>>()?;
        let vein_stack = if rules.vein_functions.is_empty() { None } else { Some(build_stack(&rules.vein_functions, &mut noises)?) };
        let eroded_badlands = biomes.id("minecraft:eroded_badlands").filter(|b| possible.contains(b));
        let mut frozen_oceans = 0u128;
        for name in ["minecraft:frozen_ocean", "minecraft:deep_frozen_ocean"] {
            if let Some(b) = biomes.id(name).filter(|b| possible.contains(b)) {
                frozen_oceans |= 1u128 << b;
            }
        }
        let mut n = |id: &str| crate::density::stack::NoiseFactory::noise(&mut noises, id);
        let terrain_builder = if eroded_badlands.is_some() || frozen_oceans != 0 || rules.uses_badlands {
            Some(TerrainBuilder::new(
                &base_random,
                TerrainNoises {
                    clay_bands_offset: n("minecraft:clay_bands_offset")?,
                    badlands_pillar: n("minecraft:badlands_pillar")?,
                    badlands_surface: n("minecraft:badlands_surface")?,
                    badlands_pillar_roof: n("minecraft:badlands_pillar_roof")?,
                    iceberg_pillar: n("minecraft:iceberg_pillar")?,
                    iceberg_pillar_roof: n("minecraft:iceberg_pillar_roof")?,
                    iceberg_surface: n("minecraft:iceberg_surface")?,
                },
            ))
        } else {
            None
        };
        let surface = SurfaceSystem {
            rule,
            noises: rule_noises,
            surface_noise: n("minecraft:surface")?,
            secondary_noise: n("minecraft:surface_secondary")?,
            terrain: terrain_builder,
            vein_stack,
            eroded_badlands,
            frozen_oceans,
        };
        let carvers = crate::carver::Carvers::load(data, &biomes, &possible)?;
        let decoration = crate::feature::Decoration::load(data, &biomes, dimension, &possible).context("loading features")?;
        let spawn_targets = crate::spawn::parse_spawn_targets(&settings).context("reading spawn targets")?;

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
            biomes,
            biome_source,
            biome_zoom_seed: obfuscate_seed(seed),
            surface,
            carvers,
            decoration,
            spawn_targets,
            has_ceiling: dim_type.bool_or("has_ceiling", false),
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

    /// The vertical range height providers resolve against (`WorldGenerationContext`).
    pub(crate) fn gen_context(&self) -> crate::providers::GenContext {
        crate::providers::GenContext { min_y: self.gen_min_y, height: self.gen_height, sea_level: self.sea_level }
    }

    /// The features stage ("decoration") of the chunk at the center of `region`, which must
    /// provide that chunk and its eight neighbours after [`WorldGenerator::generate_terrain`].
    /// Features write into the neighbours too, as vanilla's do; structures are not placed.
    pub fn decorate(&self, chunk_x: i32, chunk_z: i32, region: &mut dyn crate::feature::RegionAccess) {
        self.decoration.decorate(self, chunk_x, chunk_z, region);
    }

    /// The block properties generation uses (for heightmaps of chunks read back from disk).
    pub fn block_flags(&self) -> &BlockFlags {
        &self.flags
    }

    /// Feature types the data uses that this crate does not implement (they place nothing).
    pub fn unsupported_features(&self) -> &[String] {
        &self.decoration.unsupported
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
        self.generate_biomes(&mut chunk);
        if last < Stage::Noise {
            return chunk;
        }
        let mut aquifer = self.generate_noise(&mut chunk, &mut estimator);
        if last < Stage::Surface {
            return chunk;
        }
        crate::surface::build_surface(self, &self.surface, &mut chunk, &mut estimator);
        if last < Stage::Carvers {
            return chunk;
        }
        crate::carver::carve(self, &mut chunk, aquifer.as_mut(), &mut estimator);
        chunk.prime_heightmaps(&self.flags);
        chunk.snapshot_wg_heightmaps();
        chunk
    }

    /// Runs the terrain stages (biomes, noise, surface, carvers) of one chunk. The result
    /// does not depend on any other chunk.
    pub fn generate_terrain(&self, chunk_x: i32, chunk_z: i32) -> ProtoChunk {
        self.generate_until(chunk_x, chunk_z, Stage::Carvers)
    }

    /// The biome stage: one biome per 4x4x4 cell.
    pub fn generate_biomes(&self, chunk: &mut ProtoChunk) {
        let mut climate = ClimateSampler::new(&self.climate);
        let (qx, qz) = (chunk.chunk_x << 2, chunk.chunk_z << 2);
        match &self.biome_source {
            BiomeSource::Fixed(b) => chunk.biomes.fill(*b),
            BiomeSource::End { .. } => {
                let b = self.biome_source.biome(qx, 0, qz, &mut climate);
                chunk.biomes.fill(b);
            }
            BiomeSource::MultiNoise(_) => {
                climate.fill_volume(DensityVolume::new(4, (self.height >> 2) as usize, 4, chunk.start_x(), self.min_y, chunk.start_z(), 4, 4, 4));
                for section in (self.min_y >> 4)..=((self.min_y + self.height - 1) >> 4) {
                    let qy = section << 2;
                    for x in 0..4 {
                        for y in 0..4 {
                            for z in 0..4 {
                                let b = self.biome_source.biome(qx + x, qy + y, qz + z, &mut climate);
                                chunk.set_biome(qx + x, qy + y, qz + z, b);
                            }
                        }
                    }
                }
            }
        }
    }

    /// The biome at a quart position, computed directly from the climate (vanilla's
    /// "uncached" noise biome). Used for cells of chunks that are not generated yet.
    pub fn biome_at_quart(&self, qx: i32, qy: i32, qz: i32) -> u8 {
        self.biome_at_quart_with(&mut ClimateSampler::new(&self.climate), qx, qy, qz)
    }

    /// [`WorldGenerator::biome_at_quart`] with a caller-provided climate sampler.
    pub fn biome_at_quart_with(&self, climate: &mut ClimateSampler, qx: i32, qy: i32, qz: i32) -> u8 {
        let bottom = self.min_y >> 2;
        let top = bottom + (self.height >> 2) - 1;
        self.biome_source.biome(qx, qy.clamp(bottom, top), qz, climate)
    }

    /// A sampler of the six climate parameters (the multi-noise biome inputs).
    pub fn climate_sampler(&self) -> ClimateSampler<'_> {
        ClimateSampler::new(&self.climate)
    }

    /// The name of a biome network id.
    pub fn biome_name(&self, id: u8) -> &str {
        &self.biomes.get(id).name
    }

    /// The noise stage: terrain shape, aquifers and the default fluid. Returns the aquifer,
    /// which the carver stage reuses.
    pub(crate) fn generate_noise(&self, chunk: &mut ProtoChunk, estimator: &mut SurfaceEstimator) -> Option<Aquifer> {
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
        aquifer
    }
}
