//! Turns the JSON tables in `data/<version>/` into Rust source.
//!
//! The tables are produced by `tools/datagen` (see its README for provenance). Everything
//! generated here is plain constant data; lookups by name are built lazily at runtime in
//! `src/lib.rs`.

use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

const VERSION_DIR: &str = "26.3";

fn data_dir() -> PathBuf {
    Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../data").join(VERSION_DIR)
}

fn load(name: &str) -> Value {
    let path = data_dir().join(name);
    println!("cargo:rerun-if-changed={}", path.display());
    let text = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("missing data table {}: {e}\nRun tools/datagen (see tools/datagen/README.md) to regenerate data/{VERSION_DIR}/.", path.display())
    });
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

fn exists(name: &str) -> bool {
    let path = data_dir().join(name);
    println!("cargo:rerun-if-changed={}", path.display());
    path.exists()
}

fn out(name: &str, contents: &str) {
    let path = Path::new(&env::var("OUT_DIR").unwrap()).join(name);
    fs::write(&path, contents).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

fn out_bytes(name: &str, contents: &[u8]) {
    let path = Path::new(&env::var("OUT_DIR").unwrap()).join(name);
    fs::write(&path, contents).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

/// `minecraft:debug/block_value` -> `DEBUG_BLOCK_VALUE`
fn const_name(id: &str) -> String {
    let path = id.split_once(':').map_or(id, |(_, p)| p);
    let mut s: String = path.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

fn str_array(items: &[Value]) -> Vec<&str> {
    items.iter().map(|v| v.as_str().expect("expected a string")).collect()
}

fn gen_version() {
    let v = load("version.json");
    let mut s = String::new();
    writeln!(s, "/// Minecraft version these tables describe.").unwrap();
    writeln!(s, "pub const VERSION_NAME: &str = {:?};", v["id"].as_str().unwrap()).unwrap();
    writeln!(s, "/// Network protocol version.").unwrap();
    writeln!(s, "pub const PROTOCOL_VERSION: i32 = {};", v["protocol"].as_i64().unwrap()).unwrap();
    writeln!(s, "/// World data version written to and expected in saved chunks.").unwrap();
    writeln!(s, "pub const DATA_VERSION: i32 = {};", v["data_version"].as_i64().unwrap()).unwrap();
    writeln!(s, "pub const DATA_PACK_VERSION: (i32, i32) = ({}, {});", v["data_pack_version"][0], v["data_pack_version"][1]).unwrap();
    writeln!(s, "pub const RESOURCE_PACK_VERSION: (i32, i32) = ({}, {});", v["resource_pack_version"][0], v["resource_pack_version"][1]).unwrap();
    out("version.rs", &s);
}

fn gen_packets() {
    let v = load("packets.json");
    let mut s = String::new();
    for state in ["handshake", "status", "login", "configuration", "play"] {
        writeln!(s, "pub mod {state} {{").unwrap();
        for side in ["serverbound", "clientbound"] {
            let names = str_array(v[state][side].as_array().unwrap_or_else(|| panic!("packets.json lacks {state}.{side}")));
            writeln!(s, "    pub mod {side} {{").unwrap();
            let mut seen = BTreeSet::new();
            for (id, name) in names.iter().enumerate() {
                let c = const_name(name);
                assert!(seen.insert(c.clone()), "duplicate packet constant {c} in {state}.{side}");
                writeln!(s, "        pub const {c}: i32 = {id};").unwrap();
            }
            writeln!(s, "        /// Packet names indexed by id.").unwrap();
            writeln!(s, "        pub const NAMES: &[&str] = &{names:?};").unwrap();
            writeln!(s, "    }}").unwrap();
        }
        writeln!(s, "}}").unwrap();
    }
    out("packets.rs", &s);
}

/// `registries.json`: every built-in registry as a name table indexed by protocol id.
fn gen_registries() {
    let mut s = String::new();
    if !exists("registries.json") {
        writeln!(s, "pub const REGISTRIES: &[(&str, &[&str])] = &[];").unwrap();
        out("registries.rs", &s);
        return;
    }
    let v = load("registries.json");
    let obj = v.as_object().expect("registries.json must be an object");
    let mut index = String::from("/// Every extracted registry: `(registry id, entry names indexed by protocol id)`.\npub const REGISTRIES: &[(&str, &[&str])] = &[\n");
    for (registry, entries) in obj {
        let names = str_array(entries.as_array().expect("registry must be an array"));
        let c = const_name(registry);
        writeln!(s, "/// `{registry}` entries indexed by protocol id.").unwrap();
        writeln!(s, "pub const {c}: &[&str] = &{names:?};").unwrap();
        writeln!(index, "    ({registry:?}, {c}),").unwrap();
    }
    index.push_str("];\n");
    s.push_str(&index);
    out("registries.rs", &s);
}

/// `blocks.json` + `block_states.json`.
fn gen_blocks() {
    let mut s = String::new();
    if !exists("blocks.json") {
        writeln!(s, "pub const BLOCKS: &[BlockInfo] = &[];").unwrap();
        writeln!(s, "pub const STATE_COUNT: u32 = 0;").unwrap();
        writeln!(s, "static STATE_TO_BLOCK_RAW: &[u8] = &[];").unwrap();
        writeln!(s, "pub(crate) static STATE_LUMINANCE: &[u8] = &[];").unwrap();
        writeln!(s, "pub(crate) static STATE_OPACITY: &[u8] = &[];").unwrap();
        writeln!(s, "pub(crate) static STATE_FLAGS: &[u8] = &[];").unwrap();
        out("blocks.rs", &s);
        return;
    }
    let v = load("blocks.json");
    let blocks = v["blocks"].as_array().expect("blocks.json: blocks");
    let state_count = v["state_count"].as_u64().expect("blocks.json: state_count") as usize;
    assert!(blocks.len() < u16::MAX as usize, "block index must fit u16");

    // Property value lists repeat heavily (facing, waterlogged, ...): intern them.
    let mut value_sets: Vec<Vec<String>> = Vec::new();
    let mut intern = |values: Vec<String>| -> usize {
        if let Some(i) = value_sets.iter().position(|v| *v == values) {
            return i;
        }
        value_sets.push(values);
        value_sets.len() - 1
    };

    let mut state_to_block = vec![0u16; state_count];
    let mut table = String::from("/// Every block in registry (protocol id) order.\npub const BLOCKS: &[BlockInfo] = &[\n");
    let mut expected_min = 0usize;
    for (index, b) in blocks.iter().enumerate() {
        let name = b["name"].as_str().unwrap();
        assert_eq!(b["id"].as_u64().unwrap() as usize, index, "block ids must be contiguous ({name})");
        let min = b["min_state"].as_u64().unwrap() as usize;
        assert_eq!(min, expected_min, "state ids must be contiguous ({name})");
        let default = b["default_state"].as_u64().unwrap() as usize;
        let mut props = String::new();
        let mut count = 1usize;
        for p in b["properties"].as_array().unwrap() {
            let values: Vec<String> = p["values"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
            count *= values.len();
            let set = intern(values);
            write!(props, "({:?}, PROPERTY_VALUES_{set}), ", p["name"].as_str().unwrap()).unwrap();
        }
        assert!(default >= min && default < min + count, "default state out of range ({name})");
        for st in min..min + count {
            state_to_block[st] = index as u16;
        }
        expected_min = min + count;
        let item = match b["item"].as_str() {
            Some(i) => format!("Some({i:?})"),
            None => "None".into(),
        };
        let hardness = b["hardness"].as_f64().map_or("f32::NAN".to_string(), |h| format!("{h:?}"));
        let blast = b["blast_resistance"].as_f64().map_or("f32::NAN".to_string(), |h| format!("{h:?}"));
        writeln!(
            table,
            "    BlockInfo {{ name: {name:?}, min_state: {min}, state_count: {count}, default_state: {default}, properties: &[{props}], item: {item}, hardness: {hardness}, blast_resistance: {blast} }},"
        )
        .unwrap();
    }
    assert_eq!(expected_min, state_count, "state_count does not match the sum of block states");
    table.push_str("];\n");

    for (i, values) in value_sets.iter().enumerate() {
        writeln!(s, "const PROPERTY_VALUES_{i}: &[&str] = &{values:?};").unwrap();
    }
    s.push_str(&table);
    writeln!(s, "/// Total number of block states.").unwrap();
    writeln!(s, "pub const STATE_COUNT: u32 = {state_count};").unwrap();

    let mut raw = Vec::with_capacity(state_count * 2);
    for b in &state_to_block {
        raw.extend_from_slice(&b.to_le_bytes());
    }
    out_bytes("state_to_block.bin", &raw);
    writeln!(s, "static STATE_TO_BLOCK_RAW: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/state_to_block.bin\"));").unwrap();

    let states = load("block_states.json");
    for (key, file) in [("luminance", "state_luminance.bin"), ("opacity", "state_opacity.bin"), ("flags", "state_flags.bin")] {
        let arr = states[key].as_array().unwrap_or_else(|| panic!("block_states.json: {key}"));
        assert_eq!(arr.len(), state_count, "block_states.json: {key} has the wrong length");
        let bytes: Vec<u8> = arr.iter().map(|v| v.as_u64().unwrap_or(0) as u8).collect();
        out_bytes(file, &bytes);
        writeln!(
            s,
            "pub(crate) static STATE_{}: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{file}\"));",
            key.to_ascii_uppercase()
        )
        .unwrap();
    }
    out("blocks.rs", &s);
}

/// `items.json`.
fn gen_items() {
    let mut s = String::new();
    if !exists("items.json") {
        writeln!(s, "pub const ITEMS: &[ItemInfo] = &[];").unwrap();
        out("items.rs", &s);
        return;
    }
    let v = load("items.json");
    writeln!(s, "/// Every item in registry (protocol id) order.").unwrap();
    writeln!(s, "pub const ITEMS: &[ItemInfo] = &[").unwrap();
    for (index, item) in v["items"].as_array().expect("items.json: items").iter().enumerate() {
        let name = item["name"].as_str().unwrap();
        assert_eq!(item["id"].as_u64().unwrap() as usize, index, "item ids must be contiguous ({name})");
        let block = match item["block"].as_str() {
            Some(b) => format!("Some({b:?})"),
            None => "None".into(),
        };
        writeln!(s, "    ItemInfo {{ name: {name:?}, max_stack_size: {}, block: {block} }},", item["max_stack_size"].as_u64().unwrap_or(64)).unwrap();
    }
    writeln!(s, "];").unwrap();
    out("items.rs", &s);
}

/// `synced_registries.json`, `tags.json`, `feature_flags.json`, `dimension_types.json`.
fn gen_synced() {
    let mut s = String::new();
    if !exists("synced_registries.json") {
        writeln!(s, "pub const SYNCED_REGISTRIES: &[(&str, &[&str])] = &[];").unwrap();
        writeln!(s, "pub const TAGS: &[(&str, &[(&str, &[i32])])] = &[];").unwrap();
        writeln!(s, "pub const FEATURE_FLAGS: &[&str] = &[\"minecraft:vanilla\"];").unwrap();
        writeln!(s, "pub const DIMENSION_TYPES: &[DimensionTypeInfo] = &[];").unwrap();
        out("synced.rs", &s);
        return;
    }
    let v = load("synced_registries.json");
    writeln!(s, "/// Registries synchronised to clients in the configuration phase, in send order,").unwrap();
    writeln!(s, "/// each with its entries in network id order.").unwrap();
    writeln!(s, "pub const SYNCED_REGISTRIES: &[(&str, &[&str])] = &[").unwrap();
    for registry in str_array(v["order"].as_array().expect("synced_registries.json: order")) {
        let entries = str_array(v["entries"][registry].as_array().unwrap_or_else(|| panic!("synced_registries.json: entries for {registry}")));
        writeln!(s, "    ({registry:?}, &{entries:?}),").unwrap();
    }
    writeln!(s, "];").unwrap();

    let tags = load("tags.json");
    writeln!(s, "/// Tags sent to clients: `(registry, [(tag, numeric entry ids)])`.").unwrap();
    writeln!(s, "pub const TAGS: &[(&str, &[(&str, &[i32])])] = &[").unwrap();
    for (registry, table) in tags.as_object().expect("tags.json must be an object") {
        writeln!(s, "    ({registry:?}, &[").unwrap();
        for (tag, ids) in table.as_object().expect("tag table must be an object") {
            let ids: Vec<i64> = ids.as_array().unwrap().iter().map(|i| i.as_i64().unwrap()).collect();
            writeln!(s, "        ({tag:?}, &{ids:?}),").unwrap();
        }
        writeln!(s, "    ]),").unwrap();
    }
    writeln!(s, "];").unwrap();

    let flags = load("feature_flags.json");
    writeln!(s, "/// Feature flags a vanilla server enables by default.").unwrap();
    writeln!(s, "pub const FEATURE_FLAGS: &[&str] = &{:?};", str_array(flags.as_array().expect("feature_flags.json"))).unwrap();

    let dims = load("dimension_types.json");
    writeln!(s, "/// The dimension type definitions the server needs to size its worlds.").unwrap();
    writeln!(s, "pub const DIMENSION_TYPES: &[DimensionTypeInfo] = &[").unwrap();
    for (name, d) in dims.as_object().expect("dimension_types.json must be an object") {
        writeln!(
            s,
            "    DimensionTypeInfo {{ name: {name:?}, min_y: {}, height: {}, logical_height: {}, has_skylight: {}, has_ceiling: {}, fast_lava: {}, coordinate_scale: {:?}, water_evaporates: {}, infiniburn: {:?} }},",
            d["min_y"].as_i64().expect("min_y"),
            d["height"].as_i64().expect("height"),
            d["logical_height"].as_i64().unwrap_or_else(|| d["height"].as_i64().unwrap()),
            d["has_skylight"].as_bool().unwrap_or(true),
            d["has_ceiling"].as_bool().unwrap_or(false),
            d["attributes"]["minecraft:gameplay/fast_lava"].as_bool().unwrap_or(false),
            d["coordinate_scale"].as_f64().unwrap_or(1.0),
            d["attributes"]["minecraft:gameplay/water_evaporates"].as_bool().unwrap_or(false),
            d["infiniburn"].as_str().unwrap_or("#minecraft:infiniburn_overworld").trim_start_matches('#'),
        )
        .unwrap();
    }
    writeln!(s, "];").unwrap();
    out("synced.rs", &s);
}

/// `block_behaviour.json`: per-state support faces, physics flags, collision faces and note
/// block instruments (see `tools/datagen/physics.py`).
fn gen_physics() {
    let mut s = String::new();
    let names = ["support", "flags", "collision_faces", "instrument"];
    if !exists("block_behaviour.json") {
        writeln!(s, "pub const INSTRUMENTS: &[&str] = &[\"harp\"];").unwrap();
        for name in names {
            writeln!(s, "static {}: &[u8] = &[];", name.to_ascii_uppercase()).unwrap();
        }
        out("physics.rs", &s);
        return;
    }
    let v = load("block_behaviour.json");
    let states = load("blocks.json")["state_count"].as_u64().expect("blocks.json: state_count") as usize;
    let instruments = str_array(v["instruments"].as_array().expect("block_behaviour.json: instruments"));
    writeln!(s, "/// Note block instruments, in the order of `note_block`'s `instrument` property.").unwrap();
    writeln!(s, "pub const INSTRUMENTS: &[&str] = &{instruments:?};").unwrap();
    for name in names {
        let arr = v[name].as_array().unwrap_or_else(|| panic!("block_behaviour.json: {name}"));
        assert_eq!(arr.len(), states, "block_behaviour.json: {name} has the wrong length");
        let bytes: Vec<u8> = arr.iter().map(|x| x.as_u64().expect("a number") as u8).collect();
        let file = format!("physics_{name}.bin");
        out_bytes(&file, &bytes);
        writeln!(s, "static {}: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{file}\"));", name.to_ascii_uppercase()).unwrap();
    }
    out("physics.rs", &s);
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    gen_version();
    gen_packets();
    gen_registries();
    gen_blocks();
    gen_items();
    gen_synced();
    gen_physics();
}
