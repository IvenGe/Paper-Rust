//! What features need to know about block states: physical flags, tags, fluids, property
//! edits, survival rules and the shape updates applied at the edge of trees.
//!
//! The survival rules follow the test block registry of
//! `pumpkin-world/src/generation/proto_chunk_test/stage_parity.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, extended to the blocks vanilla
//! features place. The per-state physics table is described in `data/26.3/README.md`.

use crate::data::DataReader;
use anyhow::{anyhow, Result};
use paper_data::blocks;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use super::world::{Dir, Pos, World};

/// A set of block (registry) ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockSet {
    words: Vec<u64>,
}

impl BlockSet {
    pub fn from_ids(ids: impl IntoIterator<Item = u32>) -> Self {
        let mut s = Self::default();
        for id in ids {
            s.insert(id);
        }
        s
    }

    pub fn insert(&mut self, id: u32) {
        let w = (id >> 6) as usize;
        if self.words.len() <= w {
            self.words.resize(w + 1, 0);
        }
        self.words[w] |= 1 << (id & 63);
    }

    #[inline]
    pub fn contains_block(&self, id: u32) -> bool {
        self.words.get((id >> 6) as usize).is_some_and(|w| w & (1 << (id & 63)) != 0)
    }

    /// Whether the block of `state` is in the set.
    #[inline]
    pub fn has(&self, state: u32) -> bool {
        self.contains_block(blocks::block_id_of_state(state))
    }
}

// Per-state bits: the extracted flags (low 16), sturdy faces (next 8), then derived bits.
const IS_AIR: u32 = 1 << 0;
const REPLACEABLE: u32 = 1 << 4;
const IS_LIQUID: u32 = 1 << 5;
const IS_SOLID: u32 = 1 << 6;
const IS_FULL_CUBE: u32 = 1 << 7;
const IS_SOLID_RENDER: u32 = 1 << 10;
const BURNABLE: u32 = 1 << 1;
const SIDE_SHIFT: u32 = 16;
const UP_CENTER: u32 = 1 << (SIDE_SHIFT + 7);
const DOWN_CENTER: u32 = 1 << (SIDE_SHIFT + 6);
const WATERLOGGED: u32 = 1 << 24;
const FLUID_WATER: u32 = 1 << 25;
const FLUID_LAVA: u32 = 1 << 26;
const FLUID_SOURCE: u32 = 1 << 27;
const LEAVES: u32 = 1 << 28;
const COLLISION: u32 = 1 << 29;
const BLOCKS_MOTION: u32 = 1 << 30;

/// The fluid type of a block state (`FluidState.getType()`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fluid {
    Empty,
    Water,
    FlowingWater,
    Lava,
    FlowingLava,
}

impl Fluid {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.strip_prefix("minecraft:").unwrap_or(name) {
            "empty" => Fluid::Empty,
            "water" => Fluid::Water,
            "flowing_water" => Fluid::FlowingWater,
            "lava" => Fluid::Lava,
            "flowing_lava" => Fluid::FlowingLava,
            _ => return None,
        })
    }

    /// `FluidTags.WATER`.
    pub fn is_water(self) -> bool {
        matches!(self, Fluid::Water | Fluid::FlowingWater)
    }

    pub fn is_lava(self) -> bool {
        matches!(self, Fluid::Lava | Fluid::FlowingLava)
    }
}

/// How a block decides whether it can stay where it is (`canSurvive`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Survival {
    Always,
    /// The block below is in a tag.
    OnTag(usize),
    DoublePlant(usize),
    MangrovePropagule,
    LeafLitter,
    CactusFlower,
    Cactus,
    SugarCane,
    Mushroom,
    SmallDripleaf,
    BigDripleaf,
    SporeBlossom,
    HangingRoots,
    Carpet,
    Seagrass,
    TallSeagrass,
    Cocoa,
    SeaPickle,
    LilyPad,
    Fire,
    SoulFire,
    SnowLayer,
    CaveVines,
    GlowLichen,
    PointedDripstone,
    ChorusFlower,
    TwistingVines,
    WeepingVines,
    Kelp,
    Bamboo,
    /// Coral plants and fans: a sturdy top face below.
    CoralPlant,
    /// Coral wall fans: a sturdy face behind.
    CoralWallFan,
}

/// Tags the survival rules use, by index into `Blocks::survival_tags`.
const SURVIVAL_TAGS: &[&str] = &[
    "minecraft:supports_vegetation",
    "minecraft:supports_dry_vegetation",
    "minecraft:supports_azalea",
    "minecraft:supports_bamboo",
    "minecraft:supports_warped_fungus",
    "minecraft:supports_crimson_fungus",
    "minecraft:supports_warped_roots",
    "minecraft:supports_crimson_roots",
    "minecraft:supports_nether_sprouts",
    "minecraft:supports_nether_wart",
    "minecraft:supports_mangrove_propagule",
    "minecraft:supports_hanging_mangrove_propagule",
    "minecraft:support_override_cactus_flower",
    "minecraft:supports_cactus",
    "minecraft:supports_sugar_cane",
    "minecraft:supports_sugar_cane_adjacently",
    "minecraft:overrides_mushroom_light_requirement",
    "minecraft:supports_small_dripleaf",
    "minecraft:supports_big_dripleaf",
    "minecraft:cannot_support_seagrass",
    "minecraft:supports_cocoa",
    "minecraft:supports_lily_pad",
    "minecraft:soul_fire_base_blocks",
    "minecraft:cannot_support_snow_layer",
    "minecraft:support_override_snow_layer",
    "minecraft:cannot_support_kelp",
    "minecraft:supports_chorus_flower",
    "minecraft:supports_chorus_plant",
];

fn tag_index(name: &str) -> usize {
    SURVIVAL_TAGS.iter().position(|t| *t == name).expect("survival tag")
}

/// Named states the features place or test.
#[derive(Clone, Debug)]
pub struct Known {
    pub air: u32,
    pub cave_air: u32,
    pub void_air: u32,
    pub water: u32,
    pub lava: u32,
    pub stone: u32,
    pub dirt: u32,
    pub grass_block: u32,
    pub ice: u32,
    pub snow: u32,
    pub vine: u32,
    pub bedrock: u32,
    pub chest: u32,
    pub spawner: u32,
    pub cobblestone: u32,
    pub mossy_cobblestone: u32,
    pub magma_block: u32,
    pub bee_nest: u32,
    pub cocoa: u32,
    pub creaking_heart: u32,
    pub pale_hanging_moss: u32,
    pub pale_moss_carpet: u32,
    pub small_dripleaf: u32,
    pub tall_seagrass: u32,
    pub kelp: u32,
    pub kelp_plant: u32,
    pub seagrass: u32,
    pub bubble_column: u32,
    pub sugar_cane: u32,
    pub cactus: u32,
    pub big_dripleaf: u32,
    pub big_dripleaf_stem: u32,
    pub bamboo: u32,
    pub bamboo_sapling: u32,
    pub podzol: u32,
    pub mycelium: u32,
    pub snow_block: u32,
    pub powder_snow: u32,
    pub packed_ice: u32,
    pub blue_ice: u32,
    pub obsidian: u32,
    pub end_stone: u32,
    pub glowstone: u32,
    pub fire: u32,
    pub netherrack: u32,
    pub basalt: u32,
    pub blackstone: u32,
    pub chorus_plant: u32,
    pub chorus_flower: u32,
    pub end_portal: u32,
    pub end_gateway: u32,
    pub iron_bars: u32,
    pub end_crystal_base: u32,
    pub weeping_vines: u32,
    pub weeping_vines_plant: u32,
    pub twisting_vines: u32,
    pub twisting_vines_plant: u32,
    pub nether_wart_block: u32,
    pub warped_wart_block: u32,
    pub shroomlight: u32,
    pub sea_pickle: u32,
    pub pointed_dripstone: u32,
    pub dripstone_block: u32,
    pub cave_vines: u32,
    pub cave_vines_plant: u32,
    pub glow_lichen: u32,
    pub sculk_vein: u32,
    pub mud: u32,
    pub clay: u32,
    pub gravel: u32,
    pub sand: u32,
    pub sandstone: u32,
    pub moss_block: u32,
    pub rooted_dirt: u32,
    pub hanging_roots: u32,
}

fn st(name: &str) -> u32 {
    blocks::default_state(name).unwrap_or_else(|| panic!("block {name} is missing from paper-data"))
}

impl Known {
    fn load() -> Self {
        Self {
            air: st("minecraft:air"),
            cave_air: st("minecraft:cave_air"),
            void_air: st("minecraft:void_air"),
            water: st("minecraft:water"),
            lava: st("minecraft:lava"),
            stone: st("minecraft:stone"),
            dirt: st("minecraft:dirt"),
            grass_block: st("minecraft:grass_block"),
            ice: st("minecraft:ice"),
            snow: st("minecraft:snow"),
            vine: st("minecraft:vine"),
            bedrock: st("minecraft:bedrock"),
            chest: st("minecraft:chest"),
            spawner: st("minecraft:spawner"),
            cobblestone: st("minecraft:cobblestone"),
            mossy_cobblestone: st("minecraft:mossy_cobblestone"),
            magma_block: st("minecraft:magma_block"),
            bee_nest: st("minecraft:bee_nest"),
            cocoa: st("minecraft:cocoa"),
            creaking_heart: st("minecraft:creaking_heart"),
            pale_hanging_moss: st("minecraft:pale_hanging_moss"),
            pale_moss_carpet: st("minecraft:pale_moss_carpet"),
            small_dripleaf: st("minecraft:small_dripleaf"),
            tall_seagrass: st("minecraft:tall_seagrass"),
            kelp: st("minecraft:kelp"),
            kelp_plant: st("minecraft:kelp_plant"),
            seagrass: st("minecraft:seagrass"),
            bubble_column: st("minecraft:bubble_column"),
            sugar_cane: st("minecraft:sugar_cane"),
            cactus: st("minecraft:cactus"),
            big_dripleaf: st("minecraft:big_dripleaf"),
            big_dripleaf_stem: st("minecraft:big_dripleaf_stem"),
            bamboo: st("minecraft:bamboo"),
            bamboo_sapling: st("minecraft:bamboo_sapling"),
            podzol: st("minecraft:podzol"),
            mycelium: st("minecraft:mycelium"),
            snow_block: st("minecraft:snow_block"),
            powder_snow: st("minecraft:powder_snow"),
            packed_ice: st("minecraft:packed_ice"),
            blue_ice: st("minecraft:blue_ice"),
            obsidian: st("minecraft:obsidian"),
            end_stone: st("minecraft:end_stone"),
            glowstone: st("minecraft:glowstone"),
            fire: st("minecraft:fire"),
            netherrack: st("minecraft:netherrack"),
            basalt: st("minecraft:basalt"),
            blackstone: st("minecraft:blackstone"),
            chorus_plant: st("minecraft:chorus_plant"),
            chorus_flower: st("minecraft:chorus_flower"),
            end_portal: st("minecraft:end_portal"),
            end_gateway: st("minecraft:end_gateway"),
            iron_bars: st("minecraft:iron_bars"),
            end_crystal_base: st("minecraft:bedrock"),
            weeping_vines: st("minecraft:weeping_vines"),
            weeping_vines_plant: st("minecraft:weeping_vines_plant"),
            twisting_vines: st("minecraft:twisting_vines"),
            twisting_vines_plant: st("minecraft:twisting_vines_plant"),
            nether_wart_block: st("minecraft:nether_wart_block"),
            warped_wart_block: st("minecraft:warped_wart_block"),
            shroomlight: st("minecraft:shroomlight"),
            sea_pickle: st("minecraft:sea_pickle"),
            pointed_dripstone: st("minecraft:pointed_dripstone"),
            dripstone_block: st("minecraft:dripstone_block"),
            cave_vines: st("minecraft:cave_vines"),
            cave_vines_plant: st("minecraft:cave_vines_plant"),
            glow_lichen: st("minecraft:glow_lichen"),
            sculk_vein: st("minecraft:sculk_vein"),
            mud: st("minecraft:mud"),
            clay: st("minecraft:clay"),
            gravel: st("minecraft:gravel"),
            sand: st("minecraft:sand"),
            sandstone: st("minecraft:sandstone"),
            moss_block: st("minecraft:moss_block"),
            rooted_dirt: st("minecraft:rooted_dirt"),
            hanging_roots: st("minecraft:hanging_roots"),
        }
    }
}

/// Block tags features consult in code.
#[derive(Clone, Debug)]
pub struct CodeTags {
    pub logs: Arc<BlockSet>,
    pub leaves: Arc<BlockSet>,
    pub replaceable_by_trees: Arc<BlockSet>,
    pub features_cannot_replace: Arc<BlockSet>,
    pub lava_pool_stone_cannot_replace: Arc<BlockSet>,
    pub prevents_nearby_leaf_decay: Arc<BlockSet>,
    pub snow: Arc<BlockSet>,
    pub dirt: Arc<BlockSet>,
    pub base_stone_overworld: Arc<BlockSet>,
    pub small_flowers: Arc<BlockSet>,
    pub saplings: Arc<BlockSet>,
    pub ice: Arc<BlockSet>,
    pub replaceable_by_mushrooms: Arc<BlockSet>,
    pub geode_invalid_blocks: Arc<BlockSet>,
    pub sculk_replaceable_world_gen: Arc<BlockSet>,
    pub coral_blocks: Arc<BlockSet>,
    pub corals: Arc<BlockSet>,
    pub wall_corals: Arc<BlockSet>,
}

/// Everything the feature code needs to know about block states.
pub struct Blocks {
    info: Vec<u32>,
    survival: Vec<Survival>,
    survival_tags: Vec<Arc<BlockSet>>,
    double_plant: BlockSet,
    snowy_dirt: BlockSet,
    tag_cache: HashMap<String, Arc<BlockSet>>,
    pub known: Known,
    pub tags: CodeTags,
}

fn physics_table() -> Result<Vec<u8>> {
    static TABLE: &[u8] = include_bytes!("../../../../data/26.3/worldgen/block_physics.bin.gz");
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(TABLE).read_to_end(&mut raw)?;
    if raw.len() != blocks::STATE_COUNT as usize * 3 {
        return Err(anyhow!("block physics table has {} bytes for {} states", raw.len(), blocks::STATE_COUNT));
    }
    Ok(raw)
}

impl Blocks {
    pub fn load(data: &DataReader) -> Result<Self> {
        let raw = physics_table()?;
        let count = blocks::STATE_COUNT as usize;
        let motion = data.block_tag("minecraft:blocks_motion")?;
        let leaves = data.block_tag("minecraft:leaves")?;
        let water_blocks = [st("minecraft:kelp"), st("minecraft:kelp_plant"), st("minecraft:seagrass"), st("minecraft:tall_seagrass"), st("minecraft:bubble_column")]
            .map(blocks::block_id_of_state);
        let water_block = blocks::block_id_of_state(st("minecraft:water"));
        let lava_block = blocks::block_id_of_state(st("minecraft:lava"));
        let mut info = vec![0u32; count];
        for (state, v) in info.iter_mut().enumerate() {
            let flags = u16::from_le_bytes([raw[state * 3], raw[state * 3 + 1]]) as u32;
            let sides = raw[state * 3 + 2] as u32;
            let s = state as u32;
            let block = blocks::block_id_of_state(s);
            let mut bits = (flags & 0xffff) | (sides << SIDE_SHIFT);
            if blocks::property(s, "waterlogged") == Some("true") {
                bits |= WATERLOGGED | FLUID_WATER | FLUID_SOURCE;
            }
            if water_blocks.contains(&block) {
                bits |= FLUID_WATER | FLUID_SOURCE;
            }
            if block == water_block || block == lava_block {
                bits |= if block == water_block { FLUID_WATER } else { FLUID_LAVA };
                if blocks::property(s, "level") == Some("0") {
                    bits |= FLUID_SOURCE;
                }
            }
            if leaves.contains(&block) {
                bits |= LEAVES;
            }
            if motion.contains(&block) {
                bits |= BLOCKS_MOTION;
            }
            if blocks::has_collision(s) {
                bits |= COLLISION;
            }
            *v = bits;
        }

        let mut tag_cache = HashMap::new();
        let mut tag = |name: &str| -> Result<Arc<BlockSet>> {
            if let Some(t) = tag_cache.get(name) {
                return Ok(Arc::clone(t));
            }
            let set = Arc::new(BlockSet::from_ids(data.block_tag(name)?.iter().copied()));
            tag_cache.insert(name.to_owned(), Arc::clone(&set));
            Ok(set)
        };
        let survival_tags = SURVIVAL_TAGS.iter().map(|t| tag(t)).collect::<Result<Vec<_>>>()?;
        let tags = CodeTags {
            logs: tag("minecraft:logs")?,
            leaves: tag("minecraft:leaves")?,
            replaceable_by_trees: tag("minecraft:replaceable_by_trees")?,
            features_cannot_replace: tag("minecraft:features_cannot_replace")?,
            lava_pool_stone_cannot_replace: tag("minecraft:lava_pool_stone_cannot_replace")?,
            prevents_nearby_leaf_decay: tag("minecraft:prevents_nearby_leaf_decay")?,
            snow: tag("minecraft:snow")?,
            dirt: tag("minecraft:dirt")?,
            base_stone_overworld: tag("minecraft:base_stone_overworld")?,
            small_flowers: tag("minecraft:small_flowers")?,
            saplings: tag("minecraft:saplings")?,
            ice: tag("minecraft:ice")?,
            replaceable_by_mushrooms: tag("minecraft:replaceable_by_mushrooms")?,
            geode_invalid_blocks: tag("minecraft:geode_invalid_blocks")?,
            sculk_replaceable_world_gen: tag("minecraft:sculk_replaceable_world_gen")?,
            coral_blocks: tag("minecraft:coral_blocks")?,
            corals: tag("minecraft:corals")?,
            wall_corals: tag("minecraft:wall_corals")?,
        };

        let known = Known::load();
        let block_count = blocks::BLOCKS.len();
        let mut survival = vec![Survival::Always; block_count];
        let mut double_plant = BlockSet::default();
        let mut snowy_dirt = BlockSet::default();
        let id = |name: &str| blocks::id(name).unwrap_or_else(|| panic!("block {name} is missing from paper-data")) as usize;
        let vegetation = tag_index("minecraft:supports_vegetation");
        for name in ["tall_grass", "large_fern", "sunflower", "lilac", "rose_bush", "peony", "pitcher_plant"] {
            survival[id(name)] = Survival::DoublePlant(vegetation);
            double_plant.insert(id(name) as u32);
        }
        for name in [
            "short_grass", "fern", "sweet_berry_bush", "wildflowers", "pink_petals", "bush", "red_shrub", "firefly_bush",
            "closed_eyeblossom", "open_eyeblossom", "wither_rose", "torchflower", "shelf_mushroom",
        ] {
            if let Some(b) = blocks::id(name) {
                survival[b as usize] = Survival::OnTag(vegetation);
            }
        }
        for b in tags.small_flowers.words.iter().enumerate().flat_map(|(w, bits)| (0..64).filter(move |i| bits & (1 << i) != 0).map(move |i| w * 64 + i)) {
            if survival[b] == Survival::Always {
                survival[b] = Survival::OnTag(vegetation);
            }
        }
        for b in tags.saplings.words.iter().enumerate().flat_map(|(w, bits)| (0..64).filter(move |i| bits & (1 << i) != 0).map(move |i| w * 64 + i)) {
            survival[b] = Survival::OnTag(vegetation);
        }
        let dry = tag_index("minecraft:supports_dry_vegetation");
        for name in ["dead_bush", "short_dry_grass", "tall_dry_grass"] {
            survival[id(name)] = Survival::OnTag(dry);
        }
        for name in ["azalea", "flowering_azalea"] {
            survival[id(name)] = Survival::OnTag(tag_index("minecraft:supports_azalea"));
        }
        for (name, t) in [
            ("warped_fungus", "minecraft:supports_warped_fungus"),
            ("crimson_fungus", "minecraft:supports_crimson_fungus"),
            ("warped_roots", "minecraft:supports_warped_roots"),
            ("crimson_roots", "minecraft:supports_crimson_roots"),
            ("nether_sprouts", "minecraft:supports_nether_sprouts"),
            ("nether_wart", "minecraft:supports_nether_wart"),
        ] {
            survival[id(name)] = Survival::OnTag(tag_index(t));
        }
        survival[id("bamboo")] = Survival::Bamboo;
        survival[id("bamboo_sapling")] = Survival::Bamboo;
        survival[id("mangrove_propagule")] = Survival::MangrovePropagule;
        survival[id("leaf_litter")] = Survival::LeafLitter;
        survival[id("cactus_flower")] = Survival::CactusFlower;
        survival[id("cactus")] = Survival::Cactus;
        survival[id("sugar_cane")] = Survival::SugarCane;
        survival[id("brown_mushroom")] = Survival::Mushroom;
        survival[id("red_mushroom")] = Survival::Mushroom;
        survival[id("small_dripleaf")] = Survival::SmallDripleaf;
        double_plant.insert(id("small_dripleaf") as u32);
        survival[id("big_dripleaf")] = Survival::BigDripleaf;
        survival[id("spore_blossom")] = Survival::SporeBlossom;
        survival[id("hanging_roots")] = Survival::HangingRoots;
        survival[id("moss_carpet")] = Survival::Carpet;
        survival[id("pale_moss_carpet")] = Survival::Carpet;
        survival[id("seagrass")] = Survival::Seagrass;
        survival[id("tall_seagrass")] = Survival::TallSeagrass;
        double_plant.insert(id("tall_seagrass") as u32);
        survival[id("cocoa")] = Survival::Cocoa;
        survival[id("sea_pickle")] = Survival::SeaPickle;
        survival[id("lily_pad")] = Survival::LilyPad;
        survival[id("fire")] = Survival::Fire;
        survival[id("soul_fire")] = Survival::SoulFire;
        survival[id("snow")] = Survival::SnowLayer;
        survival[id("cave_vines")] = Survival::CaveVines;
        survival[id("cave_vines_plant")] = Survival::CaveVines;
        survival[id("glow_lichen")] = Survival::GlowLichen;
        survival[id("pointed_dripstone")] = Survival::PointedDripstone;
        survival[id("chorus_flower")] = Survival::ChorusFlower;
        survival[id("twisting_vines")] = Survival::TwistingVines;
        survival[id("twisting_vines_plant")] = Survival::TwistingVines;
        survival[id("weeping_vines")] = Survival::WeepingVines;
        survival[id("weeping_vines_plant")] = Survival::WeepingVines;
        survival[id("kelp")] = Survival::Kelp;
        survival[id("kelp_plant")] = Survival::Kelp;
        for coral in ["tube", "brain", "bubble", "fire", "horn"] {
            for dead in ["", "dead_"] {
                survival[id(&format!("{dead}{coral}_coral"))] = Survival::CoralPlant;
                survival[id(&format!("{dead}{coral}_coral_fan"))] = Survival::CoralPlant;
                survival[id(&format!("{dead}{coral}_coral_wall_fan"))] = Survival::CoralWallFan;
            }
        }
        for name in ["grass_block", "podzol", "mycelium"] {
            snowy_dirt.insert(id(name) as u32);
        }

        Ok(Self { info, survival, survival_tags, double_plant, snowy_dirt, tag_cache, known, tags })
    }

    /// A block tag (without `#`), resolved while the generator is being built.
    pub fn tag(&mut self, data: &DataReader, name: &str) -> Result<Arc<BlockSet>> {
        let name = crate::data::full_id(name);
        if let Some(t) = self.tag_cache.get(&name) {
            return Ok(Arc::clone(t));
        }
        let set = Arc::new(BlockSet::from_ids(data.block_tag(&name)?.iter().copied()));
        self.tag_cache.insert(name, Arc::clone(&set));
        Ok(set)
    }

    #[inline]
    fn bits(&self, state: u32) -> u32 {
        self.info.get(state as usize).copied().unwrap_or(IS_AIR)
    }

    #[inline]
    pub fn is_air(&self, state: u32) -> bool {
        self.bits(state) & IS_AIR != 0
    }

    /// `canBeReplaced()`.
    #[inline]
    pub fn is_replaceable(&self, state: u32) -> bool {
        self.bits(state) & REPLACEABLE != 0
    }

    /// `isSolid()` (the legacy "solid" flag).
    #[inline]
    pub fn is_solid(&self, state: u32) -> bool {
        self.bits(state) & IS_SOLID != 0
    }

    /// `liquid()`.
    #[inline]
    pub fn is_liquid(&self, state: u32) -> bool {
        self.bits(state) & IS_LIQUID != 0
    }

    #[inline]
    pub fn is_full_cube(&self, state: u32) -> bool {
        self.bits(state) & IS_FULL_CUBE != 0
    }

    /// `isSolidRender()`.
    #[inline]
    pub fn is_solid_render(&self, state: u32) -> bool {
        self.bits(state) & IS_SOLID_RENDER != 0
    }

    #[inline]
    pub fn is_burnable(&self, state: u32) -> bool {
        self.bits(state) & BURNABLE != 0
    }

    #[inline]
    pub fn has_collision(&self, state: u32) -> bool {
        self.bits(state) & COLLISION != 0
    }

    #[inline]
    pub fn blocks_motion(&self, state: u32) -> bool {
        self.bits(state) & BLOCKS_MOTION != 0
    }

    #[inline]
    pub fn is_leaves(&self, state: u32) -> bool {
        self.bits(state) & LEAVES != 0
    }

    /// `isFaceSturdy(side)` with full support.
    #[inline]
    pub fn sturdy(&self, state: u32, side: Dir) -> bool {
        self.bits(state) & (1 << (SIDE_SHIFT + side.index() as u32)) != 0
    }

    /// `isFaceSturdy(side, SupportType.CENTER)`.
    pub fn center_sturdy(&self, state: u32, side: Dir) -> bool {
        if self.sturdy(state, side) {
            return true;
        }
        match side {
            Dir::Down => self.bits(state) & DOWN_CENTER != 0,
            Dir::Up => self.bits(state) & UP_CENTER != 0,
            _ => false,
        }
    }

    #[inline]
    pub fn fluid(&self, state: u32) -> Fluid {
        let b = self.bits(state);
        let source = b & FLUID_SOURCE != 0;
        if b & FLUID_WATER != 0 {
            if source {
                Fluid::Water
            } else {
                Fluid::FlowingWater
            }
        } else if b & FLUID_LAVA != 0 {
            if source {
                Fluid::Lava
            } else {
                Fluid::FlowingLava
            }
        } else {
            Fluid::Empty
        }
    }

    /// The state has a `waterlogged` property set to true.
    #[inline]
    pub fn is_waterlogged(&self, state: u32) -> bool {
        self.bits(state) & WATERLOGGED != 0
    }

    #[inline]
    pub fn is_block(&self, state: u32, other: u32) -> bool {
        blocks::block_id_of_state(state) == blocks::block_id_of_state(other)
    }

    /// `state.is(BlockTags.LEAVES)`-like check against a code tag.
    #[inline]
    pub fn in_tag(&self, state: u32, tag: &BlockSet) -> bool {
        tag.has(state)
    }

    // Property helpers.

    /// `trySetValue`: the state with a property changed, or unchanged if it has no such
    /// property or value.
    pub fn with(&self, state: u32, property: &str, value: &str) -> u32 {
        blocks::with_property(state, property, value).unwrap_or(state)
    }

    pub fn with_bool(&self, state: u32, property: &str, value: bool) -> u32 {
        self.with(state, property, if value { "true" } else { "false" })
    }

    pub fn with_int(&self, state: u32, property: &str, value: i32) -> u32 {
        self.with(state, property, &value.to_string())
    }

    pub fn prop(&self, state: u32, property: &str) -> Option<&'static str> {
        blocks::property(state, property)
    }

    pub fn prop_bool(&self, state: u32, property: &str) -> Option<bool> {
        self.prop(state, property).map(|v| v == "true")
    }

    pub fn prop_int(&self, state: u32, property: &str) -> Option<i32> {
        self.prop(state, property).and_then(|v| v.parse().ok())
    }

    pub fn has_prop(&self, state: u32, property: &str) -> bool {
        blocks::block_of_state(state).properties.iter().any(|(n, _)| *n == property)
    }

    /// `copyWaterloggedFrom` / setting `waterlogged` when the block has the property.
    pub fn waterlogged(&self, state: u32, water: bool) -> u32 {
        self.with_bool(state, "waterlogged", water)
    }

    /// `LeavesBlock.getOptionalDistanceAt`.
    pub fn leaf_distance(&self, state: u32) -> Option<i32> {
        if self.tags.prevents_nearby_leaf_decay.has(state) {
            Some(0)
        } else if self.is_leaves(state) {
            self.prop_int(state, "distance")
        } else {
            None
        }
    }

    /// `BlockState.canSurvive(level, pos)` for the blocks features place.
    pub fn can_survive(&self, w: &World, state: u32, pos: Pos) -> bool {
        let block = blocks::block_id_of_state(state) as usize;
        let rule = self.survival.get(block).copied().unwrap_or(Survival::Always);
        let below = || w.get(pos.down());
        let tag = |i: usize| &self.survival_tags[i];
        match rule {
            Survival::Always => true,
            Survival::OnTag(t) => tag(t).has(below()),
            Survival::DoublePlant(t) => {
                if self.prop(state, "half") == Some("upper") {
                    let b = below();
                    self.is_block(b, state) && self.prop(b, "half") == Some("lower")
                } else {
                    tag(t).has(below())
                }
            }
            Survival::MangrovePropagule => {
                if self.prop_bool(state, "hanging") == Some(true) {
                    tag(tag_index("minecraft:supports_hanging_mangrove_propagule")).has(w.get(pos.up()))
                } else {
                    tag(tag_index("minecraft:supports_mangrove_propagule")).has(below())
                }
            }
            Survival::LeafLitter => self.sturdy(below(), Dir::Up),
            Survival::CactusFlower => {
                let b = below();
                tag(tag_index("minecraft:support_override_cactus_flower")).has(b) || self.center_sturdy(b, Dir::Up)
            }
            Survival::Cactus => {
                for d in Dir::HORIZONTAL {
                    let n = w.get(pos.rel(d));
                    if self.is_solid(n) || self.fluid(n).is_lava() {
                        return false;
                    }
                }
                let b = below();
                (self.is_block(b, self.known.cactus) || tag(tag_index("minecraft:supports_cactus")).has(b)) && !self.is_liquid(w.get(pos.up()))
            }
            Survival::SugarCane => {
                let b = below();
                if self.is_block(b, self.known.sugar_cane) {
                    return true;
                }
                if tag(tag_index("minecraft:supports_sugar_cane")).has(b) {
                    let adj = tag(tag_index("minecraft:supports_sugar_cane_adjacently"));
                    for d in Dir::HORIZONTAL {
                        let n = w.get(pos.down().rel(d));
                        if self.fluid(n).is_water() || adj.has(n) {
                            return true;
                        }
                    }
                }
                false
            }
            Survival::Mushroom => {
                let b = below();
                tag(tag_index("minecraft:overrides_mushroom_light_requirement")).has(b) || self.is_solid_render(b)
            }
            Survival::SmallDripleaf => {
                if self.prop(state, "half") == Some("upper") {
                    let b = below();
                    return self.is_block(b, state) && self.prop(b, "half") == Some("lower");
                }
                let b = below();
                tag(tag_index("minecraft:supports_small_dripleaf")).has(b)
                    || (self.fluid(w.get(pos)) == Fluid::Water && tag(tag_index("minecraft:supports_vegetation")).has(b))
            }
            Survival::BigDripleaf => {
                let b = below();
                self.is_block(b, self.known.big_dripleaf) || self.is_block(b, self.known.big_dripleaf_stem) || tag(tag_index("minecraft:supports_big_dripleaf")).has(b)
            }
            Survival::SporeBlossom => self.center_sturdy(w.get(pos.up()), Dir::Down) && !self.fluid(w.get(pos)).is_water(),
            Survival::HangingRoots => self.sturdy(w.get(pos.up()), Dir::Down),
            Survival::Carpet => !self.is_air(below()),
            Survival::Seagrass => {
                let b = below();
                self.sturdy(b, Dir::Up) && !tag(tag_index("minecraft:cannot_support_seagrass")).has(b)
            }
            Survival::TallSeagrass => {
                if self.prop(state, "half") == Some("upper") {
                    let b = below();
                    return self.is_block(b, self.known.tall_seagrass) && self.prop(b, "half") == Some("lower");
                }
                let b = below();
                self.fluid(w.get(pos)) == Fluid::Water && self.sturdy(b, Dir::Up) && !tag(tag_index("minecraft:cannot_support_seagrass")).has(b)
            }
            Survival::Cocoa => {
                let facing = self.prop(state, "facing").and_then(Dir::parse).unwrap_or(Dir::North);
                tag(tag_index("minecraft:supports_cocoa")).has(w.get(pos.rel(facing)))
            }
            Survival::CoralPlant => self.sturdy(below(), Dir::Up),
            Survival::CoralWallFan => match self.prop(state, "facing").and_then(Dir::parse) {
                Some(facing) => self.sturdy(w.get(pos.rel(facing.opposite())), facing),
                None => true,
            },
            Survival::SeaPickle => {
                let b = below();
                self.sturdy(b, Dir::Up) || self.has_collision(b)
            }
            Survival::LilyPad => {
                let b = below();
                let here = w.get(pos);
                (self.fluid(b) == Fluid::Water || tag(tag_index("minecraft:supports_lily_pad")).has(b)) && self.fluid(here) == Fluid::Empty
            }
            Survival::Fire => {
                if self.sturdy(below(), Dir::Up) {
                    return true;
                }
                Dir::ALL.iter().any(|d| self.is_burnable(w.get(pos.rel(*d))))
            }
            Survival::SoulFire => tag(tag_index("minecraft:soul_fire_base_blocks")).has(below()),
            Survival::SnowLayer => {
                let b = below();
                if tag(tag_index("minecraft:cannot_support_snow_layer")).has(b) {
                    return false;
                }
                tag(tag_index("minecraft:support_override_snow_layer")).has(b)
                    || self.is_full_cube(b)
                    || (self.is_block(b, self.known.snow) && self.prop_int(b, "layers") == Some(8))
            }
            Survival::CaveVines => {
                let above = w.get(pos.up());
                self.is_block(above, self.known.cave_vines) || self.is_block(above, self.known.cave_vines_plant) || self.sturdy(above, Dir::Down)
            }
            Survival::GlowLichen => true,
            Survival::PointedDripstone => true,
            Survival::ChorusFlower => true,
            Survival::TwistingVines => {
                let b = below();
                self.is_block(b, self.known.twisting_vines) || self.is_block(b, self.known.twisting_vines_plant) || self.sturdy(b, Dir::Up)
            }
            Survival::WeepingVines => {
                let a = w.get(pos.up());
                self.is_block(a, self.known.weeping_vines) || self.is_block(a, self.known.weeping_vines_plant) || self.sturdy(a, Dir::Down)
            }
            Survival::Kelp => {
                let b = below();
                if self.is_block(b, self.known.kelp) || self.is_block(b, self.known.kelp_plant) {
                    return true;
                }
                !tag(tag_index("minecraft:cannot_support_kelp")).has(b) && self.sturdy(b, Dir::Up)
            }
            Survival::Bamboo => tag(tag_index("minecraft:supports_bamboo")).has(below()),
        }
    }

    pub fn is_double_plant(&self, state: u32) -> bool {
        self.double_plant.has(state)
    }

    /// `BlockState.updateShape` for the blocks that react at the edge of a placed tree:
    /// vines lose unsupported faces, plants that cannot survive and broken double plants
    /// turn into air, and snowy dirt follows the block above. `dir` points from `pos` to
    /// the neighbour whose state is `neighbor`.
    pub fn update_shape(&self, w: &World, state: u32, pos: Pos, dir: Dir, neighbor: u32) -> u32 {
        let block = blocks::block_id_of_state(state);
        if block == blocks::block_id_of_state(self.known.vine) {
            if dir == Dir::Down {
                return state;
            }
            return self.updated_vine(w, state, pos);
        }
        if self.snowy_dirt.contains_block(block) {
            if dir == Dir::Up {
                return self.with_bool(state, "snowy", self.tags.snow.has(neighbor));
            }
            return state;
        }
        if self.double_plant.contains_block(block) {
            let upper = self.prop(state, "half") == Some("upper");
            let vertical = matches!(dir, Dir::Up | Dir::Down);
            let towards_other_half = vertical && (!upper == (dir == Dir::Up));
            if towards_other_half && !(self.is_block(neighbor, state) && self.prop(neighbor, "half") != self.prop(state, "half")) {
                return self.known.air;
            }
            if !upper && dir == Dir::Down && !self.can_survive(w, state, pos) {
                return self.known.air;
            }
            return state;
        }
        match self.survival.get(block as usize) {
            Some(Survival::OnTag(_) | Survival::Mushroom | Survival::LeafLitter | Survival::Carpet | Survival::Seagrass) => {
                if dir == Dir::Down && !self.can_survive(w, state, pos) {
                    return self.known.air;
                }
                state
            }
            Some(Survival::Cocoa) => {
                let facing = self.prop(state, "facing").and_then(Dir::parse).unwrap_or(Dir::North);
                if dir == facing && !self.can_survive(w, state, pos) {
                    return self.known.air;
                }
                state
            }
            _ => state,
        }
    }

    /// `MultifaceBlock.canAttachTo`: the face of `support` is full in its support shape or in
    /// its collision shape (approximated by the full-cube flag).
    pub fn can_attach_to(&self, support_state: u32, face: Dir) -> bool {
        self.sturdy(support_state, face) || self.is_full_cube(support_state)
    }

    /// `VineBlock.getUpdatedState` followed by the air check of `updateShape`.
    fn updated_vine(&self, w: &World, state: u32, pos: Pos) -> u32 {
        // Vanilla checks the top face of the block above for the `up` face.
        let acceptable = |support: Pos, face: Dir| self.can_attach_to(w.get(support), face);
        let mut new = state;
        if self.prop_bool(state, "up") == Some(true) {
            new = self.with_bool(new, "up", acceptable(pos.up(), Dir::Up));
        }
        let above = w.get(pos.up());
        let above_vine = self.is_block(above, self.known.vine);
        for d in Dir::HORIZONTAL {
            let name = d.name();
            if self.prop_bool(new, name) == Some(true) {
                let mut ok = acceptable(pos.rel(d), d.opposite());
                if !ok {
                    ok = above_vine && self.prop_bool(above, name) == Some(true);
                }
                new = self.with_bool(new, name, ok);
            }
        }
        let has_faces = ["up", "north", "east", "south", "west"].iter().any(|f| self.prop_bool(new, f) == Some(true));
        if has_faces {
            new
        } else {
            self.known.air
        }
    }
}
