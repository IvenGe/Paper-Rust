# paper-worldgen

Vanilla Minecraft 26.3 world generation for paper-rs: the overworld, the nether and the
end, block for block as vanilla generates them, minus structures.

## Usage

```rust
use paper_worldgen::{DataSource, Dimension, Region, WorldGenerator};

/// Any source of the vanilla jar's data files, e.g. the jar opened with the `zip` crate.
struct Jar(std::sync::Mutex<zip::ZipArchive<std::fs::File>>);

impl DataSource for Jar {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut archive = self.0.lock().ok()?;
        let mut file = archive.by_name(path).ok()?;
        let mut out = Vec::new();
        file.read_to_end(&mut out).ok()?;
        Some(out)
    }
}

fn example(jar: &Jar, seed: i64) -> anyhow::Result<()> {
    // Built once per dimension and seed (about 0.1 s); `Send + Sync`, share it between
    // worker threads.
    let gen = WorldGenerator::new(jar, Dimension::Overworld, seed)?;
    let (min_y, height, sea_level) = (gen.min_y(), gen.height(), gen.sea_level());

    // Terrain: biomes, noise (shape, aquifers, ore veins), surface and bedrock, carvers.
    // Depends on nothing but the chunk position.
    let chunk = gen.generate_terrain(3, -7);
    let state: u32 = chunk.get(5, 64, 9); // block state id (the protocol's global palette)

    // Features: decorating a chunk needs it and its eight neighbours with terrain, and
    // writes into all nine (trees cross chunk borders).
    let (cx, cz) = (3, -7);
    let mut chunks = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks.push(gen.generate_terrain(cx + dx, cz + dz));
        }
    }
    let mut region = Region::new(cx, cz, chunks);
    gen.decorate(cx, cz, &mut region);
    for ((x, y, z), block_entity) in &region.block_entities { /* spawners, chests, ... */ }
    for entity in &region.entities { /* end crystals */ }
    let chunks = region.into_chunks(); // the 3x3 chunks, `(dz + 1) * 3 + dx + 1`

    // Where vanilla would put the world spawn for this seed.
    let (x, y, z) = gen.find_spawn();
    Ok(())
}
```

A server keeps its own chunk store and implements `RegionAccess` over it (a `HashMap<(i32,
i32), ProtoChunk>` implementation is included); `decorate` reads and writes only the 3x3
chunks around the decorated one. Like vanilla, a chunk is final once it and all eight
neighbours are decorated, and where neighbouring features overlap the result depends on the
order chunks are decorated in.

`examples/decorate.rs` generates and decorates one chunk and prints its hash, timing, block
entities and entities; `examples/bench.rs` measures throughput.

## Data

The generator is built at runtime from the worldgen data of the vanilla server jar
(`data/minecraft/worldgen/**`, `data/minecraft/dimension_type/**`, block tags), read
through the `DataSource` trait. The server implements it over the jar it downloads at
startup; the tests implement it over a local jar with the `zip` crate (see
`tests/common/mod.rs`, jar path from `PAPER_RS_VANILLA_JAR`; the tests pass without
checking anything when the jar is missing). No Mojang JSON is committed here.

Two tables are compiled in (see `data/26.3/README.md`): the multi-noise biome parameter
trees, which vanilla builds in code, and per block state physics flags (sturdy faces,
solidity, collision) taken from Pumpkin's extracted `blocks.json`.

## What is generated

| Stage | Status |
| --- | --- |
| Biomes (multi-noise, nether, end) | matches the vanilla fixtures |
| Noise: terrain shape, aquifers, ore veins | matches the vanilla fixtures |
| Surface rules, bedrock | matches the vanilla fixtures |
| Carvers (caves, canyons, nether caves) | matches the vanilla fixtures |
| Features | see below |
| Structures (villages, mineshafts, ...) | not generated |
| Initial world spawn | `find_spawn`, see below |

Features follow vanilla's decoration driver (feature order across biomes, per feature
seeds, placement modifiers, the region's read/write rules and heightmaps). Implemented:
every placement modifier, block predicate and state provider, and the feature types ores,
scattered ores, trees (all trunk and foliage placers, roots and decorators), fallen trees,
vegetation and waterlogged vegetation patches, simple blocks, block columns, all selectors,
sequences and overlays, disks, springs, lakes, dungeons (with spawner and loot data),
underwater magma, freezing and snow, geodes, root systems, multiface growth, vines, bamboo,
kelp and seagrass (data driven in 26.3), corals (coral trees and claws), forest rocks,
blue ice, ice spikes, icebergs, huge mushrooms, pointed dripstone and sulfur spikes with
their clusters, large dripstone; in the nether huge fungi, deltas, blob replacement, basalt
columns and pillars, glowstone, vines; in the end spikes with their crystals, return
gateways, small end islands, the arrival platform and chorus plants.

Not implemented (they place nothing; `WorldGenerator::unsupported_features` lists them):

* `template` features, which place structure template NBT files: desert wells and the
  templates of sulfur springs.
* `fossil`, also template based.
* `sculk_patch` (deep dark sculk patches).
* Where one of these is part of a `sequence`, the features after it are skipped as
  well, as vanilla does when a step of a sequence places nothing.
* Mobs spawned during generation, lighting.

### Verification

* All fixture chunks from Pumpkin's vanilla 26.3 dumps match after the features stage:
  5 overworld, 4 nether and 4 end chunks. Fixture chunks that contain structure pieces in
  vanilla are compared with Pumpkin with structures turned off instead (4 overworld, 1
  nether); everything else in them matches vanilla.
* Feature types the fixtures do not exercise are checked against Pumpkin (structures off)
  on 25 more chunks with end spikes, gateways, chorus, end islands, ice spikes, icebergs,
  forest rocks, huge mushrooms and corals (`tests/features.rs`). Where Pumpkin departs from
  vanilla 26.3 (a few random draws and tags in ice spikes, icebergs, forest rocks and huge
  mushrooms), this crate follows vanilla, and the references were made with those spots of
  Pumpkin corrected.
* Pointed dripstone, dripstone clusters and large dripstone have no reference: Pumpkin's
  versions are placeholders. They follow vanilla 26.3's algorithms.
* A few features compute with `sin`, `cos`, `pow` and `log` (icebergs, large dripstone,
  the spawn search); Rust's and Java's results can differ in the last bit, which changes a
  block only when a value lands exactly on a rounding boundary.

### Spawn

`find_spawn` follows `MinecraftServer.setInitialSpawn`: the climate search for the noise
settings' spawn targets picks a chunk, then the 11x11 chunks around it are tried in a spiral
for the first column with a full top face under open air, as `PlayerSpawnFinder` does. The
chunks it examines are generated and decorated (about 0.2 s). Two approximations: structures
are missing, and "full top face of the collision shape" is taken as "full cube or sturdy
top face". The nether uses the same search with its fixed spawn height; the end returns its
arrival point (100, 50, 0).

## Performance

On the 4 vCPU development container (release build, seed 12345, 144 chunks):

| | build | terrain, 1 thread | terrain, 4 threads | decoration, 1 thread |
| --- | --- | --- | --- | --- |
| Overworld | 91 ms | 8.8 ms/chunk | 2.4 ms/chunk | 2.1 ms/chunk |
| Nether | 55 ms | 2.6 ms/chunk | 1.1 ms/chunk | 0.5 ms/chunk |
| End | 9 ms | 0.9 ms/chunk | 0.4 ms/chunk | 0.01 ms/chunk |

`cargo run --release -p paper-worldgen --example bench` reproduces these.

## Provenance

The algorithms are ported from [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin)
(GPL-3.0, commit e9dc12e9), a Rust server that reproduces vanilla 26.3 world generation,
and adapted to build everything from the jar's data at runtime instead of Pumpkin's
build-time code generation. Each file derived from Pumpkin names its source files in its
header. Where Pumpkin's feature code differs from vanilla 26.3 or is missing, the code was
written to vanilla's behaviour; such files say so in their header.

The reference outputs in `tests/fixtures/` are vanilla 26.3 dumps from Pumpkin's
`assets/tests/` (converted to gzip-compressed little-endian `u16` arrays or gzipped JSON).
