# paper-worldgen

Vanilla Minecraft 26.3 world generation for paper-rs: the overworld, the nether and the
end, block for block as vanilla generates them, structures included.

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
    // Built once per dimension and seed (about 0.6 s for the overworld, most of it parsing
    // the structure templates); `Send + Sync`, share it between worker threads.
    let gen = WorldGenerator::new(jar, Dimension::Overworld, seed)?;
    let (min_y, height, sea_level) = (gen.min_y(), gen.height(), gen.sea_level());

    // Terrain: biomes, noise (shape, aquifers, ore veins, the terrain adaptation around
    // structures), surface and bedrock, carvers. Depends on nothing but the chunk position.
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
    for ((x, y, z), nbt) in &region.block_entity_nbt { /* other block entities, as NBT */ }
    for entity in &region.entities { /* end crystals */ }
    for nbt in &region.entity_nbt { /* structure mobs, item frames, ... as entity NBT */ }
    let chunks = region.into_chunks(); // the 3x3 chunks, `(dz + 1) * 3 + dx + 1`

    // Where vanilla would put the world spawn for this seed.
    let (x, y, z) = gen.find_spawn();

    // Structures: the starts in a chunk, `/locate`, the stronghold rings.
    for start in gen.structure_starts(cx, cz) { let (id, pieces) = (start.id(), start.pieces()); }
    if let Some(found) = gen.locate("minecraft:village_plains", 0, 0, 100) {
        let (x, z) = (found.x, found.z);
    }
    let rings: Vec<(i32, i32)> = gen.ring_positions("minecraft:stronghold");
    // The starts that reach into a chunk (vanilla's `structures.References`), and the mobs
    // structures spawn instead of the biome's (guardians in monuments, witches in huts).
    let refs: Vec<(String, Vec<(i32, i32)>)> = gen.structure_references(cx, cz);
    let spawns = gen.structure_spawns_at(-1140, 50, -240, "monster");
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
| Structures | see below |
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

Template features (desert wells, sulfur springs) and fossils place their structure
templates as vanilla does.

Sculk patches (`sculk_patch`, in the deep dark and in ancient cities) spread charge with
vanilla's world generation sculk spreader: sculk, sculk veins and the sensors and
shriekers that grow on sculk (in 26.3 the catalyst and the ancient cities' shriekers are
steps of the data's `sequence`). Sensors, shriekers and catalysts get their block entities
from the server, as vanilla's region leaves them for the chunk to create.

Every feature type of 26.3's data is implemented (`WorldGenerator::unsupported_features`
lists types a data pack uses that are not; they place nothing, and the steps of a
`sequence` after one are skipped, as vanilla does when a step places nothing). Not done:
mobs spawned during generation, lighting.

### Structures

Structure sets and placements (random spread with its spread types, exclusion zones and
frequency reducers, concentric rings for strongholds), structure starts with their biome
checks and noise-only heights, and placement during decoration at each structure's step
before that step's features, every piece clipped to the chunk, as
`ChunkGenerator.applyBiomeDecoration` does. Starts are computed once per (set, chunk) and
shared between threads. Generated:

* strongholds, nether fortresses, mineshafts (both kinds);
* jigsaw structures (villages, pillager outposts, bastions, ancient cities, trail ruins,
  trial chambers, abandoned camps): template pools, pool aliases, the jigsaw assembly with
  priorities and the expansion hack, all 26.3 processors, and terrain adaptation (beard
  thin, beard box, bury, encapsulate) in the noise stage;
* shipwrecks, ocean ruins, ruined portals, buried treasure, desert pyramids, jungle temples,
  swamp huts, igloos, end cities and nether fossils;
* ocean monuments: the surrounding biome check, the room graph and room fitters, the
  building with its rooms and the three elder guardians. As in vanilla the start holds one
  piece (the building) and the rooms are its children, and the terrain is not beardified:
  the building replaces everything around and inside itself with water (air above sea
  level, ice kept) and fills its pillars down to the floor;
* woodland mansions: the floor grids, the piece placer (outer walls, roofs, corridors and
  carpets, inner walls and doors, the rooms of each floor), the `woodland_mansion/*`
  templates with their data markers (loot chests, vindicators, evokers, allays) and the
  cobblestone foundation of `afterPlace`.

`unsupported_structures` lists structure types that are not generated (none in 26.3's
data).

`structure_spawns_at` answers vanilla's `getMobsAt` for structures from their
`spawn_overrides`: the guardians that spawn in monuments (natural spawning, not
generation), witches and cats in swamp huts, pillagers at outposts and the fortress mobs.

Block entities come out typed where `BlockEntity` can express them (containers with a
loot table and seed, spawners) and as vanilla block entity NBT otherwise
(`RegionAccess::add_block_entity_nbt`, e.g. trial spawners, vaults, decorated pots, signs);
template entities and mobs structures spawn (villagers, witches, drowned, shulkers, the end
city's elytra item frame) come out as entity NBT (`RegionAccess::add_entity_nbt`), without
vanilla's `finalizeSpawn`, which the server should run. Not done: the neighbour shape
updates vanilla runs after placing non-jigsaw templates and post-processing of placed
blocks (vanilla's `markPosForPostprocessing`). Pieces that settle their height when first
placed (shipwrecks, swamp huts, jungle temples, desert pyramids) keep it in the shared
start for the generator's lifetime and save it with the chunk (below); like vanilla, the
settled height depends on which chunk is decorated first.

### Saving structure starts

A start's pieces settle state while they are placed: the height of temples and
shipwrecks, the chests and spawners already placed. Recomputing the starts from the seed
after a restart would lose it, and a structure half placed before the restart would
continue at another height. So a server saves the starts with their chunk and hands them
back to the generator when it reads the chunk, in vanilla's chunk NBT format:

* `save_structures(cx, cz)` returns the `structures` compound vanilla saves in a chunk:
  `starts` (the chunk's starts by structure id: `id`, `ChunkX`, `ChunkZ`, `references`
  and the pieces as `Children`, each with `id`, `BB`, `O`, `GD` and its type's fields)
  and `References` (per structure, the packed positions of the start chunks reaching into
  the chunk).
* `load_structures(cx, cz, &structures)` reads such a compound (saved by this crate or by
  vanilla) and puts its starts into the generator's start cache, replacing what it would
  compute there; sets that could start in the chunk but have no saved start get none.
  Every piece type is read back from its fields (ocean monuments are rebuilt from the
  seed, as vanilla does; jigsaw pieces find their pool element among the pools' elements
  by its encoding). A start that cannot be read (a pool element or piece type unknown
  here, e.g. from a data pack) is computed from the seed instead and reported.
* `structure_start_chunks(cx, cz)` lists the chunks within 8 chunks whose starts can reach
  into a chunk (the placement candidates `createReferences` looks at).

```rust
// Saving a chunk.
chunk_nbt.insert("structures", gen.save_structures(cx, cz));

// Reading a chunk from disk (once per chunk read).
if let Some(structures) = chunk_nbt.compound("structures") {
    gen.load_structures(cx, cz, structures);
}

// Before generating or decorating a chunk: the saved starts that reach into it must be
// loaded first, so new chunks continue the structures of old ones.
for (sx, sz) in gen.structure_start_chunks(cx, cz) {
    if !structures_loaded.contains(&(sx, sz)) && storage.contains(sx, sz) {
        if let Some(structures) = storage.read(sx, sz)?.compound("structures") {
            gen.load_structures(sx, sz, structures);
        }
        structures_loaded.insert((sx, sz));
    }
}
```

`References` need not be read back: they are recomputed from the starts. Loading a chunk's
structures again after its start was used would bring back the older state; the server
should load them once per chunk it reads from disk.

### Verification

* All fixture chunks from Pumpkin's vanilla 26.3 dumps match vanilla after every stage,
  structures included: 5 overworld, 4 nether and 4 end chunks, among them a trial chambers
  chunk (encapsulating beardifier, templates, processors), three mineshaft chunks and a
  nether fortress chunk. With structures turned off the same chunks match Pumpkin with
  structures disabled.
* `tests/fixtures/structure_starts.json.gz` (vanilla's starts in 2000 chunks per dimension,
  from Pumpkin's `assets/tests/`) is matched piece for piece (type, generation depth,
  orientation, bounding box; jigsaw pieces also their template, position, rotation, ground
  level delta and junctions): 2 abandoned camps, 2 bastions, 2 buried treasures, 3 end
  cities, 4 fortresses, 38 mineshafts, 1 ocean monument, 96 nether fossils, 10 ocean ruins,
  6 ruined portals, 4 shipwrecks, 1 stronghold, 1 trail ruins, 5 trial chambers and 1 plains
  village.
* The same dump's structure references (every start reaching into each of its 1178 chunks
  with references, 1524 references in all three dimensions, extracted into
  `tests/fixtures/structure_references.json.gz`) all match.
* Ocean monuments: only the start (and its references) has a vanilla reference. The rooms
  were checked by hand against vanilla's `OceanMonumentPieces`; the test places the
  fixture's monument and checks the core's eight gold blocks, the three elder guardians and
  that nothing but monument blocks, water, ice and water plants is left in its volume.
* Woodland mansions: the dump has none. The piece layouts (every piece's template,
  position, rotation and mirror) match the Java layouts Pumpkin's test records for four
  seeds; the test places a mansion of seed 0 and checks its blocks, loot chests, mobs and
  foundation.
* Saved starts: every start of the dump (176) and starts of the structures it lacks
  (temples, an igloo, a mansion, an ancient city, an outpost, villages) are saved, read
  back by a new generator with the same pieces and saved again unchanged; a desert
  pyramid placed in one chunk, saved and loaded continues identically in its other
  chunks, where without loading one of them settles it two blocks lower. No chunk NBT
  saved by vanilla was available here, so reading vanilla's own saves is untested beyond
  following its format.
* Desert pyramids, jungle temples, swamp huts, igloos, fossils and the other jigsaw
  structures have no reference here; tests check that they are found and place blocks.
* Feature types the fixtures do not exercise are checked against Pumpkin (structures off)
  on 25 more chunks with end spikes, gateways, chorus, end islands, ice spikes, icebergs,
  forest rocks, huge mushrooms and corals (`tests/features.rs`). Where Pumpkin departs from
  vanilla 26.3 (a few random draws and tags in ice spikes, icebergs, forest rocks and huge
  mushrooms), this crate follows vanilla, and the references were made with those spots of
  Pumpkin corrected.
* Pointed dripstone, dripstone clusters and large dripstone have no reference: Pumpkin's
  versions are placeholders. They follow vanilla 26.3's algorithms.
* Sculk patches have no vanilla reference either (no fixture chunk reaches the deep
  dark). They follow vanilla 26.3's spreader, checked against Pumpkin's port, which this
  crate departs from where Pumpkin departs from vanilla (see `src/feature/kinds/sculk.rs`);
  the test decorates the chunks around an ancient city and checks the sculk, veins,
  sensors and catalysts, that every vein face rests on a block, and that a second run is
  identical.
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
| Overworld | 587 ms | 9.3 ms/chunk (7.7 without structures) | 3.4 ms/chunk (2.4) | 2.3 ms/chunk (2.3) |
| Nether | 291 ms | 2.7 ms/chunk (2.0) | 1.1 ms/chunk (1.1) | 0.4 ms/chunk (0.5) |
| End | 27 ms | 0.7 ms/chunk (0.5) | 0.3 ms/chunk (0.2) | 0.01 ms/chunk (0.01) |

The terrain times include generating the structure starts the chunks need and the
beardifier; building the generator parses the structure templates the dimension's
structures can use (about 1300 in the overworld). `cargo run --release -p paper-worldgen
--example bench` reproduces these (each dimension with structures on and off).

## Provenance

The algorithms are ported from [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin)
(GPL-3.0, commit e9dc12e9), a Rust server that reproduces vanilla 26.3 world generation,
and adapted to build everything from the jar's data at runtime instead of Pumpkin's
build-time code generation. Each file derived from Pumpkin names its source files in its
header. Where Pumpkin's feature code differs from vanilla 26.3 or is missing, the code was
written to vanilla's behaviour; such files say so in their header. The structure code
(`src/structure/`, `src/feature/kinds/template.rs`) is not from Pumpkin: it is written to
vanilla 26.3's behaviour, and each file names the vanilla classes it follows.

The reference outputs in `tests/fixtures/` are vanilla 26.3 dumps from Pumpkin's
`assets/tests/` (converted to gzip-compressed little-endian `u16` arrays or gzipped JSON).
