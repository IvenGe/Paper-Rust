//! Features that place structure templates: `template` (desert wells, sulfur springs) and
//! `fossil`.
//!
//! Written to vanilla 26.3's `TemplateFeature` and `FossilFeature`.

use super::super::parse::Parser;
use super::super::world::{Dir, Pos, World};
use super::super::Place;
use crate::chunk::Heightmap;
use crate::data::JsonExt;
use crate::random::{Random, WorldgenRandom};
use crate::structure::bbox::BoundingBox;
use crate::structure::processor::Processor;
use crate::structure::template::{PlaceSettings, Template};
use crate::structure::transform::{Mirror, Rotation};
use anyhow::{anyhow, Result};
use paper_data::blocks;
use serde_json::Value;
use std::sync::Arc;

pub(crate) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(Some(match t {
        "template" => {
            let mut entries = Vec::new();
            for e in v.field("templates")?.as_array().ok_or_else(|| anyhow!("bad templates"))? {
                let data = e.field("data")?;
                let template = p.templates.load(p.data, data.str_field("id")?)?;
                let rotations = match data.get("rotations").and_then(Value::as_array) {
                    Some(list) => list.iter().filter_map(|r| r.as_str().and_then(Rotation::parse)).collect(),
                    None => Rotation::ALL.to_vec(),
                };
                entries.push((template, rotations, e.i64_or("weight", 1) as i32));
            }
            let processors = match v.get("processors") {
                Some(l) => {
                    let mut lists = std::mem::take(&mut p.processor_lists);
                    let l = lists.list(p, l);
                    p.processor_lists = lists;
                    l?
                }
                None => Arc::new(Vec::new()),
            };
            Box::new(TemplateFeature { entries, processors })
        }
        "fossil" => {
            let names = |k: &str| -> Result<Vec<String>> { Ok(v.field(k)?.as_array().ok_or_else(|| anyhow!("bad {k}"))?.iter().filter_map(|n| n.as_str().map(str::to_owned)).collect()) };
            let fossils = names("fossil_structures")?.iter().map(|n| p.templates.load(p.data, n)).collect::<Result<Vec<_>>>()?;
            let overlays = names("overlay_structures")?.iter().map(|n| p.templates.load(p.data, n)).collect::<Result<Vec<_>>>()?;
            if fossils.len() != overlays.len() || fossils.is_empty() {
                return Err(anyhow!("fossil and overlay structures differ in number"));
            }
            let mut lists = std::mem::take(&mut p.processor_lists);
            let fossil_processors = lists.list(p, v.field("fossil_processors")?);
            let overlay_processors = lists.list(p, v.field("overlay_processors")?);
            p.processor_lists = lists;
            Box::new(Fossil { fossils, overlays, fossil_processors: fossil_processors?, overlay_processors: overlay_processors?, max_empty_corners: v.i64_or("max_empty_corners_allowed", 0) as i32 })
        }
        _ => return Ok(None),
    }))
}

/// `TemplateFeature`.
struct TemplateFeature {
    entries: Vec<(Arc<Template>, Vec<Rotation>, i32)>,
    processors: Arc<Vec<Arc<Processor>>>,
}

impl Place for TemplateFeature {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let total: i32 = self.entries.iter().map(|e| e.2).sum();
        if total <= 0 {
            return false;
        }
        let mut i = r.next_bounded(total);
        let Some((template, rotations, _)) = self.entries.iter().find(|e| {
            i -= e.2;
            i < 0
        }) else {
            return false;
        };
        if rotations.is_empty() {
            return false;
        }
        let rotation = rotations[r.next_bounded(rotations.len() as i32) as usize];
        // `getRotatedOffset`: half the size against the rotated negative axis.
        let (wx, _, wz) = rotation.rotate(Dir::West).offset();
        let (nx, _, nz) = rotation.rotate(Dir::North).offset();
        let (hx, hz) = (template.size.0 / 2, template.size.2 / 2);
        let pos = Pos::new(origin.x + wx * hx + nx * hz, origin.y, origin.z + wz * hx + nz * hz);
        let mut settings = PlaceSettings { rotation, processors: self.processors.to_vec(), random: Some(r), ..PlaceSettings::default() };
        template.place_buffered(w, pos, pos, &mut settings, None, None)
    }
}

/// `FossilFeature`.
struct Fossil {
    fossils: Vec<Arc<Template>>,
    overlays: Vec<Arc<Template>>,
    fossil_processors: Arc<Vec<Arc<Processor>>>,
    overlay_processors: Arc<Vec<Arc<Processor>>>,
    max_empty_corners: i32,
}

impl Place for Fossil {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let rotation = Rotation::random(r);
        let index = r.next_bounded(self.fossils.len() as i32) as usize;
        let (base, overlay) = (&self.fossils[index], &self.overlays[index]);
        let (cx, cz) = (origin.x >> 4, origin.z >> 4);
        let bbox = BoundingBox::new((cx << 4) - 16, w.min_y, (cz << 4) - 16, (cx << 4) + 15 + 16, w.max_y(), (cz << 4) + 15 + 16);
        let (sx, _, sz) = base.size_rotated(rotation);
        let low = Pos::new(origin.x + (-sx) / 2, origin.y, origin.z + (-sz) / 2);
        let mut lowest = origin.y;
        for x in 0..sx {
            for z in 0..sz {
                lowest = lowest.min(w.height_at(Heightmap::OceanFloorWg, low.x + x, low.z + z));
            }
        }
        let y = (lowest - 15 - r.next_bounded(10)).max(w.min_y + 10);
        let target = base.zero_position_with_transform(Pos::new(low.x, y, low.z), Mirror::None, rotation);
        let bb = base.bounding_box(target, rotation, Pos::new(0, 0, 0), Mirror::None);
        let mut empty = 0;
        for x in [bb.min_x, bb.max_x] {
            for y in [bb.min_y, bb.max_y] {
                for z in [bb.min_z, bb.max_z] {
                    let name = blocks::block_of_state(w.get(Pos::new(x, y, z))).name;
                    if w.b.is_air(w.get(Pos::new(x, y, z))) || name == "minecraft:lava" || name == "minecraft:water" {
                        empty += 1;
                    }
                }
            }
        }
        if empty > self.max_empty_corners {
            return false;
        }
        for (template, processors) in [(base, &self.fossil_processors), (overlay, &self.overlay_processors)] {
            let mut settings = PlaceSettings { rotation, bbox: Some(bbox), processors: processors.to_vec(), random: Some(&mut *r), ..PlaceSettings::default() };
            template.place_buffered(w, target, target, &mut settings, None, None);
        }
        true
    }
}
