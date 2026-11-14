//! Structure templates (`StructureTemplate`): the `.nbt` files of `data/<ns>/structure`,
//! read when the generator is built, and their placement (`placeInWorld`) with processors.
//!
//! Written to vanilla 26.3's `StructureTemplate`, `StructurePlaceSettings` and
//! `StructureTemplate.JigsawBlockInfo`. Not done: the neighbour shape updates vanilla runs
//! after placing a template whose settings do not have `knownShape` (only legacy template
//! structures; jigsaw pieces always have it), and `Mob.finalizeSpawn` of template entities
//! (left to whoever spawns the returned entity NBT).

use super::bbox::BoundingBox;
use super::processor::{Info, Processor};
use super::transform::{self, transform_pos, transform_vec, Mirror, Rotation};
use crate::data::{split_id, DataReader};
use crate::feature::blockinfo::Fluid;
use crate::feature::world::{Dir, Pos, World};
use crate::random::{position_seed, Random, WorldgenRandom};
use anyhow::{anyhow, Context, Result};
use paper_data::blocks;
use paper_nbt::{Compound, Tag};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

/// A block of a template palette (`StructureBlockInfo`), at its template-relative position.
#[derive(Clone, Debug)]
pub(crate) struct BlockInfo {
    pub pos: Pos,
    pub state: u32,
    pub nbt: Option<Arc<Compound>>,
}

/// `StructureTemplate.JigsawBlockInfo`.
#[derive(Clone, Debug)]
pub(crate) struct JigsawBlock {
    pub pos: Pos,
    pub front: Dir,
    pub top: Dir,
    /// `JointType.ROLLABLE`.
    pub rollable: bool,
    pub name: Arc<str>,
    pub pool: Arc<str>,
    pub target: Arc<str>,
    pub placement_priority: i32,
    pub selection_priority: i32,
}

/// `StructureTemplate.Palette`: the blocks in placement order and the jigsaw blocks among
/// them.
pub(crate) struct Palette {
    pub blocks: Vec<BlockInfo>,
    pub jigsaws: Vec<JigsawBlock>,
}

/// `StructureTemplate.StructureEntityInfo`.
pub(crate) struct EntityInfo {
    pub pos: (f64, f64, f64),
    pub block_pos: Pos,
    pub nbt: Compound,
}

/// A parsed template.
pub struct Template {
    /// The template's id (`minecraft:end_city/base_floor`).
    pub id: String,
    pub size: (i32, i32, i32),
    pub(crate) palettes: Vec<Palette>,
    pub(crate) entities: Vec<EntityInfo>,
}

fn int_list(c: &Compound, key: &str) -> [i32; 3] {
    let mut out = [0; 3];
    if let Some(l) = c.list(key) {
        for (i, t) in l.iter().take(3).enumerate() {
            out[i] = t.as_i32().unwrap_or(0);
        }
    }
    out
}

/// `NbtUtils.readBlockState`: unknown blocks read as air, unknown properties are ignored.
fn read_state(c: &Compound) -> u32 {
    let air = blocks::default_state("minecraft:air").unwrap_or(0);
    let Some(name) = c.string("id").or_else(|| c.string("Name")) else { return air };
    let Some(mut state) = blocks::default_state(name) else { return air };
    if let Some(props) = c.compound("properties").or_else(|| c.compound("Properties")) {
        for (k, v) in props.iter() {
            if let Some(v) = v.as_str() {
                if let Some(s) = blocks::with_property(state, k, v) {
                    state = s;
                }
            }
        }
    }
    state
}

/// A block state string as jigsaw blocks store it (`minecraft:x[a=b]{nbt}`); air if invalid.
pub(crate) fn parse_state_string(s: &str) -> u32 {
    let s = s.split('{').next().unwrap_or(s);
    blocks::parse_state(s).unwrap_or_else(|| blocks::default_state("minecraft:air").unwrap_or(0))
}

/// Whether placing `state` creates a block entity (`hasBlockEntity`), and whether it is a
/// `RandomizableContainer` (which gets a fresh `LootTableSeed` when placed from a template).
pub(crate) fn block_entity_kind(state: u32) -> Option<bool> {
    let name = blocks::block_of_state(state).name;
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    let container = matches!(name, "chest" | "trapped_chest" | "barrel" | "dispenser" | "dropper" | "hopper" | "crafter" | "decorated_pot")
        || name.ends_with("copper_chest")
        || name.ends_with("shulker_box");
    if container {
        return Some(true);
    }
    let other = matches!(
        name,
        "bell" | "blast_furnace" | "brewing_stand" | "campfire" | "soul_campfire" | "comparator" | "creaking_heart" | "ender_chest" | "furnace"
            | "smoker" | "jigsaw" | "lectern" | "sculk_sensor" | "calibrated_sculk_sensor" | "sculk_catalyst" | "sculk_shrieker" | "spawner"
            | "structure_block" | "suspicious_sand" | "suspicious_gravel" | "trial_spawner" | "vault" | "beehive" | "bee_nest" | "beacon"
            | "conduit" | "daylight_detector" | "enchanting_table" | "end_gateway" | "end_portal" | "jukebox" | "chiseled_bookshelf"
            | "command_block" | "chain_command_block" | "repeating_command_block" | "moving_piston" | "test_block" | "test_instance_block"
    ) || name.ends_with("_sign")
        || name.ends_with("_banner")
        || name.ends_with("_bed")
        || name.ends_with("_skull")
        || name.ends_with("_head")
        || name.ends_with("golem_statue")
        || name.ends_with("_shelf");
    other.then_some(false)
}

fn jigsaw_block(info: &BlockInfo) -> Option<JigsawBlock> {
    let nbt = info.nbt.as_deref()?;
    let orientation = blocks::property(info.state, "orientation")?;
    let (front, top) = orientation.split_once('_')?;
    let (front, top) = (Dir::parse(front)?, Dir::parse(top)?);
    let id = |k: &str| -> Arc<str> { crate::data::full_id(nbt.string(k).unwrap_or("minecraft:empty")).into() };
    let rollable = match nbt.string("joint") {
        Some(j) => j == "rollable",
        None => !front.is_horizontal(),
    };
    Some(JigsawBlock {
        pos: info.pos,
        front,
        top,
        rollable,
        name: id("name"),
        pool: id("pool"),
        target: id("target"),
        placement_priority: nbt.i32("placement_priority").unwrap_or(0),
        selection_priority: nbt.i32("selection_priority").unwrap_or(0),
    })
}

impl Template {
    /// `StructureTemplate.load`.
    pub(crate) fn parse(gzipped: &[u8]) -> Result<Self> {
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(gzipped).read_to_end(&mut raw)?;
        let (_, root) = paper_nbt::read_named(&mut &raw[..], usize::MAX).map_err(|e| anyhow!("{e:?}"))?;
        let [sx, sy, sz] = int_list(&root, "size");
        let palette_lists: Vec<&[Tag]> = match root.list("palettes") {
            Some(p) => p.iter().filter_map(Tag::as_list).collect(),
            None => vec![root.list("palette").unwrap_or(&[])],
        };
        let raw_blocks: Vec<(Pos, i32, Option<Arc<Compound>>)> = root
            .list("blocks")
            .unwrap_or(&[])
            .iter()
            .filter_map(Tag::as_compound)
            .map(|b| {
                let [x, y, z] = int_list(b, "pos");
                (Pos::new(x, y, z), b.i32("state").unwrap_or(0), b.compound("nbt").map(|c| Arc::new(c.clone())))
            })
            .collect();
        let air = blocks::default_state("minecraft:air").unwrap_or(0);
        let jigsaw = blocks::default_state("minecraft:jigsaw").map(blocks::block_id_of_state);
        let mut palettes = Vec::with_capacity(palette_lists.len());
        for list in palette_lists {
            let states: Vec<u32> = list.iter().map(|t| t.as_compound().map(read_state).unwrap_or(air)).collect();
            let (mut full, mut other, mut entities) = (Vec::new(), Vec::new(), Vec::new());
            for (pos, id, nbt) in &raw_blocks {
                let state = states.get(*id as usize).copied().unwrap_or(air);
                let info = BlockInfo { pos: *pos, state, nbt: nbt.clone() };
                if info.nbt.is_some() {
                    entities.push(info);
                } else if !is_dynamic_shape(state) && is_collision_full_block(state) {
                    full.push(info);
                } else {
                    other.push(info);
                }
            }
            let key = |b: &BlockInfo| (b.pos.y, b.pos.x, b.pos.z);
            full.sort_by_key(key);
            other.sort_by_key(key);
            entities.sort_by_key(key);
            let mut blocks_list = full;
            blocks_list.append(&mut other);
            blocks_list.append(&mut entities);
            let jigsaws = blocks_list.iter().filter(|b| Some(blocks::block_id_of_state(b.state)) == jigsaw).filter_map(jigsaw_block).collect();
            palettes.push(Palette { blocks: blocks_list, jigsaws });
        }
        let mut entities = Vec::new();
        for e in root.list("entities").unwrap_or(&[]).iter().filter_map(Tag::as_compound) {
            let Some(pos) = e.list("pos").filter(|p| p.len() == 3) else { continue };
            let d = |i: usize| pos[i].as_f64().unwrap_or(0.0);
            let [bx, by, bz] = int_list(e, "blockPos");
            if let Some(nbt) = e.compound("nbt") {
                entities.push(EntityInfo { pos: (d(0), d(1), d(2)), block_pos: Pos::new(bx, by, bz), nbt: nbt.clone() });
            }
        }
        Ok(Self { id: String::new(), size: (sx, sy, sz), palettes, entities })
    }

    /// `getSize(rotation)`.
    pub fn size_rotated(&self, rotation: Rotation) -> (i32, i32, i32) {
        match rotation {
            Rotation::Clockwise90 | Rotation::CounterClockwise90 => (self.size.2, self.size.1, self.size.0),
            _ => self.size,
        }
    }

    /// `getZeroPositionWithTransform`.
    pub fn zero_position_with_transform(&self, zero: Pos, mirror: Mirror, rotation: Rotation) -> Pos {
        let (sx, sz) = (self.size.0 - 1, self.size.2 - 1);
        let mx = if mirror == Mirror::FrontBack { sx } else { 0 };
        let mz = if mirror == Mirror::LeftRight { sz } else { 0 };
        let (dx, dz) = match rotation {
            Rotation::None => (mx, mz),
            Rotation::Clockwise90 => (sz - mz, mx),
            Rotation::Clockwise180 => (sx - mx, sz - mz),
            Rotation::CounterClockwise90 => (mz, sx - mx),
        };
        Pos::new(zero.x + dx, zero.y, zero.z + dz)
    }

    /// `getBoundingBox(position, rotation, pivot, mirror)`.
    pub fn bounding_box(&self, position: Pos, rotation: Rotation, pivot: Pos, mirror: Mirror) -> BoundingBox {
        let (sx, sy, sz) = self.size;
        let a = transform_pos(Pos::new(0, 0, 0), mirror, rotation, pivot);
        let b = transform_pos(Pos::new(sx - 1, sy - 1, sz - 1), mirror, rotation, pivot);
        BoundingBox::from_corners(a, b).moved(position.x, position.y, position.z)
    }

    /// `getJigsaws(position, rotation)`: the jigsaw blocks of the palette at `position` (the
    /// palette random is positional, so this draws nothing from any shared random).
    pub(crate) fn jigsaws(&self, position: Pos, rotation: Rotation) -> Vec<JigsawBlock> {
        if self.palettes.is_empty() {
            return Vec::new();
        }
        let palette = &self.palettes[palette_index(self.palettes.len(), &mut WorldgenRandom::legacy(position_seed(position.x, position.y, position.z)))];
        palette
            .jigsaws
            .iter()
            .map(|j| {
                let p = transform_pos(j.pos, Mirror::None, rotation, Pos::new(0, 0, 0));
                JigsawBlock { pos: Pos::new(p.x + position.x, p.y + position.y, p.z + position.z), front: rotation.rotate(j.front), top: rotation.rotate(j.top), ..j.clone() }
            })
            .collect()
    }

    /// `filterBlocks(position, settings, block)`: the blocks of `block` (a block id) at their
    /// world positions, rotated, inside the settings' box.
    pub(crate) fn filter_blocks(&self, position: Pos, settings: &mut PlaceSettings, block: u32) -> Vec<Info> {
        if self.palettes.is_empty() {
            return Vec::new();
        }
        let index = settings.palette(self.palettes.len(), position);
        let mut out = Vec::new();
        for b in &self.palettes[index].blocks {
            if blocks::block_id_of_state(b.state) != block {
                continue;
            }
            let p = settings.world_pos(b.pos, position);
            if settings.bbox.is_some_and(|bb| !bb.is_inside(p)) {
                continue;
            }
            out.push(Info { pos: p, state: transform::rotate_state(b.state, settings.rotation), nbt: b.nbt.clone() });
        }
        out
    }

    /// `placeInWorld(level, position, referencePos, settings, random, flags)`; `random` is the
    /// piece's random (container loot seeds). Returns whether anything could be placed.
    pub(crate) fn place(&self, w: &mut World, position: Pos, reference: Pos, settings: &mut PlaceSettings, random: &mut WorldgenRandom) -> bool {
        self.place_buffered(w, position, reference, settings, Some(random), None)
    }

    /// [`Template::place`], collecting the block entities in `buffer` (when given) instead of
    /// handing them to the world, so that data markers can still change them. Without a
    /// `random`, container loot seeds come from the settings' random (features place
    /// templates with their own random as both).
    pub(crate) fn place_buffered(
        &self,
        w: &mut World,
        position: Pos,
        reference: Pos,
        settings: &mut PlaceSettings,
        mut random: Option<&mut WorldgenRandom>,
        mut buffer: Option<&mut Vec<(Pos, Compound)>>,
    ) -> bool {
        if self.palettes.is_empty() {
            return false;
        }
        let index = settings.palette(self.palettes.len(), position);
        let list = &self.palettes[index].blocks;
        if (list.is_empty() && (settings.ignore_entities || self.entities.is_empty())) || self.size.0 < 1 || self.size.1 < 1 || self.size.2 < 1 {
            return false;
        }
        let processed = process_block_infos(w, position, reference, settings, list);
        let b = w.b;
        let waterlog = settings.waterlog;
        let mut to_fill: Vec<Pos> = Vec::new();
        let mut locked: HashSet<Pos> = HashSet::new();
        for info in processed {
            let p = info.pos;
            if settings.bbox.is_some_and(|bb| !bb.is_inside(p)) {
                continue;
            }
            let previous = if waterlog { Some(b.fluid(w.get(p))) } else { None };
            let state = transform::transform(info.state, settings.mirror, settings.rotation);
            if !w.set(p, state) {
                continue;
            }
            if let Some(nbt) = &info.nbt {
                if let Some(container) = block_entity_kind(state) {
                    let mut nbt = (**nbt).clone();
                    if container {
                        let seed = match random.as_deref_mut() {
                            Some(r) => r.next_i64(),
                            None => settings.with_random(p, |r| r.next_i64()),
                        };
                        nbt.insert("LootTableSeed", seed);
                    }
                    match buffer.as_deref_mut() {
                        Some(b) => {
                            b.retain(|(q, _)| *q != p);
                            b.push((p, nbt));
                        }
                        None => w.add_block_entity_nbt(p, nbt),
                    }
                }
            }
            if let Some(previous) = previous {
                if matches!(b.fluid(state), Fluid::Water | Fluid::Lava) {
                    locked.insert(p);
                } else if b.has_prop(state, "waterlogged") {
                    if previous == Fluid::Water && !b.is_waterlogged(state) {
                        w.set(p, b.waterlogged(state, true));
                    }
                    if !matches!(previous, Fluid::Water | Fluid::Lava) {
                        to_fill.push(p);
                    }
                }
            }
        }
        let mut filled = true;
        while filled && !to_fill.is_empty() {
            filled = false;
            let mut i = 0;
            while i < to_fill.len() {
                let p = to_fill[i];
                let mut fluid = b.fluid(w.get(p));
                for d in [Dir::Up, Dir::North, Dir::East, Dir::South, Dir::West] {
                    if matches!(fluid, Fluid::Water | Fluid::Lava) {
                        break;
                    }
                    let n = p.rel(d);
                    let nf = b.fluid(w.get(n));
                    if matches!(nf, Fluid::Water | Fluid::Lava) && !locked.contains(&n) {
                        fluid = nf;
                    }
                }
                if matches!(fluid, Fluid::Water | Fluid::Lava) {
                    let state = w.get(p);
                    if b.has_prop(state, "waterlogged") {
                        if fluid == Fluid::Water && !b.is_waterlogged(state) {
                            w.set(p, b.waterlogged(state, true));
                        }
                        filled = true;
                        to_fill.remove(i);
                        continue;
                    }
                }
                i += 1;
            }
        }
        if !settings.ignore_entities {
            self.place_entities(w, position, settings);
        }
        true
    }

    /// `placeEntities`: entity NBT with its position and rotation transformed, no UUID.
    fn place_entities(&self, w: &mut World, position: Pos, settings: &PlaceSettings) {
        for e in &self.entities {
            let bp = transform_pos(e.block_pos, settings.mirror, settings.rotation, settings.pivot);
            let bp = Pos::new(bp.x + position.x, bp.y + position.y, bp.z + position.z);
            if settings.bbox.is_some_and(|bb| !bb.is_inside(bp)) {
                continue;
            }
            let mut tag = e.nbt.clone();
            let (x, y, z) = transform_vec(e.pos, settings.mirror, settings.rotation, settings.pivot);
            let pos = (x + position.x as f64, y + position.y as f64, z + position.z as f64);
            tag.insert("Pos", Tag::List(vec![Tag::Double(pos.0), Tag::Double(pos.1), Tag::Double(pos.2)]));
            tag.remove("UUID");
            if tag.contains_key("block_pos") {
                tag.insert("block_pos", Tag::IntArray(vec![bp.x, bp.y, bp.z]));
            }
            // `Entity.rotate` and `mirror`.
            let (yaw, pitch) = match tag.list("Rotation") {
                Some(r) if r.len() == 2 => (r[0].as_f64().unwrap_or(0.0) as f32, r[1].as_f64().unwrap_or(0.0) as f32),
                _ => (0.0, 0.0),
            };
            let wrapped = wrap_degrees(yaw);
            let rotated = match settings.rotation {
                Rotation::Clockwise180 => wrapped + 180.0,
                Rotation::CounterClockwise90 => wrapped + 270.0,
                Rotation::Clockwise90 => wrapped + 90.0,
                Rotation::None => wrapped,
            };
            let mirrored = match settings.mirror {
                Mirror::FrontBack => -wrapped,
                Mirror::LeftRight => 180.0 - wrapped,
                Mirror::None => wrapped,
            };
            let yaw = rotated + (mirrored - yaw);
            tag.insert("Rotation", Tag::List(vec![Tag::Float(yaw), Tag::Float(pitch)]));
            w.add_entity_nbt(tag);
        }
    }
}

/// `Mth.wrapDegrees(float)`.
fn wrap_degrees(d: f32) -> f32 {
    let mut r = d % 360.0;
    if r >= 180.0 {
        r -= 360.0;
    }
    if r < -180.0 {
        r += 360.0;
    }
    r
}

fn is_collision_full_block(state: u32) -> bool {
    let boxes = paper_data::physics::collision_boxes(state);
    boxes.len() == 1 && {
        let b = &boxes[0];
        b.min.iter().all(|v| *v <= 0.0) && b.max.iter().all(|v| *v >= 1.0)
    }
}

/// Blocks built with `dynamicShape()`.
fn is_dynamic_shape(state: u32) -> bool {
    let name = blocks::block_of_state(state).name;
    name.ends_with("shulker_box") || name == "minecraft:moving_piston" || name == "minecraft:scaffolding"
}

/// `getRandomPalette`: `random.nextInt(size)`.
fn palette_index(size: usize, r: &mut WorldgenRandom) -> usize {
    r.next_bounded(size as i32) as usize
}

/// `StructurePlaceSettings`.
pub(crate) struct PlaceSettings<'r> {
    pub mirror: Mirror,
    pub rotation: Rotation,
    pub pivot: Pos,
    pub bbox: Option<BoundingBox>,
    pub ignore_entities: bool,
    /// `LiquidSettings.APPLY_WATERLOGGING`.
    pub waterlog: bool,
    pub processors: Vec<Arc<Processor>>,
    /// `setRandom`; without one, randoms are seeded from positions.
    pub random: Option<&'r mut WorldgenRandom>,
}

impl Default for PlaceSettings<'_> {
    fn default() -> Self {
        Self { mirror: Mirror::None, rotation: Rotation::None, pivot: Pos::new(0, 0, 0), bbox: None, ignore_entities: false, waterlog: true, processors: Vec::new(), random: None }
    }
}

impl PlaceSettings<'_> {
    /// `getRandom(pos)`, handed to `f`.
    pub fn with_random<T>(&mut self, p: Pos, f: impl FnOnce(&mut WorldgenRandom) -> T) -> T {
        match self.random.as_deref_mut() {
            Some(r) => f(r),
            None => f(&mut WorldgenRandom::legacy(position_seed(p.x, p.y, p.z))),
        }
    }

    fn palette(&mut self, size: usize, position: Pos) -> usize {
        self.with_random(position, |r| palette_index(size, r))
    }

    /// `calculateRelativePosition(settings, pos).offset(position)`.
    pub fn world_pos(&self, local: Pos, position: Pos) -> Pos {
        let p = transform_pos(local, self.mirror, self.rotation, self.pivot);
        Pos::new(p.x + position.x, p.y + position.y, p.z + position.z)
    }
}

/// `StructureTemplate.processBlockInfos`.
pub(crate) fn process_block_infos(w: &World, position: Pos, reference: Pos, settings: &mut PlaceSettings, list: &[BlockInfo]) -> Vec<Info> {
    let only_in_chunk = !settings.processors.iter().any(|p| p.evaluates_entire_piece());
    let processors = settings.processors.clone();
    let mut original: Vec<Pos> = Vec::new();
    let mut processed: Vec<Info> = Vec::new();
    for b in list {
        let p = settings.world_pos(b.pos, position);
        if only_in_chunk && settings.bbox.is_some_and(|bb| !bb.is_inside(p)) {
            continue;
        }
        let mut info = Some(Info { pos: p, state: b.state, nbt: b.nbt.clone() });
        for proc in &processors {
            let Some(i) = info else { break };
            info = proc.process(w, position, reference, b.pos, i, settings);
        }
        if let Some(i) = info {
            processed.push(i);
            original.push(b.pos);
        }
    }
    for proc in &processors {
        processed = proc.finalize(w, position, reference, &original, processed, settings);
    }
    processed
}

/// The templates of a dimension, read and parsed when the generator is built.
#[derive(Default)]
pub(crate) struct Templates {
    map: HashMap<String, Arc<Template>>,
}

impl Templates {
    /// Reads and parses `data/<ns>/structure/<path>.nbt`.
    pub fn load(&mut self, data: &DataReader, id: &str) -> Result<Arc<Template>> {
        let id = crate::data::full_id(id);
        if let Some(t) = self.map.get(&id) {
            return Ok(t.clone());
        }
        let (ns, path) = split_id(&id);
        let file = format!("data/{ns}/structure/{path}.nbt");
        // `StructureTemplateManager.getOrCreate`: a missing template is an empty one.
        let t = match data.bytes(&file) {
            Some(bytes) => {
                let mut t = Template::parse(&bytes).with_context(|| format!("parsing {file}"))?;
                t.id = id.clone();
                Arc::new(t)
            }
            None => Arc::new(Template { id: id.clone(), size: (0, 0, 0), palettes: Vec::new(), entities: Vec::new() }),
        };
        self.map.insert(id, t.clone());
        Ok(t)
    }

}
