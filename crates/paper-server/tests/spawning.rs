//! Where mobs spawn: the 26.3 biomes' spawn lists, animals placed when chunks are generated
//! (on the ground under trees, on each animal's own ground), and the spawn rules of mobs
//! that spawn their own way.

mod common;

use common::*;
use paper_data::blocks;
use paper_server::server::mobs::{is_slime_chunk, SpawnReason};
use paper_server::world::chunk::{BiomeStorage, ChunkPos};
use std::sync::Arc;

/// The mobs standing in a chunk: (type, y).
fn mobs_in(s: &paper_server::server::Server, pos: ChunkPos) -> Vec<(String, f64)> {
    s.worlds[0].entities.values().filter(|e| e.chunk == pos && e.mob().is_some()).map(|e| (e.info.name.clone(), e.pos.y)).collect()
}

/// Loads a chunk and removes the animals it got when it was generated.
fn fresh_chunk(s: &mut paper_server::server::Server, pos: ChunkPos) {
    assert!(s.load_chunk_now(0, pos));
    for e in s.worlds[0].entities.values_mut().filter(|e| e.chunk == pos) {
        e.removed = true;
    }
    s.worlds[0].entities.retain(|_, e| !e.removed);
}

#[test]
fn the_biomes_spawn_lists_come_from_their_attributes() {
    if paper_server::vanilla::test_jar().is_none() {
        eprintln!("skipping: no vanilla jar");
        return;
    }
    let server = start_server(|_| {});
    server
        .handle
        .run(|s| {
            // Plains: farm animals, and herds of two to six horses.
            let plains = s.biome_spawners("minecraft:plains", "creature");
            let horse = plains.iter().find(|e| e.entity == "minecraft:horse").expect("horses in plains");
            assert_eq!((horse.weight, horse.min, horse.max), (5, 2, 6));
            assert!(plains.iter().any(|e| e.entity == "minecraft:cow" && e.min == 4 && e.max == 4));
            // Rabbits and camels in deserts, fewer groups of animals in snowy plains.
            let desert: Vec<String> = s.biome_spawners("minecraft:desert", "creature").into_iter().map(|e| e.entity).collect();
            assert_eq!(desert, ["minecraft:rabbit", "minecraft:camel"]);
            assert_eq!(s.biome_spawns("minecraft:snowy_plains").creature_probability, 0.07);
            assert_eq!(s.biome_spawns("minecraft:plains").creature_probability, 0.1);
            // The nether's own monsters, no monsters in mushroom fields, fish in the ocean.
            let nether: Vec<String> = s.biome_spawners("minecraft:nether_wastes", "monster").into_iter().map(|e| e.entity).collect();
            assert!(nether.contains(&"minecraft:ghast".to_owned()) && nether.contains(&"minecraft:zombified_piglin".to_owned()), "{nether:?}");
            assert!(!nether.contains(&"minecraft:zombie".to_owned()));
            assert!(s.biome_spawners("minecraft:mushroom_fields", "monster").is_empty());
            assert!(s.biome_spawners("minecraft:ocean", "water_ambient").iter().any(|e| e.entity == "minecraft:cod"));
        })
        .unwrap();
    server.stop();
}

#[test]
fn generated_chunks_get_animals_on_the_ground_under_trees() {
    let server = start_server(|_| {});
    let found = server
        .handle
        .run(|s| {
            // A roof of leaves over a whole chunk of the flat world's grass.
            let pos = ChunkPos::new(6, 6);
            fresh_chunk(s, pos);
            let leaves = blocks::with_property(blocks::default_state("minecraft:oak_leaves").unwrap(), "persistent", "true").unwrap();
            for x in 96..112 {
                for z in 96..112 {
                    s.worlds[0].set_block(x, -56, z, leaves);
                }
            }
            for _ in 0..100 {
                s.spawn_for_chunk_generation(0, pos);
            }
            mobs_in(s, pos)
        })
        .unwrap();
    assert!(!found.is_empty(), "no animals under the trees");
    // On the grass at Y -61, not on the leaves.
    assert!(found.iter().all(|(_, y)| *y == -60.0), "{found:?}");
    server.stop();
}

#[test]
fn desert_chunks_get_rabbits_on_sand() {
    if paper_server::vanilla::test_jar().is_none() {
        eprintln!("skipping: no vanilla jar");
        return;
    }
    let server = start_server(|_| {});
    let found = server
        .handle
        .run(|s| {
            let pos = ChunkPos::new(9, 9);
            fresh_chunk(s, pos);
            let desert = paper_data::synced::id("minecraft:worldgen/biome", "minecraft:desert").unwrap() as u8;
            let sand = blocks::default_state("minecraft:sand").unwrap();
            let chunk = s.worlds[0].chunks.get_mut(&pos).unwrap();
            for section in &mut chunk.sections {
                Arc::make_mut(section).biomes = BiomeStorage::Uniform(desert);
            }
            for x in 144..160 {
                for z in 144..160 {
                    s.worlds[0].set_block(x, -61, z, sand);
                }
            }
            for _ in 0..100 {
                s.spawn_for_chunk_generation(0, pos);
            }
            mobs_in(s, pos)
        })
        .unwrap();
    assert!(found.iter().any(|(name, _)| name == "minecraft:rabbit"), "{found:?}");
    assert!(found.iter().all(|(name, y)| matches!(name.as_str(), "minecraft:rabbit" | "minecraft:camel") && *y == -60.0), "{found:?}");
    server.stop();
}

#[test]
fn mobs_that_spawn_their_own_way_keep_their_rules() {
    let server = start_server(|_| {});
    server
        .handle
        .run(|s| {
            fresh_chunk(s, ChunkPos::new(0, 0));
            // Bats only below sea level (-63 in the flat world), never on the grass in daylight.
            assert!((0..200).all(|_| !s.can_spawn_at(0, "minecraft:bat", 3, -60, 3, SpawnReason::Natural)));
            // Fish need water.
            assert!(!s.can_spawn_at(0, "minecraft:cod", 3, -60, 3, SpawnReason::Natural));
            // Animals stand on grass in daylight, but not on glass.
            assert!(s.can_spawn_at(0, "minecraft:cow", 3, -60, 3, SpawnReason::Natural));
            s.worlds[0].set_block(3, -61, 3, blocks::default_state("minecraft:glass").unwrap());
            assert!(!s.can_spawn_at(0, "minecraft:cow", 3, -60, 3, SpawnReason::Natural));
            // Slimes spawn underground only in slime chunks; one chunk in ten is one.
            let seed = s.worlds[0].seed;
            let slime_chunks = (0..40).flat_map(|x| (0..40).map(move |z| (x, z))).filter(|&(x, z)| is_slime_chunk(seed, x, z)).count();
            assert!((100..220).contains(&slime_chunks), "{slime_chunks} slime chunks in 1600");
        })
        .unwrap();
    server.stop();
}

/// The slime chunks of seed 12345 around the origin, as `java.util.Random` gives them for
/// `WorldgenRandom.seedSlimeChunk`.
#[test]
fn slime_chunks_follow_the_seed() {
    let ours: Vec<(i32, i32)> = (-8..8).flat_map(|x| (-8..8).map(move |z| (x, z))).filter(|&(x, z)| is_slime_chunk(12345, x, z)).collect();
    let java = [
        (-8, -7), (-8, -2), (-7, 1), (-7, 3), (-5, 2), (-4, 0), (-3, 7), (-2, -4), (-2, 1), (-2, 4), (-1, 2), (0, -6),
        (0, -2), (0, 7), (3, 0), (3, 5), (4, -6), (4, -5), (4, -3), (4, 1), (5, -3), (6, -2), (6, 7),
    ];
    assert_eq!(ours, java);
}
