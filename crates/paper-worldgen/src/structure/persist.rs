//! Structure starts saved with their chunk and read back: vanilla's chunk NBT `structures`
//! compound (`starts`, keyed by structure id, and `References`).
//!
//! Written to vanilla 26.3's `SerializableChunkData.packStructureData`,
//! `unpackStructureStart` and `unpackStructureReferences`, `StructureStart.createTag` and
//! `loadStaticStart`, `StructurePiece.createTag` and the `addAdditionalSaveData` and NBT
//! constructors of every piece type (each structure module saves and reads its own
//! pieces). Ocean monuments save their building only and rebuild its rooms from the seed,
//! as vanilla's `regeneratePiecesAfterLoad` does.
//!
//! Loaded starts replace the ones the generator would compute, so the state pieces settle
//! when first placed (the height of temples and shipwrecks, chests and spawners already
//! placed) carries over a restart.

use super::bbox::BoundingBox;
use super::piece::PieceBase;
use super::placement::pack_chunk;
use super::{Piece, StructureStart, Structures};
use crate::data::full_id;
use crate::feature::world::Dir;
use crate::generator::WorldGenerator;
use anyhow::{anyhow, Result};
use paper_nbt::{Compound, Tag};
use serde_json::Value;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

/// `BoundingBox.CODEC`: six ints.
pub(crate) fn bbox_tag(b: &BoundingBox) -> Tag {
    Tag::IntArray(vec![b.min_x, b.min_y, b.min_z, b.max_x, b.max_y, b.max_z])
}

pub(crate) fn read_bbox(t: &Tag) -> Result<BoundingBox> {
    let v: Vec<i32> = match t {
        Tag::IntArray(a) => a.clone(),
        Tag::List(l) => l.iter().filter_map(Tag::as_i32).collect(),
        _ => Vec::new(),
    };
    if v.len() != 6 {
        return Err(anyhow!("bad bounding box {t:?}"));
    }
    Ok(BoundingBox::new(v[0], v[1], v[2], v[3], v[4], v[5]))
}

/// `Direction.get2DDataValue`, -1 for none.
pub(crate) fn orientation_value(d: Option<Dir>) -> i32 {
    match d {
        Some(Dir::South) => 0,
        Some(Dir::West) => 1,
        Some(Dir::North) => 2,
        Some(Dir::East) => 3,
        _ => -1,
    }
}

/// `Direction.from2DDataValue`.
pub(crate) fn from_2d(v: i32) -> Dir {
    [Dir::South, Dir::West, Dir::North, Dir::East][v.rem_euclid(4) as usize]
}

pub(crate) fn get_bool(t: &Compound, key: &str) -> bool {
    t.get(key).and_then(Tag::as_i64).is_some_and(|v| v != 0)
}

pub(crate) fn get_i32(t: &Compound, key: &str) -> i32 {
    t.get(key).and_then(Tag::as_i64).unwrap_or(0) as i32
}

pub(crate) fn get_str<'a>(t: &'a Compound, key: &str) -> &'a str {
    t.string(key).unwrap_or("")
}

/// A piece's common fields (`id`, `BB`, `O`, `GD`), its id one of `ids`.
pub(crate) fn read_base(t: &Compound, ids: &[&'static str]) -> Result<PieceBase> {
    let id = full_id(get_str(t, "id"));
    let kind = ids.iter().copied().find(|k| *k == id).ok_or_else(|| anyhow!("unexpected piece type {id}"))?;
    let bbox = read_bbox(t.get("BB").ok_or_else(|| anyhow!("piece {id} without BB"))?)?;
    let o = t.get("O").and_then(Tag::as_i64).unwrap_or(-1) as i32;
    let orientation = (o >= 0).then(|| from_2d(o));
    Ok(PieceBase::new(kind, get_i32(t, "GD"), bbox).with_orientation(orientation))
}

/// `StructurePiece.createTag`.
pub(crate) fn piece_tag(p: &dyn Piece) -> Compound {
    let b = p.base();
    let mut t = Compound::new();
    t.insert("id", b.kind);
    t.insert("BB", bbox_tag(&p.bbox()));
    t.insert("O", orientation_value(b.orientation));
    t.insert("GD", b.gen_depth);
    p.save(&mut t);
    t
}

/// A codec value (a pool element) as NBT: objects as compounds, arrays as lists, booleans as
/// bytes, whole numbers as ints, others as floats.
pub(crate) fn json_to_nbt(v: &Value) -> Tag {
    match v {
        Value::Null => Tag::Compound(Compound::new()),
        Value::Bool(b) => Tag::Byte(*b as i8),
        Value::Number(n) => match n.as_i64() {
            Some(i) if i32::try_from(i).is_ok() => Tag::Int(i as i32),
            Some(i) => Tag::Long(i),
            None => Tag::Float(n.as_f64().unwrap_or(0.0) as f32),
        },
        Value::String(s) => Tag::String(s.clone()),
        Value::Array(a) => Tag::List(a.iter().map(json_to_nbt).collect()),
        Value::Object(o) => {
            let mut c = Compound::new();
            for (k, v) in o {
                c.insert(k.clone(), json_to_nbt(v));
            }
            Tag::Compound(c)
        }
    }
}

/// NBT read back as a codec value (bytes as booleans).
pub(crate) fn nbt_to_json(t: &Tag) -> Value {
    match t {
        Tag::Byte(b) => Value::Bool(*b != 0),
        Tag::Short(v) => Value::from(*v),
        Tag::Int(v) => Value::from(*v),
        Tag::Long(v) => Value::from(*v),
        Tag::Float(v) => Value::from(*v as f64),
        Tag::Double(v) => Value::from(*v),
        Tag::String(s) => Value::String(s.clone()),
        Tag::List(l) => Value::Array(l.iter().map(nbt_to_json).collect()),
        Tag::Compound(c) => Value::Object(c.iter().map(|(k, v)| (k.clone(), nbt_to_json(v))).collect()),
        Tag::ByteArray(a) => Value::Array(a.iter().map(|b| Value::from(*b as i8)).collect()),
        Tag::IntArray(a) => Value::Array(a.iter().map(|v| Value::from(*v)).collect()),
        Tag::LongArray(a) => Value::Array(a.iter().map(|v| Value::from(*v)).collect()),
    }
}

/// A comparison key for codec values that survives the trip through NBT: sorted keys,
/// whole numbers as integers, others at float precision, ids without `minecraft:`.
pub(crate) fn value_key(v: &Value) -> String {
    let mut out = String::new();
    fn go(v: &Value, out: &mut String) {
        match v {
            Value::Null => out.push('n'),
            Value::Bool(b) => out.push_str(if *b { "t" } else { "f" }),
            Value::Number(n) => {
                let f = n.as_f64().unwrap_or(0.0);
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    out.push_str(&format!("{}", f as i64));
                } else {
                    out.push_str(&format!("{:?}", f as f32));
                }
            }
            Value::String(s) => {
                out.push('"');
                out.push_str(s.strip_prefix("minecraft:").unwrap_or(s));
                out.push('"');
            }
            Value::Array(a) => {
                out.push('[');
                for e in a {
                    go(e, out);
                    out.push(',');
                }
                out.push(']');
            }
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                out.push('{');
                for k in keys {
                    out.push_str(k);
                    out.push(':');
                    go(&o[k], out);
                    out.push(',');
                }
                out.push('}');
            }
        }
    }
    go(v, &mut out);
    out
}

impl StructureStart {
    /// `createTag`: the start as vanilla saves it in its chunk (`id`, `ChunkX`, `ChunkZ`,
    /// `references` and the pieces as `Children`).
    pub fn to_nbt(&self) -> Compound {
        let mut c = Compound::new();
        c.insert("id", &*self.id);
        c.insert("ChunkX", self.chunk_x);
        c.insert("ChunkZ", self.chunk_z);
        c.insert("references", self.references.load(Ordering::Relaxed));
        c.insert("Children", Tag::List(self.pieces.iter().map(|p| Tag::Compound(piece_tag(p.as_ref()))).collect()));
        c
    }

    /// How often the start was referenced by maps and `/locate` with `skipKnown`
    /// (`StructureStart.references`, saved with it).
    pub fn references(&self) -> i32 {
        self.references.load(Ordering::Relaxed)
    }

    /// `addReference`.
    pub fn add_reference(&self) {
        self.references.fetch_add(1, Ordering::Relaxed);
    }
}

/// What [`WorldGenerator::load_structures`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedStructures {
    /// The structures whose starts were read and now replace the computed ones.
    pub starts: Vec<String>,
    /// Starts whose pieces could not be read (unknown piece types or pool elements, e.g.
    /// from a data pack), with the reason: they were computed from the seed instead.
    pub recomputed: Vec<(String, String)>,
    /// Starts of structures this generator does not know, dropped.
    pub unknown: Vec<String>,
}

impl Structures {
    /// The structure sets that hold a structure.
    fn sets_of(&self, structure: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.sets.len()).filter(move |&s| self.sets[s].entries.iter().any(|(i, _)| *i == structure))
    }

    /// `StructureStart.loadStaticStart` for a start of `structure` in chunk `(x, z)`.
    pub(crate) fn read_start(&self, gen: &WorldGenerator, structure: usize, x: i32, z: i32, t: &Compound) -> Result<StructureStart> {
        let def = &self.defs[structure];
        let children: Vec<&Compound> = t.list("Children").unwrap_or(&[]).iter().filter_map(Tag::as_compound).collect();
        let pieces = def.generator.load_pieces(gen, x, z, &children)?;
        let bb = BoundingBox::encapsulating(pieces.iter().map(|p| p.bbox())).ok_or_else(|| anyhow!("a start without pieces"))?;
        let bbox = if def.adaptation != super::TerrainAdjustment::None { bb.inflated(12) } else { bb };
        Ok(StructureStart { structure, id: def.id.clone(), chunk_x: x, chunk_z: z, pieces, bbox, references: AtomicI32::new(get_i32(t, "references")) })
    }

    /// The `structures` compound of a chunk.
    pub(crate) fn save_chunk(&self, gen: &WorldGenerator, x: i32, z: i32) -> Compound {
        let mut starts = Compound::new();
        for start in self.starts_in_chunk(gen, x, z) {
            starts.insert(start.id().to_owned(), start.to_nbt());
        }
        let mut references = Compound::new();
        if self.enabled {
            for (structure, refs) in self.references(gen, x, z, |_| true) {
                let packed: Vec<i64> = refs.iter().map(|s| pack_chunk(s.chunk_x, s.chunk_z)).collect();
                references.insert(self.defs[structure].id.to_string(), Tag::LongArray(packed));
            }
        }
        let mut out = Compound::new();
        out.insert("starts", starts);
        out.insert("References", references);
        out
    }

    /// Reads the `structures` compound of a chunk and puts its starts into the start cache,
    /// in place of what the generator would compute there: the chunk's starts are exactly
    /// the saved ones.
    pub(crate) fn load_chunk(&self, gen: &WorldGenerator, x: i32, z: i32, nbt: &Compound) -> LoadedStructures {
        let mut report = LoadedStructures::default();
        let Some(starts) = nbt.compound("starts") else { return report };
        let mut filled: Vec<usize> = Vec::new();
        for (key, tag) in starts.iter() {
            let Some(t) = tag.as_compound() else { continue };
            let id = full_id(t.string("id").unwrap_or(key));
            if id == "minecraft:INVALID" || id == "INVALID" {
                continue;
            }
            let Some(structure) = self.structure_index(&id) else {
                report.unknown.push(id);
                continue;
            };
            let start = match self.read_start(gen, structure, x, z, t) {
                Ok(s) => Arc::new(s),
                Err(e) => {
                    report.recomputed.push((id.clone(), format!("{e:#}")));
                    let computed = self.sets_of(structure).find_map(|set| self.compute(gen, set, x, z).filter(|s| s.structure == structure));
                    match computed {
                        Some(s) => s,
                        None => continue,
                    }
                }
            };
            // Under the set whose placement starts here (any set of the structure otherwise).
            let set = self.sets_of(structure).find(|&s| self.sets[s].possible && self.is_structure_chunk(gen, s, x, z)).or_else(|| self.sets_of(structure).next());
            if let Some(set) = set {
                self.starts.put((set as u16, x, z), Some(start));
                filled.push(set);
                report.starts.push(id);
            }
        }
        // Sets that could start here but have no saved start: none.
        for set in 0..self.sets.len() {
            if self.sets[set].possible && !filled.contains(&set) && self.is_structure_chunk(gen, set, x, z) {
                self.starts.put((set as u16, x, z), None);
            }
        }
        report
    }

    /// The chunks within 8 chunks of `(x, z)` that may hold a start reaching into it (the
    /// placement candidates `createReferences` looks at).
    pub(crate) fn start_chunks_near(&self, gen: &WorldGenerator, x: i32, z: i32) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        if !self.enabled {
            return out;
        }
        for (si, set) in self.sets.iter().enumerate() {
            if !set.possible {
                continue;
            }
            for c in self.candidates(gen, si, x, z, 8) {
                if !out.contains(&c) && self.is_structure_chunk(gen, si, c.0, c.1) {
                    out.push(c);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_survive_nbt() {
        let v: Value = serde_json::json!({"element_type": "minecraft:single_pool_element", "location": "minecraft:a/b", "processors": {"processors": [{"processor_type": "minecraft:rule", "probability": 0.1, "limit": 5, "flag": true}]}, "projection": "rigid"});
        let back = nbt_to_json(&json_to_nbt(&v));
        assert_eq!(value_key(&v), value_key(&back));
        let mut stripped = v.clone();
        stripped["location"] = Value::String("a/b".into());
        assert_eq!(value_key(&v), value_key(&stripped));
    }
}
