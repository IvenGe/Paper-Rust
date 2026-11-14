//! Jigsaw structures: template pools and their elements, pool aliases, and the jigsaw
//! assembly (`JigsawStructure`, `JigsawPlacement`, `PoolElementStructurePiece`).
//!
//! Written to vanilla 26.3's `JigsawStructure.findGenerationPoint`, `JigsawPlacement.addPieces`
//! and `Placer.tryPlacingChildren`, `StructureTemplatePool`, the pool elements and
//! `SequencedPriorityIterator`. The free space vanilla tracks as voxel shapes is a box minus
//! the boxes placed in it (all boxes are block aligned, so this is exact).

use super::bbox::BoundingBox;
use super::legacy::rotation_name;
use super::persist::{get_i32, get_str, json_to_nbt, nbt_to_json, read_base, value_key};
use super::piece::{PieceBase, PlaceCtx};
use super::processor::{BuiltinProcessors, Processor, ProcessorLists};
use super::template::{JigsawBlock, PlaceSettings, Template, Templates};
use super::transform::Rotation;
use super::{GenCtx, JigsawInfo, Junction, Piece, StructureGen};
use crate::chunk::Heightmap;
use crate::data::{full_id, DataReader, JsonExt};
use crate::feature::parse::Parser;
use crate::feature::placement::PlacedFeature;
use crate::feature::world::{Dir, Pos};
use crate::generator::WorldGenerator;
use crate::providers::{GenContext, HeightProvider};
use crate::random::{Random, WorldgenRandom};
use anyhow::{anyhow, bail, Result};
use paper_nbt::{Compound, Tag};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

const EMPTY_POOL: &str = "minecraft:empty";

/// `StructurePoolElement`.
pub(crate) enum ElementKind {
    Single { location: Arc<str>, template: Arc<Template>, processors: Arc<Vec<Arc<Processor>>>, legacy: bool, waterlog: Option<bool> },
    List(Vec<Arc<Element>>),
    Feature(Arc<PlacedFeature>),
    Empty,
}

pub(crate) struct Element {
    pub kind: ElementKind,
    /// `Projection.RIGID` (otherwise `TERRAIN_MATCHING`).
    pub rigid: bool,
    /// The element as the pool encodes it (saved with the pieces as `pool_element`).
    pub json: Value,
}

/// Sorts jigsaw blocks by selection priority, highest first (a stable sort, as `List.sort`).
fn shuffle_and_sort(mut v: Vec<JigsawBlock>, r: &mut WorldgenRandom) -> Vec<JigsawBlock> {
    shuffle(&mut v, r);
    v.sort_by_key(|j| std::cmp::Reverse(j.selection_priority));
    v
}

/// `Util.shuffle`.
pub(crate) fn shuffle<T>(v: &mut [T], r: &mut WorldgenRandom) {
    for i in (2..=v.len()).rev() {
        let j = r.next_bounded(i as i32) as usize;
        v.swap(i - 1, j);
    }
}

impl Element {
    pub fn is_empty(&self) -> bool {
        matches!(self.kind, ElementKind::Empty)
    }

    /// `getShuffledJigsawBlocks`.
    fn shuffled_jigsaws(&self, position: Pos, rotation: Rotation, r: &mut WorldgenRandom) -> Vec<JigsawBlock> {
        match &self.kind {
            ElementKind::Single { template, .. } => shuffle_and_sort(template.jigsaws(position, rotation), r),
            ElementKind::List(elements) => elements[0].shuffled_jigsaws(position, rotation, r),
            ElementKind::Feature(_) => {
                let empty: Arc<str> = EMPTY_POOL.into();
                vec![JigsawBlock {
                    pos: position,
                    front: Dir::Down,
                    top: Dir::South,
                    rollable: true,
                    // No name: attaches to any target.
                    name: "".into(),
                    pool: empty.clone(),
                    target: empty,
                    placement_priority: 0,
                    selection_priority: 0,
                }]
            }
            ElementKind::Empty => Vec::new(),
        }
    }

    /// `getBoundingBox(manager, position, rotation)`.
    fn bounding_box(&self, position: Pos, rotation: Rotation) -> BoundingBox {
        match &self.kind {
            ElementKind::Single { template, .. } => template.bounding_box(position, rotation, Pos::new(0, 0, 0), super::transform::Mirror::None),
            ElementKind::List(elements) => BoundingBox::encapsulating(elements.iter().filter(|e| !e.is_empty()).map(|e| e.bounding_box(position, rotation))).unwrap_or(BoundingBox::single(position)),
            ElementKind::Feature(_) | ElementKind::Empty => BoundingBox::single(position),
        }
    }

    /// `place(...)` with `keepJigsaws` false.
    fn place(&self, ctx: &mut PlaceCtx, position: Pos, rotation: Rotation, waterlog: bool, builtins: &BuiltinProcessors) -> bool {
        match &self.kind {
            ElementKind::Single { template, processors, legacy, waterlog: over, .. } => {
                let mut list = Vec::with_capacity(processors.len() + 3);
                if !legacy {
                    list.push(builtins.structure_block.clone());
                }
                list.push(builtins.jigsaw_replacement.clone());
                list.extend(processors.iter().cloned());
                if !self.rigid {
                    list.push(builtins.gravity.clone());
                }
                if *legacy {
                    list.push(builtins.structure_and_air.clone());
                }
                let mut settings = PlaceSettings {
                    rotation,
                    bbox: Some(ctx.chunk_box),
                    waterlog: over.unwrap_or(waterlog),
                    processors: list,
                    ..PlaceSettings::default()
                };
                template.place(ctx.w, position, ctx.reference, &mut settings, ctx.r)
            }
            ElementKind::List(elements) => {
                for e in elements {
                    if !e.place(ctx, position, rotation, waterlog, builtins) {
                        return false;
                    }
                }
                true
            }
            ElementKind::Feature(f) => f.place(ctx.w, ctx.r, position),
            ElementKind::Empty => true,
        }
    }

    fn location(&self) -> Option<&str> {
        match &self.kind {
            ElementKind::Single { location, .. } => Some(location),
            ElementKind::List(e) => e.first().and_then(|e| e.location()),
            _ => None,
        }
    }
}

/// `StructureTemplatePool`: the elements repeated by weight.
pub(crate) struct Pool {
    elements: Vec<Arc<Element>>,
    fallback: Arc<str>,
    /// `getMaxSize`: the tallest element.
    max_size: i32,
}

impl Pool {
    fn random(&self, r: &mut WorldgenRandom) -> Option<&Arc<Element>> {
        if self.elements.is_empty() {
            return None;
        }
        Some(&self.elements[r.next_bounded(self.elements.len() as i32) as usize])
    }
}

/// The template pools of a dimension.
#[derive(Default)]
pub(crate) struct Pools {
    pools: HashMap<Arc<str>, Pool>,
    /// The elements of all pools by their encoded form (`value_key`), for pieces read back.
    by_key: OnceLock<HashMap<String, Arc<Element>>>,
}

impl Pools {
    fn get(&self, name: &str) -> Option<&Pool> {
        self.pools.get(name)
    }

    /// The element a saved `pool_element` encodes.
    fn element(&self, encoded: &Value) -> Option<Arc<Element>> {
        let index = self.by_key.get_or_init(|| {
            let mut m = HashMap::new();
            for pool in self.pools.values() {
                for e in &pool.elements {
                    m.entry(value_key(&e.json)).or_insert_with(|| e.clone());
                }
            }
            m
        });
        index.get(&value_key(encoded)).cloned()
    }
}

/// Collects the pools the jigsaw structures reach while the generator is built.
#[derive(Default)]
pub(crate) struct PoolLoader {
    queue: Vec<String>,
    cell: Arc<OnceLock<Pools>>,
}

impl PoolLoader {
    fn want(&mut self, name: &str) {
        self.queue.push(full_id(name));
    }

    /// Loads every queued pool and the pools their elements' jigsaw blocks name.
    pub fn finish(&mut self, data: &DataReader, parser: &mut Parser, procs: &mut ProcessorLists, templates: &mut Templates) -> Result<()> {
        let mut pools: HashMap<Arc<str>, Pool> = HashMap::new();
        let mut missing: Vec<String> = Vec::new();
        self.queue.push(EMPTY_POOL.to_owned());
        while let Some(name) = self.queue.pop() {
            if pools.contains_key(name.as_str()) || missing.contains(&name) {
                continue;
            }
            let path = {
                let (ns, p) = crate::data::split_id(&name);
                format!("data/{ns}/worldgen/template_pool/{p}.json")
            };
            if !data.exists(&path) {
                missing.push(name);
                continue;
            }
            let json = data.json(&path)?;
            let fallback = full_id(json.get("fallback").and_then(Value::as_str).unwrap_or(EMPTY_POOL));
            let mut elements = Vec::new();
            for e in json.get("elements").and_then(Value::as_array).into_iter().flatten() {
                let weight = e.i64_or("weight", 1).max(0) as usize;
                let element = Arc::new(parse_element(e.field("element")?, None, data, parser, procs, templates).map_err(|err| err.context(format!("pool {name}")))?);
                collect_pools(&element, &mut self.queue);
                for _ in 0..weight {
                    elements.push(element.clone());
                }
            }
            let max_size = elements.iter().filter(|e| !e.is_empty()).map(|e| e.bounding_box(Pos::new(0, 0, 0), Rotation::None).y_span()).max().unwrap_or(0);
            self.queue.push(fallback.clone());
            pools.insert(name.into(), Pool { elements, fallback: fallback.into(), max_size });
        }
        let _ = self.cell.set(Pools { pools, by_key: OnceLock::new() });
        Ok(())
    }
}

fn collect_pools(e: &Element, out: &mut Vec<String>) {
    match &e.kind {
        ElementKind::Single { template, .. } => {
            for p in &template.palettes {
                for j in &p.jigsaws {
                    out.push(j.pool.to_string());
                }
            }
        }
        ElementKind::List(l) => l.iter().for_each(|e| collect_pools(e, out)),
        _ => {}
    }
}

fn parse_element(v: &Value, projection: Option<bool>, data: &DataReader, parser: &mut Parser, procs: &mut ProcessorLists, templates: &mut Templates) -> Result<Element> {
    let t = v.str_field("element_type")?;
    let t = t.strip_prefix("minecraft:").unwrap_or(t);
    let rigid = projection.unwrap_or_else(|| v.get("projection").and_then(Value::as_str) != Some("terrain_matching"));
    let kind = match t {
        "single_pool_element" | "legacy_single_pool_element" => {
            let location = full_id(v.str_field("location")?);
            let template = templates.load(data, &location)?;
            let processors = match v.get("processors") {
                Some(p) => procs.list(parser, p)?,
                None => Arc::new(Vec::new()),
            };
            let waterlog = v.get("override_liquid_settings").and_then(Value::as_str).map(|s| s == "apply_waterlogging");
            ElementKind::Single { location: location.into(), template, processors, legacy: t.starts_with("legacy"), waterlog }
        }
        "list_pool_element" => {
            let list = v.field("elements")?.as_array().ok_or_else(|| anyhow!("bad list element"))?;
            if list.is_empty() {
                bail!("list pool element without elements");
            }
            ElementKind::List(list.iter().map(|e| parse_element(e, Some(rigid), data, parser, procs, templates).map(Arc::new)).collect::<Result<_>>()?)
        }
        "feature_pool_element" => ElementKind::Feature(parser.placed(v.field("feature")?)?),
        "empty_pool_element" => ElementKind::Empty,
        other => bail!("unknown pool element {other}"),
    };
    Ok(Element { kind, rigid, json: v.clone() })
}

/// `PoolAliasBinding`.
enum Alias {
    Direct(Arc<str>, Arc<str>),
    Random(Arc<str>, Vec<(Arc<str>, i32)>),
    Group(Vec<(Vec<Alias>, i32)>),
}

fn weighted<'a, T>(items: &'a [(T, i32)], r: &mut WorldgenRandom) -> Option<&'a T> {
    let total: i32 = items.iter().map(|(_, w)| *w).sum();
    if total <= 0 {
        return None;
    }
    let mut i = r.next_bounded(total);
    for (item, w) in items {
        i -= w;
        if i < 0 {
            return Some(item);
        }
    }
    None
}

impl Alias {
    fn parse(v: &Value) -> Result<Self> {
        let t = v.str_field("type")?;
        Ok(match t.strip_prefix("minecraft:").unwrap_or(t) {
            "direct" => Alias::Direct(full_id(v.str_field("alias")?).into(), full_id(v.str_field("target")?).into()),
            "random" => Alias::Random(
                full_id(v.str_field("alias")?).into(),
                v.field("targets")?.as_array().ok_or_else(|| anyhow!("bad alias targets"))?.iter().map(|e| Ok((full_id(e.str_field("data")?).into(), e.i64_or("weight", 1) as i32))).collect::<Result<_>>()?,
            ),
            "random_group" => Alias::Group(
                v.field("groups")?
                    .as_array()
                    .ok_or_else(|| anyhow!("bad alias groups"))?
                    .iter()
                    .map(|g| Ok((g.field("data")?.as_array().ok_or_else(|| anyhow!("bad alias group"))?.iter().map(Alias::parse).collect::<Result<_>>()?, g.i64_or("weight", 1) as i32)))
                    .collect::<Result<_>>()?,
            ),
            other => bail!("unknown pool alias {other}"),
        })
    }

    fn targets(&self, out: &mut Vec<String>) {
        match self {
            Alias::Direct(_, t) => out.push(t.to_string()),
            Alias::Random(_, ts) => out.extend(ts.iter().map(|(t, _)| t.to_string())),
            Alias::Group(gs) => gs.iter().for_each(|(g, _)| g.iter().for_each(|a| a.targets(out))),
        }
    }

    fn resolve(&self, r: &mut WorldgenRandom, out: &mut HashMap<Arc<str>, Arc<str>>) {
        match self {
            Alias::Direct(a, t) => {
                out.insert(a.clone(), t.clone());
            }
            Alias::Random(a, ts) => {
                if let Some(t) = weighted(ts, r) {
                    out.insert(a.clone(), t.clone());
                }
            }
            Alias::Group(gs) => {
                if let Some(g) = weighted(gs, r) {
                    for a in g {
                        a.resolve(r, out);
                    }
                }
            }
        }
    }
}

/// A configured `JigsawStructure`.
pub(crate) struct Jigsaw {
    start_pool: Arc<str>,
    start_jigsaw: Option<Arc<str>>,
    max_depth: i32,
    start_height: HeightProvider,
    expansion_hack: bool,
    project_to: Option<Heightmap>,
    max_h: i32,
    max_v: i32,
    aliases: Vec<Alias>,
    padding_bottom: i32,
    padding_top: i32,
    waterlog: bool,
    pools: Arc<OnceLock<Pools>>,
    builtins: Arc<BuiltinProcessors>,
}

impl Jigsaw {
    pub fn parse(v: &Value, pools: &mut PoolLoader, builtins: Arc<BuiltinProcessors>) -> Result<Self> {
        let start_pool = full_id(v.str_field("start_pool")?);
        pools.want(&start_pool);
        let aliases: Vec<Alias> = v.get("pool_aliases").and_then(Value::as_array).into_iter().flatten().map(Alias::parse).collect::<Result<_>>()?;
        let mut targets = Vec::new();
        for a in &aliases {
            a.targets(&mut targets);
        }
        for t in targets {
            pools.want(&t);
        }
        let (max_h, max_v) = match v.field("max_distance_from_center")? {
            Value::Object(_) => {
                let d = v.field("max_distance_from_center")?;
                (d.i64_or("horizontal", 80) as i32, d.i64_or("vertical", 4064) as i32)
            }
            d => {
                let n = d.as_i64().ok_or_else(|| anyhow!("bad max_distance_from_center"))? as i32;
                (n, n)
            }
        };
        let (padding_bottom, padding_top) = match v.get("dimension_padding") {
            None => (0, 0),
            Some(Value::Object(_)) => {
                let d = v.field("dimension_padding")?;
                (d.i64_or("bottom", 0) as i32, d.i64_or("top", 0) as i32)
            }
            Some(d) => {
                let n = d.as_i64().unwrap_or(0) as i32;
                (n, n)
            }
        };
        Ok(Self {
            start_pool: start_pool.into(),
            start_jigsaw: v.get("start_jigsaw_name").and_then(Value::as_str).map(|s| full_id(s).into()),
            max_depth: v.i64_or("size", 0) as i32,
            start_height: HeightProvider::parse(v.field("start_height")?)?,
            expansion_hack: v.get("use_expansion_hack").and_then(Value::as_bool).unwrap_or(false),
            project_to: v.get("project_start_to_heightmap").and_then(Value::as_str).and_then(Heightmap::parse),
            max_h,
            max_v,
            aliases,
            padding_bottom,
            padding_top,
            waterlog: v.get("liquid_settings").and_then(Value::as_str) != Some("ignore_waterlogging"),
            pools: pools.cell.clone(),
            builtins,
        })
    }
}

/// The free space a piece may be placed in: a box minus the boxes already placed in it.
struct Free {
    base: BoundingBox,
    holes: Vec<BoundingBox>,
}

impl Free {
    fn fits(&self, b: &BoundingBox) -> bool {
        let base = &self.base;
        b.min_x >= base.min_x && b.min_y >= base.min_y && b.min_z >= base.min_z && b.max_x <= base.max_x && b.max_y <= base.max_y && b.max_z <= base.max_z && !self.holes.iter().any(|h| h.intersects(b))
    }
}

struct PieceState {
    piece: usize,
    free: usize,
    depth: i32,
}

/// `JigsawBlock.canAttach` (an empty target name is vanilla's `null`, which matches any
/// target).
fn can_attach(source: &JigsawBlock, target: &JigsawBlock) -> bool {
    source.front == target.front.opposite() && (source.rollable || source.top == target.top) && (target.name.is_empty() || source.target == target.name)
}

impl StructureGen for Jigsaw {
    /// `PoolElementStructurePiece`'s NBT constructor: the element is found among the pools'
    /// elements by its encoding.
    fn load_pieces(&self, _gen: &WorldGenerator, _x: i32, _z: i32, children: &[&Compound]) -> Result<Vec<Box<dyn Piece>>> {
        let pools = self.pools.get().ok_or_else(|| anyhow!("template pools not loaded"))?;
        children
            .iter()
            .map(|t| {
                let base = read_base(t, &["minecraft:jigsaw"])?;
                let encoded = nbt_to_json(t.get("pool_element").ok_or_else(|| anyhow!("jigsaw piece without pool_element"))?);
                let element = pools.element(&encoded).ok_or_else(|| anyhow!("unknown pool element {encoded}"))?;
                let position = Pos::new(get_i32(t, "PosX"), get_i32(t, "PosY"), get_i32(t, "PosZ"));
                let rotation = Rotation::parse(get_str(t, "rotation")).unwrap_or_default();
                let waterlog = match t.string("liquid_settings") {
                    Some(s) => s != "ignore_waterlogging",
                    None => self.waterlog,
                };
                let mut piece = JigsawPiece::new(element, position, get_i32(t, "ground_level_delta"), rotation, base.bbox, waterlog, self.builtins.clone());
                piece.base.gen_depth = base.gen_depth;
                for j in t.list("junctions").unwrap_or(&[]).iter().filter_map(Tag::as_compound) {
                    piece.info.junctions.push(Junction {
                        source_x: get_i32(j, "source_x"),
                        source_ground_y: get_i32(j, "source_ground_y"),
                        source_z: get_i32(j, "source_z"),
                        delta_y: get_i32(j, "delta_y"),
                        dest_rigid: get_str(j, "dest_proj") != "terrain_matching",
                    });
                }
                Ok(Box::new(piece) as Box<dyn Piece>)
            })
            .collect()
    }

    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        let pools = self.pools.get()?;
        let gen = ctx.gen;
        let gctx = GenContext { min_y: gen.min_y.max(gen.gen_min_y), height: gen.height.min(gen.gen_height), sea_level: gen.sea_level };
        let height = self.start_height.sample(&mut ctx.r, &gctx);
        let start = Pos::new(ctx.min_block_x(), height, ctx.min_block_z());
        let mut aliases: HashMap<Arc<str>, Arc<str>> = HashMap::new();
        if !self.aliases.is_empty() {
            let mut r = WorldgenRandom::legacy(ctx.seed);
            let f = r.fork_positional();
            let mut r = WorldgenRandom::new(f.at(start.x, start.y, start.z));
            for a in &self.aliases {
                a.resolve(&mut r, &mut aliases);
            }
        }
        let lookup = |name: &Arc<str>| -> Arc<str> { aliases.get(name).cloned().unwrap_or_else(|| name.clone()) };
        let rotation = Rotation::random(&mut ctx.r);
        let center_pool = pools.get(&lookup(&self.start_pool)).or_else(|| pools.get(&self.start_pool))?;
        let element = center_pool.random(&mut ctx.r)?.clone();
        if element.is_empty() {
            return None;
        }
        let anchored = match &self.start_jigsaw {
            Some(name) => element.shuffled_jigsaws(start, rotation, &mut ctx.r).into_iter().find(|j| j.name == *name)?.pos,
            None => start,
        };
        let local = Pos::new(anchored.x - start.x, anchored.y - start.y, anchored.z - start.z);
        let adjusted = Pos::new(start.x - local.x, start.y - local.y, start.z - local.z);
        let bbox = element.bounding_box(adjusted, rotation);
        let mut center = JigsawPiece::new(element, adjusted, 1, rotation, bbox, self.waterlog, self.builtins.clone());
        let center_x = (bbox.max_x + bbox.min_x) / 2;
        let center_z = (bbox.max_z + bbox.min_z) / 2;
        let bottom_y = match self.project_to {
            None => adjusted.y,
            Some(hm) => {
                if !ctx.could_exist_in_column(center_x, center_z, ctx.min_y(), ctx.max_y()) {
                    return None;
                }
                start.y + ctx.cols.first_free(center_x, center_z, hm)
            }
        };
        center.translate(0, bottom_y - (bbox.min_y + center.info.ground_level_delta), 0);
        let center_bb = center.base.bbox;
        if (self.padding_bottom, self.padding_top) != (0, 0) {
            let b = center.base.bbox;
            if b.min_y < ctx.min_y() + self.padding_bottom || b.max_y > ctx.max_y() - self.padding_top {
                return None;
            }
        }
        let center_y = bottom_y + local.y;
        if !ctx.valid_biome(Pos::new(center_x, center_y, center_z)) || self.max_depth <= 0 {
            return None;
        }
        let lo_y = (center_y - self.max_v).max(ctx.min_y() + self.padding_bottom);
        let hi_y = (center_y + self.max_v + 1).min(ctx.max_y() + 1 - self.padding_top) - 1;
        let base = BoundingBox::new(center_x - self.max_h, lo_y, center_z - self.max_h, center_x + self.max_h, hi_y, center_z + self.max_h);
        let mut placer = Placer {
            pools,
            lookup: &lookup,
            max_depth: self.max_depth,
            expansion_hack: self.expansion_hack,
            pieces: vec![center],
            frees: vec![Free { base, holes: vec![center_bb] }],
            placing: BTreeMap::new(),
            waterlog: self.waterlog,
            builtins: &self.builtins,
        };
        placer.try_placing_children(ctx, 0, 0, 0);
        while let Some(state) = placer.next() {
            placer.try_placing_children(ctx, state.piece, state.free, state.depth);
        }
        Some(placer.pieces.into_iter().map(|p| Box::new(p) as Box<dyn Piece>).collect())
    }
}

struct Placer<'a> {
    pools: &'a Pools,
    lookup: &'a dyn Fn(&Arc<str>) -> Arc<str>,
    max_depth: i32,
    expansion_hack: bool,
    pieces: Vec<JigsawPiece>,
    frees: Vec<Free>,
    /// `SequencedPriorityIterator`: the highest priority first, in insertion order.
    placing: BTreeMap<i32, VecDeque<PieceState>>,
    waterlog: bool,
    builtins: &'a Arc<BuiltinProcessors>,
}

impl Placer<'_> {
    fn next(&mut self) -> Option<PieceState> {
        let mut entry = self.placing.last_entry()?;
        let s = entry.get_mut().pop_front();
        if entry.get().is_empty() {
            entry.remove();
        }
        s
    }

    fn max_size(&self, pool: &str) -> i32 {
        self.pools.get(pool).map_or(0, |p| p.max_size)
    }

    /// `tryPlacingChildren`.
    fn try_placing_children(&mut self, ctx: &mut GenCtx, source: usize, context_free: usize, depth: i32) {
        let (element, position, rotation, source_bb) = {
            let p = &self.pieces[source];
            (p.element.clone(), p.position, p.rotation, p.base.bbox)
        };
        let source_rigid = element.rigid;
        let source_box_y = source_bb.min_y;
        let mut source_free: Option<usize> = None;
        for sj in element.shuffled_jigsaws(position, rotation, &mut ctx.r) {
            let dir = sj.front;
            let target_pos = sj.pos.rel(dir);
            let sj_local_y = sj.pos.y - source_box_y;
            let mut base_height = i32::MIN;
            let pool_name = (self.lookup)(&sj.pool);
            let Some(pool) = self.pools.get(&pool_name) else { continue };
            if pool.elements.is_empty() && &*pool_name != EMPTY_POOL {
                continue;
            }
            let Some(fallback) = self.pools.get(&pool.fallback) else { continue };
            if fallback.elements.is_empty() && &*pool.fallback != EMPTY_POOL {
                continue;
            }
            let children_free = if source_bb.is_inside(target_pos) {
                *source_free.get_or_insert_with(|| {
                    self.frees.push(Free { base: source_bb, holes: Vec::new() });
                    self.frees.len() - 1
                })
            } else {
                context_free
            };
            let mut targets: Vec<Arc<Element>> = Vec::new();
            if depth != self.max_depth {
                let mut v = pool.elements.clone();
                shuffle(&mut v, &mut ctx.r);
                targets.extend(v);
            }
            let mut v = fallback.elements.clone();
            shuffle(&mut v, &mut ctx.r);
            targets.extend(v);
            let priority = sj.placement_priority;
            'elements: for te in targets {
                if te.is_empty() {
                    break;
                }
                for rot in Rotation::shuffled(&mut ctx.r) {
                    let tjs = te.shuffled_jigsaws(Pos::new(0, 0, 0), rot, &mut ctx.r);
                    let hack_box = te.bounding_box(Pos::new(0, 0, 0), rot);
                    let expand_to = if self.expansion_hack && hack_box.y_span() <= 16 {
                        tjs.iter()
                            .map(|tj| {
                                if !hack_box.is_inside(tj.pos.rel(tj.front)) {
                                    return 0;
                                }
                                let child = (self.lookup)(&tj.pool);
                                let fallback = self.pools.get(&child).map(|p| p.fallback.clone());
                                self.max_size(&child).max(fallback.map_or(0, |f| self.max_size(&f)))
                            })
                            .max()
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    for tj in &tjs {
                        if !can_attach(&sj, tj) {
                            continue;
                        }
                        let raw_pos = Pos::new(target_pos.x - tj.pos.x, target_pos.y - tj.pos.y, target_pos.z - tj.pos.z);
                        let raw_bb = te.bounding_box(raw_pos, rot);
                        let target_rigid = te.rigid;
                        let tj_local_y = tj.pos.y;
                        let delta_y = sj_local_y - tj_local_y + dir.offset().1;
                        let target_box_y = if source_rigid && target_rigid {
                            source_box_y + delta_y
                        } else {
                            if base_height == i32::MIN {
                                base_height = ctx.cols.first_free(sj.pos.x, sj.pos.z, Heightmap::WorldSurfaceWg);
                            }
                            base_height - tj_local_y
                        };
                        let y_offset = target_box_y - raw_bb.min_y;
                        let mut target_bb = raw_bb.moved(0, y_offset, 0);
                        let target_box_pos = Pos::new(raw_pos.x, raw_pos.y + y_offset, raw_pos.z);
                        if expand_to > 0 {
                            let size = (expand_to + 1).max(target_bb.max_y - target_bb.min_y);
                            target_bb.encapsulate_pos(Pos::new(target_bb.min_x, target_bb.min_y + size, target_bb.min_z));
                        }
                        if !self.frees[children_free].fits(&target_bb) {
                            continue;
                        }
                        self.frees[children_free].holes.push(target_bb);
                        let source_gld = self.pieces[source].info.ground_level_delta;
                        let target_gld = if target_rigid { source_gld - delta_y } else { 1 };
                        let mut piece = JigsawPiece::new(te.clone(), target_box_pos, target_gld, rot, target_bb, self.waterlog, self.builtins.clone());
                        let junction_y = if source_rigid {
                            source_box_y + sj_local_y
                        } else if target_rigid {
                            target_box_y + tj_local_y
                        } else {
                            if base_height == i32::MIN {
                                base_height = ctx.cols.first_free(sj.pos.x, sj.pos.z, Heightmap::WorldSurfaceWg);
                            }
                            base_height + delta_y / 2
                        };
                        self.pieces[source].info.junctions.push(Junction {
                            source_x: target_pos.x,
                            source_ground_y: junction_y - sj_local_y + source_gld,
                            source_z: target_pos.z,
                            delta_y,
                            dest_rigid: target_rigid,
                        });
                        piece.info.junctions.push(Junction { source_x: sj.pos.x, source_ground_y: junction_y - tj_local_y + target_gld, source_z: sj.pos.z, delta_y: -delta_y, dest_rigid: source_rigid });
                        self.pieces.push(piece);
                        if depth < self.max_depth {
                            self.placing.entry(priority).or_default().push_back(PieceState { piece: self.pieces.len() - 1, free: children_free, depth: depth + 1 });
                        }
                        break 'elements;
                    }
                }
            }
        }
    }
}

/// `PoolElementStructurePiece`.
pub(crate) struct JigsawPiece {
    base: PieceBase,
    element: Arc<Element>,
    position: Pos,
    rotation: Rotation,
    info: JigsawInfo,
    waterlog: bool,
    builtins: Arc<BuiltinProcessors>,
}

impl JigsawPiece {
    fn new(element: Arc<Element>, position: Pos, ground_level_delta: i32, rotation: Rotation, bbox: BoundingBox, waterlog: bool, builtins: Arc<BuiltinProcessors>) -> Self {
        let rigid = element.rigid;
        Self { base: PieceBase::new("minecraft:jigsaw", 0, bbox), element, position, rotation, info: JigsawInfo { rigid, ground_level_delta, junctions: Vec::new() }, waterlog, builtins }
    }

    /// The template of the piece's element (`location`), the position and the rotation.
    pub fn describe(&self) -> (Option<&str>, Pos, Rotation) {
        (self.element.location(), self.position, self.rotation)
    }
}

impl Piece for JigsawPiece {
    fn base(&self) -> &PieceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PieceBase {
        &mut self.base
    }

    fn post_process(&self, ctx: &mut PlaceCtx) {
        self.element.place(ctx, self.position, self.rotation, self.waterlog, &self.builtins);
    }

    fn jigsaw(&self) -> Option<&JigsawInfo> {
        Some(&self.info)
    }

    /// `PoolElementStructurePiece.addAdditionalSaveData`.
    fn save(&self, t: &mut Compound) {
        t.insert("PosX", self.position.x);
        t.insert("PosY", self.position.y);
        t.insert("PosZ", self.position.z);
        t.insert("ground_level_delta", self.info.ground_level_delta);
        t.insert("pool_element", json_to_nbt(&self.element.json));
        t.insert("rotation", rotation_name(self.rotation));
        let junctions = self
            .info
            .junctions
            .iter()
            .map(|j| {
                let mut c = Compound::new();
                c.insert("source_x", j.source_x);
                c.insert("source_ground_y", j.source_ground_y);
                c.insert("source_z", j.source_z);
                c.insert("delta_y", j.delta_y);
                c.insert("dest_proj", if j.dest_rigid { "rigid" } else { "terrain_matching" });
                Tag::Compound(c)
            })
            .collect();
        t.insert("junctions", Tag::List(junctions));
        t.insert("liquid_settings", if self.waterlog { "apply_waterlogging" } else { "ignore_waterlogging" });
    }

    fn translate(&mut self, dx: i32, dy: i32, dz: i32) {
        self.base.bbox.move_by(dx, dy, dz);
        self.position = Pos::new(self.position.x + dx, self.position.y + dy, self.position.z + dz);
    }

    fn jigsaw_details(&self) -> Option<(Option<&str>, Pos, Rotation)> {
        Some(self.describe())
    }
}
