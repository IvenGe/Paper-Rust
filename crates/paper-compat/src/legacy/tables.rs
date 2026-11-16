//! What 26.3's ids are for clients from before the flattening: blocks, items and sounds from
//! `data/compat/legacy.json` (ViaBackwards and ViaRewind's mappings, composed), and the
//! smaller tables (entities, biomes, enchantments, effects, particles) by name.

use base64::Engine;
use serde_json::Value;
use std::sync::OnceLock;

/// The legacy ids of one old version.
pub struct Tables {
    /// `id << 4 | meta` of each 26.3 block state.
    blocks: Vec<u16>,
    /// `id << 16 | damage` of each 26.3 item.
    items: Vec<u32>,
    /// The legacy sound name of each 26.3 sound event.
    sounds: Vec<Option<&'static str>>,
}

fn decode(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(text).expect("legacy table")
}

impl Tables {
    /// The tables of `version` (`1.8`).
    pub fn load(version: &str) -> &'static Tables {
        static DOC: OnceLock<Value> = OnceLock::new();
        static V1_8: OnceLock<Tables> = OnceLock::new();
        let doc = DOC.get_or_init(|| serde_json::from_str(include_str!("../../../../data/compat/legacy.json")).expect("legacy.json"));
        let cell = match version {
            "1.8" => &V1_8,
            other => panic!("no legacy tables for {other}"),
        };
        cell.get_or_init(|| {
            let v = &doc[version];
            let blocks = decode(v["blocks"].as_str().unwrap_or("")).chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let items = decode(v["items"].as_str().unwrap_or("")).chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            let names: &'static Vec<Value> = doc["sound_names"].as_array().expect("sound names");
            let sounds = decode(v["sounds"].as_str().unwrap_or(""))
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .map(|i| if i == 0xFFFF { None } else { names.get(i as usize).and_then(Value::as_str) })
                .collect();
            Tables { blocks, items, sounds }
        })
    }

    /// The legacy block (`id << 4 | meta`) of a 26.3 block state.
    pub fn block(&self, state: u32) -> u16 {
        self.blocks.get(state as usize).copied().unwrap_or(0)
    }

    /// The legacy item (id, damage) of a 26.3 item.
    pub fn item(&self, item: i32) -> (i16, i16) {
        let packed = self.items.get(item as usize).copied().unwrap_or(1 << 16);
        ((packed >> 16) as i16, (packed & 0xFFFF) as i16)
    }

    /// The 26.3 item for a legacy item (the first that maps to it, else stone).
    pub fn item_new(&self, id: i16, damage: i16) -> i32 {
        let packed = (id as u32) << 16 | damage as u16 as u32;
        let exact = self.items.iter().position(|v| *v == packed);
        let any = || self.items.iter().position(|v| *v >> 16 == id as u32);
        exact.or_else(any).map_or(1, |i| i as i32)
    }

    /// The legacy sound of a 26.3 sound event.
    pub fn sound(&self, sound: i32) -> Option<&'static str> {
        self.sounds.get(sound as usize).copied().flatten()
    }

    /// The legacy biome id of a 26.3 biome (by its index in the biome registry).
    pub fn biome(&self, biome: u32) -> u8 {
        let name = paper_data::synced::entries("minecraft:worldgen/biome").and_then(|e| e.get(biome as usize).copied()).unwrap_or("");
        legacy_biome(name)
    }
}

/// How an entity is spawned on a client from before 1.9.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EntityKind {
    /// `spawn_entity_living` with this mob id.
    Mob(u8),
    /// `spawn_entity` with this object id (and object data).
    Object(u8, i32),
    Player,
    Painting,
    ExperienceOrb,
    Lightning,
}

/// The legacy entity of a 26.3 entity type (by name); `None`: the client is not told.
pub fn entity(name: &str) -> Option<EntityKind> {
    use EntityKind::*;
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    Some(match name {
        "player" => Player,
        "painting" => Painting,
        "experience_orb" => ExperienceOrb,
        "lightning_bolt" => Lightning,
        "creeper" => Mob(50),
        "skeleton" | "stray" | "wither_skeleton" | "bogged" | "parched" => Mob(51),
        "spider" => Mob(52),
        "giant" => Mob(53),
        "zombie" | "husk" | "drowned" | "zombie_villager" => Mob(54),
        "slime" | "sulfur_cube" => Mob(55),
        "ghast" | "happy_ghast" => Mob(56),
        "zombified_piglin" | "piglin" | "piglin_brute" | "zoglin" => Mob(57),
        "enderman" => Mob(58),
        "cave_spider" => Mob(59),
        "silverfish" => Mob(60),
        "blaze" | "breeze" => Mob(61),
        "magma_cube" => Mob(62),
        "ender_dragon" => Mob(63),
        "wither" => Mob(64),
        "bat" | "phantom" | "vex" | "allay" | "bee" | "parrot" => Mob(65),
        "witch" | "evoker" | "illusioner" | "vindicator" | "pillager" => Mob(66),
        "endermite" | "shulker" => Mob(67),
        "guardian" | "elder_guardian" => Mob(68),
        "pig" | "hoglin" | "strider" => Mob(90),
        "sheep" | "goat" => Mob(91),
        "cow" | "panda" | "sniffer" => Mob(92),
        "chicken" | "turtle" => Mob(93),
        "squid" | "glow_squid" | "axolotl" | "dolphin" | "cod" | "salmon" | "tropical_fish" | "pufferfish" | "tadpole" | "nautilus" | "zombie_nautilus" => Mob(94),
        "wolf" | "fox" | "polar_bear" => Mob(95),
        "mooshroom" => Mob(96),
        "snow_golem" => Mob(97),
        "ocelot" | "cat" => Mob(98),
        "iron_golem" | "copper_golem" | "warden" | "ravager" | "creaking" => Mob(99),
        "horse" | "donkey" | "mule" | "skeleton_horse" | "zombie_horse" | "llama" | "trader_llama" | "camel" | "camel_husk" => Mob(100),
        "rabbit" | "frog" | "armadillo" => Mob(101),
        "villager" | "wandering_trader" => Mob(120),
        "item" => Object(2, 1),
        "tnt" => Object(50, 0),
        "end_crystal" => Object(51, 0),
        "arrow" | "spectral_arrow" | "trident" => Object(60, 0),
        "snowball" => Object(61, 0),
        "egg" => Object(62, 0),
        "fireball" | "dragon_fireball" => Object(63, 0),
        "small_fireball" | "wind_charge" | "breeze_wind_charge" => Object(64, 0),
        "ender_pearl" => Object(65, 0),
        "wither_skull" => Object(66, 0),
        "falling_block" => Object(70, 0),
        "item_frame" | "glow_item_frame" => Object(71, 0),
        "eye_of_ender" => Object(72, 0),
        "splash_potion" | "lingering_potion" => Object(73, 0),
        "experience_bottle" => Object(75, 0),
        "firework_rocket" => Object(76, 0),
        "leash_knot" => Object(77, 0),
        "armor_stand" => Object(78, 0),
        "fishing_bobber" => Object(90, 0),
        "minecart" => Object(10, 0),
        "chest_minecart" => Object(10, 1),
        "furnace_minecart" => Object(10, 2),
        "tnt_minecart" => Object(10, 3),
        "spawner_minecart" => Object(10, 4),
        "hopper_minecart" => Object(10, 5),
        "command_block_minecart" => Object(10, 6),
        _ if name.ends_with("_boat") || name.ends_with("_raft") => Object(1, 0),
        _ => return None,
    })
}

/// The legacy biome id of a 26.3 biome name.
pub fn legacy_biome(name: &str) -> u8 {
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "ocean" | "warm_ocean" | "lukewarm_ocean" | "cold_ocean" => 0,
        "plains" | "meadow" | "cherry_grove" | "the_void" | "dripstone_caves" | "lush_caves" | "deep_dark" => 1,
        "desert" => 2,
        "windswept_hills" | "stony_peaks" | "jagged_peaks" | "frozen_peaks" => 3,
        "forest" => 4,
        "taiga" => 5,
        "swamp" | "mangrove_swamp" => 6,
        "river" => 7,
        "nether_wastes" | "soul_sand_valley" | "crimson_forest" | "warped_forest" | "basalt_deltas" => 8,
        "the_end" | "end_highlands" | "end_midlands" | "small_end_islands" | "end_barrens" => 9,
        "frozen_ocean" | "deep_frozen_ocean" => 10,
        "frozen_river" => 11,
        "snowy_plains" => 12,
        "snowy_slopes" => 13,
        "mushroom_fields" => 14,
        "beach" => 16,
        "jungle" | "bamboo_jungle" => 21,
        "sparse_jungle" => 23,
        "deep_ocean" | "deep_lukewarm_ocean" | "deep_cold_ocean" => 24,
        "stony_shore" => 25,
        "snowy_beach" => 26,
        "birch_forest" => 27,
        "dark_forest" | "pale_garden" => 29,
        "snowy_taiga" | "grove" => 30,
        "old_growth_pine_taiga" => 32,
        "windswept_forest" => 34,
        "savanna" => 35,
        "savanna_plateau" => 36,
        "badlands" => 37,
        "wooded_badlands" => 38,
        "sunflower_plains" => 129,
        "windswept_gravelly_hills" => 131,
        "flower_forest" => 132,
        "ice_spikes" => 140,
        "old_growth_birch_forest" => 155,
        "old_growth_spruce_taiga" => 160,
        "windswept_savanna" => 163,
        "eroded_badlands" => 165,
        _ => 1,
    }
}

/// The legacy enchantment id of a 26.3 enchantment name.
pub fn enchantment(name: &str) -> Option<i16> {
    Some(match name.strip_prefix("minecraft:").unwrap_or(name) {
        "protection" => 0,
        "fire_protection" => 1,
        "feather_falling" => 2,
        "blast_protection" => 3,
        "projectile_protection" => 4,
        "respiration" => 5,
        "aqua_affinity" => 6,
        "thorns" => 7,
        "depth_strider" => 8,
        "sharpness" => 16,
        "smite" => 17,
        "bane_of_arthropods" => 18,
        "knockback" => 19,
        "fire_aspect" => 20,
        "looting" => 21,
        "efficiency" => 32,
        "silk_touch" => 33,
        "unbreaking" => 34,
        "fortune" => 35,
        "power" => 48,
        "punch" => 49,
        "flame" => 50,
        "infinity" => 51,
        "luck_of_the_sea" => 61,
        "lure" => 62,
        _ => return None,
    })
}

/// The legacy effect id (1-23) of a 26.3 mob effect id: the first 23 kept their order.
pub fn effect(id: i32) -> Option<i8> {
    (0..23).contains(&id).then_some(id as i8 + 1)
}

/// The legacy particle of a 26.3 particle name: its id, and how many VarInts of data
/// follow (item and block particles).
pub fn particle(name: &str) -> Option<(i32, u8)> {
    Some(match name.strip_prefix("minecraft:").unwrap_or(name) {
        "poof" => (0, 0),
        "explosion" => (1, 0),
        "explosion_emitter" => (2, 0),
        "firework" => (3, 0),
        "bubble" | "bubble_pop" | "bubble_column_up" | "current_down" => (4, 0),
        "splash" => (5, 0),
        "fishing" => (6, 0),
        "underwater" => (7, 0),
        "crit" => (9, 0),
        "enchanted_hit" => (10, 0),
        "smoke" | "white_smoke" => (11, 0),
        "large_smoke" | "campfire_cosy_smoke" | "campfire_signal_smoke" => (12, 0),
        "effect" => (13, 0),
        "instant_effect" => (14, 0),
        "entity_effect" => (15, 0),
        "witch" => (17, 0),
        "dripping_water" | "falling_water" => (18, 0),
        "dripping_lava" | "falling_lava" => (19, 0),
        "angry_villager" => (20, 0),
        "happy_villager" | "composter" => (21, 0),
        "mycelium" => (22, 0),
        "note" => (23, 0),
        "portal" | "reverse_portal" => (24, 0),
        "enchant" => (25, 0),
        "flame" | "soul_fire_flame" | "small_flame" => (26, 0),
        "lava" => (27, 0),
        "cloud" => (29, 0),
        "dust" | "dust_color_transition" => (30, 0),
        "item_snowball" => (31, 0),
        "item_slime" => (33, 0),
        "heart" => (34, 0),
        "item" => (36, 2),
        "block" | "block_marker" | "block_crumble" => (37, 1),
        "falling_dust" | "dust_pillar" => (38, 1),
        "rain" => (39, 0),
        "elder_guardian" => (41, 0),
        _ => return None,
    })
}
