//! Structure processors (`StructureProcessor` and its 26.3 implementations) and processor
//! lists (`worldgen/processor_list`).

use super::template::{parse_state_string, PlaceSettings};
use crate::chunk::Heightmap;
use crate::data::{full_id, JsonExt};
use crate::feature::blockinfo::BlockSet;
use crate::feature::parse::Parser;
use crate::feature::predicate::RuleTest;
use crate::feature::world::{Dir, Pos, World};
use crate::providers::IntProvider;
use crate::random::{position_seed, Random, WorldgenRandom};
use anyhow::{anyhow, bail, Result};
use paper_data::blocks;
use paper_nbt::{Compound, Tag};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// A block being processed (`StructureBlockInfo` at its world position).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Info {
    pub pos: Pos,
    pub state: u32,
    pub nbt: Option<Arc<Compound>>,
}

/// `PosRuleTest`.
#[derive(Debug)]
pub(crate) enum PosRule {
    True,
    Linear { min_chance: f32, max_chance: f32, min_dist: i32, max_dist: i32 },
    AxisAligned { min_chance: f32, max_chance: f32, min_dist: i32, max_dist: i32, axis: Dir },
}

fn clamped_lerp(factor: f32, min: f32, max: f32) -> f32 {
    if factor < 0.0 {
        min
    } else if factor > 1.0 {
        max
    } else {
        min + factor * (max - min)
    }
}

impl PosRule {
    fn test(&self, world: Pos, reference: Pos, r: &mut WorldgenRandom) -> bool {
        let (dist, min_chance, max_chance, min_dist, max_dist) = match *self {
            PosRule::True => return true,
            PosRule::Linear { min_chance, max_chance, min_dist, max_dist } => (world.manhattan(reference), min_chance, max_chance, min_dist, max_dist),
            PosRule::AxisAligned { min_chance, max_chance, min_dist, max_dist, axis } => {
                let (sx, sy, sz) = axis.offset();
                let xd = ((world.x - reference.x) * sx).abs() as f32;
                let yd = ((world.y - reference.y) * sy).abs() as f32;
                let zd = ((world.z - reference.z) * sz).abs() as f32;
                ((xd + yd + zd) as i32, min_chance, max_chance, min_dist, max_dist)
            }
        };
        let rnd = r.next_f32();
        let t = (dist as f32 - min_dist as f32) / (max_dist as f32 - min_dist as f32);
        rnd <= clamped_lerp(t, min_chance, max_chance)
    }
}

/// `RuleBlockEntityModifier`.
#[derive(Debug)]
pub(crate) enum BlockEntityModifier {
    Passthrough,
    Clear,
    AppendLoot(String),
    AppendStatic(Compound),
}

impl BlockEntityModifier {
    fn apply(&self, r: &mut WorldgenRandom, existing: Option<Arc<Compound>>) -> Option<Arc<Compound>> {
        match self {
            BlockEntityModifier::Passthrough => existing,
            BlockEntityModifier::Clear => Some(Arc::new(Compound::new())),
            BlockEntityModifier::AppendLoot(table) => {
                let mut tag = existing.map(|c| (*c).clone()).unwrap_or_default();
                tag.insert("LootTable", table.as_str());
                tag.insert("LootTableSeed", r.next_i64());
                Some(Arc::new(tag))
            }
            BlockEntityModifier::AppendStatic(data) => {
                let mut tag = existing.map(|c| (*c).clone()).unwrap_or_default();
                for (k, v) in data.iter() {
                    tag.insert(k.clone(), v.clone());
                }
                Some(Arc::new(tag))
            }
        }
    }
}

/// `ProcessorRule`.
#[derive(Debug)]
pub(crate) struct ProcessorRule {
    input: RuleTest,
    location: RuleTest,
    position: PosRule,
    output: u32,
    modifier: BlockEntityModifier,
}

/// A structure processor.
#[derive(Debug)]
pub(crate) enum Processor {
    /// `BlockIgnoreProcessor` (block ids).
    Ignore(Vec<u32>),
    JigsawReplacement,
    Gravity { heightmap: Heightmap, offset: i32 },
    Rule(Vec<ProcessorRule>),
    BlockRot { blocks: Option<Arc<BlockSet>>, integrity: f32 },
    Protected(Arc<BlockSet>),
    Capped { delegate: Box<Processor>, limit: IntProvider },
    BlockAge { mossiness: f32, stairs: Arc<BlockSet>, slabs: Arc<BlockSet>, walls: Arc<BlockSet> },
    BlackstoneReplace,
    LavaSubmerged,
    Nop,
}

fn block(name: &str) -> u32 {
    blocks::id(name).unwrap_or(0)
}

fn st(name: &str) -> u32 {
    blocks::default_state(name).unwrap_or(0)
}

/// `withPropertiesOf`: the default state of `block` with the properties `from` shares.
fn with_properties_of(block_name: &str, from: u32) -> u32 {
    let mut s = st(block_name);
    for (k, v) in blocks::properties_of(from) {
        if let Some(n) = blocks::with_property(s, k, v) {
            s = n;
        }
    }
    s
}

fn blackstone_replacement(block_id: u32) -> Option<&'static str> {
    static MAP: std::sync::OnceLock<HashMap<u32, &'static str>> = std::sync::OnceLock::new();
    let map = MAP.get_or_init(|| {
        [
            ("cobblestone", "blackstone"),
            ("mossy_cobblestone", "blackstone"),
            ("stone", "polished_blackstone"),
            ("stone_bricks", "polished_blackstone_bricks"),
            ("mossy_stone_bricks", "polished_blackstone_bricks"),
            ("cobblestone_stairs", "blackstone_stairs"),
            ("mossy_cobblestone_stairs", "blackstone_stairs"),
            ("stone_stairs", "polished_blackstone_stairs"),
            ("stone_brick_stairs", "polished_blackstone_brick_stairs"),
            ("mossy_stone_brick_stairs", "polished_blackstone_brick_stairs"),
            ("cobblestone_slab", "blackstone_slab"),
            ("mossy_cobblestone_slab", "blackstone_slab"),
            ("smooth_stone_slab", "polished_blackstone_slab"),
            ("stone_slab", "polished_blackstone_slab"),
            ("stone_brick_slab", "polished_blackstone_brick_slab"),
            ("mossy_stone_brick_slab", "polished_blackstone_brick_slab"),
            ("stone_brick_wall", "polished_blackstone_brick_wall"),
            ("mossy_stone_brick_wall", "polished_blackstone_brick_wall"),
            ("cobblestone_wall", "blackstone_wall"),
            ("mossy_cobblestone_wall", "blackstone_wall"),
            ("chiseled_stone_bricks", "chiseled_polished_blackstone"),
            ("cracked_stone_bricks", "cracked_polished_blackstone_bricks"),
            ("iron_bars", "iron_chain"),
        ]
        .into_iter()
        .filter_map(|(a, b)| Some((blocks::id(&format!("minecraft:{a}"))?, b)))
        .collect()
    });
    map.get(&block_id).copied()
}

impl Processor {
    pub fn evaluates_entire_piece(&self) -> bool {
        matches!(self, Processor::Capped { .. })
    }

    /// `processBlock`. `local` is the block's template-relative position.
    pub fn process(&self, w: &World, _position: Pos, reference: Pos, local: Pos, info: Info, settings: &mut PlaceSettings) -> Option<Info> {
        let b = w.b;
        match self {
            Processor::Ignore(ids) => (!ids.contains(&blocks::block_id_of_state(info.state))).then_some(info),
            Processor::JigsawReplacement => {
                if blocks::block_of_state(info.state).name != "minecraft:jigsaw" {
                    return Some(info);
                }
                let Some(nbt) = &info.nbt else { return Some(info) };
                let state = parse_state_string(nbt.string("final_state").unwrap_or("minecraft:air"));
                if blocks::block_of_state(state).name == "minecraft:structure_void" {
                    return None;
                }
                Some(Info { pos: info.pos, state, nbt: None })
            }
            Processor::Gravity { heightmap, offset } => {
                let h = w.height_at(*heightmap, info.pos.x, info.pos.z) + offset;
                Some(Info { pos: Pos::new(info.pos.x, h + local.y, info.pos.z), ..info })
            }
            Processor::Rule(rules) => {
                let mut r = WorldgenRandom::legacy(position_seed(info.pos.x, info.pos.y, info.pos.z));
                for rule in rules {
                    let matches = rule.input.test(info.state, info.pos.y, &mut r)
                        && (matches!(rule.location, RuleTest::True) || rule.location.test(w.get(info.pos), info.pos.y, &mut r))
                        && rule.position.test(info.pos, reference, &mut r);
                    if matches {
                        let nbt = rule.modifier.apply(&mut r, info.nbt);
                        return Some(Info { pos: info.pos, state: rule.output, nbt });
                    }
                }
                Some(info)
            }
            Processor::BlockRot { blocks: set, integrity } => settings.with_random(info.pos, |r| {
                if set.as_ref().is_some_and(|s| !s.has(info.state)) || r.next_f32() <= *integrity {
                    Some(info)
                } else {
                    None
                }
            }),
            Processor::Protected(set) => (!set.has(w.get(info.pos))).then_some(info),
            Processor::Capped { .. } | Processor::Nop => Some(info),
            Processor::BlockAge { mossiness, stairs, slabs, walls } => {
                let m = *mossiness;
                let state = info.state;
                let name = blocks::block_of_state(state).name;
                let new_state = settings.with_random(info.pos, |r| {
                    let random_stairs = |r: &mut WorldgenRandom, block: &str| {
                        let facing = [Dir::North, Dir::East, Dir::South, Dir::West][r.next_bounded(4) as usize];
                        let half = ["top", "bottom"][r.next_bounded(2) as usize];
                        let s = blocks::with_property(st(block), "facing", facing.name()).unwrap_or(st(block));
                        blocks::with_property(s, "half", half).unwrap_or(s)
                    };
                    let pick = |r: &mut WorldgenRandom, non_mossy: &[u32], mossy: &[u32]| {
                        if r.next_f32() < m {
                            mossy[r.next_bounded(mossy.len() as i32) as usize]
                        } else {
                            non_mossy[r.next_bounded(non_mossy.len() as i32) as usize]
                        }
                    };
                    if matches!(name, "minecraft:stone_bricks" | "minecraft:stone" | "minecraft:chiseled_stone_bricks") {
                        if r.next_f32() >= 0.5 {
                            return None;
                        }
                        let non_mossy = [st("minecraft:cracked_stone_bricks"), random_stairs(r, "minecraft:stone_brick_stairs")];
                        let mossy = [st("minecraft:mossy_stone_bricks"), random_stairs(r, "minecraft:mossy_stone_brick_stairs")];
                        Some(pick(r, &non_mossy, &mossy))
                    } else if stairs.has(state) {
                        if r.next_f32() >= 0.5 {
                            return None;
                        }
                        let mossy = [with_properties_of("minecraft:mossy_stone_brick_stairs", state), st("minecraft:mossy_stone_brick_slab")];
                        Some(pick(r, &[st("minecraft:stone_slab"), st("minecraft:stone_brick_slab")], &mossy))
                    } else if slabs.has(state) {
                        (r.next_f32() < m).then(|| with_properties_of("minecraft:mossy_stone_brick_slab", state))
                    } else if walls.has(state) {
                        (r.next_f32() < m).then(|| with_properties_of("minecraft:mossy_stone_brick_wall", state))
                    } else if name == "minecraft:obsidian" {
                        (r.next_f32() < 0.15).then(|| st("minecraft:crying_obsidian"))
                    } else {
                        None
                    }
                });
                match new_state {
                    Some(s) => Some(Info { state: s, ..info }),
                    None => Some(info),
                }
            }
            Processor::BlackstoneReplace => {
                let Some(new_block) = blackstone_replacement(blocks::block_id_of_state(info.state)) else { return Some(info) };
                let mut s = st(&format!("minecraft:{new_block}"));
                for prop in ["facing", "half", "type"] {
                    if let Some(v) = blocks::property(info.state, prop) {
                        if let Some(n) = blocks::with_property(s, prop, v) {
                            s = n;
                        }
                    }
                }
                Some(Info { state: s, ..info })
            }
            Processor::LavaSubmerged => {
                let was_lava = blocks::block_of_state(w.get(info.pos)).name == "minecraft:lava";
                if was_lava && !b.is_full_cube(info.state) {
                    Some(Info { state: st("minecraft:lava"), ..info })
                } else {
                    Some(info)
                }
            }
        }
    }

    /// `finalizeProcessing`.
    pub fn finalize(&self, w: &World, position: Pos, reference: Pos, original: &[Pos], mut processed: Vec<Info>, settings: &mut PlaceSettings) -> Vec<Info> {
        let Processor::Capped { delegate, limit } = self else { return processed };
        if limit.max_inclusive() == 0 || processed.is_empty() || original.len() != processed.len() {
            return processed;
        }
        let mut r = WorldgenRandom::legacy(w.gen.seed);
        let positional = r.fork_positional();
        let mut r = WorldgenRandom::new(positional.at(position.x, position.y, position.z));
        let max = limit.sample(&mut r).min(processed.len() as i32);
        if max < 1 {
            return processed;
        }
        let mut indices: Vec<usize> = (0..processed.len()).collect();
        for i in (2..=indices.len()).rev() {
            let j = r.next_bounded(i as i32) as usize;
            indices.swap(i - 1, j);
        }
        let mut replaced = 0;
        for index in indices {
            if replaced >= max {
                break;
            }
            let info = processed[index].clone();
            if let Some(altered) = delegate.process(w, position, reference, original[index], info.clone(), settings) {
                if altered != info {
                    replaced += 1;
                    processed[index] = altered;
                }
            }
        }
        processed
    }
}

/// Processor parsing (`processor_list` entries and inline lists), caching lists by id.
#[derive(Default)]
pub(crate) struct ProcessorLists {
    lists: HashMap<String, Arc<Vec<Arc<Processor>>>>,
}

impl ProcessorLists {
    /// A processor list: an id, `{"processors": [...]}` or a bare list.
    pub fn list(&mut self, parser: &mut Parser, v: &Value) -> Result<Arc<Vec<Arc<Processor>>>> {
        match v {
            Value::String(id) => {
                let id = full_id(id);
                if let Some(l) = self.lists.get(&id) {
                    return Ok(l.clone());
                }
                let json = parser.data.worldgen("processor_list", &id)?;
                let l = self.list(parser, &json)?;
                self.lists.insert(id, l.clone());
                Ok(l)
            }
            Value::Object(o) => self.list(parser, o.get("processors").ok_or_else(|| anyhow!("processor list without processors"))?),
            Value::Array(a) => Ok(Arc::new(a.iter().map(|p| Self::processor(parser, p).map(Arc::new)).collect::<Result<_>>()?)),
            _ => bail!("bad processor list {v}"),
        }
    }

    pub fn processor(parser: &mut Parser, v: &Value) -> Result<Processor> {
        let t = v.str_field("processor_type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        Ok(match t {
            "block_ignore" => Processor::Ignore(
                v.field("blocks")?
                    .as_array()
                    .ok_or_else(|| anyhow!("bad blocks"))?
                    .iter()
                    .map(|b| crate::data::parse_block_state(b).map(blocks::block_id_of_state))
                    .collect::<Result<_>>()?,
            ),
            "jigsaw_replacement" => Processor::JigsawReplacement,
            "gravity" => Processor::Gravity {
                heightmap: Heightmap::parse(v.get("heightmap").and_then(Value::as_str).unwrap_or("WORLD_SURFACE_WG")).ok_or_else(|| anyhow!("bad heightmap"))?,
                offset: v.i64_or("offset", 0) as i32,
            },
            "rule" => {
                let mut rules = Vec::new();
                for r in v.field("rules")?.as_array().ok_or_else(|| anyhow!("bad rules"))? {
                    let position = match r.get("position_predicate") {
                        None => PosRule::True,
                        Some(p) => {
                            let pt = p.str_field("predicate_type")?;
                            let (min_chance, max_chance) = (p.f64_or("min_chance", 0.0) as f32, p.f64_or("max_chance", 0.0) as f32);
                            let (min_dist, max_dist) = (p.i64_or("min_dist", 0) as i32, p.i64_or("max_dist", 0) as i32);
                            match pt.strip_prefix("minecraft:").unwrap_or(pt) {
                                "always_true" => PosRule::True,
                                "linear_pos" => PosRule::Linear { min_chance, max_chance, min_dist, max_dist },
                                "axis_aligned_linear_pos" => {
                                    let axis = match p.get("axis").and_then(Value::as_str).unwrap_or("y") {
                                        "x" => Dir::East,
                                        "z" => Dir::South,
                                        _ => Dir::Up,
                                    };
                                    PosRule::AxisAligned { min_chance, max_chance, min_dist, max_dist, axis }
                                }
                                other => bail!("unknown position predicate {other}"),
                            }
                        }
                    };
                    let modifier = match r.get("block_entity_modifier") {
                        None => BlockEntityModifier::Passthrough,
                        Some(m) => {
                            let mt = m.str_field("type")?;
                            match mt.strip_prefix("minecraft:").unwrap_or(mt) {
                                "passthrough" => BlockEntityModifier::Passthrough,
                                "clear" => BlockEntityModifier::Clear,
                                "append_loot" => BlockEntityModifier::AppendLoot(full_id(m.str_field("loot_table")?)),
                                "append_static" => BlockEntityModifier::AppendStatic(json_to_compound(m.field("data")?)),
                                other => bail!("unknown block entity modifier {other}"),
                            }
                        }
                    };
                    rules.push(ProcessorRule {
                        input: parser.rule_test(r.field("input_predicate")?)?,
                        location: parser.rule_test(r.field("location_predicate")?)?,
                        position,
                        output: crate::data::parse_block_state(r.field("output_state")?)?,
                        modifier,
                    });
                }
                Processor::Rule(rules)
            }
            "block_rot" => Processor::BlockRot {
                blocks: match v.get("rottable_blocks") {
                    Some(b) => Some(parser.block_set(b)?),
                    None => None,
                },
                integrity: v.f64_or("integrity", 1.0) as f32,
            },
            "protected_blocks" => Processor::Protected(parser.block_set(v.field("value")?)?),
            "capped" => Processor::Capped { delegate: Box::new(Self::processor(parser, v.field("delegate")?)?), limit: parser.int_provider(v.field("limit")?)? },
            "block_age" => Processor::BlockAge {
                mossiness: v.f64_or("mossiness", 0.0) as f32,
                stairs: parser.tag("minecraft:stairs")?,
                slabs: parser.tag("minecraft:slabs")?,
                walls: parser.tag("minecraft:walls")?,
            },
            "blackstone_replace" => Processor::BlackstoneReplace,
            "lava_submerged_block" => Processor::LavaSubmerged,
            "nop" => Processor::Nop,
            other => bail!("unknown structure processor {other}"),
        })
    }
}

/// Static NBT written as JSON in processor rules (`append_static`).
fn json_to_compound(v: &Value) -> Compound {
    fn tag(v: &Value) -> Tag {
        match v {
            Value::Bool(b) => Tag::Byte(*b as i8),
            Value::Number(n) => match n.as_i64() {
                Some(i) if i32::try_from(i).is_ok() => Tag::Int(i as i32),
                Some(i) => Tag::Long(i),
                None => Tag::Double(n.as_f64().unwrap_or(0.0)),
            },
            Value::String(s) => Tag::String(s.clone()),
            Value::Array(a) => Tag::List(a.iter().map(tag).collect()),
            Value::Object(o) => Tag::Compound(o.iter().map(|(k, v)| (k.clone(), tag(v))).collect()),
            Value::Null => Tag::Byte(0),
        }
    }
    match tag(v) {
        Tag::Compound(c) => c,
        _ => Compound::new(),
    }
}

/// Built-in processors vanilla adds itself.
pub(crate) struct BuiltinProcessors {
    pub structure_block: Arc<Processor>,
    pub structure_and_air: Arc<Processor>,
    pub jigsaw_replacement: Arc<Processor>,
    pub gravity: Arc<Processor>,
}

impl BuiltinProcessors {
    pub fn new() -> Self {
        Self {
            structure_block: Arc::new(Processor::Ignore(vec![block("minecraft:structure_block")])),
            structure_and_air: Arc::new(Processor::Ignore(vec![block("minecraft:air"), block("minecraft:structure_block")])),
            jigsaw_replacement: Arc::new(Processor::JigsawReplacement),
            gravity: Arc::new(Processor::Gravity { heightmap: Heightmap::WorldSurfaceWg, offset: -1 }),
        }
    }
}
