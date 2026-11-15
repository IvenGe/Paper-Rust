//! Mob variants: the cow, pig or chicken of a biome (an entry of a variant registry, picked
//! by its spawn conditions), rabbits and foxes of their biomes, variants passed on to the
//! young, shown to players and saved.

mod common;

use common::*;
use paper_protocol::packets::play;
use paper_server::server::mobs::SpawnReason;
use paper_server::server::variants;
use paper_server::world::chunk::{BiomeStorage, ChunkPos};
use paper_server::world::entity::Vec3;
use std::sync::Arc;

/// Makes every block of a loaded chunk the given biome.
fn set_biome(s: &mut paper_server::server::Server, pos: ChunkPos, biome: &str) {
    assert!(s.load_chunk_now(0, pos));
    let id = paper_data::synced::id("minecraft:worldgen/biome", biome).unwrap() as u8;
    for section in &mut s.worlds[0].chunks.get_mut(&pos).unwrap().sections {
        Arc::make_mut(section).biomes = BiomeStorage::Uniform(id);
    }
}

fn variant_of(s: &paper_server::server::Server, id: i32) -> i32 {
    s.worlds[0].entities[&id].mob().unwrap().variant
}

#[test]
fn farm_animals_take_the_variant_of_their_biome() {
    let server = start_server(|_| {});
    let found = server
        .handle
        .run(|s| {
            set_biome(s, ChunkPos::new(9, 9), "minecraft:snowy_plains");
            set_biome(s, ChunkPos::new(-9, 9), "minecraft:desert");
            let mut found = Vec::new();
            for animal in ["minecraft:cow", "minecraft:pig", "minecraft:chicken"] {
                let registry = variants::registry_of(animal).unwrap();
                for at in [Vec3::new(152.5, -60.0, 152.5), Vec3::new(-136.5, -60.0, 152.5), Vec3::new(8.5, -60.0, 8.5)] {
                    let id = s.spawn_mob(0, animal, at, SpawnReason::Natural).unwrap();
                    found.push((animal, variants::key_of(registry, variant_of(s, id)).unwrap()));
                }
            }
            found
        })
        .unwrap();
    // Snowy plains, desert, and the plains of the test world.
    for chunk in found.chunks(3) {
        assert_eq!([chunk[0].1, chunk[1].1, chunk[2].1], ["minecraft:cold", "minecraft:warm", "minecraft:temperate"], "{}", chunk[0].0);
    }
    server.stop();
}

#[test]
fn rabbits_and_foxes_of_snowy_places_are_white() {
    let server = start_server(|_| {});
    let (rabbits, foxes, desert_rabbit) = server
        .handle
        .run(|s| {
            set_biome(s, ChunkPos::new(9, 9), "minecraft:snowy_plains");
            set_biome(s, ChunkPos::new(-9, 9), "minecraft:desert");
            let snowy = Vec3::new(152.5, -60.0, 152.5);
            let rabbits: Vec<i32> = (0..20).map(|_| s.spawn_mob(0, "minecraft:rabbit", snowy, SpawnReason::Natural).unwrap()).collect();
            let foxes: Vec<i32> = (0..5).map(|_| s.spawn_mob(0, "minecraft:fox", snowy, SpawnReason::Natural).unwrap()).collect();
            let (rabbits, foxes): (Vec<i32>, Vec<i32>) = (rabbits.iter().map(|id| variant_of(s, *id)).collect(), foxes.iter().map(|id| variant_of(s, *id)).collect());
            let desert = s.spawn_mob(0, "minecraft:rabbit", Vec3::new(-136.5, -60.0, 152.5), SpawnReason::Natural).unwrap();
            (rabbits, foxes, variant_of(s, desert))
        })
        .unwrap();
    // White, or white with black splotches; snow foxes; a gold rabbit in the desert.
    assert!(rabbits.iter().all(|v| *v == 1 || *v == 3), "{rabbits:?}");
    assert!(foxes.iter().all(|v| *v == 1), "{foxes:?}");
    assert_eq!(desert_rabbit, 4);
    server.stop();
}

#[test]
fn the_young_take_a_parents_variant() {
    let server = start_server(|_| {});
    let (calves, foals) = server
        .handle
        .run(|s| {
            let r = variants::registry_of("minecraft:cow").unwrap();
            let (cold, warm) = (variants::id_of(r, "minecraft:cold").unwrap(), variants::id_of(r, "minecraft:warm").unwrap());
            let at = Vec3::new(8.5, -60.0, 8.5);
            let calves: Vec<i32> = (0..40).map(|_| s.offspring_variant(0, "minecraft:cow", at, cold, warm).unwrap()).collect();
            // A black horse with white dots and a white one without markings.
            let foals: Vec<i32> = (0..200).map(|_| s.offspring_variant(0, "minecraft:horse", at, 4 | 3 << 8, 0).unwrap()).collect();
            ((calves, cold, warm), foals)
        })
        .unwrap();
    let (calves, cold, warm) = calves;
    assert!(calves.iter().all(|v| *v == cold || *v == warm) && calves.contains(&cold) && calves.contains(&warm), "{calves:?}");
    // Mostly a parent's coat and markings, now and then others.
    let from_parents = foals.iter().filter(|v| (**v & 0xFF == 4 || **v & 0xFF == 0) && (*v >> 8 == 3 || *v >> 8 == 0)).count();
    assert!(from_parents > 100 && from_parents < 200, "{from_parents} of 200");
    assert!(foals.iter().all(|v| (0..7).contains(&(v & 0xFF)) && (0..5).contains(&(v >> 8))), "{foals:?}");
    server.stop();
}

#[test]
fn variants_are_shown_and_saved() {
    let server = start_server(|_| {});
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    let (cow, cold) = server
        .handle
        .run(|s| {
            set_biome(s, ChunkPos::new(1, 0), "minecraft:snowy_plains");
            let cow = s.spawn_mob(0, "minecraft:cow", Vec3::new(20.5, -60.0, 8.5), SpawnReason::Natural).unwrap();
            (cow, variants::id_of("minecraft:cow_variant", "minecraft:cold").unwrap())
        })
        .unwrap();
    // DATA_VARIANT_ID (18) as a `cow_variant`.
    let serializer = paper_data::registries::id("paper-rs:entity_data_serializer", "cow_variant").unwrap() as u8;
    alice.find(TIMEOUT, |m: &play::SetEntityData| m.entity_id.0 == cow && m.metadata.0.windows(3).any(|w| w == [18, serializer, cold as u8])).expect("the cold cow's variant");
    let (saved, loaded) = server
        .handle
        .run(move |s| {
            let nbt = paper_server::world::entity_nbt::entity_to_nbt(&s.worlds[0].entities[&cow]).unwrap();
            let loaded = paper_server::world::entity_nbt::entity_from_nbt(cow, &nbt).unwrap().mob().unwrap().variant;
            (nbt.string("variant").map(str::to_owned), loaded)
        })
        .unwrap();
    assert_eq!(saved.as_deref(), Some("minecraft:cold"));
    assert_eq!(loaded, cold);
    server.stop();
}
