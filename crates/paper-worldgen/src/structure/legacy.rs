//! The structures that are not jigsaw structures but place templates (shipwrecks, ocean
//! ruins, nether fossils, ruined portals, end cities, igloos), on a common
//! `TemplateStructurePiece`, and buried treasure.
//!
//! Written to vanilla 26.3's `TemplateStructurePiece`, `ShipwreckStructure`/`ShipwreckPieces`,
//! `OceanRuinStructure`/`OceanRuinPieces`, `NetherFossilStructure`/`NetherFossilPieces`,
//! `RuinedPortalStructure`/`RuinedPortalPiece`, `EndCityStructure`/`EndCityPieces`,
//! `IglooStructure`/`IglooPieces` and `BuriedTreasureStructure`/`BuriedTreasurePieces`.
//! Mobs these structures spawn (drowned, shulkers) and the end city's elytra item frame are
//! returned as entity NBT; vanilla's `finalizeSpawn` of the drowned is left to the caller.
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
    RuinedPortal(PortalProperties),
    EndCity,
    Igloo { part: IglooPart, offset: Pos },
}

/// `RuinedPortalPiece.VerticalPlacement`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortalPlacement {
    OnLandSurface,
    PartlyBuried,
    OnOceanFloor,
    InMountain,
    Underground,
    InNether,
}

impl PortalPlacement {
    fn parse(s: &str) -> Self {
        match s {
            "partly_buried" => Self::PartlyBuried,
            "on_ocean_floor" => Self::OnOceanFloor,
            "in_mountain" => Self::InMountain,
            "underground" => Self::Underground,
            "in_nether" => Self::InNether,
            _ => Self::OnLandSurface,
        }
    }

    fn heightmap(self) -> Heightmap {
        if self == Self::OnOceanFloor {
            Heightmap::OceanFloorWg
        } else {
            Heightmap::WorldSurfaceWg
        }
    }
}

/// `RuinedPortalPiece.Properties` and the placement.
#[derive(Clone, Debug)]
pub(crate) struct PortalProperties {
    placement: PortalPlacement,
    cold: bool,
    overgrown: bool,
    vines: bool,
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
    ignore_entities: bool,
    waterlog: bool,
}

impl TemplatePiece {
    fn new(kind_id: &'static str, template: Arc<Template>, position: Pos, rotation: Rotation, mirror: Mirror, pivot: Pos, processors: Vec<Arc<Processor>>, kind: TemplateKind) -> Self {
        let bbox = template.bounding_box(position, rotation, pivot, mirror);
        let base = PieceBase::new(kind_id, 0, bbox).with_orientation(Some(Dir::North));
        Self { base, template, position: Mutex::new((position, false)), rotation, mirror, pivot, processors, kind, ignore_entities: false, waterlog: true }
    }

    fn settings(&self, bbox: BoundingBox) -> PlaceSettings<'static> {
        PlaceSettings {
            mirror: self.mirror,
            rotation: self.rotation,
            pivot: self.pivot,
            bbox: Some(bbox),
            processors: self.processors.clone(),
            ignore_entities: self.ignore_entities,
            waterlog: self.waterlog,
            random: None,
        }
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
            TemplateKind::EndCity => {
                if id.starts_with("Chest") {
                    if ctx.chunk_box.is_inside(pos.down()) {
                        set_loot(ctx, entities, pos.down(), "minecraft:chests/end_city_treasure");
                    }
                } else if ctx.chunk_box.is_inside(pos) {
                    if id.starts_with("Sentry") {
                        let mut nbt = Compound::new();
                        nbt.insert("id", "minecraft:shulker");
                        nbt.insert("Pos", paper_nbt::Tag::List(vec![(pos.x as f64 + 0.5).into(), (pos.y as f64).into(), (pos.z as f64 + 0.5).into()]));
                        ctx.w.add_entity_nbt(nbt);
                    } else if id.starts_with("Elytra") {
                        let facing = self.rotation.rotate(Dir::South);
                        let mut item = Compound::new();
                        item.insert("id", "minecraft:elytra");
                        item.insert("count", 1i32);
                        let mut nbt = Compound::new();
                        nbt.insert("id", "minecraft:item_frame");
                        nbt.insert("Pos", paper_nbt::Tag::List(vec![(pos.x as f64 + 0.5).into(), (pos.y as f64 + 0.5).into(), (pos.z as f64 + 0.5).into()]));
                        nbt.insert("block_pos", paper_nbt::Tag::IntArray(vec![pos.x, pos.y, pos.z]));
                        nbt.insert("Facing", facing.index() as i8);
                        nbt.insert("Item", item);
                        ctx.w.add_entity_nbt(nbt);
                    }
                }
            }
            TemplateKind::Igloo { .. } => {
                if id == "chest" {
                    ctx.w.set(pos, st("minecraft:air"));
                    set_loot(ctx, entities, pos.down(), "minecraft:chests/igloo_chest");
                }
            }
            TemplateKind::NetherFossil | TemplateKind::RuinedPortal(_) => {}
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
            TemplateKind::EndCity => {
                let position = self.position.lock().unwrap_or_else(|e| e.into_inner()).0;
                self.place_template(ctx, position, chunk_box);
            }
            TemplateKind::Igloo { part, offset } => {
                let tp = self.position.lock().unwrap_or_else(|e| e.into_inner()).0;
                let rel = transform_pos(Pos::new(3 - offset.x, 0, -offset.z), Mirror::None, self.rotation, self.pivot);
                let height = ctx.w.height_at(Heightmap::WorldSurfaceWg, tp.x + rel.x, tp.z + rel.z);
                let placed = Pos::new(tp.x, tp.y + height - 90 - 1, tp.z);
                self.place_template(ctx, placed, chunk_box);
                if *part == IglooPart::Top {
                    let rel = transform_pos(Pos::new(3, 0, 5), Mirror::None, self.rotation, self.pivot);
                    let trap = Pos::new(placed.x + rel.x, placed.y + rel.y, placed.z + rel.z);
                    let below = ctx.w.get(trap.down());
                    if !ctx.w.b.is_air(below) && blocks::block_of_state(below).name != "minecraft:ladder" {
                        ctx.w.set(trap, st("minecraft:snow_block"));
                    }
                }
            }
            TemplateKind::RuinedPortal(props) => {
                let position = self.position.lock().unwrap_or_else(|e| e.into_inner()).0;
                let bb = self.template.bounding_box(position, self.rotation, self.pivot, self.mirror);
                if !chunk_box.is_inside(bb.center()) {
                    return;
                }
                let mut bbox = chunk_box;
                bbox.encapsulate(&bb);
                self.place_template(ctx, position, bbox);
                portal_decorate(ctx, props, &bb);
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

/// The netherrack, magma, vines and leaves around a ruined portal (`spreadNetherrack`,
/// `addNetherrackDripColumnsBelowPortal`, `maybeAddVines`, `maybeAddLeavesAbove`).
fn portal_decorate(ctx: &mut PlaceCtx, p: &PortalProperties, bb: &BoundingBox) {
    let b = ctx.w.b;
    let block_of = blocks::block_id_of_state;
    let (air, obsidian, lava) = (block_of(st("minecraft:air")), block_of(st("minecraft:obsidian")), block_of(st("minecraft:lava")));
    let (netherrack, magma, vine) = (st("minecraft:netherrack"), st("minecraft:magma_block"), st("minecraft:vine"));
    let can_replace = |state: u32| {
        let id = block_of(state);
        !(id == air || id == obsidian || b.tags.features_cannot_replace.has(state) || (p.placement != PortalPlacement::InNether && id == lava))
    };
    let place = |ctx: &mut PlaceCtx, pos: Pos| {
        if can_replace(ctx.w.get(pos)) {
            let state = if !p.cold && ctx.r.next_f32() < 0.07 { magma } else { netherrack };
            ctx.w.set(pos, state);
        }
    };
    let drip = |ctx: &mut PlaceCtx, pos: Pos| {
        let mut pos = pos;
        place(ctx, pos);
        let mut cap = 8;
        while cap > 0 && ctx.r.next_f32() < 0.5 {
            pos = pos.down();
            cap -= 1;
            place(ctx, pos);
        }
    };
    let leaves = |ctx: &mut PlaceCtx, pos: Pos| {
        if ctx.r.next_f32() < 0.5 && block_of(ctx.w.get(pos)) == block_of(netherrack) && ctx.w.is_air(pos.up()) {
            ctx.w.set(pos.up(), super::piece::with_bool(st("minecraft:jungle_leaves"), "persistent", true));
        }
    };
    // `spreadNetherrack`.
    let follow = matches!(p.placement, PortalPlacement::OnLandSurface | PortalPlacement::OnOceanFloor);
    let center = bb.center();
    const PROBABILITY: [f32; 14] = [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.9, 0.9, 0.8, 0.7, 0.6, 0.4, 0.2];
    let max_distance = PROBABILITY.len() as i32;
    let average_width = (bb.x_span() + bb.z_span()) / 2;
    let adjustment = ctx.r.next_bounded(1.max(8 - average_width / 2));
    for x in center.x - max_distance..=center.x + max_distance {
        for z in center.z - max_distance..=center.z + max_distance {
            let distance = (x - center.x).abs() + (z - center.z).abs();
            let adjusted = 0.max(distance + adjustment);
            if adjusted >= max_distance {
                continue;
            }
            if ctx.r.next_f64() >= PROBABILITY[adjusted as usize] as f64 {
                continue;
            }
            let surface = ctx.w.height_at(p.placement.heightmap(), x, z) - 1;
            let y = if follow { surface } else { bb.min_y.min(surface) };
            let pos = Pos::new(x, y, z);
            if (y - bb.min_y).abs() > 3 || !can_replace(ctx.w.get(pos)) {
                continue;
            }
            place(ctx, pos);
            if p.overgrown {
                leaves(ctx, pos);
            }
            drip(ctx, pos.down());
        }
    }
    // `addNetherrackDripColumnsBelowPortal`.
    for x in bb.min_x + 1..bb.max_x {
        for z in bb.min_z + 1..bb.max_z {
            let pos = Pos::new(x, bb.min_y, z);
            if block_of(ctx.w.get(pos)) == block_of(netherrack) {
                drip(ctx, pos.down());
            }
        }
    }
    if p.vines || p.overgrown {
        for z in bb.min_z..=bb.max_z {
            for y in bb.min_y..=bb.max_y {
                for x in bb.min_x..=bb.max_x {
                    let pos = Pos::new(x, y, z);
                    if p.vines {
                        let state = ctx.w.get(pos);
                        if !b.is_air(state) && block_of(state) != block_of(vine) {
                            let dir = Dir::HORIZONTAL[ctx.r.next_bounded(4) as usize];
                            let n = pos.rel(dir);
                            if ctx.w.is_air(n) && paper_data::physics::collision_face_full(state, dir.index()) {
                                ctx.w.set(n, super::piece::with_bool(vine, dir.opposite().name(), true));
                            }
                        }
                    }
                    if p.overgrown {
                        leaves(ctx, pos);
                    }
                }
            }
        }
    }
}

const PORTALS: &[&str] = &[
    "ruined_portal/portal_1", "ruined_portal/portal_2", "ruined_portal/portal_3", "ruined_portal/portal_4", "ruined_portal/portal_5",
    "ruined_portal/portal_6", "ruined_portal/portal_7", "ruined_portal/portal_8", "ruined_portal/portal_9", "ruined_portal/portal_10",
];
const GIANT_PORTALS: &[&str] = &["ruined_portal/giant_portal_1", "ruined_portal/giant_portal_2", "ruined_portal/giant_portal_3"];

/// `RuinedPortalStructure.Setup` with its processors.
struct PortalSetup {
    placement: PortalPlacement,
    air_pocket_probability: f32,
    can_be_cold: bool,
    overgrown: bool,
    vines: bool,
    weight: f32,
    /// The processors for `[cold][air pocket]`.
    processors: [[Vec<Arc<Processor>>; 2]; 2],
}

/// `RuinedPortalStructure`.
pub(crate) struct RuinedPortal {
    setups: Vec<PortalSetup>,
    portals: Vec<Arc<Template>>,
    giant: Vec<Arc<Template>>,
}

fn replace_rule(source: &str, probability: Option<f32>, target: &str) -> Value {
    let input = match probability {
        Some(p) => serde_json::json!({"predicate_type": "minecraft:random_block_match", "block": source, "probability": p}),
        None => serde_json::json!({"predicate_type": "minecraft:block_match", "block": source}),
    };
    serde_json::json!({"input_predicate": input, "location_predicate": {"predicate_type": "minecraft:always_true"}, "output_state": {"Name": target}})
}

impl RuinedPortal {
    pub fn load(l: &mut Loader, json: &Value) -> Result<Self> {
        let mut setups = Vec::new();
        for s in json.field("setups")?.as_array().into_iter().flatten() {
            let placement = PortalPlacement::parse(s.get("placement").and_then(Value::as_str).unwrap_or("on_land_surface"));
            let mossiness = s.f64_or("mossiness", 0.0);
            let blackstone = s.get("replace_with_blackstone").and_then(Value::as_bool).unwrap_or(false);
            let mut processors: [[Vec<Arc<Processor>>; 2]; 2] = Default::default();
            for (cold, row) in processors.iter_mut().enumerate() {
                for (air, list) in row.iter_mut().enumerate() {
                    let mut rules = vec![replace_rule("minecraft:gold_block", Some(0.3), "minecraft:air")];
                    rules.push(if placement == PortalPlacement::OnOceanFloor {
                        replace_rule("minecraft:lava", None, "minecraft:magma_block")
                    } else if cold == 1 {
                        replace_rule("minecraft:lava", None, "minecraft:netherrack")
                    } else {
                        replace_rule("minecraft:lava", Some(0.2), "minecraft:magma_block")
                    });
                    if cold == 0 {
                        rules.push(replace_rule("minecraft:netherrack", Some(0.07), "minecraft:magma_block"));
                    }
                    let mut v = vec![if air == 1 { l.builtins.structure_and_air.clone() } else { l.builtins.structure_block.clone() }];
                    let mut defs = vec![
                        serde_json::json!({"processor_type": "minecraft:rule", "rules": rules}),
                        serde_json::json!({"processor_type": "minecraft:block_age", "mossiness": mossiness}),
                        serde_json::json!({"processor_type": "minecraft:protected_blocks", "value": "#minecraft:features_cannot_replace"}),
                        serde_json::json!({"processor_type": "minecraft:lava_submerged_block"}),
                    ];
                    if blackstone {
                        defs.push(serde_json::json!({"processor_type": "minecraft:blackstone_replace"}));
                    }
                    for d in &defs {
                        v.push(Arc::new(super::processor::ProcessorLists::processor(&mut l.parser, d)?));
                    }
                    *list = v;
                }
            }
            setups.push(PortalSetup {
                placement,
                air_pocket_probability: s.f64_or("air_pocket_probability", 0.0) as f32,
                can_be_cold: s.get("can_be_cold").and_then(Value::as_bool).unwrap_or(false),
                overgrown: s.get("overgrown").and_then(Value::as_bool).unwrap_or(false),
                vines: s.get("vines").and_then(Value::as_bool).unwrap_or(false),
                weight: s.f64_or("weight", 1.0) as f32,
                processors,
            });
        }
        if setups.is_empty() {
            anyhow::bail!("ruined portal without setups");
        }
        Ok(Self { setups, portals: templates(l, PORTALS)?, giant: templates(l, GIANT_PORTALS)? })
    }

    /// `findSuitableY`.
    fn suitable_y(ctx: &mut GenCtx, placement: PortalPlacement, air_pocket: bool, surface: i32, y_span: i32, bb: &BoundingBox) -> i32 {
        let min_y = ctx.min_y() + 15;
        let r = &mut ctx.r;
        let between = |r: &mut WorldgenRandom, a: i32, b: i32| r.next_bounded(b - a + 1) + a;
        let within = |r: &mut WorldgenRandom, a: i32, b: i32| if a < b { r.next_bounded(b - a + 1) + a } else { b };
        let new_y = match placement {
            PortalPlacement::InNether => {
                if air_pocket {
                    between(r, 32, 100)
                } else if r.next_f32() < 0.5 {
                    between(r, 27, 29)
                } else {
                    between(r, 29, 100)
                }
            }
            PortalPlacement::InMountain => within(r, 70, surface - y_span),
            PortalPlacement::Underground => within(r, min_y, surface - y_span),
            PortalPlacement::PartlyBuried => surface - y_span + between(r, 2, 8),
            _ => surface,
        };
        let corners = [(bb.min_x, bb.min_z), (bb.max_x, bb.min_z), (bb.min_x, bb.max_z), (bb.max_x, bb.max_z)];
        let b = &ctx.gen.decoration.blocks;
        let ocean = placement == PortalPlacement::OnOceanFloor;
        let mut y = new_y;
        'down: while y > min_y {
            let mut solid = 0;
            for &(x, z) in &corners {
                let state = ctx.cols.block(x, y, z);
                if if ocean { b.blocks_motion(state) } else { !b.is_air(state) } {
                    solid += 1;
                    if solid == 3 {
                        break 'down;
                    }
                }
            }
            y -= 1;
        }
        y
    }
}

impl StructureGen for RuinedPortal {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        let r = &mut ctx.r;
        let setup = if self.setups.len() > 1 {
            let total: f32 = self.setups.iter().map(|s| s.weight).sum();
            let mut pick_value = r.next_f32();
            let mut chosen = None;
            for s in &self.setups {
                pick_value -= s.weight / total;
                if pick_value < 0.0 {
                    chosen = Some(s);
                    break;
                }
            }
            chosen?
        } else {
            &self.setups[0]
        };
        let air_pocket = match setup.air_pocket_probability {
            p if p == 0.0 => false,
            p if p == 1.0 => true,
            p => r.next_f32() < p,
        };
        let template = if r.next_f32() < 0.05 { pick(&self.giant, r) } else { pick(&self.portals, r) }.clone();
        let rotation = Rotation::random(r);
        let mirror = if r.next_f32() < 0.5 { Mirror::None } else { Mirror::FrontBack };
        let pivot = Pos::new(template.size.0 / 2, 0, template.size.2 / 2);
        let base = Pos::new(ctx.min_block_x(), 0, ctx.min_block_z());
        let bb = template.bounding_box(base, rotation, pivot, mirror);
        let center = bb.center();
        let surface = ctx.cols.base_height(center.x, center.z, setup.placement.heightmap()) - 1;
        let y = Self::suitable_y(ctx, setup.placement, air_pocket, surface, bb.y_span(), &bb);
        let origin = Pos::new(base.x, y, base.z);
        if !ctx.valid_biome(origin) {
            return None;
        }
        let gen = ctx.gen;
        let cold = setup.can_be_cold && gen.biomes.get(ctx.cols.biome_at(origin)).cold_enough_to_snow(origin.x, origin.y, origin.z, gen.sea_level);
        let props = PortalProperties { placement: setup.placement, cold, overgrown: setup.overgrown, vines: setup.vines };
        let processors = setup.processors[cold as usize][air_pocket as usize].clone();
        Some(vec![Box::new(TemplatePiece::new("minecraft:rupo", template, origin, rotation, mirror, pivot, processors, TemplateKind::RuinedPortal(props)))])
    }
}

/// `EndCityPieces`: the piece generators, with the bridge generator's `shipCreated`.
struct EndCityBuilder<'a> {
    city: &'a EndCity,
    ship_created: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    HouseTower,
    Tower,
    TowerBridge,
    FatTower,
}

const TOWER_BRIDGES: [(Rotation, (i32, i32, i32)); 4] =
    [(Rotation::None, (1, -1, 0)), (Rotation::Clockwise90, (6, -1, 1)), (Rotation::CounterClockwise90, (0, -1, 5)), (Rotation::Clockwise180, (5, -1, 6))];
const FAT_TOWER_BRIDGES: [(Rotation, (i32, i32, i32)); 4] =
    [(Rotation::None, (4, -1, 0)), (Rotation::Clockwise90, (12, -1, 4)), (Rotation::CounterClockwise90, (0, -1, 8)), (Rotation::Clockwise180, (8, -1, 12))];

impl EndCityBuilder<'_> {
    /// `EndCityPieces.addPiece`: a piece connected to `parent` at `offset`.
    fn add(&self, out: &mut Vec<TemplatePiece>, parent: usize, offset: (i32, i32, i32), name: &str, rotation: Rotation, overwrite: bool) -> usize {
        let (pp, pr) = {
            let p = &out[parent];
            (p.position.lock().unwrap_or_else(|e| e.into_inner()).0, p.rotation)
        };
        let mut child = self.city.piece(name, pp, rotation, overwrite);
        let o = transform_pos(Pos::new(offset.0, offset.1, offset.2), Mirror::None, pr, Pos::new(0, 0, 0));
        child.translate(o.x, o.y, o.z);
        out.push(child);
        out.len() - 1
    }

    /// `recursiveChildren`. Vanilla passes each generator its own child list and nested
    /// calls get that list: here all pieces are in one list, a generator's children follow
    /// `pieces[..start]`, and children are checked against `pieces[outer..start]` (the
    /// list vanilla would have passed) and kept or dropped as a group.
    #[allow(clippy::too_many_arguments)]
    fn recursive(&mut self, section: Section, depth: i32, parent: usize, offset: Option<(i32, i32, i32)>, pieces: &mut Vec<TemplatePiece>, outer: usize, r: &mut WorldgenRandom) -> bool {
        if depth > 8 {
            return false;
        }
        let start = pieces.len();
        let parent_depth = pieces[parent].base.gen_depth;
        if self.generate(section, depth, parent, offset, pieces, start, r) {
            let tag = r.next_i32();
            let mut collision = false;
            for i in start..pieces.len() {
                pieces[i].base.gen_depth = tag;
                let bb = pieces[i].base.bbox;
                if let Some(c) = pieces[outer..start].iter().find(|p| p.base.bbox.intersects(&bb)) {
                    if c.base.gen_depth != parent_depth {
                        collision = true;
                        break;
                    }
                }
            }
            if !collision {
                return true;
            }
        }
        pieces.truncate(start);
        false
    }

    /// `SectionGenerator.generate`, adding to `pieces` from `start` on (pieces before
    /// `start` are only parents).
    #[allow(clippy::too_many_arguments)]
    fn generate(&mut self, section: Section, depth: i32, parent: usize, offset: Option<(i32, i32, i32)>, pieces: &mut Vec<TemplatePiece>, start: usize, r: &mut WorldgenRandom) -> bool {
        let rotation = pieces[parent].rotation;
        match section {
            Section::HouseTower => {
                if depth > 8 {
                    return false;
                }
                let mut last = self.add(pieces, parent, offset.unwrap_or((0, 0, 0)), "base_floor", rotation, true);
                match r.next_bounded(3) {
                    0 => {
                        self.add(pieces, last, (-1, 4, -1), "base_roof", rotation, true);
                    }
                    1 => {
                        last = self.add(pieces, last, (-1, 0, -1), "second_floor_2", rotation, false);
                        last = self.add(pieces, last, (-1, 8, -1), "second_roof", rotation, false);
                        self.recursive(Section::Tower, depth + 1, last, None, pieces, start, r);
                    }
                    _ => {
                        last = self.add(pieces, last, (-1, 0, -1), "second_floor_2", rotation, false);
                        last = self.add(pieces, last, (-1, 4, -1), "third_floor_2", rotation, false);
                        last = self.add(pieces, last, (-1, 8, -1), "third_roof", rotation, true);
                        self.recursive(Section::Tower, depth + 1, last, None, pieces, start, r);
                    }
                }
                true
            }
            Section::Tower => {
                let (x, z) = (3 + r.next_bounded(2), 3 + r.next_bounded(2));
                let mut last = self.add(pieces, parent, (x, -3, z), "tower_base", rotation, true);
                last = self.add(pieces, last, (0, 7, 0), "tower_piece", rotation, true);
                let mut bridge = if r.next_bounded(3) == 0 { Some(last) } else { None };
                let height = 1 + r.next_bounded(3);
                for i in 0..height {
                    last = self.add(pieces, last, (0, 4, 0), "tower_piece", rotation, true);
                    if i < height - 1 && r.next_bool() {
                        bridge = Some(last);
                    }
                }
                if let Some(bridge) = bridge {
                    for (rot, pos) in TOWER_BRIDGES {
                        if r.next_bool() {
                            let b = self.add(pieces, bridge, pos, "bridge_end", rotation.then(rot), true);
                            self.recursive(Section::TowerBridge, depth + 1, b, None, pieces, start, r);
                        }
                    }
                    self.add(pieces, last, (-1, 4, -1), "tower_top", rotation, true);
                } else if depth == 7 {
                    self.add(pieces, last, (-1, 4, -1), "tower_top", rotation, true);
                } else {
                    return self.recursive(Section::FatTower, depth + 1, last, None, pieces, start, r);
                }
                true
            }
            Section::TowerBridge => {
                let length = r.next_bounded(4) + 1;
                let mut last = self.add(pieces, parent, (0, 0, -4), "bridge_piece", rotation, true);
                pieces[last].base.gen_depth = -1;
                let mut next_y = 0;
                for _ in 0..length {
                    if r.next_bool() {
                        last = self.add(pieces, last, (0, next_y, -4), "bridge_piece", rotation, true);
                        next_y = 0;
                    } else {
                        last = if r.next_bool() {
                            self.add(pieces, last, (0, next_y, -4), "bridge_steep_stairs", rotation, true)
                        } else {
                            self.add(pieces, last, (0, next_y, -8), "bridge_gentle_stairs", rotation, true)
                        };
                        next_y = 4;
                    }
                }
                if !self.ship_created && r.next_bounded(10 - depth) == 0 {
                    let (x, z) = (-8 + r.next_bounded(8), -70 + r.next_bounded(10));
                    self.add(pieces, last, (x, next_y, z), "ship", rotation, true);
                    self.ship_created = true;
                } else if !self.recursive(Section::HouseTower, depth + 1, last, Some((-3, next_y + 1, -11)), pieces, start, r) {
                    return false;
                }
                last = self.add(pieces, last, (4, next_y, 0), "bridge_end", rotation.then(Rotation::Clockwise180), true);
                pieces[last].base.gen_depth = -1;
                true
            }
            Section::FatTower => {
                let mut last = self.add(pieces, parent, (-3, 4, -3), "fat_tower_base", rotation, true);
                last = self.add(pieces, last, (0, 4, 0), "fat_tower_middle", rotation, true);
                for _ in 0..2 {
                    if r.next_bounded(3) == 0 {
                        break;
                    }
                    last = self.add(pieces, last, (0, 8, 0), "fat_tower_middle", rotation, true);
                    for (rot, pos) in FAT_TOWER_BRIDGES {
                        if r.next_bool() {
                            let b = self.add(pieces, last, pos, "bridge_end", rotation.then(rot), true);
                            self.recursive(Section::TowerBridge, depth + 1, b, None, pieces, start, r);
                        }
                    }
                }
                self.add(pieces, last, (-2, 8, -2), "fat_tower_top", rotation, true);
                true
            }
        }
    }
}

/// `EndCityStructure`.
pub(crate) struct EndCity {
    templates: std::collections::HashMap<&'static str, Arc<Template>>,
    structure_block: Arc<Processor>,
    structure_and_air: Arc<Processor>,
}

const END_CITY_TEMPLATES: &[&str] = &[
    "base_floor", "base_roof", "bridge_end", "bridge_gentle_stairs", "bridge_piece", "bridge_steep_stairs", "fat_tower_base", "fat_tower_middle",
    "fat_tower_top", "second_floor_1", "second_floor_2", "second_roof", "ship", "third_floor_1", "third_floor_2", "third_roof", "tower_base",
    "tower_piece", "tower_top",
];

impl EndCity {
    pub fn load(l: &mut Loader) -> Result<Self> {
        let mut templates = std::collections::HashMap::new();
        for name in END_CITY_TEMPLATES {
            templates.insert(*name, l.templates.load(l.data, &format!("minecraft:end_city/{name}"))?);
        }
        Ok(Self { templates, structure_block: l.builtins.structure_block.clone(), structure_and_air: l.builtins.structure_and_air.clone() })
    }

    fn piece(&self, name: &str, position: Pos, rotation: Rotation, overwrite: bool) -> TemplatePiece {
        let template = self.templates[name].clone();
        let processor = if overwrite { self.structure_block.clone() } else { self.structure_and_air.clone() };
        let mut p = TemplatePiece::new("minecraft:ecp", template, position, rotation, Mirror::None, Pos::new(0, 0, 0), vec![processor], TemplateKind::EndCity);
        p.ignore_entities = true;
        p
    }
}

impl StructureGen for EndCity {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        let (x, z) = (ctx.min_block_x() + 7, ctx.min_block_z() + 7);
        let (min_y, max_y) = (ctx.min_y() - 1, ctx.max_y());
        if !ctx.could_exist_in_column(x, z, min_y, max_y) {
            return None;
        }
        let rotation = Rotation::random(&mut ctx.r);
        let (ox, oz) = match rotation {
            Rotation::None => (5, 5),
            Rotation::Clockwise90 => (-5, 5),
            Rotation::Clockwise180 => (-5, -5),
            Rotation::CounterClockwise90 => (5, -5),
        };
        let y = ctx.cols.lowest_y(x, z, ox, oz);
        if y < 60 {
            return None;
        }
        let start = Pos::new(x, y, z);
        if !ctx.valid_biome(start) {
            return None;
        }
        // `startHouseTower`.
        let mut builder = EndCityBuilder { city: self, ship_created: false };
        let mut pieces = vec![self.piece("base_floor", start, rotation, true)];
        let mut last = builder.add(&mut pieces, 0, (-1, 0, -1), "second_floor_1", rotation, false);
        last = builder.add(&mut pieces, last, (-1, 4, -1), "third_floor_1", rotation, false);
        last = builder.add(&mut pieces, last, (-1, 8, -1), "third_roof", rotation, true);
        builder.recursive(Section::Tower, 1, last, None, &mut pieces, 0, &mut ctx.r);
        Some(pieces.into_iter().map(|p| Box::new(p) as Box<dyn Piece>).collect())
    }
}

const IGLOO_TOP: &str = "igloo/top";
const IGLOO_MIDDLE: &str = "igloo/middle";
const IGLOO_BOTTOM: &str = "igloo/bottom";

/// `IglooStructure`.
pub(crate) struct Igloo {
    top: Arc<Template>,
    middle: Arc<Template>,
    bottom: Arc<Template>,
    structure_block: Arc<Processor>,
}

impl Igloo {
    pub fn load(l: &mut Loader) -> Result<Self> {
        let t = templates(l, &[IGLOO_TOP, IGLOO_MIDDLE, IGLOO_BOTTOM])?;
        Ok(Self { top: t[0].clone(), middle: t[1].clone(), bottom: t[2].clone(), structure_block: l.builtins.structure_block.clone() })
    }

    fn piece(&self, which: IglooPart, position: Pos, rotation: Rotation, depth: i32) -> TemplatePiece {
        let (template, pivot, offset) = match which {
            IglooPart::Top => (&self.top, Pos::new(3, 5, 5), Pos::new(0, 0, 0)),
            IglooPart::Middle => (&self.middle, Pos::new(1, 3, 1), Pos::new(2, -3, 4)),
            IglooPart::Bottom => (&self.bottom, Pos::new(3, 6, 7), Pos::new(0, -3, -2)),
        };
        let pos = Pos::new(position.x + offset.x, position.y + offset.y - depth, position.z + offset.z);
        let mut p = TemplatePiece::new("minecraft:iglu", template.clone(), pos, rotation, Mirror::None, pivot, vec![self.structure_block.clone()], TemplateKind::Igloo { part: which, offset });
        p.waterlog = false;
        p
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IglooPart {
    Top,
    Middle,
    Bottom,
}

impl StructureGen for Igloo {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        let stub = ctx.on_top_of_chunk_center(Heightmap::WorldSurfaceWg)?;
        if !ctx.valid_biome(stub) {
            return None;
        }
        let start = Pos::new(ctx.min_block_x(), 90, ctx.min_block_z());
        let r = &mut ctx.r;
        let rotation = Rotation::random(r);
        let mut pieces: Vec<Box<dyn Piece>> = Vec::new();
        if r.next_f64() < 0.5 {
            let depth = r.next_bounded(8) + 4;
            pieces.push(Box::new(self.piece(IglooPart::Bottom, start, rotation, depth * 3)));
            for i in 0..depth - 1 {
                pieces.push(Box::new(self.piece(IglooPart::Middle, start, rotation, i * 3)));
            }
        }
        pieces.push(Box::new(self.piece(IglooPart::Top, start, rotation, 0)));
        Some(pieces)
    }
}

/// `BuriedTreasurePieces.BuriedTreasurePiece`.
struct BuriedTreasurePiece {
    base: PieceBase,
}

impl Piece for BuriedTreasurePiece {
    fn base(&self) -> &PieceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PieceBase {
        &mut self.base
    }

    fn post_process(&self, ctx: &mut PlaceCtx) {
        let b = ctx.w.b;
        let (x, z) = (self.base.bbox.min_x, self.base.bbox.min_z);
        let mut pos = Pos::new(x, ctx.w.height_at(Heightmap::OceanFloorWg, x, z), z);
        let base_blocks: Vec<u32> = ["sandstone", "stone", "andesite", "granite", "diorite"].iter().map(|n| blocks::id(&format!("minecraft:{n}")).unwrap_or(0)).collect();
        let liquid = |s: u32| matches!(blocks::block_of_state(s).name, "minecraft:water" | "minecraft:lava");
        while pos.y > ctx.w.min_y {
            let current = ctx.w.get(pos);
            let below = ctx.w.get(pos.down());
            if base_blocks.contains(&blocks::block_id_of_state(below)) {
                let soft = if b.is_air(current) || liquid(current) { st("minecraft:sand") } else { current };
                for d in Dir::ALL {
                    let rel = pos.rel(d);
                    let rs = ctx.w.get(rel);
                    if b.is_air(rs) || liquid(rs) {
                        let brs = ctx.w.get(rel.down());
                        if (b.is_air(brs) || liquid(brs)) && d != Dir::Up {
                            ctx.w.set(rel, below);
                        } else {
                            ctx.w.set(rel, soft);
                        }
                    }
                }
                super::piece::create_chest_at(ctx, pos, "minecraft:chests/buried_treasure", None);
                return;
            }
            pos = pos.down();
        }
    }
}

/// `BuriedTreasureStructure`.
pub(crate) struct BuriedTreasure;

impl StructureGen for BuriedTreasure {
    fn generate(&self, ctx: &mut GenCtx) -> Option<Vec<Box<dyn Piece>>> {
        let stub = ctx.on_top_of_chunk_center(Heightmap::OceanFloorWg)?;
        if !ctx.valid_biome(stub) {
            return None;
        }
        let p = Pos::new(ctx.min_block_x() + 9, 90, ctx.min_block_z() + 9);
        Some(vec![Box::new(BuriedTreasurePiece { base: PieceBase::new("minecraft:btp", 0, BoundingBox::single(p)) })])
    }
}
