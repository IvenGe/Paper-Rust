//! Surface features without a data-driven form: forest rocks (`block_blob`), blue ice,
//! ice spikes (`spike`), icebergs, huge mushrooms and coral trees and claws.
//!
//! Derived from `pumpkin-world/src/generation/feature/features/{forest_rock,blue_ice,
//! ice_spike,iceberg,huge_brown_mushroom,huge_red_mushroom,coral/*}.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, and checked against vanilla 26.3
//! (`BlockBlobFeature`, `BlueIceFeature`, `SpikeFeature`, `IcebergFeature`,
//! `AbstractHugeMushroomFeature` and its two implementations, `CoralTreeFeature`,
//! `CoralClawFeature`), which differs from Pumpkin in a few random draws.

use crate::data::JsonExt;
use crate::feature::parse::Parser;
use crate::feature::placement::PlacedFeature;
use crate::feature::predicate::BlockPredicate;
use crate::feature::provider::StateProvider;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::Place;
use crate::random::{Random, WorldgenRandom};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub(super) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(Some(match t {
        "block_blob" => Box::new(BlockBlob { state: p.state(v.field("state")?)?, can_place_on: p.predicate(v.field("can_place_on")?)? }),
        "blue_ice" => Box::new(BlueIce),
        "spike" => Box::new(Spike {
            state: p.state(v.field("state")?)?,
            can_place_on: p.predicate(v.field("can_place_on")?)?,
            can_replace: p.predicate(v.field("can_replace")?)?,
        }),
        "iceberg" => Box::new(Iceberg { state: p.state(v.field("state")?)? }),
        "huge_brown_mushroom" | "huge_red_mushroom" => Box::new(HugeMushroom {
            red: t == "huge_red_mushroom",
            cap: p.provider(v.field("cap_provider")?)?,
            stem: p.provider(v.field("stem_provider")?)?,
            foliage_radius: v.i64_or("foliage_radius", 2) as i32,
            can_place_on: p.predicate(v.field("can_place_on")?)?,
        }),
        "coral_tree" => Box::new(CoralTree(p.placed(v.field("feature")?)?)),
        "coral_claw" => Box::new(CoralClaw(p.placed(v.field("feature")?)?)),
        _ => return Ok(None),
    }))
}

/// `Util.shuffle`: Fisher-Yates from the end with `nextInt(i)`.
fn shuffle<T>(r: &mut WorldgenRandom, v: &mut [T]) {
    for i in (2..=v.len()).rev() {
        let j = r.next_bounded(i as i32) as usize;
        v.swap(i - 1, j);
    }
}

/// `BlockBlobFeature`: the mossy cobblestone rocks of old growth taigas.
struct BlockBlob {
    state: u32,
    can_place_on: BlockPredicate,
}

impl Place for BlockBlob {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let mut origin = origin;
        while origin.y > w.min_y + 3 {
            if self.can_place_on.test(w, origin.down()) {
                break;
            }
            origin = origin.down();
        }
        if origin.y <= w.min_y + 3 {
            return false;
        }
        for _ in 0..3 {
            let xr = r.next_bounded(2);
            let yr = r.next_bounded(2);
            let zr = r.next_bounded(2);
            let tr = (xr + yr + zr) as f32 * 0.333 + 0.5;
            let limit = (tr * tr) as f64;
            for z in origin.z - zr..=origin.z + zr {
                for y in origin.y - yr..=origin.y + yr {
                    for x in origin.x - xr..=origin.x + xr {
                        let p = Pos::new(x, y, z);
                        if p.dist_sq(origin) <= limit {
                            w.set(p, self.state);
                        }
                    }
                }
            }
            let dx = -1 + r.next_bounded(2);
            let dy = -r.next_bounded(2);
            let dz = -1 + r.next_bounded(2);
            origin = origin.add(dx, dy, dz);
        }
        true
    }
}

/// `BlueIceFeature`: blue ice under packed ice in frozen oceans.
struct BlueIce;

impl Place for BlueIce {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let k = &w.b.known;
        let (water, packed_ice, ice, blue_ice) = (k.water, k.packed_ice, k.ice, k.blue_ice);
        if origin.y > w.sea_level - 1 {
            return false;
        }
        if !w.b.is_block(w.get(origin), water) && !w.b.is_block(w.get(origin.down()), water) {
            return false;
        }
        let found = Dir::ALL.iter().any(|&d| d != Dir::Down && w.b.is_block(w.get(origin.rel(d)), packed_ice));
        if !found {
            return false;
        }
        w.set(origin, blue_ice);
        for _ in 0..200 {
            let y_off = r.next_bounded(5) - r.next_bounded(6);
            let mut xz = 3;
            if y_off < 2 {
                xz += y_off / 2;
            }
            if xz < 1 {
                continue;
            }
            let dx = r.next_bounded(xz) - r.next_bounded(xz);
            let dz = r.next_bounded(xz) - r.next_bounded(xz);
            let p = origin.add(dx, y_off, dz);
            let s = w.get(p);
            let open = w.b.is_air(s) || w.b.is_block(s, water) || w.b.is_block(s, packed_ice) || w.b.is_block(s, ice);
            if open && Dir::ALL.iter().any(|&d| w.b.is_block(w.get(p.rel(d)), blue_ice)) {
                w.set(p, blue_ice);
            }
        }
        true
    }
}

/// `SpikeFeature`: the packed ice spikes of ice spike plains.
struct Spike {
    state: u32,
    can_place_on: BlockPredicate,
    can_replace: BlockPredicate,
}

impl Spike {
    fn try_set(&self, w: &mut World, p: Pos) {
        if w.is_air(p) || self.can_replace.test(w, p) {
            w.set(p, self.state);
        }
    }
}

impl Place for Spike {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let mut origin = origin;
        while w.is_air(origin) && origin.y > w.min_y + 2 {
            origin = origin.down();
        }
        if !self.can_place_on.test(w, origin) {
            return false;
        }
        origin = origin.up_n(r.next_bounded(4));
        let height = r.next_bounded(4) + 7;
        let width = height / 4 + r.next_bounded(2);
        if width > 1 && r.next_bounded(60) == 0 {
            origin = origin.up_n(10 + r.next_bounded(30));
        }
        for y_off in 0..height {
            let scale = (1.0 - y_off as f32 / height as f32) * width as f32;
            let new_width = scale.ceil() as i32;
            for xo in -new_width..=new_width {
                let dx = xo.abs() as f32 - 0.25;
                for zo in -new_width..=new_width {
                    let dz = zo.abs() as f32 - 0.25;
                    if (xo != 0 || zo != 0) && dx * dx + dz * dz > scale * scale {
                        continue;
                    }
                    if (xo == -new_width || xo == new_width || zo == -new_width || zo == new_width) && r.next_f32() > 0.75 {
                        continue;
                    }
                    self.try_set(w, origin.add(xo, y_off, zo));
                    if y_off != 0 && new_width > 1 {
                        self.try_set(w, origin.add(xo, -y_off, zo));
                    }
                }
            }
        }
        let pillar = (width - 1).clamp(0, 1);
        for xo in -pillar..=pillar {
            for zo in -pillar..=pillar {
                let mut cursor = origin.add(xo, -1, zo);
                let mut run = 50;
                if xo.abs() == 1 && zo.abs() == 1 {
                    run = r.next_bounded(5);
                }
                while cursor.y > 50 {
                    let s = w.get(cursor);
                    if !w.b.is_air(s) && !self.can_replace.test(w, cursor) && s != self.state {
                        break;
                    }
                    w.set(cursor, self.state);
                    cursor = cursor.down();
                    run -= 1;
                    if run <= 0 {
                        cursor = cursor.add(0, -(r.next_bounded(5) + 1), 0);
                        run = r.next_bounded(5);
                    }
                }
            }
        }
        true
    }
}

/// `IcebergFeature`.
struct Iceberg {
    state: u32,
}

struct IcebergShape {
    origin: Pos,
    snow_on_top: bool,
    angle: f64,
    ellipse_a: i32,
    ellipse_c: i32,
    ellipse: bool,
}

impl Iceberg {
    fn is_iceberg(w: &World, s: u32) -> bool {
        let k = &w.b.known;
        w.b.is_block(s, k.packed_ice) || w.b.is_block(s, k.snow_block) || w.b.is_block(s, k.blue_ice)
    }

    fn radius_round(r: &mut WorldgenRandom, y_off: i32, height: i32, width: i32) -> i32 {
        let k = 3.5 - r.next_f32();
        let mut scale = (1.0 - (y_off as f64 * y_off as f64) as f32 / (height as f32 * k)) * width as f32;
        if height > 15 + r.next_bounded(5) {
            let temp = if y_off < 3 + r.next_bounded(6) { y_off / 2 } else { y_off };
            scale = (1.0 - temp as f32 / (height as f32 * k * 0.4)) * width as f32;
        }
        (scale / 2.0).ceil() as i32
    }

    fn radius_ellipse(y_off: i32, height: i32, width: i32) -> i32 {
        let scale = (1.0 - (y_off as f64 * y_off as f64) as f32 / (height as f32 * 1.0)) * width as f32;
        (scale / 2.0).ceil() as i32
    }

    fn radius_steep(r: &mut WorldgenRandom, y_off: i32, height: i32, width: i32) -> i32 {
        let k = 1.0 + r.next_f32() / 2.0;
        let scale = (1.0 - y_off as f32 / (height as f32 * k)) * width as f32;
        (scale / 2.0).ceil() as i32
    }

    fn ellipse_c(y_off: i32, height: i32, c: i32) -> i32 {
        if y_off > 0 && height - y_off <= 3 {
            c - (4 - (height - y_off))
        } else {
            c
        }
    }

    fn signed_distance_circle(xo: i32, zo: i32, radius: i32, r: &mut WorldgenRandom) -> f64 {
        let off = 10.0 * r.next_f32().clamp(0.2, 0.8) / radius as f32;
        off as f64 + (xo as f64) * (xo as f64) + (zo as f64) * (zo as f64) - (radius as f64) * (radius as f64)
    }

    fn signed_distance_ellipse(xo: i32, zo: i32, ox: i32, oz: i32, a: i32, c: i32, angle: f64) -> f64 {
        let (dx, dz) = ((xo - ox) as f64, (zo - oz) as f64);
        let u = (dx * angle.cos() - dz * angle.sin()) / a as f64;
        let v = (dx * angle.sin() + dz * angle.cos()) / c as f64;
        u * u + v * v - 1.0
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_block(&self, w: &mut World, r: &mut WorldgenRandom, s: &IcebergShape, height: i32, xo: i32, y_off: i32, zo: i32, radius: i32, a: i32) {
        let dist = if s.ellipse {
            Self::signed_distance_ellipse(xo, zo, 0, 0, a, Self::ellipse_c(y_off, height, s.ellipse_c), s.angle)
        } else {
            Self::signed_distance_circle(xo, zo, radius, r)
        };
        if dist >= 0.0 {
            return;
        }
        let p = s.origin.add(xo, y_off, zo);
        let compare = if s.ellipse { -0.5 } else { (-6 - r.next_bounded(3)) as f64 };
        if dist > compare && r.next_f64() > 0.9 {
            return;
        }
        self.set_block(w, r, p, height - y_off, height, s);
    }

    fn set_block(&self, w: &mut World, r: &mut WorldgenRandom, p: Pos, h_diff: i32, height: i32, s: &IcebergShape) {
        let k = &w.b.known;
        let (snow_block, ice, water) = (k.snow_block, k.ice, k.water);
        let state = w.get(p);
        if !(w.b.is_air(state) || w.b.is_block(state, snow_block) || w.b.is_block(state, ice) || w.b.is_block(state, water)) {
            return;
        }
        let randomness = !s.ellipse || r.next_f64() > 0.05;
        let divisor = if s.ellipse { 3 } else { 2 };
        if s.snow_on_top
            && !w.b.is_block(state, water)
            && (h_diff as f64) <= r.next_bounded(1.max(height / divisor)) as f64 + height as f64 * 0.6
            && randomness
        {
            w.set(p, snow_block);
        } else {
            w.set(p, self.state);
        }
    }

    fn smooth(w: &mut World, s: &IcebergShape, width: i32, height: i32) {
        let a = if s.ellipse { s.ellipse_a } else { width / 2 };
        let (air, snow) = (w.b.known.air, w.b.known.snow);
        for x in -a..=a {
            for z in -a..=a {
                for y_off in 0..=height {
                    let p = s.origin.add(x, y_off, z);
                    let state = w.get(p);
                    if !(Self::is_iceberg(w, state) || w.b.is_block(state, snow)) {
                        continue;
                    }
                    if w.is_air(p.down()) {
                        w.set(p, air);
                        w.set(p.up(), air);
                    } else if Self::is_iceberg(w, state) {
                        let sides = [p.rel(Dir::West), p.rel(Dir::East), p.rel(Dir::North), p.rel(Dir::South)];
                        let open = sides.iter().filter(|&&q| !Self::is_iceberg(w, w.get(q))).count();
                        if open >= 3 {
                            w.set(p, air);
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn carve(w: &mut World, radius: i32, y_off: i32, s: &IcebergShape, under_water: bool, angle: f64, local: (i32, i32)) {
        let a = radius + 1 + s.ellipse_a / 3;
        let c = (radius - 3).min(3) + s.ellipse_c / 2 - 1;
        let k = &w.b.known;
        let (snow_block, water, air, snow) = (k.snow_block, k.water, k.air, k.snow);
        for xo in -a..a {
            for zo in -a..a {
                if Self::signed_distance_ellipse(xo, zo, local.0, local.1, a, c, angle) >= 0.0 {
                    continue;
                }
                let p = s.origin.add(xo, y_off, zo);
                let state = w.get(p);
                if Self::is_iceberg(w, state) || w.b.is_block(state, snow_block) {
                    if under_water {
                        w.set(p, water);
                    } else {
                        w.set(p, air);
                        if w.b.is_block(w.get(p.up()), snow) {
                            w.set(p.up(), air);
                        }
                    }
                }
            }
        }
    }

    fn generate_cut_out(w: &mut World, r: &mut WorldgenRandom, s: &IcebergShape, width: i32, height: i32) {
        let sign_x = if r.next_bool() { -1 } else { 1 };
        let sign_z = if r.next_bool() { -1 } else { 1 };
        let mut x_off = r.next_bounded((width / 2 - 2).max(1));
        if r.next_bool() {
            x_off = width / 2 + 1 - r.next_bounded((width - width / 2 - 1).max(1));
        }
        let mut z_off = r.next_bounded((width / 2 - 2).max(1));
        if r.next_bool() {
            z_off = width / 2 + 1 - r.next_bounded((width - width / 2 - 1).max(1));
        }
        if s.ellipse {
            x_off = r.next_bounded((s.ellipse_a - 5).max(1));
            z_off = x_off;
        }
        let local = (sign_x * x_off, sign_z * z_off);
        let angle = if s.ellipse { s.angle + std::f64::consts::FRAC_PI_2 } else { r.next_f64() * 2.0 * std::f64::consts::PI };
        for y_off in 0..height - 3 {
            let radius = Self::radius_round(r, y_off, height, width);
            Self::carve(w, radius, y_off, s, false, angle, local);
        }
        let mut y_off = -1;
        while y_off > -height + r.next_bounded(5) {
            let radius = Self::radius_steep(r, -y_off, height, width);
            Self::carve(w, radius, y_off, s, true, angle, local);
            y_off -= 1;
        }
    }
}

impl Place for Iceberg {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let origin = Pos::new(origin.x, w.sea_level, origin.z);
        let snow_on_top = r.next_f64() > 0.7;
        let angle = r.next_f64() * 2.0 * std::f64::consts::PI;
        let ellipse_a = 11 - r.next_bounded(5);
        let ellipse_c = 3 + r.next_bounded(3);
        let ellipse = r.next_f64() > 0.7;
        let mut over_water = if ellipse { r.next_bounded(6) + 6 } else { r.next_bounded(15) + 3 };
        if !ellipse && r.next_f64() > 0.9 {
            over_water += r.next_bounded(19) + 7;
        }
        let under_water = (over_water + r.next_bounded(11)).min(18);
        let width = (over_water + r.next_bounded(7) - r.next_bounded(5)).min(11);
        let a = if ellipse { ellipse_a } else { 11 };
        let s = IcebergShape { origin, snow_on_top, angle, ellipse_a, ellipse_c, ellipse };
        for xo in -a..a {
            for zo in -a..a {
                for y_off in 0..over_water {
                    let radius = if ellipse { Self::radius_ellipse(y_off, over_water, width) } else { Self::radius_round(r, y_off, over_water, width) };
                    if ellipse || xo < radius {
                        self.generate_block(w, r, &s, over_water, xo, y_off, zo, radius, a);
                    }
                }
            }
        }
        Self::smooth(w, &s, width, over_water);
        for xo in -a..a {
            for zo in -a..a {
                let mut y_off = -1;
                while y_off > -under_water {
                    let new_a = if ellipse {
                        let sq = (y_off as f64 * y_off as f64) as f32;
                        (a as f32 * (1.0 - sq / (under_water as f32 * 8.0))).ceil() as i32
                    } else {
                        a
                    };
                    let radius = Self::radius_steep(r, -y_off, under_water, width);
                    if xo < radius {
                        self.generate_block(w, r, &s, under_water, xo, y_off, zo, radius, new_a);
                    }
                    y_off -= 1;
                }
            }
        }
        let cut_out = if ellipse { r.next_f64() > 0.1 } else { r.next_f64() > 0.7 };
        if cut_out {
            Self::generate_cut_out(w, r, &s, width, over_water);
        }
        true
    }
}

/// `HugeBrownMushroomFeature` and `HugeRedMushroomFeature`.
struct HugeMushroom {
    red: bool,
    cap: Arc<StateProvider>,
    stem: Arc<StateProvider>,
    foliage_radius: i32,
    can_place_on: BlockPredicate,
}

impl HugeMushroom {
    /// `getTreeRadiusForHeight(trunkHeight, treeHeight, leafRadius, yo)`.
    fn radius_for_height(&self, tree_height: i32, yo: i32) -> i32 {
        if self.red {
            if (yo < tree_height && yo >= tree_height - 3) || yo == tree_height {
                self.foliage_radius
            } else {
                0
            }
        } else if yo <= 3 {
            0
        } else {
            self.foliage_radius
        }
    }

    fn is_valid_position(&self, w: &World, origin: Pos, tree_height: i32) -> bool {
        let y = origin.y;
        if y < w.min_y + 1 || y + tree_height + 1 > w.max_y() {
            return false;
        }
        if !self.can_place_on.test(w, origin.down()) {
            return false;
        }
        for dy in 0..=tree_height {
            // Vanilla passes -1 for the tree height here.
            let radius = self.radius_for_height(-1, dy);
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let s = w.get(origin.add(dx, dy, dz));
                    if !w.b.is_air(s) && !w.b.tags.leaves.has(s) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn place_block(w: &mut World, p: Pos, state: u32) {
        let current = w.get(p);
        if w.b.is_air(current) || w.b.tags.replaceable_by_mushrooms.has(current) {
            w.set(p, state);
        }
    }

    fn make_cap(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos, tree_height: i32) {
        let b = w.b;
        let sides = |s: u32| ["west", "east", "north", "south"].iter().all(|p| b.has_prop(s, p));
        if self.red {
            for dy in tree_height - 3..=tree_height {
                let radius = if dy < tree_height { self.foliage_radius } else { self.foliage_radius - 1 };
                let center = self.foliage_radius - 2;
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        let x_edge = dx == -radius || dx == radius;
                        let z_edge = dz == -radius || dz == radius;
                        if dy < tree_height && x_edge == z_edge {
                            continue;
                        }
                        let mut state = self.cap.get(w, r, origin);
                        if sides(state) && b.has_prop(state, "up") {
                            state = b.with_bool(state, "up", dy >= tree_height - 1);
                            state = b.with_bool(state, "west", dx < -center);
                            state = b.with_bool(state, "east", dx > center);
                            state = b.with_bool(state, "north", dz < -center);
                            state = b.with_bool(state, "south", dz > center);
                        }
                        Self::place_block(w, origin.add(dx, dy, dz), state);
                    }
                }
            }
        } else {
            let rr = self.foliage_radius;
            for dx in -rr..=rr {
                for dz in -rr..=rr {
                    let (min_x, max_x, min_z, max_z) = (dx == -rr, dx == rr, dz == -rr, dz == rr);
                    let x_edge = min_x || max_x;
                    let z_edge = min_z || max_z;
                    if x_edge && z_edge {
                        continue;
                    }
                    let west = min_x || (z_edge && dx == 1 - rr);
                    let east = max_x || (z_edge && dx == rr - 1);
                    let north = min_z || (x_edge && dz == 1 - rr);
                    let south = max_z || (x_edge && dz == rr - 1);
                    let mut state = self.cap.get(w, r, origin);
                    if sides(state) {
                        state = b.with_bool(state, "west", west);
                        state = b.with_bool(state, "east", east);
                        state = b.with_bool(state, "north", north);
                        state = b.with_bool(state, "south", south);
                    }
                    Self::place_block(w, origin.add(dx, tree_height, dz), state);
                }
            }
        }
    }
}

impl Place for HugeMushroom {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let mut tree_height = r.next_bounded(3) + 4;
        if r.next_bounded(12) == 0 {
            tree_height *= 2;
        }
        if !self.is_valid_position(w, origin, tree_height) {
            return false;
        }
        self.make_cap(w, r, origin, tree_height);
        for dy in 0..tree_height {
            let state = self.stem.get(w, r, origin);
            Self::place_block(w, origin.up_n(dy), state);
        }
        true
    }
}

/// `CoralTreeFeature`: a trunk with branches of the configured placed feature.
struct CoralTree(Arc<PlacedFeature>);

impl Place for CoralTree {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let mut p = origin;
        let trunk = r.next_bounded(3) + 1;
        for _ in 0..trunk {
            if !self.0.place(w, r, p) {
                return true;
            }
            p = p.up();
        }
        let top = p;
        let branches = r.next_bounded(3) + 2;
        let mut dirs = Dir::HORIZONTAL;
        shuffle(r, &mut dirs);
        for &d in &dirs[..branches as usize] {
            p = top.rel(d);
            let height = r.next_bounded(5) + 2;
            let mut segment = 0;
            for j in 0..height {
                if !self.0.place(w, r, p) {
                    break;
                }
                segment += 1;
                p = p.up();
                if j == 0 || (segment >= 2 && r.next_f32() < 0.25) {
                    p = p.rel(d);
                    segment = 0;
                }
            }
        }
        true
    }
}

/// `CoralClawFeature`.
struct CoralClaw(Arc<PlacedFeature>);

impl Place for CoralClaw {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        if !self.0.place(w, r, origin) {
            return false;
        }
        let claw = Dir::random_horizontal(r);
        let branches = r.next_bounded(2) + 2;
        let mut possible = [claw, claw.clockwise(), claw.counter_clockwise()];
        shuffle(r, &mut possible);
        for &branch in &possible[..branches as usize] {
            let mut p = origin;
            let sideways = r.next_bounded(2) + 1;
            p = p.rel(branch);
            let (segment, inway) = if branch == claw {
                (claw, r.next_bounded(3) + 2)
            } else {
                p = p.up();
                let d = [branch, Dir::Up][r.next_bounded(2) as usize];
                (d, r.next_bounded(3) + 3)
            };
            for _ in 0..sideways {
                if !self.0.place(w, r, p) {
                    break;
                }
                p = p.rel(segment);
            }
            p = p.rel(segment.opposite()).up();
            for _ in 0..inway {
                p = p.rel(claw);
                if !self.0.place(w, r, p) {
                    break;
                }
                if r.next_f32() < 0.25 {
                    p = p.up();
                }
            }
        }
        true
    }
}
