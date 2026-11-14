//! Structure starts saved in vanilla's chunk NBT format and read back by a new generator
//! (a server restart).

mod common;

use common::{generator, jar};
use paper_nbt::Compound;
use paper_worldgen::{Dimension, ProtoChunk, Region, WorldGenerator};

/// A generator as a restarted server builds it: nothing cached.
fn fresh(dimension: Dimension) -> Option<WorldGenerator> {
    Some(WorldGenerator::new(jar()?, dimension, 0).expect("building the generator"))
}

/// Through the binary format and back.
fn binary(c: &Compound) -> Compound {
    let mut buf = Vec::new();
    paper_nbt::write_named(&mut buf, "", c);
    paper_nbt::read_named(&mut &buf[..], usize::MAX).expect("reading NBT").1
}

/// The pieces of a chunk's starts, comparable.
fn describe(g: &WorldGenerator, x: i32, z: i32) -> Vec<String> {
    g.structure_starts(x, z).iter().map(|s| format!("{} {:?} {:?}", s.id(), s.bbox, s.pieces())).collect()
}

/// Saves the structures of each chunk with `from`, loads them into `to` and checks the
/// pieces and the saved form are the same.
fn round_trip(from: &WorldGenerator, to: &WorldGenerator, chunks: &[(i32, i32)]) -> usize {
    let mut n = 0;
    for &(x, z) in chunks {
        let saved = binary(&from.save_structures(x, z));
        let starts = saved.compound("starts").expect("starts");
        assert!(saved.compound("References").is_some());
        let report = to.load_structures(x, z, &saved);
        assert!(report.recomputed.is_empty() && report.unknown.is_empty(), "chunk ({x}, {z}): {report:?}");
        assert_eq!(report.starts.len(), starts.len());
        assert_eq!(describe(to, x, z), describe(from, x, z), "chunk ({x}, {z})");
        assert_eq!(binary(&to.save_structures(x, z)), saved, "chunk ({x}, {z}) saved again");
        n += report.starts.len();
    }
    n
}

fn fixture_chunks(key: &str) -> Vec<(i32, i32)> {
    let data = common::json_fixture("structure_starts");
    data[key].as_array().unwrap().iter().map(|s| (s["x"].as_i64().unwrap() as i32, s["z"].as_i64().unwrap() as i32)).collect()
}

#[test]
fn fixture_starts_survive_a_restart() {
    for (dimension, key) in [(Dimension::Overworld, "overworld"), (Dimension::Nether, "the_nether"), (Dimension::End, "the_end")] {
        let (Some(g), Some(restarted)) = (generator(dimension, 0), fresh(dimension)) else { return };
        let chunks = fixture_chunks(key);
        let n = round_trip(g, &restarted, &chunks);
        eprintln!("{dimension:?}: {n} starts read back");
        assert_eq!(n, chunks.len());
    }
}

#[test]
fn other_structures_survive_a_restart() {
    let (Some(g), Some(restarted)) = (generator(Dimension::Overworld, 0), fresh(Dimension::Overworld)) else { return };
    let mut chunks = Vec::new();
    for id in [
        "minecraft:swamp_hut", "minecraft:jungle_pyramid", "minecraft:desert_pyramid", "minecraft:igloo", "minecraft:mansion", "minecraft:ancient_city",
        "minecraft:pillager_outpost", "minecraft:village_desert", "minecraft:village_snowy",
    ] {
        let found = g.locate(id, 0, 0, 100).unwrap_or_else(|| panic!("no {id}"));
        chunks.push((found.chunk_x, found.chunk_z));
    }
    assert_eq!(round_trip(g, &restarted, &chunks), chunks.len());
}

fn region(g: &WorldGenerator, cx: i32, cz: i32) -> Region {
    let mut chunks = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks.push(g.generate_terrain(cx + dx, cz + dz));
        }
    }
    let mut region = Region::new(cx, cz, chunks);
    g.decorate(cx, cz, &mut region);
    region
}

fn same_blocks(a: &ProtoChunk, b: &ProtoChunk) -> bool {
    (a.min_y..a.min_y + a.height).all(|y| (0..16).all(|z| (0..16).all(|x| a.get(x, y, z) == b.get(x, y, z))))
}

/// A structure placed in part before a restart continues after it as it would have without
/// one: the height a temple settled in its first chunk carries over.
#[test]
fn partly_placed_structures_continue_after_a_restart() {
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    // A desert pyramid settles at the lowest ground of the chunk placed first, minus a
    // random 0 to 2: the chunks of one pyramid must agree.
    let found = g.locate("minecraft:desert_pyramid", 0, 0, 100).expect("a desert pyramid");
    let (sx, sz) = (found.chunk_x, found.chunk_z);
    let start = g.structure_starts(sx, sz).into_iter().find(|s| s.id() == "minecraft:desert_pyramid").unwrap();
    let piece = start.pieces()[0].bbox;
    let pyramid = |g: &WorldGenerator| g.structure_starts(sx, sz).into_iter().find(|s| s.id() == "minecraft:desert_pyramid").unwrap().pieces()[0].bbox;
    let chunks: Vec<(i32, i32)> = (piece.min_z >> 4..=piece.max_z >> 4).flat_map(|z| (piece.min_x >> 4..=piece.max_x >> 4).map(move |x| (x, z))).collect();
    assert!(chunks.len() > 1, "the pyramid spans chunks");
    let first = chunks[chunks.len() - 1];

    // Before the restart: a server places the pyramid's first chunk and saves.
    let Some(before) = fresh(Dimension::Overworld) else { return };
    region(&before, first.0, first.1);
    let settled = pyramid(&before);
    let saved = binary(&before.save_structures(sx, sz));
    eprintln!("pyramid at chunk ({sx}, {sz}): settled at y {} in chunk {first:?}", settled.min_y);

    let mut seams = 0;
    for &second in &chunks[..chunks.len() - 1] {
        // Without a restart it goes on with another chunk.
        let continued = region(&before, second.0, second.1);
        // After the restart the saved start is loaded first.
        let Some(after) = fresh(Dimension::Overworld) else { return };
        let needed = after.structure_start_chunks(second.0, second.1);
        assert!(needed.contains(&(sx, sz)) && needed.iter().all(|&(x, z)| x.abs_diff(second.0) <= 8 && z.abs_diff(second.1) <= 8));
        after.load_structures(sx, sz, &saved);
        let resumed = region(&after, second.0, second.1);
        assert_eq!(pyramid(&after), settled, "the settled height is kept");
        assert!(same_blocks(continued.center(), resumed.center()), "chunk {second:?} continues the first");
        assert_eq!(continued.block_entities, resumed.block_entities);
        // Without the saved start the chunk settles the pyramid on its own (the random part
        // of the height is drawn per chunk): a seam where it settles differently.
        let Some(forgetful) = fresh(Dimension::Overworld) else { return };
        let seamed = region(&forgetful, second.0, second.1);
        let own = pyramid(&forgetful);
        eprintln!("chunk {second:?} alone settles it at y {}", own.min_y);
        if own != settled {
            seams += 1;
            assert!(!same_blocks(continued.center(), seamed.center()));
        }
    }
    assert!(seams > 0, "some chunk of this pyramid settles it differently on its own");
}

#[test]
fn chunks_without_starts_stay_without() {
    // A chunk saved where the placement would start a structure but none was generated
    // (here: an empty `starts`) keeps none after loading.
    let Some(g) = generator(Dimension::Overworld, 0) else { return };
    let (x, z) = fixture_chunks("overworld")[0];
    assert!(!g.structure_starts(x, z).is_empty());
    let Some(restarted) = fresh(Dimension::Overworld) else { return };
    let mut empty = Compound::new();
    empty.insert("starts", Compound::new());
    empty.insert("References", Compound::new());
    let report = restarted.load_structures(x, z, &empty);
    assert!(report.starts.is_empty());
    assert!(restarted.structure_starts(x, z).is_empty());
    // A compound without `starts` (saved before structures were generated) changes nothing.
    let Some(other) = fresh(Dimension::Overworld) else { return };
    other.load_structures(x, z, &Compound::new());
    assert_eq!(describe(&other, x, z), describe(g, x, z));
}
