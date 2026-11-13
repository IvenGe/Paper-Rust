//! Tables vanilla defines in code: block collision shapes and behaviour flags, default item
//! components, entity types and their metadata layout. Derived from the Pumpkin project's
//! extracted 26.3 assets by `tools/datagen/pumpkin_extra.py`; parsed on first use.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

fn normalize(name: &str) -> std::borrow::Cow<'_, str> {
    if name.contains(':') {
        std::borrow::Cow::Borrowed(name)
    } else {
        std::borrow::Cow::Owned(format!("minecraft:{name}"))
    }
}

/// Block collision shapes and behaviour flags.
pub mod physics {
    use super::*;

    /// An axis-aligned box in block-local coordinates (0..1, but shapes like fences reach 1.5).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Aabb {
        pub min: [f64; 3],
        pub max: [f64; 3],
    }

    pub const TOOL_REQUIRED: u16 = 1 << 2;
    pub const BURNABLE: u16 = 1 << 1;
    pub const REPLACEABLE: u16 = 1 << 4;
    pub const LIQUID: u16 = 1 << 5;
    pub const SOLID: u16 = 1 << 6;
    pub const FULL_CUBE: u16 = 1 << 7;
    pub const SOLID_BLOCK: u16 = 1 << 8;
    pub const RANDOM_TICKS: u16 = 1 << 9;
    pub const ANALOG_OUTPUT: u16 = 1 << 12;
    pub const WATERLOGGED: u16 = 1 << 13;

    /// Per-block values that differ from the defaults.
    #[derive(Clone, Debug, Default)]
    pub struct BlockPhysics {
        pub loot_table: Option<String>,
        pub slipperiness: f32,
        pub velocity_multiplier: f32,
        pub jump_velocity_multiplier: f32,
    }

    struct Tables {
        shape_sets: Vec<Vec<Aabb>>,
        state_shape_sets: Vec<u16>,
        state_flags: Vec<u16>,
        blocks: HashMap<String, BlockPhysics>,
        block_entity_types: Vec<String>,
    }

    fn tables() -> &'static Tables {
        static T: OnceLock<Tables> = OnceLock::new();
        T.get_or_init(|| {
            let v: Value = serde_json::from_str(include_str!("../../../data/26.3/block_physics.json")).expect("block_physics.json");
            let shapes: Vec<Aabb> = v["shapes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| {
                    let f: Vec<f64> = s.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
                    Aabb { min: [f[0], f[1], f[2]], max: [f[3], f[4], f[5]] }
                })
                .collect();
            let shape_sets = v["shape_sets"].as_array().unwrap().iter().map(|set| set.as_array().unwrap().iter().map(|i| shapes[i.as_u64().unwrap() as usize]).collect()).collect();
            let blocks = v["blocks"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(name, b)| {
                    (
                        name.clone(),
                        BlockPhysics {
                            loot_table: b["loot_table"].as_str().map(str::to_owned),
                            slipperiness: b["slipperiness"].as_f64().unwrap_or(0.6) as f32,
                            velocity_multiplier: b["velocity_multiplier"].as_f64().unwrap_or(1.0) as f32,
                            jump_velocity_multiplier: b["jump_velocity_multiplier"].as_f64().unwrap_or(1.0) as f32,
                        },
                    )
                })
                .collect();
            Tables {
                shape_sets,
                state_shape_sets: v["state_shape_sets"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u16).collect(),
                state_flags: v["state_flags"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u16).collect(),
                blocks,
                block_entity_types: v["block_entity_types"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
            }
        })
    }

    /// The collision boxes of a block state (empty for air, plants, ...).
    pub fn collision_boxes(state: u32) -> &'static [Aabb] {
        let t = tables();
        t.state_shape_sets.get(state as usize).map_or(&[], |set| t.shape_sets[*set as usize].as_slice())
    }

    pub fn flags(state: u32) -> u16 {
        tables().state_flags.get(state as usize).copied().unwrap_or(0)
    }

    pub fn has_flag(state: u32, flag: u16) -> bool {
        flags(state) & flag != 0
    }

    /// Whether the block only drops items when mined with the right tool.
    pub fn requires_correct_tool(state: u32) -> bool {
        has_flag(state, TOOL_REQUIRED)
    }

    static DEFAULT: BlockPhysics = BlockPhysics { loot_table: None, slipperiness: 0.6, velocity_multiplier: 1.0, jump_velocity_multiplier: 1.0 };

    pub fn block(name: &str) -> &'static BlockPhysics {
        tables().blocks.get(normalize(name).as_ref()).unwrap_or(&DEFAULT)
    }

    /// The loot table that decides a block's drops, e.g. `minecraft:blocks/stone`.
    pub fn loot_table(name: &str) -> Option<&'static str> {
        block(name).loot_table.as_deref()
    }

    /// Block entity type names in registry order.
    pub fn block_entity_types() -> &'static [String] {
        &tables().block_entity_types
    }
}

/// Default item components.
pub mod item_data {
    use super::*;

    #[derive(Clone, Debug, Default)]
    pub struct Food {
        pub nutrition: i32,
        pub saturation: f32,
        pub can_always_eat: bool,
    }

    #[derive(Clone, Debug)]
    pub struct ToolRule {
        /// A tag (`#minecraft:mineable/pickaxe`) or block names.
        pub blocks: Vec<String>,
        pub speed: Option<f32>,
        pub correct_for_drops: Option<bool>,
    }

    #[derive(Clone, Debug, Default)]
    pub struct Tool {
        pub rules: Vec<ToolRule>,
        pub default_mining_speed: f32,
        pub damage_per_block: i32,
    }

    #[derive(Clone, Debug, Default)]
    pub struct ItemData {
        pub max_damage: Option<i32>,
        pub food: Option<Food>,
        /// Seconds it takes to eat or drink (only for consumables).
        pub consume_seconds: Option<f32>,
        pub use_remainder: Option<String>,
        pub tool: Option<Tool>,
        /// Equipment slot (`head`, `chest`, `legs`, `feet`, `body`, `saddle`, `offhand`).
        pub equip_slot: Option<String>,
        /// Attribute modifiers: (attribute, amount, operation, slot).
        pub modifiers: Vec<(String, f64, String, String)>,
        /// Furnace fuel: the `context_int_provider` entry giving the burn time
        /// (e.g. `minecraft:cooking/time_coal`), resolved from the vanilla data.
        pub fuel: Option<String>,
        pub fire_resistant: bool,
        /// All of the item's default components (without the `minecraft:` prefix).
        pub raw: Option<Value>,
    }

    impl ItemData {
        /// Sum of the `add_value` modifiers of `attribute` that apply in `slot`.
        pub fn modifier(&self, attribute: &str, slot: &str) -> f64 {
            self.modifiers.iter().filter(|(a, _, op, s)| a == attribute && op == "add_value" && (s == slot || s == "any")).map(|(_, v, _, _)| *v).sum()
        }
    }

    fn table() -> &'static HashMap<String, ItemData> {
        static T: OnceLock<(Value, HashMap<String, ItemData>)> = OnceLock::new();
        let (_, map) = T.get_or_init(|| {
            let v: Value = serde_json::from_str(include_str!("../../../data/26.3/item_components.json")).expect("item_components.json");
            let mut map = HashMap::new();
            for (name, c) in v["items"].as_object().unwrap() {
                let food = c.get("food").map(|f| Food {
                    nutrition: f["nutrition"].as_i64().unwrap_or(0) as i32,
                    saturation: f["saturation"].as_f64().unwrap_or(0.0) as f32,
                    can_always_eat: f["can_always_eat"].as_bool().unwrap_or(false),
                });
                let consume_seconds = c.get("consumable").map(|x| x["consume_seconds"].as_f64().unwrap_or(1.6) as f32);
                let tool = c.get("tool").map(|t| Tool {
                    rules: t["rules"]
                        .as_array()
                        .map(|rules| {
                            rules
                                .iter()
                                .map(|r| ToolRule {
                                    blocks: match &r["blocks"] {
                                        Value::String(s) => vec![s.clone()],
                                        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                                        _ => Vec::new(),
                                    },
                                    speed: r["speed"].as_f64().map(|s| s as f32),
                                    correct_for_drops: r["correct_for_drops"].as_bool(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    default_mining_speed: t["default_mining_speed"].as_f64().unwrap_or(1.0) as f32,
                    damage_per_block: t["damage_per_block"].as_i64().unwrap_or(1) as i32,
                });
                let modifiers = c
                    .get("attribute_modifiers")
                    .and_then(Value::as_array)
                    .map(|mods| {
                        mods.iter()
                            .map(|m| {
                                (
                                    m["type"].as_str().unwrap_or("").to_owned(),
                                    m["amount"].as_f64().unwrap_or(0.0),
                                    m["operation"].as_str().unwrap_or("add_value").to_owned(),
                                    m["slot"].as_str().unwrap_or("any").to_owned(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                map.insert(
                    name.clone(),
                    ItemData {
                        max_damage: c.get("max_damage").and_then(Value::as_i64).map(|d| d as i32),
                        food,
                        consume_seconds,
                        use_remainder: c.get("use_remainder").and_then(|r| r["id"].as_str()).map(str::to_owned),
                        tool,
                        equip_slot: c.get("equippable").and_then(|e| e["slot"].as_str()).map(str::to_owned),
                        modifiers,
                        fuel: c.get("cooking_fuel").and_then(|f| f["burn_time"].as_str()).map(str::to_owned),
                        fire_resistant: c.get("damage_resistant").is_some(),
                        raw: Some(c.clone()),
                    },
                );
            }
            (v, map)
        });
        map
    }

    static EMPTY: OnceLock<ItemData> = OnceLock::new();

    /// The default components of an item (empty for items without any of interest).
    pub fn get(name: &str) -> &'static ItemData {
        table().get(normalize(name).as_ref()).unwrap_or_else(|| EMPTY.get_or_init(ItemData::default))
    }

    pub fn by_id(id: u32) -> &'static ItemData {
        crate::items::get(id).map_or_else(|| EMPTY.get_or_init(ItemData::default), |info| get(info.name))
    }
}

/// Entity types: sizes, attributes, categories and the metadata layout.
pub mod entity_types {
    use super::*;

    #[derive(Clone, Debug)]
    pub struct EntityTypeInfo {
        pub name: String,
        pub id: i32,
        /// `MONSTER`, `CREATURE`, `AMBIENT`, `WATER_CREATURE`, `MISC`, ...
        pub category: String,
        pub width: f32,
        pub height: f32,
        pub eye_height: f32,
        pub mob: bool,
        pub fire_immune: bool,
        /// In chunks.
        pub tracking_range: i32,
        pub update_interval: i32,
        pub track_deltas: bool,
        pub attributes: HashMap<String, f64>,
        pub max_health: f32,
        pub experience: i32,
        pub limit_per_chunk: i32,
        /// Spawn placement (`ON_GROUND`, `IN_WATER`, ...) and heightmap.
        pub spawn_location: String,
        pub spawn_heightmap: String,
        pub hurt_sound: Option<String>,
        pub death_sound: Option<String>,
        pub ambient_sound: Option<String>,
        /// Metadata fields: name -> (index, serializer id).
        pub data: HashMap<String, (u8, u8)>,
    }

    impl EntityTypeInfo {
        pub fn attribute(&self, name: &str) -> Option<f64> {
            self.attributes.get(name).copied()
        }

        /// Metadata index of a field such as `DATA_HEALTH_ID`.
        pub fn data_index(&self, field: &str) -> Option<u8> {
            self.data.get(field).map(|(i, _)| *i)
        }
    }

    struct Tables {
        by_name: HashMap<String, EntityTypeInfo>,
        names_by_id: HashMap<i32, String>,
    }

    fn tables() -> &'static Tables {
        static T: OnceLock<Tables> = OnceLock::new();
        T.get_or_init(|| {
            let v: Value = serde_json::from_str(include_str!("../../../data/26.3/entities.json")).expect("entities.json");
            let d: Value = serde_json::from_str(include_str!("../../../data/26.3/entity_data.json")).expect("entity_data.json");
            let mut by_name = HashMap::new();
            let mut names_by_id = HashMap::new();
            for (name, e) in v["entities"].as_object().unwrap() {
                let dim: Vec<f64> = e["dimension"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                let data = d["entities"].get(name).and_then(Value::as_object).map(|fields| {
                    fields
                        .iter()
                        .map(|(k, v)| (k.clone(), (v[0].as_u64().unwrap_or(0) as u8, v[1].as_u64().unwrap_or(0) as u8)))
                        .collect()
                });
                let info = EntityTypeInfo {
                    name: name.clone(),
                    id: e["id"].as_i64().unwrap_or(0) as i32,
                    category: e["category"].as_str().unwrap_or("MISC").to_owned(),
                    width: dim.first().copied().unwrap_or(0.6) as f32,
                    height: dim.get(1).copied().unwrap_or(1.8) as f32,
                    eye_height: e["eye_height"].as_f64().unwrap_or(1.62) as f32,
                    mob: e["mob"].as_bool().unwrap_or(false),
                    fire_immune: e["fire_immune"].as_bool().unwrap_or(false),
                    tracking_range: e["tracking_range"].as_i64().unwrap_or(5) as i32,
                    update_interval: e["update_interval"].as_i64().unwrap_or(3) as i32,
                    track_deltas: e["track_deltas"].as_bool().unwrap_or(false),
                    attributes: e["attributes"].as_object().map(|a| a.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect()).unwrap_or_default(),
                    max_health: e["max_health"].as_f64().unwrap_or(0.0) as f32,
                    experience: e["experience_reward"].as_i64().unwrap_or(0) as i32,
                    limit_per_chunk: e["limit_per_chunk"].as_i64().unwrap_or(4) as i32,
                    spawn_location: e["spawn_restriction"]["location"].as_str().unwrap_or("NO_RESTRICTIONS").to_owned(),
                    spawn_heightmap: e["spawn_restriction"]["heightmap"].as_str().unwrap_or("MOTION_BLOCKING_NO_LEAVES").to_owned(),
                    hurt_sound: e["hurt_sound"].as_str().map(str::to_owned),
                    death_sound: e["death_sound"].as_str().map(str::to_owned),
                    ambient_sound: e["ambient_sound"].as_str().map(str::to_owned),
                    data: data.unwrap_or_default(),
                };
                names_by_id.insert(info.id, name.clone());
                by_name.insert(name.clone(), info);
            }
            Tables { by_name, names_by_id }
        })
    }

    pub fn by_name(name: &str) -> Option<&'static EntityTypeInfo> {
        tables().by_name.get(normalize(name).as_ref())
    }

    pub fn by_id(id: i32) -> Option<&'static EntityTypeInfo> {
        let t = tables();
        t.names_by_id.get(&id).and_then(|n| t.by_name.get(n))
    }

    pub fn all() -> impl Iterator<Item = &'static EntityTypeInfo> {
        tables().by_name.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_flags_items_entities() {
        let stone = crate::blocks::default_state("minecraft:stone").unwrap();
        assert_eq!(physics::collision_boxes(stone), &[physics::Aabb { min: [0.0; 3], max: [1.0; 3] }]);
        assert!(physics::collision_boxes(crate::blocks::AIR).is_empty());
        let slab = crate::blocks::parse_state("minecraft:stone_slab[type=bottom]").unwrap();
        assert_eq!(physics::collision_boxes(slab)[0].max[1], 0.5);
        assert!(physics::requires_correct_tool(stone));
        assert!(!physics::requires_correct_tool(crate::blocks::default_state("minecraft:dirt").unwrap()));
        assert_eq!(physics::loot_table("minecraft:stone"), Some("minecraft:blocks/stone"));
        assert!((physics::block("minecraft:ice").slipperiness - 0.98).abs() < 1e-6);

        let bread = item_data::get("minecraft:bread");
        assert_eq!(bread.food.as_ref().unwrap().nutrition, 5);
        let pick = item_data::get("minecraft:diamond_pickaxe");
        assert_eq!(pick.max_damage, Some(1561));
        assert!(pick.tool.as_ref().unwrap().rules.iter().any(|r| r.blocks == ["#minecraft:mineable/pickaxe"] && r.speed == Some(8.0)));
        assert_eq!(pick.modifier("minecraft:attack_damage", "mainhand"), 4.0);
        let chest = item_data::get("minecraft:diamond_chestplate");
        assert_eq!(chest.equip_slot.as_deref(), Some("chest"));
        assert_eq!(chest.modifier("minecraft:armor", "chest"), 8.0);
        assert_eq!(item_data::get("minecraft:coal").fuel.as_deref(), Some("minecraft:cooking/time_coal"));

        let zombie = entity_types::by_name("minecraft:zombie").unwrap();
        assert_eq!(zombie.category, "MONSTER");
        assert_eq!(zombie.max_health, 20.0);
        assert_eq!(zombie.attribute("minecraft:attack_damage").or(zombie.attribute("attack_damage")), Some(3.0));
        assert_eq!(zombie.data_index("DATA_SHARED_FLAGS_ID"), Some(0));
        assert!(zombie.data_index("DATA_HEALTH_ID").is_some());
        let item = entity_types::by_name("minecraft:item").unwrap();
        assert_eq!((item.width, item.height), (0.25, 0.25));
        assert_eq!(entity_types::by_id(item.id).unwrap().name, "minecraft:item");
    }
}

/// Game rules (`minecraft:game_rule`) and their defaults.
pub mod game_rules {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Value {
        Bool(bool),
        Int(i32),
    }

    impl Value {
        pub fn as_bool(self) -> bool {
            match self {
                Value::Bool(b) => b,
                Value::Int(i) => i != 0,
            }
        }

        pub fn as_int(self) -> i32 {
            match self {
                Value::Bool(b) => b as i32,
                Value::Int(i) => i,
            }
        }
    }

    impl std::fmt::Display for Value {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Value::Bool(b) => write!(f, "{b}"),
                Value::Int(i) => write!(f, "{i}"),
            }
        }
    }

    #[derive(Clone, Debug)]
    pub struct GameRule {
        /// Registry name, e.g. `minecraft:keep_inventory`.
        pub name: String,
        pub default: Value,
        /// Bounds of an integer rule.
        pub min: i32,
        pub max: i32,
    }

    impl GameRule {
        /// Parses a value as typed in `/gamerule`, checking bounds.
        pub fn parse(&self, text: &str) -> Option<Value> {
            match self.default {
                Value::Bool(_) => text.parse().ok().map(Value::Bool),
                Value::Int(_) => text.parse().ok().filter(|v| (self.min..=self.max).contains(v)).map(Value::Int),
            }
        }
    }

    /// Every game rule, sorted by name.
    pub fn all() -> &'static [GameRule] {
        static T: OnceLock<Vec<GameRule>> = OnceLock::new();
        T.get_or_init(|| {
            let v: serde_json::Value = serde_json::from_str(include_str!("../../../data/26.3/game_rules.json")).expect("game_rules.json");
            let mut rules: Vec<GameRule> = v["rules"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(name, r)| {
                    let default = match &r["default"] {
                        serde_json::Value::Bool(b) => Value::Bool(*b),
                        other => Value::Int(other.as_i64().unwrap_or(0) as i32),
                    };
                    let bound = |k: &str, d: i64| r.get(k).and_then(serde_json::Value::as_i64).unwrap_or(d).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                    GameRule { name: name.clone(), default, min: bound("min", i32::MIN as i64), max: bound("max", i32::MAX as i64) }
                })
                .collect();
            rules.sort_by(|a, b| a.name.cmp(&b.name));
            rules
        })
    }

    /// A rule by name, with or without the `minecraft:` prefix; also accepts the old
    /// camelCase names (`keepInventory`).
    pub fn get(name: &str) -> Option<&'static GameRule> {
        let name = normalize(&legacy_name(name)).into_owned();
        all().iter().find(|r| r.name == name)
    }

    /// Maps pre-26 camelCase rule names to the current ones.
    pub fn legacy_name(name: &str) -> String {
        let renamed = match name {
            "doDaylightCycle" => "advance_time",
            "doWeatherCycle" => "advance_weather",
            "doTileDrops" => "block_drops",
            "doEntityDrops" => "entity_drops",
            "doMobLoot" => "mob_drops",
            "doMobSpawning" => "spawn_mobs",
            "doFireTick" => "fire_spread_radius_around_player",
            "doImmediateRespawn" => "immediate_respawn",
            "naturalRegeneration" => "natural_health_regeneration",
            "doInsomnia" => "spawn_phantoms",
            "doPatrolSpawning" => "spawn_patrols",
            "doTraderSpawning" => "spawn_wandering_traders",
            "doWardenSpawning" => "spawn_wardens",
            "disableRaids" => "raids",
            "announceAdvancements" => "show_advancement_messages",
            "commandModificationBlockLimit" => "max_block_modifications",
            "maxCommandChainLength" => "max_command_sequence_length",
            "playersSleepingPercentage" => "players_sleeping_percentage",
            "randomTickSpeed" => "random_tick_speed",
            "spawnRadius" => "respawn_radius",
            "snowAccumulationHeight" => "max_snow_accumulation_height",
            "waterSourceConversion" => "water_source_conversion",
            "lavaSourceConversion" => "lava_source_conversion",
            _ => "",
        };
        if !renamed.is_empty() {
            return renamed.to_owned();
        }
        // keepInventory -> keep_inventory and similar.
        let mut out = String::with_capacity(name.len() + 4);
        for c in name.chars() {
            if c.is_ascii_uppercase() {
                out.push('_');
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }
}
