//! The template structures that are not jigsaw structures: shipwrecks, ocean ruins and
//! nether fossils, on a common `TemplateStructurePiece`.
//!
//! Written to vanilla 26.3's `TemplateStructurePiece`, `ShipwreckStructure`/`ShipwreckPieces`,
//! `OceanRuinStructure`/`OceanRuinPieces` and `NetherFossilStructure`/`NetherFossilPieces`.
//! Pieces that settle their height when first placed (shipwrecks) keep the settled height in
//! the shared start; ocean ruins settle it again for every chunk, as vanilla does.

use super::bbox::BoundingBox;
use super::piece::{PieceBase, PlaceCtx};
use super::processor::Processor;
use super::template::{block_entity_kind, PlaceSettings, Template};
use super::transform::{transform_pos, Mirror, Rotation};
use super::{GenCtx, Loader, Piece, StructureGen};
use crate::chunk::Heightmap;
use crate::data::JsonExt;
use crate::feature::world::{Dir, Pos};
use crate::providers::{GenContext, HeightProvider};
use crate::random::{Random, WorldgenRandom};
use anyhow::Result;
use paper_data::blocks;
use paper_nbt::Compound;
use serde_json::Value;
use std::sync::{Arc, Mutex};

fn st(name: &str) -> u32 {
    blocks::default_state(name).unwrap_or(0)
}

/// What a template piece does beyond placing its template.
pub(crate) enum TemplateKind {
    Shipwreck { beached: bool, too_big: bool },
    OceanRuin { large: bool },
    NetherFossil,
}

/// `TemplateStructurePiece`.
pub(crate) struct TemplatePiece {
    base: PieceBase,
    template: Arc<Template>,
    /// `templatePosition`, and whether its height has been settled.
    position: Mutex<(Pos, bool)>,
    rotation: Rotation,
    mirror: Mirror,
    pivot: Pos,
    processors: Vec<Arc<Processor>>,
    kind: TemplateKind,
}

impl TemplatePiece {
    fn new(kind_id: &'static str, template: Arc<Template>, position: Pos, rotation: Rotation, mirror: Mirror, pivot: Pos, processors: Vec<Arc<Processor>>, kind: TemplateKind) -> Self {
        let bbox = template.bounding_box(position, rotation, pivot, mirror);
        let base = PieceBase::new(kind_id, 0, bbox).with_orientation(Some(Dir::North));
        Self { base, template, position: Mutex::new((position, false)), rotation, mirror, pivot, processors, kind }
    }

    fn settings(&self, bbox: BoundingBox) -> PlaceSettings<'static> {
        PlaceSettings { mirror: self.mirror, rotation: self.rotation, pivot: self.pivot, bbox: Some(bbox), processors: self.processors.clone(), ..PlaceSettings::default() }
    }

    /// `TemplateStructurePiece.postProcess` at `position`.
    fn place_template(&self, ctx: &mut PlaceCtx, position: Pos, chunk_box: BoundingBox) {
        let mut settings = self.settings(chunk_box);
        let mut entities: Vec<(Pos, Compound)> = Vec::new();
        if self.template.place_buffered(ctx.w, position, ctx.reference, &mut settings, ctx.r, Some(&mut entities)) {
            let marker = blocks::id("minecraft:structure_block").unwrap_or(0);
            for m in self.template.filter_blocks(position, &mut settings, marker) {
                let Some(nbt) = &m.nbt else { continue };
                if nbt.string("mode").is_some_and(|s| s.eq_ignore_ascii_case("data")) {
                    let metadata = nbt.string("metadata").unwrap_or("").to_owned();
                    self.data_marker(&metadata, m.pos, ctx, &mut entities);
                }
            }
            let jigsaw = blocks::id("minecraft:jigsaw").unwrap_or(0);
            for j in self.template.filter_blocks(position, &mut settings, jigsaw) {
                if let Some(nbt) = &j.nbt {
                    ctx.w.set(j.pos, super::template::parse_state_string(nbt.string("final_state").unwrap_or("minecraft:air")));
                }
            }
        }
        for (p, nbt) in entities {
            ctx.w.add_block_entity_nbt(p, nbt);
        }
    }

    /// `handleDataMarker`.
    fn data_marker(&self, id: &str, pos: Pos, ctx: &mut PlaceCtx, entities: &mut Vec<(Pos, Compound)>) {
        match &self.kind {
            TemplateKind::Shipwreck { .. } => {
                let table = match id {
                    "map_chest" => "minecraft:chests/shipwreck_map",
                    "treasure_chest" => "minecraft:chests/shipwreck_treasure",
                    "supply_chest" => "minecraft:chests/shipwreck_supply",
                    _ => return,
                };
                set_loot(ctx, entities, pos.down(), table);
            }
            TemplateKind::OceanRuin { large } => {
                if id == "chest" {
                    let water = ctx.w.fluid(pos).is_water();
                    let chest = super::piece::with_bool(st("minecraft:chest"), "waterlogged", water);
                    ctx.w.set(pos, chest);
                    entities.retain(|(p, _)| *p != pos);
                    let table = if *large { "minecraft:chests/underwater_ruin_big" } else { "minecraft:chests/underwater_ruin_small" };
                    set_loot(ctx, entities, pos, table);
                } else if id == "drowned" {
                    let mut nbt = Compound::new();
                    nbt.insert("id", "minecraft:drowned");
                    nbt.insert("Pos", paper_nbt::Tag::List(vec![(pos.x as f64 + 0.5).into(), (pos.y as f64).into(), (pos.z as f64 + 0.5).into()]));
                    nbt.insert("Rotation", paper_nbt::Tag::List(vec![0.0f32.into(), 0.0f32.into()]));
                    nbt.insert("PersistenceRequired", true);
                    ctx.w.add_entity_nbt(nbt);
                    let state = if pos.y > ctx.w.sea_level { st("minecraft:air") } else { st("minecraft:water") };
                    ctx.w.set(pos, state);
                }
            }
            TemplateKind::NetherFossil => {}
        }
    }
}

/// `RandomizableContainer.setBlockEntityLootTable` on a block entity the template placed (or
/// on a fresh container): the loot table with a seed from the piece's random.
fn set_loot(ctx: &mut PlaceCtx, entities: &mut [(Pos, Compound)], pos: Pos, table: &str) {
    if block_entity_kind(ctx.w.get(pos)) != Some(true) {
        return;
    }
    let seed = ctx.r.next_i64();
    match entities.iter_mut().find(|(p, _)| *p == pos) {
        Some((_, nbt)) => {
            nbt.insert("LootTable", table);
            nbt.insert("LootTableSeed", seed);
        }
        None => ctx.w.add_block_entity(pos, crate::feature::BlockEntity::Loot { table: table.to_owned(), seed }),
    }
}

impl Piece for TemplatePiece {
    fn base(&self) -> &PieceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PieceBase {
        &mut self.base
    }

    fn translate(&mut self, dx: i32, dy: i32, dz: i32) {
        self.base.bbox.move_by(dx, dy, dz);
        let p = &mut self.position.get_mut().unwrap_or_else(|e| e.into_inner()).0;
        *p = Pos::new(p.x + dx, p.y + dy, p.z + dz);
    }

    fn post_process(&self, ctx: &mut PlaceCtx) {
        let chunk_box = ctx.chunk_box;
        match &self.kind {
            TemplateKind::Shipwreck { beached, too_big } => {
                let position = {
                    let mut guard = self.position.lock().unwrap_or_else(|e| e.into_inner());
                    if !guard.1 && !too_big {
                        let tp = guard.0;
                        let (sx, _, sz) = self.template.size;
                        let hm = if *beached { Heightmap::WorldSurfaceWg } else { Heightmap::OceanFloorWg };
                        let mut min_y = ctx.w.max_y() + 1;
                        let mean = if sx * sz == 0 {
                            ctx.w.height_at(hm, tp.x, tp.z)
                        } else {
                            let mut sum = 0;
                            for z in tp.z..tp.z + sz {
                                for x in tp.x..tp.x + sx {
                                    let h = ctx.w.height_at(hm, x, z);
                                    sum += h;
                                    min_y = min_y.min(h);
                                }
                            }
                            sum / (sx * sz)
                        };
                        let height = if *beached { min_y - self.template.size.1 / 2 - ctx.r.next_bounded(3) } else { mean };
                        *guard = (Pos::new(tp.x, height, tp.z), true);
                    }
                    guard.0
                };
                self.place_template(ctx, position, chunk_box);
            }
            TemplateKind::OceanRuin { .. } => {
                let position = {
                    let mut guard = self.position.lock().unwrap_or_else(|e| e.into_inner());
                    let tp = guard.0;
                    let tp = Pos::new(tp.x, ctx.w.height_at(Heightmap::OceanFloorWg, tp.x, tp.z), tp.z);
                    let c = transform_pos(Pos::new(self.template.size.0 - 1, 0, self.template.size.2 - 1), Mirror::None, self.rotation, Pos::new(0, 0, 0));
                    let corner = Pos::new(c.x + tp.x, c.y + tp.y, c.z + tp.z);
                    let tp = Pos::new(tp.x, ocean_ruin_height(ctx, tp, corner), tp.z);
                    guard.0 = tp;
                    tp
                };
                self.place_template(ctx, position, chunk_box);
            }
            TemplateKind::NetherFossil => {
                let position = self.position.lock().unwrap_or_else(|e| e.into_inner()).0;
                let fossil = self.template.bounding_box(position, self.rotation, self.pivot, self.mirror);
                let mut bbox = chunk_box;
                bbox.encapsulate(&fossil);
                self.place_template(ctx, position, bbox);
                // `placeDriedGhast`.
                let mut r = WorldgenRandom::legacy(ctx.w.seed());
                let f = r.fork_positional();
                let c = fossil.center();
                let mut r = WorldgenRandom::new(f.at(c.x, c.y, c.z));
                if r.next_f32() < 0.5 {
                    let x = fossil.min_x + r.next_bounded(fossil.x_span());
                    let y = fossil.min_y;
                    let z = fossil.min_z + r.next_bounded(fossil.z_span());
                    let p = Pos::new(x, y, z);
                    if ctx.w.is_air(p) && bbox.is_inside(p) {
                        let ghast = super::transform::rotate_state(st("minecraft:dried_ghast"), Rotation::random(&mut r));
                        ctx.w.set(p, ghast);
                    }
                }
            }
        }
    }
}

/// `OceanRuinPiece.getHeight`.
fn ocean_ruin_height(ctx: &PlaceCtx, pos: Pos, corner: Pos) -> i32 {
    let mut new_y = pos.y;
    let mut min_y = 512;
    let top_y = new_y - 1;
    let mut area = 0;
    let b = ctx.w.b;
    let ice = &b.tags.ice;
    for z in pos.z.min(corner.z)..=pos.z.max(corner.z) {
        for x in pos.x.min(corner.x)..=pos.x.max(corner.x) {
            let mut floor_y = pos.y - 1;
            loop {
                let s = ctx.w.get(Pos::new(x, floor_y, z));
                if !(b.is_air(s) || b.fluid(s).is_water() || ice.has(s)) || floor_y <= ctx.w.min_y + 1 {
                    break;
                }
                floor_y -= 1;
            }
            min_y = min_y.min(floor_y);
            if floor_y < top_y - 2 {
                area += 1;
            }
        }
    }
    let width = (pos.x - corner.x).abs();
    if top_y - min_y > 2 && area > width - 2 {
        new_y = min_y + 1;
    }
    new_y
}

/// `Util.getRandom(array, random)`.
fn pick<'a, T>(items: &'a [T], r: &mut WorldgenRandom) -> &'a T {
    &items[r.next_bounded(items.len() as i32) as usize]
}

fn templates(l: &mut Loader, names: &[&str]) -> Result<Vec<Arc<Template>>> {
    names.iter().map(|n| l.templates.load(l.data, &format!("minecraft:{n}"))).collect()
}

const SHIPWRECK_BEACHED: &[&str] = &[
    "shipwreck/with_mast", "shipwreck/sideways_full", "shipwreck/sideways_fronthalf", "shipwreck/sideways_backhalf", "shipwreck/rightsideup_full",
    "shipwreck/rightsideup_fronthalf", "shipwreck/rightsideup_backhalf", "shipwreck/with_mast_degraded", "shipwreck/rightsideup_full_degraded",
    "shipwreck/rightsideup_fronthalf_degraded", "shipwreck/rightsideup_backhalf_degraded",
];
const SHIPWRECK_OCEAN: &[&str] = &[
    "shipwreck/with_mast", "shipwreck/upsidedown_full", "shipwreck/upsidedown_fronthalf", "shipwreck/upsidedown_backhalf", "shipwreck/sideways_full",
    "shipwreck/sideways_fronthalf", "shipwreck/sideways_backhalf", "shipwreck/rightsideup_full", "shipwreck/rightsideup_fronthalf",
    "shipwreck/rightsideup_backhalf", "shipwreck/with_mast_degraded", "shipwreck/upsidedown_full_degraded", "shipwreck/upsidedown_fronthalf_degraded",
    "shipwreck/upsidedown_backhalf_degraded", "shipwreck/sideways_full_degraded", "shipwreck/sideways_fronthalf_degraded",
    "shipwreck/sideways_backhalf_degraded", "shipwreck/rightsideup_full_degraded", "shipwreck/rightsideup_fronthalf_degraded",
    "shipwreck/rightsideup_backhalf_degraded",
];

/// `ShipwreckStructure`.
pub(crate) struct Shipwreck {
    beached: bool,
    templates: Vec<Arc<Template>>,
    ignore: Arc<Processor>,
}

impl Shipwreck {
    pub fn load(l: &mut Loader, json: &Value) -> Result<Self> {
        let beached = json.get("is_beached").and_then(Value::as_bool).unwrap_or(false);
        let templates = templates(l, if beached { SHIPWRECK_BEACHED } else { SHIPWRECK_OCEAN })?;
        Ok(Self { beached, templates, ignore: l.builtins.structure_and_air.clone() })
    }
}

impl StructureGen for Shipwreck {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        let hm = if self.beached { Heightmap::WorldSurfaceWg } else { Heightmap::OceanFloorWg };
        let stub = ctx.on_top_of_chunk_center(hm)?;
        if !ctx.valid_biome(stub) {
            return None;
        }
        let rotation = Rotation::random(&mut ctx.r);
        let offset = Pos::new(ctx.min_block_x(), 90, ctx.min_block_z());
        let template = pick(&self.templates, &mut ctx.r).clone();
        let too_big = template.size.0 > 32 || template.size.1 > 32;
        let mut piece = TemplatePiece::new("minecraft:shipwreck", template, offset, rotation, Mirror::None, Pos::new(4, 0, 15), vec![self.ignore.clone()], TemplateKind::Shipwreck { beached: self.beached, too_big });
        if too_big {
            let bb = piece.base.bbox;
            let height = if self.beached {
                let min_y = ctx.cols.lowest_y(bb.min_x, bb.min_z, bb.x_span(), bb.z_span());
                min_y - piece.template.size.1 / 2 - ctx.r.next_bounded(3)
            } else {
                ctx.cols.mean_first_occupied(bb.min_x, bb.min_z, bb.x_span(), bb.z_span())
            };
            let p = piece.position.get_mut().unwrap_or_else(|e| e.into_inner());
            *p = (Pos::new(p.0.x, height, p.0.z), true);
        }
        Some(vec![Box::new(piece)])
    }
}

const WARM_RUINS: &[&str] = &[
    "underwater_ruin/warm_1", "underwater_ruin/warm_2", "underwater_ruin/warm_3", "underwater_ruin/warm_4", "underwater_ruin/warm_5", "underwater_ruin/warm_6",
    "underwater_ruin/warm_7", "underwater_ruin/warm_8",
];
const BIG_WARM_RUINS: &[&str] = &["underwater_ruin/big_warm_4", "underwater_ruin/big_warm_5", "underwater_ruin/big_warm_6", "underwater_ruin/big_warm_7"];
const COLD_RUINS: [&[&str]; 3] = [
    &[
        "underwater_ruin/brick_1", "underwater_ruin/brick_2", "underwater_ruin/brick_3", "underwater_ruin/brick_4", "underwater_ruin/brick_5",
        "underwater_ruin/brick_6", "underwater_ruin/brick_7", "underwater_ruin/brick_8",
    ],
    &[
        "underwater_ruin/cracked_1", "underwater_ruin/cracked_2", "underwater_ruin/cracked_3", "underwater_ruin/cracked_4", "underwater_ruin/cracked_5",
        "underwater_ruin/cracked_6", "underwater_ruin/cracked_7", "underwater_ruin/cracked_8",
    ],
    &[
        "underwater_ruin/mossy_1", "underwater_ruin/mossy_2", "underwater_ruin/mossy_3", "underwater_ruin/mossy_4", "underwater_ruin/mossy_5",
        "underwater_ruin/mossy_6", "underwater_ruin/mossy_7", "underwater_ruin/mossy_8",
    ],
];
const BIG_COLD_RUINS: [&[&str]; 3] = [
    &["underwater_ruin/big_brick_1", "underwater_ruin/big_brick_2", "underwater_ruin/big_brick_3", "underwater_ruin/big_brick_8"],
    &["underwater_ruin/big_cracked_1", "underwater_ruin/big_cracked_2", "underwater_ruin/big_cracked_3", "underwater_ruin/big_cracked_8"],
    &["underwater_ruin/big_mossy_1", "underwater_ruin/big_mossy_2", "underwater_ruin/big_mossy_3", "underwater_ruin/big_mossy_8"],
];

/// `OceanRuinStructure`.
pub(crate) struct OceanRuin {
    cold: bool,
    large_probability: f32,
    cluster_probability: f32,
    /// Warm: small and big ruins. Cold: brick, cracked and mossy, small then big.
    small: Vec<Vec<Arc<Template>>>,
    big: Vec<Vec<Arc<Template>>>,
    ignore: Arc<Processor>,
    archaeology: Arc<Processor>,
}

impl OceanRuin {
    pub fn load(l: &mut Loader, json: &Value) -> Result<Self> {
        let cold = json.get("biome_temp").and_then(Value::as_str) == Some("cold");
        let (small, big) = if cold {
            (COLD_RUINS.iter().map(|n| templates(l, n)).collect::<Result<_>>()?, BIG_COLD_RUINS.iter().map(|n| templates(l, n)).collect::<Result<_>>()?)
        } else {
            (vec![templates(l, WARM_RUINS)?], vec![templates(l, BIG_WARM_RUINS)?])
        };
        let (block, replacement, table) =
            if cold { ("minecraft:gravel", "minecraft:suspicious_gravel", "minecraft:archaeology/ocean_ruin_cold") } else { ("minecraft:sand", "minecraft:suspicious_sand", "minecraft:archaeology/ocean_ruin_warm") };
        let rule = serde_json::json!({
            "processor_type": "minecraft:capped",
            "limit": 5,
            "delegate": {
                "processor_type": "minecraft:rule",
                "rules": [{
                    "input_predicate": {"predicate_type": "minecraft:block_match", "block": block},
                    "location_predicate": {"predicate_type": "minecraft:always_true"},
                    "output_state": {"Name": replacement},
                    "block_entity_modifier": {"type": "minecraft:append_loot", "loot_table": table}
                }]
            }
        });
        let archaeology = Arc::new(super::processor::ProcessorLists::processor(&mut l.parser, &rule)?);
        Ok(Self {
            cold,
            large_probability: json.f64_or("large_probability", 0.0) as f32,
            cluster_probability: json.f64_or("cluster_probability", 0.0) as f32,
            small,
            big,
            ignore: l.builtins.structure_and_air.clone(),
            archaeology,
        })
    }

    fn add_piece(&self, pieces: &mut Vec<Box<dyn Piece>>, pos: Pos, rotation: Rotation, r: &mut WorldgenRandom, large: bool, integrity: f32) {
        let lists = if large { &self.big } else { &self.small };
        let mut push = |template: Arc<Template>, integrity: f32| {
            let processors = vec![Arc::new(Processor::BlockRot { blocks: None, integrity }), self.ignore.clone(), self.archaeology.clone()];
            pieces.push(Box::new(TemplatePiece::new("minecraft:orp", template, pos, rotation, Mirror::None, Pos::new(0, 0, 0), processors, TemplateKind::OceanRuin { large })));
        };
        if !self.cold {
            push(pick(&lists[0], r).clone(), integrity);
        } else {
            let i = r.next_bounded(lists[0].len() as i32) as usize;
            push(lists[0][i].clone(), integrity);
            push(lists[1][i].clone(), 0.7);
            push(lists[2][i].clone(), 0.5);
        }
    }
}

/// `Mth.nextInt(random, min, max)`.
fn next_int(r: &mut WorldgenRandom, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        r.next_bounded(max - min + 1) + min
    }
}

impl StructureGen for OceanRuin {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        let stub = ctx.on_top_of_chunk_center(Heightmap::OceanFloorWg)?;
        if !ctx.valid_biome(stub) {
            return None;
        }
        let position = Pos::new(ctx.min_block_x(), 90, ctx.min_block_z());
        let rotation = Rotation::random(&mut ctx.r);
        let r = &mut ctx.r;
        let large = r.next_f32() <= self.large_probability;
        let mut pieces: Vec<Box<dyn Piece>> = Vec::new();
        self.add_piece(&mut pieces, position, rotation, r, large, if large { 0.9 } else { 0.8 });
        if large && r.next_f32() <= self.cluster_probability {
            // `addClusterRuins`.
            let parent = Pos::new(position.x, 90, position.z);
            let c = transform_pos(Pos::new(15, 0, 15), Mirror::None, rotation, Pos::new(0, 0, 0));
            let corner = Pos::new(c.x + parent.x, c.y + parent.y, c.z + parent.z);
            let parent_bb = BoundingBox::from_corners(parent, corner);
            let o = Pos::new(parent.x.min(corner.x), parent.y, parent.z.min(corner.z));
            let mut all = Vec::with_capacity(8);
            let mut at = |r: &mut WorldgenRandom, dx: (i32, i32, i32), dz: (i32, i32, i32)| {
                let x = dx.0 + next_int(r, dx.1, dx.2);
                let z = dz.0 + next_int(r, dz.1, dz.2);
                all.push(Pos::new(o.x + x, o.y, o.z + z));
            };
            at(r, (-16, 1, 8), (16, 1, 7));
            at(r, (-16, 1, 8), (0, 1, 7));
            at(r, (-16, 1, 8), (-16, 4, 8));
            at(r, (0, 1, 7), (16, 1, 7));
            at(r, (0, 1, 7), (-16, 4, 6));
            at(r, (16, 1, 7), (16, 3, 8));
            at(r, (16, 1, 7), (0, 1, 7));
            at(r, (16, 1, 7), (-16, 4, 8));
            let ruins = next_int(r, 4, 8);
            for _ in 0..ruins {
                if all.is_empty() {
                    continue;
                }
                let idx = r.next_bounded(all.len() as i32) as usize;
                let pos = all.remove(idx);
                let next_rotation = Rotation::random(r);
                let c = transform_pos(Pos::new(5, 0, 6), Mirror::None, next_rotation, Pos::new(0, 0, 0));
                let bb = BoundingBox::from_corners(pos, Pos::new(c.x + pos.x, c.y + pos.y, c.z + pos.z));
                if !bb.intersects(&parent_bb) {
                    self.add_piece(&mut pieces, pos, next_rotation, r, false, 0.8);
                }
            }
        }
        Some(pieces)
    }
}

const NETHER_FOSSILS: &[&str] = &[
    "nether_fossils/fossil_1", "nether_fossils/fossil_2", "nether_fossils/fossil_3", "nether_fossils/fossil_4", "nether_fossils/fossil_5",
    "nether_fossils/fossil_6", "nether_fossils/fossil_7", "nether_fossils/fossil_8", "nether_fossils/fossil_9", "nether_fossils/fossil_10",
    "nether_fossils/fossil_11", "nether_fossils/fossil_12", "nether_fossils/fossil_13", "nether_fossils/fossil_14",
];

/// `NetherFossilStructure`.
pub(crate) struct NetherFossil {
    height: HeightProvider,
    templates: Vec<Arc<Template>>,
    ignore: Arc<Processor>,
}

impl NetherFossil {
    pub fn load(l: &mut Loader, json: &Value) -> Result<Self> {
        Ok(Self { height: HeightProvider::parse(json.field("height")?)?, templates: templates(l, NETHER_FOSSILS)?, ignore: l.builtins.structure_and_air.clone() })
    }
}

impl StructureGen for NetherFossil {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        let gen = ctx.gen;
        let x = ctx.min_block_x() + ctx.r.next_bounded(16);
        let z = ctx.min_block_z() + ctx.r.next_bounded(16);
        let sea_level = gen.sea_level;
        let gctx = GenContext { min_y: gen.min_y.max(gen.gen_min_y), height: gen.height.min(gen.gen_height), sea_level };
        let mut y = self.height.sample(&mut ctx.r, &gctx);
        let b = &gen.decoration.blocks;
        let soul_sand = blocks::id("minecraft:soul_sand").unwrap_or(0);
        while y > sea_level {
            let current = ctx.cols.block(x, y, z);
            y -= 1;
            let below = ctx.cols.block(x, y, z);
            if b.is_air(current) && (blocks::block_id_of_state(below) == soul_sand || b.sturdy(below, Dir::Up)) {
                break;
            }
        }
        if y <= sea_level {
            return None;
        }
        let pos = Pos::new(x, y, z);
        if !ctx.valid_biome(pos) {
            return None;
        }
        let rotation = Rotation::random(&mut ctx.r);
        let template = pick(&self.templates, &mut ctx.r).clone();
        Some(vec![Box::new(TemplatePiece::new("minecraft:nefos", template, pos, rotation, Mirror::None, Pos::new(0, 0, 0), vec![self.ignore.clone()], TemplateKind::NetherFossil))])
    }
}
