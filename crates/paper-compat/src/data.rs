//! The tables of `data/compat`: for each older version its packet ids and, for every id
//! registry that differs, the old id of each 26.3 id (made by `tools/datagen/compat.py`).

use paper_protocol::packets::State;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The packets of one state in an older version.
#[derive(Debug, Default)]
pub struct PacketMap {
    /// The old id of each 26.3 clientbound packet (-1: the old version has none).
    pub clientbound: Vec<i32>,
    /// The 26.3 id of each old serverbound packet (-1: 26.3 has none).
    pub serverbound: Vec<i32>,
    /// The old version's packet names, by id.
    pub clientbound_names: Vec<String>,
    pub serverbound_names: Vec<String>,
}

impl PacketMap {
    /// The old id of the clientbound packet called `name` (as 26.3 calls it).
    pub fn clientbound_id(&self, name: &str) -> Option<i32> {
        self.clientbound_names.iter().position(|n| n == name).map(|i| i as i32)
    }

    /// The old id of the serverbound packet called `name`.
    pub fn serverbound_id(&self, name: &str) -> Option<i32> {
        self.serverbound_names.iter().position(|n| n == name).map(|i| i as i32)
    }
}

/// Ids of one registry: 26.3 to old and back.
#[derive(Debug, Default)]
pub struct Mapping {
    to_old: Vec<i32>,
    to_new: Vec<i32>,
}

impl Mapping {
    fn identity() -> Mapping {
        Mapping::default()
    }

    fn from_runs(runs: &[Value]) -> Mapping {
        let mut to_old = Vec::new();
        for run in runs {
            let r = run.as_array().expect("a run");
            let (first, old, len) = (r[0].as_i64().unwrap() as usize, r[1].as_i64().unwrap() as i32, r[2].as_i64().unwrap() as usize);
            to_old.resize(first, -1);
            to_old.extend((0..len).map(|k| if old < 0 { -1 } else { old + k as i32 }));
        }
        let size = to_old.iter().copied().max().map_or(0, |m| m + 1) as usize;
        let mut to_new = vec![-1; size];
        for (new, &old) in to_old.iter().enumerate() {
            if old >= 0 && to_new[old as usize] < 0 {
                to_new[old as usize] = new as i32;
            }
        }
        Mapping { to_old, to_new }
    }

    /// Whether every id stays as it is.
    pub fn is_identity(&self) -> bool {
        self.to_old.is_empty()
    }

    /// The old id of a 26.3 id (-1: none).
    pub fn old(&self, id: i32) -> i32 {
        if self.to_old.is_empty() {
            return id;
        }
        self.to_old.get(id as usize).copied().unwrap_or(-1)
    }

    /// The 26.3 id of an old id (-1: none).
    pub fn current(&self, old: i32) -> i32 {
        if self.to_old.is_empty() {
            return old;
        }
        self.to_new.get(old as usize).copied().unwrap_or(-1)
    }
}

/// A data-driven registry the server sends during configuration.
#[derive(Debug)]
pub enum RegistryMap {
    /// The old version knows every entry.
    Same,
    /// The old version does not have this registry: it is not sent at all.
    Dropped,
    /// Only the entries the old version knows are sent; references to the others point at
    /// `fallback` instead.
    Filtered { keep: Vec<usize>, mapping: Mapping },
}

impl RegistryMap {
    /// The old id of an entry (its index among those sent).
    pub fn old(&self, id: i32) -> i32 {
        match self {
            RegistryMap::Same => id,
            RegistryMap::Dropped => -1,
            RegistryMap::Filtered { mapping, .. } => mapping.old(id),
        }
    }

    pub fn current(&self, old: i32) -> i32 {
        match self {
            RegistryMap::Same => old,
            RegistryMap::Dropped => -1,
            RegistryMap::Filtered { mapping, .. } => mapping.current(old),
        }
    }

    /// Whether the 26.3 entry `index` is sent.
    pub fn keeps(&self, index: usize) -> bool {
        match self {
            RegistryMap::Same => true,
            RegistryMap::Dropped => false,
            RegistryMap::Filtered { keep, .. } => keep.binary_search(&index).is_ok(),
        }
    }
}

/// Everything known about an older version.
#[derive(Debug)]
pub struct VersionData {
    pub name: String,
    pub protocol: i32,
    /// The versions a client of this protocol may give its core data pack (`26.1`, `26.1.2`).
    pub pack_versions: Vec<String>,
    /// The size of each id registry of the mapping files in this version.
    sizes: HashMap<String, usize>,
    states: HashMap<State, PacketMap>,
    mappings: HashMap<String, Mapping>,
    registries: HashMap<String, RegistryMap>,
    identity: Mapping,
    same: RegistryMap,
}

impl VersionData {
    pub fn packets(&self, state: State) -> &PacketMap {
        &self.states[&state]
    }

    /// An id registry by its key in the mapping files (`blockstates`, `items`, `entities`,
    /// `sounds`, `particles`, `blocks`, `data_component_type`, ...); identity if unchanged.
    pub fn mapping(&self, key: &str) -> &Mapping {
        self.mappings.get(key).unwrap_or(&self.identity)
    }

    /// How many ids a registry of the mapping files has in this version (`blockstates`, ...).
    pub fn size(&self, key: &str) -> usize {
        self.sizes.get(key).copied().unwrap_or(0)
    }

    /// A data-driven registry by name (`minecraft:worldgen/biome`).
    pub fn registry(&self, name: &str) -> &RegistryMap {
        self.registries.get(name).unwrap_or(&self.same)
    }

    fn parse(text: &str) -> VersionData {
        let doc: Value = serde_json::from_str(text).expect("a compat data file");
        let names = |v: &Value| v.as_array().map(|a| a.iter().map(|n| n.as_str().unwrap_or("").to_owned()).collect::<Vec<_>>()).unwrap_or_default();
        let mut states = HashMap::new();
        for state in [State::Handshake, State::Status, State::Login, State::Configuration, State::Play] {
            let packets = &doc["packets"][state.name()];
            let (clientbound_names, serverbound_names) = (names(&packets["clientbound"]), names(&packets["serverbound"]));
            let ours = native_names(state);
            let clientbound = ours.0.iter().map(|n| clientbound_names.iter().position(|o| o == n).map_or(-1, |i| i as i32)).collect();
            let serverbound = serverbound_names.iter().map(|n| ours.1.iter().position(|o| o == n).map_or(-1, |i| i as i32)).collect();
            states.insert(state, PacketMap { clientbound, serverbound, clientbound_names, serverbound_names });
        }
        let mappings = doc["mappings"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), Mapping::from_runs(v.as_array().map(Vec::as_slice).unwrap_or(&[])))).collect()).unwrap_or_default();
        let mut registries = HashMap::new();
        for (name, entry) in doc["registries"].as_object().into_iter().flatten() {
            let map = if entry["dropped"].as_bool() == Some(true) {
                RegistryMap::Dropped
            } else {
                let keep: Vec<usize> = entry["keep"].as_array().map(|a| a.iter().filter_map(Value::as_u64).map(|i| i as usize).collect()).unwrap_or_default();
                let total = paper_data::synced::SYNCED_REGISTRIES.iter().find(|(r, _)| *r == name).map_or(0, |(_, e)| e.len());
                let fallback = entry["fallback"].as_u64().unwrap_or(0) as usize;
                let fallback_old = keep.iter().position(|&k| k == fallback).unwrap_or(0) as i32;
                let to_old: Vec<i32> = (0..total).map(|i| keep.iter().position(|&k| k == i).map_or(fallback_old, |p| p as i32)).collect();
                let to_new = keep.iter().map(|&k| k as i32).collect();
                RegistryMap::Filtered { keep, mapping: Mapping { to_old, to_new } }
            };
            registries.insert(name.clone(), map);
        }
        VersionData {
            name: doc["name"].as_str().unwrap_or("").to_owned(),
            protocol: doc["protocol"].as_i64().unwrap_or(0) as i32,
            pack_versions: names(&doc["pack_versions"]),
            sizes: doc["sizes"].as_object().map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_u64()? as usize))).collect()).unwrap_or_default(),
            states,
            mappings,
            registries,
            identity: Mapping::identity(),
            same: RegistryMap::Same,
        }
    }
}

/// The 26.3 packet names of a state: clientbound, serverbound.
fn native_names(state: State) -> (&'static [&'static str], &'static [&'static str]) {
    use paper_data::packets as p;
    match state {
        State::Handshake => (&[], p::handshake::serverbound::NAMES),
        State::Status => (p::status::clientbound::NAMES, p::status::serverbound::NAMES),
        State::Login => (p::login::clientbound::NAMES, p::login::serverbound::NAMES),
        State::Configuration => (p::configuration::clientbound::NAMES, p::configuration::serverbound::NAMES),
        State::Play => (p::play::clientbound::NAMES, p::play::serverbound::NAMES),
    }
}

macro_rules! files {
    ($($name:literal),* $(,)?) => {
        /// The data of a version (by its file name), parsed once.
        pub fn load(name: &str) -> &'static VersionData {
            static CACHE: OnceLock<HashMap<&'static str, OnceLock<VersionData>>> = OnceLock::new();
            let cache = CACHE.get_or_init(|| [$(($name, OnceLock::new())),*].into_iter().collect());
            let cell = cache.get(name).unwrap_or_else(|| panic!("no compat data for {name}"));
            cell.get_or_init(|| match name {
                $($name => VersionData::parse(include_str!(concat!("../../../data/compat/", $name, ".json"))),)*
                _ => unreachable!(),
            })
        }
    };
}

files!("26.2", "26.1", "1.21.11", "1.21.9", "1.21.7", "1.21.6", "1.21.5");
