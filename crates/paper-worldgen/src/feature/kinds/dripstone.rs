//! Speleothems: pointed dripstone and sulfur spikes (`speleothem`), their clusters
//! (`speleothem_cluster`) and large dripstone columns.
//!
//! Written from vanilla 26.3 (`SpeleothemFeature`, `SpeleothemClusterFeature`,
//! `SpeleothemUtils`, `LargeDripstoneFeature`, `Column`); the Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin) versions of these features are placeholders.

use crate::chunk::Heightmap;
use crate::data::JsonExt;
use crate::feature::blockinfo::BlockSet;
use crate::feature::parse::Parser;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::Place;
use crate::math::{clamped_map, mth_cos, mth_sin};
use crate::providers::{FloatProvider, IntProvider};
use crate::random::{Random, WorldgenRandom};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub(super) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(Some(match t {
        "speleothem" => Box::new(Speleothem {
            blocks: Blocks::parse(p, v)?,
            chance_of_taller: v.f64_or("chance_of_taller_generation", 0.2) as f32,
            chance_of_directional_spread: v.f64_or("chance_of_directional_spread", 0.7) as f32,
            chance_of_spread_radius2: v.f64_or("chance_of_spread_radius2", 0.5) as f32,
            chance_of_spread_radius3: v.f64_or("chance_of_spread_radius3", 0.5) as f32,
        }),
        "speleothem_cluster" => Box::new(Cluster {
            blocks: Blocks::parse(p, v)?,
            search_range: v.i64_or("floor_to_ceiling_search_range", 12) as i32,
            height: p.int_provider(v.field("height")?)?,
            radius: p.int_provider(v.field("radius")?)?,
            max_height_diff: v.i64_or("max_stalagmite_stalactite_height_diff", 1) as i32,
            height_deviation: v.i64_or("height_deviation", 3) as i32,
            layer_thickness: p.int_provider(v.field("speleothem_block_layer_thickness")?)?,
            density: p.float_provider(v.field("density")?)?,
            wetness: p.float_provider(v.field("wetness")?)?,
            chance_at_max_distance: v.f64_or("chance_of_speleothem_at_max_distance_from_center", 0.1) as f32,
            max_distance_from_edge: v.i64_or("max_distance_from_edge_affecting_chance_of_speleothem", 3) as i32,
            max_distance_from_center: v.i64_or("max_distance_from_center_affecting_height_bias", 8) as i32,
        }),
        "large_dripstone" => Box::new(LargeDripstone {
            replaceable: p.block_set(v.field("replaceable_blocks")?)?,
            dripstone_block: p.block_id("minecraft:dripstone_block").map(|b| paper_data::blocks::BLOCKS[b as usize].default_state)?,
            search_range: v.i64_or("floor_to_ceiling_search_range", 30) as i32,
            column_radius: p.int_provider(v.field("column_radius")?)?,
            height_scale: p.float_provider(v.field("height_scale")?)?,
            max_radius_ratio: v.f64_or("max_column_radius_to_cave_height_ratio", 0.33) as f32,
            stalactite_bluntness: p.float_provider(v.field("stalactite_bluntness")?)?,
            stalagmite_bluntness: p.float_provider(v.field("stalagmite_bluntness")?)?,
            wind_speed: p.float_provider(v.field("wind_speed")?)?,
            min_radius_for_wind: v.i64_or("min_radius_for_wind", 4) as i32,
            min_bluntness_for_wind: v.f64_or("min_bluntness_for_wind", 0.6) as f32,
        }),
        _ => return Ok(None),
    }))
}

/// The base and pointed blocks of a speleothem and what its base may replace.
struct Blocks {
    base: u32,
    pointed: u32,
    replaceable: Arc<BlockSet>,
}

impl Blocks {
    fn parse(p: &mut Parser, v: &Value) -> Result<Self> {
        Ok(Self { base: p.state(v.field("base_block")?)?, pointed: p.state(v.field("pointed_block")?)?, replaceable: p.block_set(v.field("replaceable_blocks")?)? })
    }

    /// `SpeleothemUtils.isBase`.
    fn is_base(&self, w: &World, s: u32) -> bool {
        w.b.is_block(s, self.base) || self.replaceable.has(s)
    }

    /// `SpeleothemUtils.placeBaseBlockIfPossible`.
    fn place_base(&self, w: &mut World, p: Pos) -> bool {
        if self.replaceable.has(w.get(p)) {
            w.set(p, self.base);
            true
        } else {
            false
        }
    }

    /// `SpeleothemUtils.growSpeleothem`: a column of `height` pointed blocks from `start`
    /// towards `tip`, if the block behind `start` is a base.
    fn grow(&self, w: &mut World, start: Pos, tip: Dir, height: i32, merged_tip: bool) {
        if !self.is_base(w, w.get(start.rel(tip.opposite()))) {
            return;
        }
        let mut thicknesses = Vec::new();
        if height >= 3 {
            thicknesses.push("base");
            for _ in 0..height - 3 {
                thicknesses.push("middle");
            }
        }
        if height >= 2 {
            thicknesses.push("frustum");
        }
        if height >= 1 {
            thicknesses.push(if merged_tip { "tip_merge" } else { "tip" });
        }
        let b = w.b;
        let mut p = start;
        for thickness in thicknesses {
            let mut state = b.with(self.pointed, "vertical_direction", tip.name());
            state = b.with(state, "thickness", thickness);
            state = b.waterlogged(state, w.is_water_at(p));
            w.set(p, state);
            p = p.rel(tip);
        }
    }
}

/// `SpeleothemUtils.isEmptyOrWater`.
fn empty_or_water(w: &World, s: u32) -> bool {
    w.b.is_air(s) || w.b.is_block(s, w.b.known.water)
}

/// `SpeleothemUtils.isEmptyOrWaterOrLava`.
fn empty_water_or_lava(w: &World, s: u32) -> bool {
    empty_or_water(w, s) || w.b.is_block(s, w.b.known.lava)
}

/// `Column.scan`: from an open position, the solid edges above (ceiling) and below (floor)
/// within `range` blocks. `None` when `pos` itself is not open.
fn scan_column(w: &World, pos: Pos, range: i32, inside: impl Fn(u32) -> bool, edge: impl Fn(u32) -> bool) -> Option<(Option<i32>, Option<i32>)> {
    if !inside(w.get(pos)) {
        return None;
    }
    let scan = |d: Dir| {
        let mut p = pos;
        let mut i = 1;
        while i < range && inside(w.get(p)) {
            p = p.rel(d);
            i += 1;
        }
        edge(w.get(p)).then_some(p.y)
    };
    let ceiling = scan(Dir::Up);
    let floor = scan(Dir::Down);
    Some((floor, ceiling))
}

/// `SpeleothemUtils.getSpeleothemHeight`.
fn speleothem_height(distance: f64, radius: f64, scale: f64, bluntness: f64) -> f64 {
    let distance = if distance < bluntness { bluntness } else { distance };
    let r = distance / radius * 0.384;
    let part1 = 0.75 * r.powf(1.333_333_333_333_333_3);
    let part2 = r.powf(0.666_666_666_666_666_6);
    let part3 = 0.333_333_333_333_333_3 * r.ln();
    let height = (scale * (part1 - part2 - part3)).max(0.0);
    height / 0.384 * radius
}

/// `SpeleothemFeature`: a single stalactite or stalagmite with a patch of base blocks.
struct Speleothem {
    blocks: Blocks,
    chance_of_taller: f32,
    chance_of_directional_spread: f32,
    chance_of_spread_radius2: f32,
    chance_of_spread_radius3: f32,
}

impl Speleothem {
    fn patch_of_base_blocks(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) {
        self.blocks.place_base(w, pos);
        for d in Dir::HORIZONTAL {
            if r.next_f32() > self.chance_of_directional_spread {
                continue;
            }
            let p1 = pos.rel(d);
            self.blocks.place_base(w, p1);
            if r.next_f32() > self.chance_of_spread_radius2 {
                continue;
            }
            let p2 = p1.rel(Dir::random(r));
            self.blocks.place_base(w, p2);
            if r.next_f32() > self.chance_of_spread_radius3 {
                continue;
            }
            let p3 = p2.rel(Dir::random(r));
            self.blocks.place_base(w, p3);
        }
    }
}

impl Place for Speleothem {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let above = self.blocks.is_base(w, w.get(origin.up()));
        let below = self.blocks.is_base(w, w.get(origin.down()));
        let tip = match (above, below) {
            (true, true) => {
                if r.next_bool() {
                    Dir::Down
                } else {
                    Dir::Up
                }
            }
            (true, false) => Dir::Down,
            (false, true) => Dir::Up,
            (false, false) => return false,
        };
        self.patch_of_base_blocks(w, r, origin.rel(tip.opposite()));
        let height = if r.next_f32() < self.chance_of_taller && empty_or_water(w, w.get(origin.rel(tip))) { 2 } else { 1 };
        self.blocks.grow(w, origin, tip, height, false);
        true
    }
}

/// `SpeleothemClusterFeature`.
struct Cluster {
    blocks: Blocks,
    search_range: i32,
    height: IntProvider,
    radius: IntProvider,
    max_height_diff: i32,
    height_deviation: i32,
    layer_thickness: IntProvider,
    density: FloatProvider,
    wetness: FloatProvider,
    chance_at_max_distance: f32,
    max_distance_from_edge: i32,
    max_distance_from_center: i32,
}

impl Cluster {
    fn chance_of_speleothem(&self, x_radius: i32, z_radius: i32, dx: i32, dz: i32) -> f64 {
        let d = (x_radius - dx.abs()).min(z_radius - dz.abs());
        clamped_map(d as f32, 0.0, self.max_distance_from_edge as f32, self.chance_at_max_distance, 1.0) as f64
    }

    fn speleothem_height(&self, r: &mut WorldgenRandom, dx: i32, dz: i32, density: f32, max_height: i32) -> i32 {
        if r.next_f32() > density {
            return 0;
        }
        let distance = dx.abs() + dz.abs();
        let mean = clamped_map(distance as f64, 0.0, self.max_distance_from_center as f64, max_height as f64 / 2.0, 0.0) as f32;
        // `ClampedNormalFloat.sample` with `Mth.clamp` (no panic on an empty range).
        let n = mean + r.next_gaussian() as f32 * self.height_deviation as f32;
        (if n < 0.0 { 0.0 } else { n.min(max_height as f32) }) as i32
    }

    fn can_be_adjacent_to_water(w: &World, p: Pos) -> bool {
        let s = w.get(p);
        w.b.tags.base_stone_overworld.has(s) || w.b.fluid(s).is_water()
    }

    fn can_place_pool(&self, w: &World, p: Pos) -> bool {
        let s = w.get(p);
        if w.b.is_block(s, w.b.known.water) || w.b.is_block(s, self.blocks.base) || w.b.is_block(s, self.blocks.pointed) {
            return false;
        }
        if w.b.fluid(w.get(p.up())).is_water() {
            return false;
        }
        Dir::HORIZONTAL.iter().all(|&d| Self::can_be_adjacent_to_water(w, p.rel(d))) && Self::can_be_adjacent_to_water(w, p.down())
    }

    fn replace_with_base(&self, w: &mut World, first: Pos, count: i32, d: Dir) {
        let mut p = first;
        for _ in 0..count {
            if !self.blocks.place_base(w, p) {
                return;
            }
            p = p.rel(d);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place_column(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos, dx: i32, dz: i32, chance_of_water: f32, chance: f64, cluster_height: i32, density: f32) {
        let Some((base_floor, ceiling)) = scan_column(w, pos, self.search_range, |s| empty_or_water(w, s), |s| !empty_or_water(w, s)) else {
            return;
        };
        if ceiling.is_none() && base_floor.is_none() {
            return;
        }
        let want_pool = r.next_f32() < chance_of_water;
        let floor = match base_floor {
            Some(f) if want_pool && self.can_place_pool(w, pos.with_y(f)) => {
                w.set(pos.with_y(f), w.b.known.water);
                Some(f - 1)
            }
            other => other,
        };
        let want_stalactite = r.next_f64() < chance;
        let stalactite_height = match ceiling {
            Some(c) if want_stalactite && !w.b.is_block(w.get(pos.with_y(c)), w.b.known.lava) => {
                let thickness = self.layer_thickness.sample(r);
                self.replace_with_base(w, pos.with_y(c), thickness, Dir::Up);
                let max = match floor {
                    Some(f) => cluster_height.min(c - f),
                    None => cluster_height,
                };
                self.speleothem_height(r, dx, dz, density, max)
            }
            _ => 0,
        };
        let want_stalagmite = r.next_f64() < chance;
        let stalagmite_height = match floor {
            Some(f) if want_stalagmite && !w.b.is_block(w.get(pos.with_y(f)), w.b.known.lava) => {
                let thickness = self.layer_thickness.sample(r);
                self.replace_with_base(w, pos.with_y(f), thickness, Dir::Down);
                if ceiling.is_some() {
                    0.max(stalactite_height + r.next_between(-self.max_height_diff, self.max_height_diff))
                } else {
                    self.speleothem_height(r, dx, dz, density, cluster_height)
                }
            }
            _ => 0,
        };
        let (stalactite, stalagmite) = match (ceiling, floor) {
            (Some(c), Some(f)) if c - stalactite_height <= f + stalagmite_height => {
                let lowest_bottom = (c - stalactite_height).max(f + 1);
                let highest_top = (f + stalagmite_height).min(c - 1);
                let bottom = r.next_between(lowest_bottom, highest_top + 1);
                let top = bottom - 1;
                (c - bottom, top - f)
            }
            _ => (stalactite_height, stalagmite_height),
        };
        let column_height = match (ceiling, floor) {
            (Some(c), Some(f)) => Some(c - f - 1),
            _ => None,
        };
        let merge = r.next_bool() && stalactite > 0 && stalagmite > 0 && column_height == Some(stalactite + stalagmite);
        if let Some(c) = ceiling {
            self.blocks.grow(w, pos.with_y(c - 1), Dir::Down, stalactite, merge);
        }
        if let Some(f) = floor {
            self.blocks.grow(w, pos.with_y(f + 1), Dir::Up, stalagmite, merge);
        }
    }
}

impl Place for Cluster {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        if !empty_or_water(w, w.get(origin)) {
            return false;
        }
        let height = self.height.sample(r);
        let wetness = self.wetness.sample(r);
        let density = self.density.sample(r);
        let x_radius = self.radius.sample(r);
        let z_radius = self.radius.sample(r);
        for dx in -x_radius..=x_radius {
            for dz in -z_radius..=z_radius {
                let chance = self.chance_of_speleothem(x_radius, z_radius, dx, dz);
                self.place_column(w, r, origin.add(dx, 0, dz), dx, dz, wetness, chance, height, density);
            }
        }
        true
    }
}

/// `LargeDripstoneFeature`.
struct LargeDripstone {
    replaceable: Arc<BlockSet>,
    dripstone_block: u32,
    search_range: i32,
    column_radius: IntProvider,
    height_scale: FloatProvider,
    max_radius_ratio: f32,
    stalactite_bluntness: FloatProvider,
    stalagmite_bluntness: FloatProvider,
    wind_speed: FloatProvider,
    min_radius_for_wind: i32,
    min_bluntness_for_wind: f32,
}

/// `LargeDripstoneFeature.WindOffsetter`.
struct Wind {
    origin_y: i32,
    speed: Option<(f64, f64)>,
    max_offset: i32,
}

impl Wind {
    fn offset(&self, p: Pos) -> Pos {
        let Some((sx, sz)) = self.speed else { return p };
        let dy = (self.origin_y - p.y) as f64;
        let dx = ((sx * dy).floor() as i32).clamp(-self.max_offset, self.max_offset);
        let dz = ((sz * dy).floor() as i32).clamp(-self.max_offset, self.max_offset);
        p.add(dx, 0, dz)
    }
}

/// `LargeDripstoneFeature.LargeDripstone`.
struct Column {
    root: Pos,
    pointing_up: bool,
    radius: i32,
    bluntness: f64,
    scale: f64,
}

impl Column {
    fn height_at_radius(&self, r: f32) -> i32 {
        speleothem_height(r as f64, self.radius as f64, self.scale, self.bluntness) as i32
    }

    fn suitable_for_wind(&self, min_radius: i32, min_bluntness: f32) -> bool {
        self.radius >= min_radius && self.bluntness >= min_bluntness as f64
    }

    /// `isCircleMostlyEmbeddedInStone`.
    fn circle_embedded(w: &World, center: Pos, radius: i32) -> bool {
        if empty_water_or_lava(w, w.get(center)) {
            return false;
        }
        let increment = 6.0f32 / radius as f32;
        let mut angle = 0.0f32;
        while angle < std::f32::consts::TAU {
            let dx = (mth_cos(angle as f64) * radius as f32) as i32;
            let dz = (mth_sin(angle as f64) * radius as f32) as i32;
            if empty_water_or_lava(w, w.get(center.add(dx, 0, dz))) {
                return false;
            }
            angle += increment;
        }
        true
    }

    fn move_back_until_embedded(&mut self, w: &World, wind: &Wind) -> bool {
        while self.radius > 1 {
            let mut root = self.root;
            let tries = 10.min(self.height_at_radius(0.0));
            for _ in 0..tries {
                if w.b.is_block(w.get(root), w.b.known.lava) {
                    return false;
                }
                if Self::circle_embedded(w, wind.offset(root), self.radius) {
                    self.root = root;
                    return true;
                }
                root = root.rel(if self.pointing_up { Dir::Down } else { Dir::Up });
            }
            self.radius /= 2;
        }
        false
    }

    fn place_blocks(&self, w: &mut World, r: &mut WorldgenRandom, wind: &Wind, dripstone_block: u32) {
        for dx in -self.radius..=self.radius {
            for dz in -self.radius..=self.radius {
                let current = ((dx * dx + dz * dz) as f32).sqrt();
                if current > self.radius as f32 {
                    continue;
                }
                let mut height = self.height_at_radius(current);
                if height <= 0 {
                    continue;
                }
                if (r.next_f32() as f64) < 0.2 {
                    // `Mth.randomBetween(random, 0.8f, 1.0f)`.
                    height = (height as f32 * (r.next_f32() * (1.0f32 - 0.8f32) + 0.8f32)) as i32;
                }
                let mut p = self.root.add(dx, 0, dz);
                let mut out_of_stone = false;
                let max_y = if self.pointing_up { w.height_at(Heightmap::WorldSurfaceWg, p.x, p.z) } else { i32::MAX };
                for _ in 0..height {
                    if p.y >= max_y {
                        break;
                    }
                    let adjusted = wind.offset(p);
                    let s = w.get(adjusted);
                    if empty_water_or_lava(w, s) {
                        out_of_stone = true;
                        w.set(adjusted, dripstone_block);
                    } else if out_of_stone && w.b.tags.base_stone_overworld.has(s) {
                        break;
                    }
                    p = p.rel(if self.pointing_up { Dir::Up } else { Dir::Down });
                }
            }
        }
    }
}

impl Place for LargeDripstone {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        if !empty_or_water(w, w.get(origin)) {
            return false;
        }
        let edge = |s: u32| w.b.is_block(s, self.dripstone_block) || self.replaceable.has(s) || w.b.is_block(s, w.b.known.lava);
        let Some((Some(floor), Some(ceiling))) = scan_column(w, origin, self.search_range, |s| empty_or_water(w, s), edge) else {
            return false;
        };
        let range_height = ceiling - floor - 1;
        if range_height < 4 {
            return false;
        }
        let (min_radius, max_radius) = (self.column_radius.min_value(), self.column_radius.max_value());
        let max_column_radius = ((range_height as f32 * self.max_radius_ratio) as i32).clamp(min_radius, max_radius);
        let radius = r.next_between(min_radius, max_column_radius);
        let mut stalactite = Column {
            root: origin.with_y(ceiling - 1),
            pointing_up: false,
            radius,
            bluntness: self.stalactite_bluntness.sample(r) as f64,
            scale: self.height_scale.sample(r) as f64,
        };
        let mut stalagmite = Column {
            root: origin.with_y(floor + 1),
            pointing_up: true,
            radius,
            bluntness: self.stalagmite_bluntness.sample(r) as f64,
            scale: self.height_scale.sample(r) as f64,
        };
        let wind = if stalactite.suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
            && stalagmite.suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
        {
            let speed = self.wind_speed.sample(r);
            let direction = r.next_f32() * std::f32::consts::PI;
            Wind {
                origin_y: origin.y,
                speed: Some(((mth_cos(direction as f64) * speed) as f64, (mth_sin(direction as f64) * speed) as f64)),
                max_offset: 16 - radius,
            }
        } else {
            Wind { origin_y: 0, speed: None, max_offset: 0 }
        };
        let stalactite_ok = stalactite.move_back_until_embedded(w, &wind);
        let stalagmite_ok = stalagmite.move_back_until_embedded(w, &wind);
        if stalactite_ok {
            stalactite.place_blocks(w, r, &wind, self.dripstone_block);
        }
        if stalagmite_ok {
            stalagmite.place_blocks(w, r, &wind, self.dripstone_block);
        }
        true
    }
}
