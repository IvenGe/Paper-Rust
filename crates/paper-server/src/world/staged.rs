//! Vanilla world generation (`paper-worldgen`) in the chunk pipeline.
//!
//! Vanilla generates a chunk in two halves: terrain (biomes, noise, surface, carvers), which
//! depends on nothing but the chunk's position, and features ("decoration"), which read and
//! write the chunk's eight neighbours as well (trees cross chunk borders). A chunk is
//! finished once it and all eight neighbours are decorated: only then can nothing write into
//! it any more.
//!
//! The world keeps unfinished chunks apart from the playable ones, as [`ProtoEntry`]s, so
//! game logic never sees them. A decoration job takes the nine chunks it needs out of that
//! table and gives them back when it is done, which keeps two jobs from writing into the
//! same chunk at once. Finished chunks are turned into ordinary [`Chunk`]s on a worker and
//! join the world like loaded ones. Unfinished chunks are saved with vanilla's `Status`
//! (`carvers` or `features`) and their worldgen heightmaps, so a restart continues where
//! generation stopped instead of decorating a chunk twice.

use super::chunk::{BiomeStorage, BlockStorage, Chunk, ChunkPos, GenStatus, ProtoData, Section};
use super::block_updates::{flags, Level};
use super::generator::Generator;
use paper_protocol::codec::BlockPos;
use paper_data::blocks;
use paper_nbt::{Compound, Tag};
use paper_worldgen::{BlockEntity, Dimension, Entity, ProtoChunk, Region, WorldGenerator};
use std::sync::Arc;

/// The vanilla generator of one dimension.
pub struct VanillaGenerator {
    pub inner: WorldGenerator,
}

/// The vanilla jar as `paper-worldgen` reads it.
struct JarData<'a>(&'a crate::vanilla::VanillaData);

impl paper_worldgen::DataSource for JarData<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path)
    }
}

impl VanillaGenerator {
    /// Builds the generator of a dimension (`minecraft:overworld`, `minecraft:the_nether`,
    /// `minecraft:the_end`) from the worldgen data in the vanilla jar, with or without
    /// structures (`generate-structures`).
    pub fn new(data: &crate::vanilla::VanillaData, dimension: &str, seed: i64, structures: bool) -> anyhow::Result<Self> {
        let dimension = match dimension {
            "minecraft:the_nether" => Dimension::Nether,
            "minecraft:the_end" => Dimension::End,
            _ => Dimension::Overworld,
        };
        let mut inner = WorldGenerator::new(&JarData(data), dimension, seed)?;
        inner.set_structures_enabled(structures);
        if !inner.unsupported_features().is_empty() {
            log::debug!("{} generation leaves out these feature types: {}", dimension.id(), inner.unsupported_features().join(", "));
        }
        Ok(Self { inner })
    }

    /// Where vanilla would put the spawn of a new world (generates the chunks it looks at).
    pub fn find_spawn(&self) -> (i32, i32, i32) {
        self.inner.find_spawn()
    }
}

impl Generator for VanillaGenerator {
    /// Terrain only; the world decorates chunks through [`Generator::staged`].
    fn generate(&self, pos: ChunkPos, min_y: i32, height: i32) -> Chunk {
        let entry = ProtoEntry::new(self.inner.generate_terrain(pos.x, pos.z));
        entry.to_chunk(min_y, height, GenStatus::Full)
    }

    fn is_flat(&self) -> bool {
        false
    }

    fn sea_level(&self) -> i32 {
        self.inner.sea_level()
    }

    fn surface_y(&self, _x: i32, _z: i32) -> i32 {
        // New worlds take their spawn from `find_spawn`.
        self.inner.sea_level()
    }

    fn name(&self) -> &'static str {
        "vanilla"
    }

    fn staged(&self) -> Option<&VanillaGenerator> {
        Some(self)
    }
}

/// An unfinished chunk.
pub struct ProtoEntry {
    pub chunk: Box<ProtoChunk>,
    /// Its own features are placed.
    pub decorated: bool,
    /// Block entities placed by features, as chunk NBT keeps them (with `x`, `y`, `z`).
    pub block_entities: Vec<Compound>,
    /// Entities placed by features (end crystals), as entity NBT.
    pub entities: Vec<Compound>,
    /// Changed since it was last saved.
    pub dirty: bool,
    /// A read-only copy of a finished chunk lent to a decoration job.
    pub borrowed: bool,
}

impl ProtoEntry {
    /// Freshly generated terrain.
    pub fn new(chunk: ProtoChunk) -> Self {
        Self { chunk: Box::new(chunk), decorated: false, block_entities: Vec::new(), entities: Vec::new(), dirty: true, borrowed: false }
    }

    pub fn pos(&self) -> ChunkPos {
        ChunkPos::new(self.chunk.chunk_x, self.chunk.chunk_z)
    }

    /// An unfinished chunk read from disk.
    pub fn from_saved(chunk: &Chunk, generator: &VanillaGenerator) -> Self {
        let height = chunk.height();
        let mut proto = ProtoChunk::new(chunk.pos.x, chunk.pos.z, chunk.min_y, height);
        for (i, section) in chunk.sections.iter().enumerate() {
            let base = i << 12;
            match &section.blocks {
                BlockStorage::Uniform(v) => proto.blocks[base..base + 4096].fill(*v),
                BlockStorage::Dense(data) => proto.blocks[base..base + 4096].copy_from_slice(&data[..]),
            }
            let biome_base = i << 6;
            match &section.biomes {
                BiomeStorage::Uniform(b) => proto.biomes[biome_base..biome_base + 64].fill(*b),
                BiomeStorage::Dense(data) => proto.biomes[biome_base..biome_base + 64].copy_from_slice(&data[..]),
            }
        }
        proto.prime_heightmaps(generator.inner.block_flags());
        match chunk.proto.as_ref().and_then(|p| p.wg_heightmaps.as_deref()) {
            Some(maps) => proto.set_wg_heightmaps(*maps),
            None => proto.snapshot_wg_heightmaps(),
        }
        let block_entities = chunk
            .block_entities
            .iter()
            .map(|((y, z, x), data)| {
                let mut c = data.clone();
                c.insert("x", Tag::Int(chunk.pos.x * 16 + *x as i32));
                c.insert("y", Tag::Int(*y));
                c.insert("z", Tag::Int(chunk.pos.z * 16 + *z as i32));
                c
            })
            .collect();
        let entities = chunk.proto.as_ref().map(|p| p.entities.clone()).unwrap_or_default();
        Self { chunk: Box::new(proto), decorated: chunk.status != GenStatus::Terrain, block_entities, entities, dirty: false, borrowed: false }
    }

    /// A read-only copy of a finished chunk, for decorating a neighbour.
    pub fn borrowed(chunk: &Chunk, generator: &VanillaGenerator) -> Self {
        let mut entry = Self::from_saved(chunk, generator);
        entry.borrowed = true;
        entry.decorated = true;
        entry
    }

    /// The chunk in the server's layout: finished (`Full`), or for saving with its status.
    pub fn to_chunk(&self, min_y: i32, height: i32, status: GenStatus) -> Chunk {
        let proto = &self.chunk;
        let mut chunk = Chunk::empty(ChunkPos::new(proto.chunk_x, proto.chunk_z), min_y, height, 0);
        for i in 0..chunk.sections.len() {
            let mut data = Box::new([0u16; 4096]);
            data.copy_from_slice(proto.section_blocks(i));
            let mut biomes = Box::new([0u8; 64]);
            biomes.copy_from_slice(proto.section_biomes(i));
            let biomes = if biomes.iter().all(|b| *b == biomes[0]) { BiomeStorage::Uniform(biomes[0]) } else { BiomeStorage::Dense(biomes) };
            let mut section = Section { blocks: BlockStorage::Dense(data), biomes, non_air: 0, fluids: 0, ticking: 0 };
            section.compact();
            section.recount();
            chunk.sections[i] = Arc::new(section);
        }
        chunk.recompute_derived();
        // Block entities whose block is still there, and empty ones for blocks that need
        // one but got no data from their feature.
        for c in &self.block_entities {
            let mut c = c.clone();
            let (Some(x), Some(y), Some(z)) = (c.i32("x"), c.i32("y"), c.i32("z")) else { continue };
            let state = chunk.get_block((x & 15) as usize, y, (z & 15) as usize);
            if blocks::flags(state) & blocks::FLAG_BLOCK_ENTITY == 0 {
                continue;
            }
            for key in ["x", "y", "z"] {
                c.remove(key);
            }
            chunk.block_entities.insert((y, (z & 15) as u8, (x & 15) as u8), c);
        }
        add_missing_block_entities(&mut chunk);
        chunk.status = status;
        if status == GenStatus::Full {
            chunk.entity_nbt = self.entities.clone();
        } else {
            chunk.proto = Some(Box::new(ProtoData { wg_heightmaps: Some(Box::new(*proto.wg_heightmaps())), entities: self.entities.clone() }));
        }
        chunk
    }
}

/// Gives every block that keeps a block entity (chests, spawners, ...) at least an empty one.
fn add_missing_block_entities(chunk: &mut Chunk) {
    let min_y = chunk.min_y;
    for (i, section) in chunk.sections.iter().enumerate() {
        let BlockStorage::Dense(data) = &section.blocks else {
            if let BlockStorage::Uniform(v) = section.blocks {
                if blocks::flags(v as u32) & blocks::FLAG_BLOCK_ENTITY != 0 {
                    log::debug!("a whole section of block entity blocks in chunk {},{}", chunk.pos.x, chunk.pos.z);
                }
            }
            continue;
        };
        for (index, state) in data.iter().enumerate() {
            let state = *state as u32;
            if blocks::flags(state) & blocks::FLAG_BLOCK_ENTITY == 0 {
                continue;
            }
            let (x, y, z) = ((index & 15) as u8, min_y + (i as i32) * 16 + (index >> 8) as i32, ((index >> 4) & 15) as u8);
            if let Some(kind) = block_entity_type(state) {
                chunk.block_entities.entry((y, z, x)).or_insert_with(|| Compound::new().with("id", Tag::String(kind.to_owned())));
            }
        }
    }
}

/// The block entity type of a block (`minecraft:chest` for chests, `minecraft:mob_spawner`
/// for spawners, ...).
fn block_entity_type(state: u32) -> Option<&'static str> {
    let name = blocks::block_of_state(state).name;
    let kind = match name {
        "minecraft:spawner" => "minecraft:mob_spawner",
        "minecraft:suspicious_sand" | "minecraft:suspicious_gravel" => "minecraft:brushable_block",
        "minecraft:bee_nest" => "minecraft:beehive",
        n if n.ends_with("_bed") => "minecraft:bed",
        n if n.ends_with("_banner") => "minecraft:banner",
        n if n.ends_with("shulker_box") => "minecraft:shulker_box",
        n if n.ends_with("_sign") && n.contains("hanging") => "minecraft:hanging_sign",
        n if n.ends_with("_sign") => "minecraft:sign",
        n if n.ends_with("_skull") || n.ends_with("_head") => "minecraft:skull",
        "minecraft:moving_piston" => "minecraft:piston",
        "minecraft:soul_campfire" => "minecraft:campfire",
        "minecraft:trapped_chest" => "minecraft:trapped_chest",
        other => other,
    };
    paper_data::registries::id("minecraft:block_entity_type", kind).map(|_| kind)
}

/// Block entity NBT for what a feature placed at a block.
fn block_entity_nbt(state: u32, entity: &BlockEntity) -> Option<Compound> {
    let kind = block_entity_type(state)?;
    let c = Compound::new().with("id", Tag::String(kind.to_owned()));
    Some(match entity {
        BlockEntity::Spawner { entity } => c
            .with("SpawnData", Tag::Compound(Compound::new().with("entity", Tag::Compound(Compound::new().with("id", Tag::String((*entity).to_owned()))))))
            .with("Delay", Tag::Short(20))
            .with("MinSpawnDelay", Tag::Short(200))
            .with("MaxSpawnDelay", Tag::Short(800))
            .with("SpawnCount", Tag::Short(4))
            .with("MaxNearbyEntities", Tag::Short(6))
            .with("RequiredPlayerRange", Tag::Short(16))
            .with("SpawnRange", Tag::Short(4)),
        BlockEntity::Loot { table, seed } => c.with("LootTable", Tag::String(table.clone())).with("LootTableSeed", Tag::Long(*seed)),
        BlockEntity::Beehive { occupants } => c.with(
            "bees",
            Tag::List(
                occupants
                    .iter()
                    .map(|ticks| {
                        Tag::Compound(
                            Compound::new()
                                .with("entity_data", Tag::Compound(Compound::new().with("id", Tag::String("minecraft:bee".into()))))
                                .with("min_ticks_in_hive", Tag::Int(600))
                                .with("ticks_in_hive", Tag::Int(*ticks)),
                        )
                    })
                    .collect(),
            ),
        ),
        BlockEntity::EndGateway { exit, exact } => {
            let mut c = c.with("Age", Tag::Long(0)).with("ExactTeleport", Tag::Byte(*exact as i8));
            if let Some((x, y, z)) = exit {
                c.insert("exit_portal", Tag::IntArray(vec![*x, *y, *z]));
            }
            c
        }
    })
}

/// Entity NBT for what a feature spawned.
fn entity_nbt(entity: &Entity) -> Compound {
    match entity {
        Entity::EndCrystal { x, y, z, yaw, beam_target, invulnerable } => {
            let uuid = uuid::Uuid::new_v4().as_u128();
            let words: Vec<i32> = (0..4).map(|i| (uuid >> (96 - 32 * i)) as u32 as i32).collect();
            let mut c = Compound::new()
                .with("id", Tag::String("minecraft:end_crystal".into()))
                .with("Pos", Tag::List(vec![Tag::Double(*x), Tag::Double(*y), Tag::Double(*z)]))
                .with("Motion", Tag::List(vec![Tag::Double(0.0), Tag::Double(0.0), Tag::Double(0.0)]))
                .with("Rotation", Tag::List(vec![Tag::Float(*yaw), Tag::Float(0.0)]))
                .with("UUID", Tag::IntArray(words))
                .with("ShowBottom", Tag::Byte(1))
                .with("Invulnerable", Tag::Byte(*invulnerable as i8));
            if let Some((bx, by, bz)) = beam_target {
                c.insert("beam_target", Tag::IntArray(vec![*bx, *by, *bz]));
            }
            c
        }
    }
}

/// Grows a feature in the playable world (a tree from a sapling, a huge mushroom, a fungus):
/// the nine chunks around `origin` are copied, `clear` is set in the copy (where the sapling
/// stood is air while the tree grows), the feature is placed into it, and the blocks that
/// changed are set in the world (neighbours notified, clients told), with the block entities
/// the feature made (bee nests). False if it does not fit, the feature is unknown, or a chunk
/// around it is not loaded; then the world is unchanged.
pub fn place_grown_feature(level: &mut Level, feature: &str, origin: BlockPos, clear: &[(BlockPos, u32)]) -> bool {
    // The world's own vanilla generator, or the one flat worlds build for this.
    let generator = Arc::clone(&level.world.generator);
    let fallback;
    let vanilla: &VanillaGenerator = match generator.staged() {
        Some(own) => own,
        None => {
            fallback = level.world.grown_features();
            match fallback.as_deref() {
                Some(built) => built,
                None => return false,
            }
        }
    };
    if !vanilla.inner.can_grow(feature) {
        return false;
    }
    let center = ChunkPos::of_block(origin.x, origin.z);
    let mut chunks = Vec::with_capacity(9);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let Some(chunk) = level.world.chunks.get(&ChunkPos::new(center.x + dx, center.z + dz)) else { return false };
            chunks.push(*ProtoEntry::from_saved(chunk, vanilla).chunk);
        }
    }
    let before: Vec<(i32, i32, Vec<u16>)> = chunks.iter().map(|c| (c.chunk_x, c.chunk_z, c.blocks.clone())).collect();
    for (pos, state) in clear {
        if let Some(c) = chunks.iter_mut().find(|c| c.chunk_x == pos.x >> 4 && c.chunk_z == pos.z >> 4) {
            c.set(pos.x, pos.y, pos.z, *state, vanilla.inner.block_flags());
        }
    }
    let mut region = Region::new(center.x, center.z, chunks);
    if !vanilla.inner.place_grown_feature(feature, &mut region, origin.x, origin.y, origin.z, rand::random()) {
        return false;
    }
    let new_block_entities = std::mem::take(&mut region.block_entities);
    let mut changes = Vec::new();
    for chunk in region.into_chunks() {
        let Some((_, _, old)) = before.iter().find(|(x, z, _)| *x == chunk.chunk_x && *z == chunk.chunk_z) else { continue };
        for (index, (a, b)) in old.iter().zip(chunk.blocks.iter()).enumerate() {
            if a != b {
                let (section, local) = ((index >> 12) as i32, index & 4095);
                let x = chunk.chunk_x * 16 + (local & 15) as i32;
                let z = chunk.chunk_z * 16 + ((local >> 4) & 15) as i32;
                changes.push((BlockPos::new(x, chunk.min_y + section * 16 + (local >> 8) as i32, z), *b as u32));
            }
        }
    }
    // As features set blocks (`setBlock` with 19): neighbours notified, shapes already right.
    for (pos, state) in changes {
        level.set_block(pos, state, flags::NEIGHBORS | flags::CLIENTS | flags::KNOWN_SHAPE);
    }
    for ((x, y, z), block_entity) in new_block_entities {
        let state = level.state(BlockPos::new(x, y, z));
        let Some(c) = block_entity_nbt(state, &block_entity) else { continue };
        if let Some(chunk) = level.world.chunks.get_mut(&ChunkPos::of_block(x, z)) {
            chunk.block_entities.insert((y, (z & 15) as u8, (x & 15) as u8), c);
        }
    }
    true
}

/// Decorates the chunk at `center` with its eight neighbours (all nine entries, any order).
/// Runs on a worker.
pub fn decorate(generator: &VanillaGenerator, center: ChunkPos, entries: Vec<ProtoEntry>) -> Vec<ProtoEntry> {
    let mut metadata = Vec::with_capacity(entries.len());
    let mut chunks = Vec::with_capacity(entries.len());
    for entry in entries {
        metadata.push((entry.pos(), entry.decorated, entry.block_entities, entry.entities, entry.borrowed));
        chunks.push(*entry.chunk);
    }
    let mut region = Region::new(center.x, center.z, chunks);
    generator.inner.decorate(center.x, center.z, &mut region);
    if generator.inner.dimension() == Dimension::End && center == ChunkPos::new(0, 0) {
        place_exit_portal(&mut region, generator);
    }
    let new_block_entities = std::mem::take(&mut region.block_entities);
    let new_entities = std::mem::take(&mut region.entities);
    // Structures: block entities and entities from templates, as vanilla NBT.
    let new_block_entity_nbt = std::mem::take(&mut region.block_entity_nbt);
    let new_entity_nbt = std::mem::take(&mut region.entity_nbt);
    let mut out: Vec<ProtoEntry> = region
        .into_chunks()
        .into_iter()
        .map(|chunk| {
            let pos = ChunkPos::new(chunk.chunk_x, chunk.chunk_z);
            let (decorated, block_entities, entities, borrowed) = metadata.iter_mut().find(|(p, ..)| *p == pos).map(|(_, d, b, e, borrowed)| (*d, std::mem::take(b), std::mem::take(e), *borrowed)).expect("decorated chunk came back");
            ProtoEntry { chunk: Box::new(chunk), decorated: decorated || pos == center, block_entities, entities, dirty: true, borrowed }
        })
        .collect();
    for ((x, y, z), block_entity) in new_block_entities {
        let pos = ChunkPos::new(x >> 4, z >> 4);
        let Some(entry) = out.iter_mut().find(|e| e.pos() == pos) else { continue };
        let state = entry.chunk.get(x, y, z);
        if let Some(mut c) = block_entity_nbt(state, &block_entity) {
            c.insert("x", Tag::Int(x));
            c.insert("y", Tag::Int(y));
            c.insert("z", Tag::Int(z));
            entry.block_entities.retain(|b| (b.i32("x"), b.i32("y"), b.i32("z")) != (Some(x), Some(y), Some(z)));
            entry.block_entities.push(c);
        }
    }
    for ((x, y, z), mut c) in new_block_entity_nbt {
        let pos = ChunkPos::new(x >> 4, z >> 4);
        let Some(entry) = out.iter_mut().find(|e| e.pos() == pos) else { continue };
        if c.string("id").is_none() {
            let Some(kind) = block_entity_type(entry.chunk.get(x, y, z)) else { continue };
            c.insert("id", Tag::String(kind.to_owned()));
        }
        c.insert("x", Tag::Int(x));
        c.insert("y", Tag::Int(y));
        c.insert("z", Tag::Int(z));
        entry.block_entities.retain(|b| (b.i32("x"), b.i32("y"), b.i32("z")) != (Some(x), Some(y), Some(z)));
        entry.block_entities.push(c);
    }
    for entity in new_entities {
        let Entity::EndCrystal { x, z, .. } = &entity;
        let pos = ChunkPos::new((*x).floor() as i32 >> 4, (*z).floor() as i32 >> 4);
        if let Some(entry) = out.iter_mut().find(|e| e.pos() == pos) {
            entry.entities.push(entity_nbt(&entity));
        }
    }
    for mut entity in new_entity_nbt {
        let Some((x, z)) = entity.list("Pos").and_then(|p| Some((p.first()?.as_f64()?, p.get(2)?.as_f64()?))) else { continue };
        // Mobs that come with a structure stay (villagers, witches, guardians, shulkers, ...).
        entity.insert("PersistenceRequired", Tag::Byte(1));
        let pos = ChunkPos::new(x.floor() as i32 >> 4, z.floor() as i32 >> 4);
        if let Some(entry) = out.iter_mut().find(|e| e.pos() == pos) {
            entry.entities.push(entity);
        }
    }
    out
}

/// The End's exit portal (`EndDragonFight.spawnExitPortal` with `EndPodiumFeature`), open
/// from the start since there is no dragon fight: placed when the chunk at the origin is
/// decorated (its neighbours hold the whole podium).
fn place_exit_portal(region: &mut Region, generator: &VanillaGenerator) {
    use paper_worldgen::{Heightmap, RegionAccess};
    let flags = generator.inner.block_flags();
    let Some(origin_chunk) = region.chunk(0, 0) else { return };
    // The top block at the origin, below any bedrock down to y 63.
    let mut y = origin_chunk.top_y(Heightmap::MotionBlockingNoLeaves, 0, 0) - 1;
    let bedrock = blocks::default_state("minecraft:bedrock").unwrap_or(0);
    while y > 63 && region.chunk(0, 0).is_some_and(|c| c.get(0, y, 0) == bedrock) {
        y -= 1;
    }
    let y = y.max(generator.inner.min_y() + 1);
    let state = |name: &str| blocks::default_state(name).unwrap_or(0);
    let (end_stone, portal, air) = (state("minecraft:end_stone"), state("minecraft:end_portal"), blocks::AIR);
    let mut set = |x: i32, by: i32, z: i32, s: u32| {
        if let Some(chunk) = region.chunk_mut(x >> 4, z >> 4) {
            chunk.set(x, by, z, s, flags);
        }
    };
    for by in y - 1..=y + 32 {
        for z in -4..=4 {
            for x in -4..=4 {
                let d2 = (x * x + (by - y) * (by - y) + z * z) as f64;
                let inner = d2 < 2.5 * 2.5;
                if !inner && d2 >= 3.5 * 3.5 {
                    continue;
                }
                let s = if by < y {
                    if inner {
                        bedrock
                    } else {
                        end_stone
                    }
                } else if by > y {
                    air
                } else if !inner {
                    bedrock
                } else {
                    portal
                };
                set(x, by, z, s);
            }
        }
    }
    for h in 0..4 {
        set(0, y + h, 0, bedrock);
    }
    let torch = blocks::default_state("minecraft:wall_torch").unwrap_or(0);
    for (dx, dz, facing) in [(0, -1, "north"), (0, 1, "south"), (-1, 0, "west"), (1, 0, "east")] {
        let s = blocks::with_property(torch, "facing", facing).unwrap_or(torch);
        set(dx, y + 2, dz, s);
    }
}
