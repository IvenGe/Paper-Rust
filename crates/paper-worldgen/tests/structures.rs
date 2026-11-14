//! Structure placement and structure starts compared with vanilla 26.3.
//!
//! `structure_starts.json.gz` is a vanilla dump (from Pumpkin's `assets/tests/`, seed 0):
//! the starts vanilla generated in 2000 chunks per dimension, with every piece's type,
//! generation depth, orientation and bounding box (and for jigsaw pieces their position,
//! template and junctions).

mod common;

use common::generator;
use paper_worldgen::structure::transform::Rotation;
use paper_worldgen::Dimension;
use serde_json::Value;
use std::collections::BTreeMap;

fn bb(v: &Value) -> [i32; 6] {
    let a = v.as_array().unwrap();
    std::array::from_fn(|i| a[i].as_i64().unwrap() as i32)
}

/// Compares the starts of the dump for the structures this crate generates; returns the
/// number of matching starts per structure.
fn check_starts(dimension: Dimension, key: &str) -> BTreeMap<String, usize> {
    let mut matched = BTreeMap::new();
    let Some(g) = generator(dimension, 0) else { return matched };
    let data = common::json_fixture("structure_starts");
    let supported = g.supported_structures();
    let mut failures = Vec::new();
    for start in data[key].as_array().unwrap() {
        let id = start["s"].as_str().unwrap();
        if !supported.contains(&id) {
            continue;
        }
        let (x, z) = (start["x"].as_i64().unwrap() as i32, start["z"].as_i64().unwrap() as i32);
        let starts = g.structure_starts(x, z);
        let Some(s) = starts.iter().find(|s| s.id() == id) else {
            failures.push(format!("{id} at ({x}, {z}): no start (have {:?})", starts.iter().map(|s| s.id().to_owned()).collect::<Vec<_>>()));
            continue;
        };
        let b = s.bbox;
        let ours = [b.min_x, b.min_y, b.min_z, b.max_x, b.max_y, b.max_z];
        let pieces = s.pieces();
        let expected = start["p"].as_array().unwrap();
        let mut bad = None;
        if ours != bb(&start["bb"]) {
            bad = Some(format!("{id} at ({x}, {z}): bounding box {ours:?}, vanilla {:?}", bb(&start["bb"])));
        } else if pieces.len() != expected.len() {
            bad = Some(format!("{id} at ({x}, {z}): {} pieces, vanilla {}", pieces.len(), expected.len()));
        }
        for (i, (p, e)) in pieces.iter().zip(expected).enumerate() {
            let pb = [p.bbox.min_x, p.bbox.min_y, p.bbox.min_z, p.bbox.max_x, p.bbox.max_y, p.bbox.max_z];
            let kind = p.kind.strip_prefix("minecraft:").unwrap_or(p.kind);
            if pb != bb(&e["bb"]) || kind != e["t"].as_str().unwrap() || p.gen_depth as i64 != e["d"].as_i64().unwrap() || p.orientation as i64 != e["o"].as_i64().unwrap() {
                bad = Some(format!("{}{id} at ({x}, {z}) piece {i}: {kind} {pb:?} depth {} orientation {}, vanilla {e}", bad.map(|b| b + "; ").unwrap_or_default(), p.gen_depth, p.orientation));
                break;
            }
            if let Some(pos) = e.get("pos") {
                let jig = p.jigsaw.as_ref().expect("jigsaw piece");
                let rot = match p.rotation {
                    Some(Rotation::None) => "NONE",
                    Some(Rotation::Clockwise90) => "CLOCKWISE_90",
                    Some(Rotation::Clockwise180) => "CLOCKWISE_180",
                    Some(Rotation::CounterClockwise90) => "COUNTERCLOCKWISE_90",
                    None => "?",
                };
                let junctions: Vec<Value> = jig
                    .junctions
                    .iter()
                    .map(|j| serde_json::json!([j.source_x, j.source_ground_y, j.source_z, j.delta_y, if j.dest_rigid { "rigid" } else { "terrain_matching" }]))
                    .collect();
                let ours = serde_json::json!({
                    "pos": p.position.unwrap(),
                    "loc": p.element.clone(),
                    "rot": rot,
                    "gld": jig.ground_level_delta,
                    "j": junctions,
                });
                let theirs = serde_json::json!({"pos": pos, "loc": e.get("loc"), "rot": e["rot"], "gld": e["gld"], "j": e["j"]});
                if ours != theirs {
                    bad = Some(format!("{}{id} at ({x}, {z}) piece {i}: {ours}, vanilla {theirs}", bad.map(|b| b + "; ").unwrap_or_default()));
                    break;
                }
            }
        }
        if let (Some(_), Ok(dir)) = (&bad, std::env::var("STRUCTURE_DUMP_DIR")) {
            let ours: Vec<Value> = pieces
                .iter()
                .map(|p| serde_json::json!({"bb": [p.bbox.min_x, p.bbox.min_y, p.bbox.min_z, p.bbox.max_x, p.bbox.max_y, p.bbox.max_z], "pos": p.position, "loc": p.element, "j": p.jigsaw.as_ref().map(|j| j.junctions.iter().map(|j| [j.source_x, j.source_ground_y, j.source_z, j.delta_y]).collect::<Vec<_>>())}))
                .collect();
            let file = format!("{dir}/{}_{x}_{z}.json", id.replace(':', "_"));
            std::fs::write(file, serde_json::to_string(&serde_json::json!({"ours": ours, "vanilla": expected})).unwrap()).unwrap();
        }
        match bad {
            Some(m) => failures.push(m),
            None => *matched.entry(id.to_owned()).or_insert(0) += 1,
        }
    }
    eprintln!("{dimension:?}: matched {matched:?}");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
    matched
}

/// Compares the structure references of every chunk of `structure_references.json.gz` (the
/// references vanilla recorded in the chunks of the same dump that have any, extracted from
/// Pumpkin's `assets/tests/structure_starts.json`) for the structures this crate generates.
fn check_references(dimension: Dimension, key: &str) -> usize {
    let Some(g) = generator(dimension, 0) else { return 0 };
    let data = common::json_fixture("structure_references");
    let supported = g.supported_structures();
    let mut failures = Vec::new();
    let mut checked = 0;
    for row in data[key].as_array().unwrap() {
        let (x, z) = (row[0].as_i64().unwrap() as i32, row[1].as_i64().unwrap() as i32);
        let mut theirs: BTreeMap<String, Vec<(i32, i32)>> = BTreeMap::new();
        for (id, starts) in row[2].as_object().unwrap() {
            if supported.contains(&id.as_str()) {
                let mut v: Vec<(i32, i32)> = starts.as_array().unwrap().iter().map(|s| (s[0].as_i64().unwrap() as i32, s[1].as_i64().unwrap() as i32)).collect();
                v.sort();
                theirs.insert(id.clone(), v);
            }
        }
        let mut ours: BTreeMap<String, Vec<(i32, i32)>> = BTreeMap::new();
        for (id, mut starts) in g.structure_references(x, z) {
            starts.sort();
            ours.insert(id, starts);
        }
        if ours != theirs {
            failures.push(format!("chunk ({x}, {z}): ours {ours:?}, vanilla {theirs:?}"));
        }
        checked += theirs.values().map(Vec::len).sum::<usize>();
    }
    eprintln!("{dimension:?}: {checked} references checked");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures[..failures.len().min(20)].join("\n"));
    checked
}

#[test]
fn overworld_starts() {
    check_starts(Dimension::Overworld, "overworld");
}

#[test]
fn references_match_vanilla() {
    check_references(Dimension::Overworld, "overworld");
    check_references(Dimension::Nether, "the_nether");
    check_references(Dimension::End, "the_end");
}

#[test]
fn nether_starts() {
    check_starts(Dimension::Nether, "the_nether");
}

#[test]
fn end_starts() {
    check_starts(Dimension::End, "the_end");
}

/// Decorates `(cx, cz)` with freshly generated neighbours.
fn decorate(g: &paper_worldgen::WorldGenerator, cx: i32, cz: i32) -> paper_worldgen::Region {
    let mut chunks = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks.push(g.generate_terrain(cx + dx, cz + dz));
        }
    }
    let mut region = paper_worldgen::Region::new(cx, cz, chunks);
    g.decorate(cx, cz, &mut region);
    region
}

/// Counts the blocks of `name` in the center chunk of a decorated region.
fn count(region: &paper_worldgen::Region, name: &str) -> usize {
    let c = region.center();
    let mut n = 0;
    for y in c.min_y..c.min_y + c.height {
        for z in 0..16 {
            for x in 0..16 {
                if paper_data::blocks::block_of_state(c.get(x, y, z)).name == name {
                    n += 1;
                }
            }
        }
    }
    n
}

#[test]
fn locate_finds_fixture_starts() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let data = common::json_fixture("structure_starts");
    for start in data["overworld"].as_array().unwrap() {
        let id = start["s"].as_str().unwrap();
        if !matches!(id, "minecraft:village_plains" | "minecraft:trial_chambers" | "minecraft:shipwreck") {
            continue;
        }
        let (x, z) = (start["x"].as_i64().unwrap() as i32, start["z"].as_i64().unwrap() as i32);
        // Searching from the start's chunk finds that start.
        let found = g.locate(id, x * 16 + 8, z * 16 + 8, 2).unwrap_or_else(|| panic!("{id} near ({x}, {z})"));
        assert_eq!((found.chunk_x, found.chunk_z), (x, z), "{id}");
    }
    // The nearest stronghold to the origin is a start on its ring.
    let rings = g.ring_positions("minecraft:stronghold");
    assert_eq!(rings.len(), 128);
    let s = g.locate("minecraft:stronghold", 0, 0, 100).expect("a stronghold");
    assert!(rings.contains(&(s.chunk_x, s.chunk_z)));
    assert!(g.structure_starts(s.chunk_x, s.chunk_z).iter().any(|st| st.id() == "minecraft:stronghold"));
}

#[test]
fn scattered_structures_are_placed() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let checks = [
        ("minecraft:swamp_hut", "minecraft:spruce_planks"),
        ("minecraft:jungle_pyramid", "minecraft:mossy_cobblestone"),
        ("minecraft:igloo", "minecraft:snow_block"),
        ("minecraft:desert_pyramid", "minecraft:cut_sandstone"),
    ];
    for (id, block) in checks {
        let Some(found) = g.locate(id, 0, 0, 100) else { panic!("no {id}") };
        let start = g.structure_starts(found.chunk_x, found.chunk_z).into_iter().find(|s| s.id() == id).unwrap();
        let piece = start.pieces().last().unwrap().bbox;
        let region = decorate(g, piece.min_x >> 4, piece.min_z >> 4);
        let n = count(&region, block);
        eprintln!("{id} at chunk ({}, {}): {n} {block}", found.chunk_x, found.chunk_z);
        assert!(n > 0, "{id}: no {block} placed");
    }
}

#[test]
fn template_features_are_placed() {
    // Fossils (`fossil_upper` in deserts, `fossil_lower` deep down) around the desert pyramid
    // the placement test finds.
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let mut bones = 0;
    'scan: for cz in -192..-184 {
        for cx in -4..4 {
            bones += count(&decorate(g, cx, cz), "minecraft:bone_block");
            if bones > 0 {
                break 'scan;
            }
        }
    }
    eprintln!("bone blocks: {bones}");
    assert!(bones > 0);
}

/// A world of generated chunks that decorates chunks the way a server does (each chunk
/// once, its neighbours generated on demand) and collects the entities structures spawn.
struct TestWorld {
    gen: &'static paper_worldgen::WorldGenerator,
    chunks: std::collections::HashMap<(i32, i32), paper_worldgen::ProtoChunk>,
    entities: Vec<paper_nbt::Compound>,
    block_entities: Vec<((i32, i32, i32), paper_worldgen::BlockEntity)>,
    block_entity_nbt: Vec<((i32, i32, i32), paper_nbt::Compound)>,
}

impl paper_worldgen::RegionAccess for TestWorld {
    fn chunk(&self, x: i32, z: i32) -> Option<&paper_worldgen::ProtoChunk> {
        self.chunks.get(&(x, z))
    }

    fn chunk_mut(&mut self, x: i32, z: i32) -> Option<&mut paper_worldgen::ProtoChunk> {
        self.chunks.get_mut(&(x, z))
    }

    fn add_block_entity(&mut self, x: i32, y: i32, z: i32, entity: paper_worldgen::BlockEntity) {
        self.block_entities.push(((x, y, z), entity));
    }

    fn add_block_entity_nbt(&mut self, x: i32, y: i32, z: i32, nbt: paper_nbt::Compound) {
        match paper_worldgen::BlockEntity::from_nbt(&nbt) {
            Some(be) => self.block_entities.push(((x, y, z), be)),
            None => self.block_entity_nbt.push(((x, y, z), nbt)),
        }
    }

    fn add_entity_nbt(&mut self, nbt: paper_nbt::Compound) {
        self.entities.push(nbt);
    }
}

impl TestWorld {
    fn new(gen: &'static paper_worldgen::WorldGenerator) -> Self {
        Self { gen, chunks: Default::default(), entities: Vec::new(), block_entities: Vec::new(), block_entity_nbt: Vec::new() }
    }

    /// Decorates every chunk of the box, row by row.
    fn decorate_box(&mut self, min_cx: i32, min_cz: i32, max_cx: i32, max_cz: i32) {
        for cz in min_cz..=max_cz {
            for cx in min_cx..=max_cx {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        let gen = self.gen;
                        self.chunks.entry((cx + dx, cz + dz)).or_insert_with(|| gen.generate_terrain(cx + dx, cz + dz));
                    }
                }
                self.gen.decorate(cx, cz, self);
            }
        }
    }

    fn block(&self, x: i32, y: i32, z: i32) -> String {
        common::block_name(self.chunks[&(x >> 4, z >> 4)].get(x, y, z))
    }

    fn name(&self, x: i32, y: i32, z: i32) -> &'static str {
        paper_data::blocks::block_of_state(self.chunks[&(x >> 4, z >> 4)].get(x, y, z)).name
    }
}

#[test]
fn ocean_monument_is_placed() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    // The vanilla start of the fixture.
    let start = g.structure_starts(-71, -15).into_iter().find(|s| s.id() == "minecraft:monument").expect("monument start");
    let b = start.bbox;
    let mut w = TestWorld::new(g);
    w.decorate_box(b.min_x >> 4, b.min_z >> 4, b.max_x >> 4, b.max_z >> 4);
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let allowed = [
        "minecraft:prismarine", "minecraft:prismarine_bricks", "minecraft:dark_prismarine", "minecraft:sea_lantern", "minecraft:gold_block",
        "minecraft:wet_sponge", "minecraft:water", "minecraft:air", "minecraft:ice", "minecraft:packed_ice", "minecraft:blue_ice",
        "minecraft:seagrass", "minecraft:tall_seagrass", "minecraft:kelp", "minecraft:kelp_plant", "minecraft:sea_pickle",
    ];
    let mut stray = Vec::new();
    for x in b.min_x..=b.max_x {
        for z in b.min_z..=b.max_z {
            for y in b.min_y..=64 {
                let name = w.name(x, y, z);
                *counts.entry(name).or_default() += 1;
                if !allowed.contains(&name) {
                    stray.push(format!("{} at ({x}, {y}, {z})", w.block(x, y, z)));
                }
            }
        }
    }
    eprintln!("monument blocks: {counts:?}");
    assert!(stray.is_empty(), "{} blocks the monument should have carved away, e.g. {:?}", stray.len(), &stray[..stray.len().min(10)]);
    assert_eq!(counts.get("minecraft:gold_block"), Some(&8), "the core's gold");
    assert!(counts.get("minecraft:prismarine_bricks").copied().unwrap_or(0) > 5000);
    assert!(counts.get("minecraft:sea_lantern").copied().unwrap_or(0) > 50);
    let elders: Vec<_> = w.entities.iter().filter(|e| e.string("id") == Some("minecraft:elder_guardian")).collect();
    assert_eq!(elders.len(), 3, "two wing rooms and the penthouse each spawn an elder guardian");
    for e in elders {
        let pos = e.list("Pos").unwrap();
        let (x, y, z) = (pos[0].as_f64().unwrap(), pos[1].as_f64().unwrap(), pos[2].as_f64().unwrap());
        assert!(b.contains(x.floor() as i32, y as i32, z.floor() as i32), "elder guardian at ({x}, {y}, {z}) outside {b:?}");
        assert_eq!(e.bool("PersistenceRequired"), Some(true));
    }
}

#[test]
fn woodland_mansion_is_placed() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let found = g.locate("minecraft:mansion", 0, 0, 100).expect("a woodland mansion");
    let start = g.structure_starts(found.chunk_x, found.chunk_z).into_iter().find(|s| s.id() == "minecraft:mansion").unwrap();
    let pieces = start.pieces();
    eprintln!("mansion at chunk ({}, {}): {} pieces, box {:?}", found.chunk_x, found.chunk_z, pieces.len(), start.bbox);
    assert!(pieces.len() > 300, "{} pieces", pieces.len());
    assert!(pieces.iter().all(|p| p.kind == "minecraft:wmp" && p.orientation == 2));
    let b = start.bbox;
    let mut w = TestWorld::new(g);
    w.decorate_box(b.min_x >> 4, b.min_z >> 4, b.max_x >> 4, b.max_z >> 4);
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for x in b.min_x..=b.max_x {
        for z in b.min_z..=b.max_z {
            for y in b.min_y - 8..=b.max_y {
                *counts.entry(w.name(x, y, z)).or_default() += 1;
            }
        }
    }
    eprintln!("dark oak planks {:?}, cobblestone {:?}, chests {:?}", counts.get("minecraft:dark_oak_planks"), counts.get("minecraft:cobblestone"), counts.get("minecraft:chest"));
    assert!(counts.get("minecraft:dark_oak_planks").copied().unwrap_or(0) > 5000);
    assert!(counts.get("minecraft:cobblestone").copied().unwrap_or(0) > 1000, "walls and the foundation");
    assert!(!counts.contains_key("minecraft:structure_block"), "data markers are replaced");
    let chests: Vec<_> = w.block_entities.iter().filter(|(_, be)| matches!(be, paper_worldgen::BlockEntity::Loot { table, .. } if table == "minecraft:chests/woodland_mansion")).collect();
    assert!(!chests.is_empty(), "loot chests");
    for ((x, y, z), _) in &chests {
        assert_eq!(w.name(*x, *y, *z), "minecraft:chest");
    }
    let mobs: BTreeMap<&str, usize> = w.entities.iter().filter_map(|e| e.string("id")).fold(BTreeMap::new(), |mut m, id| {
        *m.entry(id).or_default() += 1;
        m
    });
    eprintln!("loot chests {}, entities {mobs:?}", chests.len());
    assert!(mobs.get("minecraft:vindicator").copied().unwrap_or(0) > 0);
    let illagers = ["minecraft:vindicator", "minecraft:evoker", "minecraft:allay"];
    for e in w.entities.iter().filter(|e| illagers.contains(&e.string("id").unwrap_or(""))) {
        let pos = e.list("Pos").unwrap();
        let (x, y, z) = (pos[0].as_f64().unwrap(), pos[1].as_f64().unwrap(), pos[2].as_f64().unwrap());
        assert!(b.contains(x.floor() as i32, y as i32, z.floor() as i32));
        assert_eq!(w.name(x.floor() as i32, y as i32, z.floor() as i32), "minecraft:air", "the marker is cleared");
    }
}

#[test]
fn ocean_monuments_spawn_guardians() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    // Inside the vanilla monument of the fixture (box -1165..-1108, 39..61, -269..-212).
    let monsters = g.structure_spawns_at(-1140, 50, -240, "monster").expect("the monument overrides monsters");
    assert_eq!(monsters.len(), 1);
    let guardian = &monsters[0];
    assert_eq!((guardian.entity.as_str(), guardian.weight, guardian.min_count, guardian.max_count), ("minecraft:guardian", 1, 2, 4));
    assert_eq!(g.structure_spawns_at(-1140, 50, -240, "axolotls"), Some(Vec::new()));
    assert_eq!(g.structure_spawns_at(-1140, 50, -240, "creature"), None);
    // Above and beside the building.
    assert_eq!(g.structure_spawns_at(-1140, 62, -240, "monster"), None);
    assert_eq!(g.structure_spawns_at(-1100, 50, -240, "monster"), None);
}

#[test]
fn generator_and_starts_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<paper_worldgen::WorldGenerator>();
    check::<paper_worldgen::structure::StructureStart>();
}

