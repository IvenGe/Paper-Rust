//! Features that reshape the ground: ores, disks, springs, lakes, dungeons, underwater
//! magma, snow and ice, geodes.
//!
//! Derived from `pumpkin-world/src/generation/feature/features/{ore,scattered_ore,disk,
//! spring_feature,lake,monster_room,underwater_magma,freeze_top_layer,geode}.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, and checked against vanilla 26.3.

use crate::chunk::Heightmap;
use crate::data::JsonExt;
use crate::feature::blockinfo::{BlockSet, Fluid};
use crate::feature::parse::Parser;
use crate::feature::predicate::{BlockPredicate, RuleTest};
use crate::feature::provider::StateProvider;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::{BlockEntity, Place};
use crate::math::{floor_i32, java_round_f32, lerp};
use crate::noise::{Noise, NoiseParameters, NormalNoise};
use crate::providers::IntProvider;
use crate::random::{LegacyRandom, Random, RandomSource, WorldgenRandom};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::Arc;

pub(super) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(Some(match t {
        "ore" => Box::new(Ore(ore_config(p, v)?)),
        "scattered_ore" => Box::new(ScatteredOre(ore_config(p, v)?)),
        "disk" => Box::new(Disk {
            provider: p.provider(v.field("state_provider")?)?,
            target: p.predicate(v.field("target")?)?,
            radius: p.int_provider(v.field("radius")?)?,
            half_height: v.i64_or("half_height", 0) as i32,
        }),
        "spring_feature" => Box::new(Spring {
            state: fluid_block_state(v.field("state")?)?,
            requires_block_below: v.bool_or("requires_block_below", true),
            rock_count: v.i64_or("rock_count", 4) as i32,
            hole_count: v.i64_or("hole_count", 1) as i32,
            valid: p.block_set(v.field("valid_blocks")?)?,
        }),
        "lake" => Box::new(Lake {
            fluid: p.provider(v.field("fluid")?)?,
            barrier: p.provider(v.field("barrier")?)?,
            can_place: match v.get("can_place_feature") {
                Some(x) => p.predicate(x)?,
                None => BlockPredicate::True,
            },
            can_replace: match v.get("can_replace_with_air_or_fluid") {
                Some(x) => p.predicate(x)?,
                None => BlockPredicate::Not(Box::new(BlockPredicate::MatchingTag((0, 0, 0), p.tag("minecraft:features_cannot_replace")?))),
            },
            can_replace_barrier: match v.get("can_replace_with_barrier") {
                Some(x) => p.predicate(x)?,
                None => BlockPredicate::Not(Box::new(BlockPredicate::MatchingTag((0, 0, 0), p.tag("minecraft:lava_pool_stone_cannot_replace")?))),
            },
        }),
        "monster_room" => Box::new(MonsterRoom),
        "underwater_magma" => Box::new(UnderwaterMagma {
            floor_search_range: v.i64_or("floor_search_range", 0) as i32,
            radius: v.i64_or("placement_radius_around_floor", 0) as i32,
            probability: v.f64_or("placement_probability_per_valid_position", 0.0) as f32,
        }),
        "freeze_top_layer" => Box::new(FreezeTopLayer),
        "geode" => Box::new(Geode::parse(p, v)?),
        _ => return Ok(None),
    }))
}

/// A fluid state as configured for springs (`{"id": "minecraft:water", "properties":
/// {"falling": "true"}}`), as its legacy block (`createLegacyBlock`).
fn fluid_block_state(v: &Value) -> Result<u32> {
    let name = match v {
        Value::String(s) => s.as_str(),
        _ => v.get("id").or_else(|| v.get("Name")).and_then(Value::as_str).ok_or_else(|| anyhow!("fluid state without id"))?,
    };
    let props = v.get("properties").or_else(|| v.get("Properties"));
    let falling = props.and_then(|p| p.get("falling")).and_then(Value::as_str) == Some("true");
    let (block, source) = match name.strip_prefix("minecraft:").unwrap_or(name) {
        "water" => ("minecraft:water", true),
        "lava" => ("minecraft:lava", true),
        "flowing_water" => ("minecraft:water", false),
        "flowing_lava" => ("minecraft:lava", false),
        other => return crate::data::parse_block_state(&Value::String(format!("minecraft:{other}"))),
    };
    let level = if source {
        0
    } else {
        let amount = props.and_then(|p| p.get("level")).and_then(Value::as_str).and_then(|l| l.parse::<i32>().ok()).unwrap_or(8);
        8 - amount.min(8) + if falling { 8 } else { 0 }
    };
    let state = paper_data::blocks::default_state(block).ok_or_else(|| anyhow!("unknown fluid block {block}"))?;
    Ok(paper_data::blocks::with_property(state, "level", &level.to_string()).unwrap_or(state))
}

struct OreConfig {
    size: i32,
    discard: f32,
    targets: Vec<(RuleTest, u32)>,
}

fn ore_config(p: &mut Parser, v: &Value) -> Result<OreConfig> {
    let mut targets = Vec::new();
    for t in v.field("targets")?.as_array().ok_or_else(|| anyhow!("targets"))? {
        targets.push((p.rule_test(t.field("target")?)?, p.state(t.field("state")?)?));
    }
    Ok(OreConfig { size: v.i64_or("size", 0) as i32, discard: v.f64_or("discard_chance_on_air_exposure", 0.0) as f32, targets })
}

impl OreConfig {
    /// `AbstractOreFeature.canPlaceOre`.
    fn can_place(&self, w: &World, state: u32, r: &mut WorldgenRandom, target: &RuleTest, pos: Pos) -> bool {
        if !target.test(state, pos.y, r) {
            return false;
        }
        if skip_air_check(r, self.discard) {
            return true;
        }
        !Dir::ALL.iter().any(|d| w.b.is_air(w.get(pos.rel(*d))))
    }
}

fn skip_air_check(r: &mut WorldgenRandom, chance: f32) -> bool {
    if chance <= 0.0 {
        true
    } else if chance >= 1.0 {
        false
    } else {
        r.next_f32() >= chance
    }
}

struct Ore(OreConfig);

impl Place for Ore {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let c = &self.0;
        let angle = r.next_f32() * std::f32::consts::PI;
        let radius = c.size as f32 / 8.0;
        let max_r = ((c.size as f32 / 16.0 * 2.0 + 1.0) / 2.0).ceil() as i32;
        let x0 = pos.x as f64 + (angle as f64).sin() * radius as f64;
        let x1 = pos.x as f64 - (angle as f64).sin() * radius as f64;
        let z0 = pos.z as f64 + (angle as f64).cos() * radius as f64;
        let z1 = pos.z as f64 - (angle as f64).cos() * radius as f64;
        let y0 = (pos.y + r.next_bounded(3) - 2) as f64;
        let y1 = (pos.y + r.next_bounded(3) - 2) as f64;
        let ceil_r = radius.ceil() as i32;
        let x_start = pos.x - ceil_r - max_r;
        let y_start = pos.y - 2 - max_r;
        let z_start = pos.z - ceil_r - max_r;
        let size = 2 * (ceil_r + max_r);
        let size_y = 2 * (2 + max_r);
        for x in x_start..=x_start + size {
            for z in z_start..=z_start + size {
                if y_start <= w.height_at(Heightmap::OceanFloorWg, x, z) {
                    return self.place_vein(w, r, [x0, x1, z0, z1, y0, y1], x_start, y_start, z_start, size, size_y);
                }
            }
        }
        false
    }
}

impl Ore {
    #[allow(clippy::too_many_arguments)]
    fn place_vein(&self, w: &mut World, r: &mut WorldgenRandom, ends: [f64; 6], x_start: i32, y_start: i32, z_start: i32, size: i32, size_y: i32) -> bool {
        let c = &self.0;
        let [x0, x1, z0, z1, y0, y1] = ends;
        let n = c.size as usize;
        let mut placed_count = 0;
        let mut placed = vec![false; (size * size_y * size) as usize];
        let mut spheres = vec![[0.0f64; 4]; n];
        for (i, s) in spheres.iter_mut().enumerate() {
            let step = i as f32 / n as f32;
            let xx = lerp(step as f64, x0, x1);
            let yy = lerp(step as f64, y0, y1);
            let zz = lerp(step as f64, z0, z1);
            let ss = r.next_f64() * n as f64 / 16.0;
            let rr = ((crate::math::mth_sin((std::f32::consts::PI * step) as f64) + 1.0) as f64 * ss + 1.0) / 2.0;
            *s = [xx, yy, zz, rr];
        }
        for i in 0..n.saturating_sub(1) {
            if spheres[i][3] <= 0.0 {
                continue;
            }
            for j in i + 1..n {
                if spheres[j][3] <= 0.0 {
                    continue;
                }
                let dr = spheres[i][3] - spheres[j][3];
                let dx = spheres[i][0] - spheres[j][0];
                let dy = spheres[i][1] - spheres[j][1];
                let dz = spheres[i][2] - spheres[j][2];
                if dr * dr > dx * dx + dy * dy + dz * dz {
                    if dr > 0.0 {
                        spheres[j][3] = -1.0;
                    } else {
                        spheres[i][3] = -1.0;
                    }
                }
            }
        }
        for s in &spheres {
            let rr = s[3];
            if rr < 0.0 {
                continue;
            }
            let (xx, yy, zz) = (s[0], s[1], s[2]);
            let x_min = floor_i32(xx - rr).max(x_start);
            let y_min = floor_i32(yy - rr).max(y_start);
            let z_min = floor_i32(zz - rr).max(z_start);
            let x_max = floor_i32(xx + rr).max(x_min);
            let y_max = floor_i32(yy + rr).max(y_min);
            let z_max = floor_i32(zz + rr).max(z_min);
            for x in x_min..=x_max {
                let xd = (x as f64 + 0.5 - xx) / rr;
                if xd * xd >= 1.0 {
                    continue;
                }
                for y in y_min..=y_max {
                    let yd = (y as f64 + 0.5 - yy) / rr;
                    if xd * xd + yd * yd >= 1.0 {
                        continue;
                    }
                    for z in z_min..=z_max {
                        let zd = (z as f64 + 0.5 - zz) / rr;
                        if xd * xd + yd * yd + zd * zd >= 1.0 || w.out_of_height(y) {
                            continue;
                        }
                        let bit = (x - x_start + (y - y_start) * size + (z - z_start) * size * size_y) as usize;
                        if placed[bit] {
                            continue;
                        }
                        placed[bit] = true;
                        let p = Pos::new(x, y, z);
                        if !w.can_write(p) {
                            continue;
                        }
                        let state = w.get(p);
                        for (target, ore) in &c.targets {
                            if c.can_place(w, state, r, target, p) {
                                w.set_raw(p, *ore);
                                placed_count += 1;
                                break;
                            }
                        }
                    }
                }
            }
        }
        placed_count > 0
    }
}

struct ScatteredOre(OreConfig);

impl Place for ScatteredOre {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let c = &self.0;
        let count = r.next_bounded(c.size + 1);
        let offset = |r: &mut WorldgenRandom, max: i32| java_round_f32((r.next_f32() - r.next_f32()) * max as f32);
        for i in 0..count {
            let max = i.min(7);
            let dx = offset(r, max);
            let dy = offset(r, max);
            let dz = offset(r, max);
            let p = pos.add(dx, dy, dz);
            let state = w.get(p);
            for (target, ore) in &c.targets {
                if c.can_place(w, state, r, target, p) {
                    w.set(p, *ore);
                    break;
                }
            }
        }
        true
    }
}

struct Disk {
    provider: Arc<StateProvider>,
    target: BlockPredicate,
    radius: IntProvider,
    half_height: i32,
}

impl Place for Disk {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let top = pos.y + self.half_height;
        let bottom = pos.y - self.half_height - 1;
        let radius = self.radius.sample(r);
        let mut any = false;
        for z in pos.z - radius..=pos.z + radius {
            for x in pos.x - radius..=pos.x + radius {
                let (dx, dz) = (x - pos.x, z - pos.z);
                if dx * dx + dz * dz > radius * radius {
                    continue;
                }
                let mut y = top;
                while y > bottom {
                    let p = Pos::new(x, y, z);
                    if self.target.test(w, p) {
                        let s = self.provider.get(w, r, p);
                        w.set(p, s);
                        any = true;
                    }
                    y -= 1;
                }
            }
        }
        any
    }
}

struct Spring {
    state: u32,
    requires_block_below: bool,
    rock_count: i32,
    hole_count: i32,
    valid: Arc<BlockSet>,
}

impl Place for Spring {
    fn place(&self, w: &mut World, _r: &mut WorldgenRandom, pos: Pos) -> bool {
        if !self.valid.has(w.get(pos.up())) {
            return false;
        }
        if self.requires_block_below && !self.valid.has(w.get(pos.down())) {
            return false;
        }
        let here = w.get(pos);
        if !w.b.is_air(here) && !self.valid.has(here) {
            return false;
        }
        let around = [Dir::West, Dir::East, Dir::North, Dir::South, Dir::Down];
        let rocks = around.iter().filter(|d| self.valid.has(w.get(pos.rel(**d)))).count() as i32;
        let holes = around.iter().filter(|d| w.is_air(pos.rel(**d))).count() as i32;
        if rocks == self.rock_count && holes == self.hole_count {
            w.set(pos, self.state);
            return true;
        }
        false
    }
}

struct Lake {
    fluid: Arc<StateProvider>,
    barrier: Arc<StateProvider>,
    can_place: BlockPredicate,
    can_replace: BlockPredicate,
    can_replace_barrier: BlockPredicate,
}

impl Place for Lake {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        if pos.y <= w.min_y + 4 {
            return false;
        }
        let origin = pos.add(-8, -4, -8);
        let mut grid = [false; 2048];
        let idx = |x: usize, y: usize, z: usize| (x * 16 + z) * 8 + y;
        let spots = r.next_bounded(4) + 4;
        for _ in 0..spots {
            let xr = r.next_f64() * 6.0 + 3.0;
            let yr = r.next_f64() * 4.0 + 2.0;
            let zr = r.next_f64() * 6.0 + 3.0;
            let xp = r.next_f64() * (16.0 - xr - 2.0) + 1.0 + xr / 2.0;
            let yp = r.next_f64() * (8.0 - yr - 4.0) + 2.0 + yr / 2.0;
            let zp = r.next_f64() * (16.0 - zr - 2.0) + 1.0 + zr / 2.0;
            for xx in 1..15 {
                for zz in 1..15 {
                    for yy in 1..7 {
                        let xd = (xx as f64 - xp) / (xr / 2.0);
                        let yd = (yy as f64 - yp) / (yr / 2.0);
                        let zd = (zz as f64 - zp) / (zr / 2.0);
                        if xd * xd + yd * yd + zd * zd < 1.0 {
                            grid[idx(xx, yy, zz)] = true;
                        }
                    }
                }
            }
        }
        let edge = |grid: &[bool; 2048], x: usize, y: usize, z: usize| {
            !grid[idx(x, y, z)]
                && (x < 15 && grid[idx(x + 1, y, z)]
                    || x > 0 && grid[idx(x - 1, y, z)]
                    || z < 15 && grid[idx(x, y, z + 1)]
                    || z > 0 && grid[idx(x, y, z - 1)]
                    || y < 7 && grid[idx(x, y + 1, z)]
                    || y > 0 && grid[idx(x, y - 1, z)])
        };
        let fluid = self.fluid.get(w, r, origin);
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..8 {
                    if edge(&grid, x, y, z) {
                        let p = origin.add(x as i32, y as i32, z as i32);
                        let s = w.get(p);
                        if y >= 4 && w.b.is_liquid(s) {
                            return false;
                        }
                        if y < 4 && !w.b.is_solid(s) && s != fluid {
                            return false;
                        }
                        if !self.can_place.test(w, p) {
                            return false;
                        }
                    }
                }
            }
        }
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..8 {
                    if grid[idx(x, y, z)] {
                        let p = origin.add(x as i32, y as i32, z as i32);
                        if self.can_replace.test(w, p) {
                            w.set(p, if y >= 4 { w.b.known.cave_air } else { fluid });
                        }
                    }
                }
            }
        }
        let barrier = self.barrier.get(w, r, origin);
        if !w.b.is_air(barrier) {
            for x in 0..16 {
                for z in 0..16 {
                    for y in 0..8 {
                        if edge(&grid, x, y, z) && (y < 4 || r.next_bounded(2) != 0) {
                            let p = origin.add(x as i32, y as i32, z as i32);
                            if w.b.is_solid(w.get(p)) && self.can_replace_barrier.test(w, p) {
                                w.set(p, barrier);
                            }
                        }
                    }
                }
            }
        }
        if w.b.fluid(fluid).is_water() {
            for x in 0..16 {
                for z in 0..16 {
                    let p = origin.add(x, 4, z);
                    let biome = w.biome(p);
                    if should_freeze(w, biome, p) && self.can_replace.test(w, p) {
                        w.set(p, w.b.known.ice);
                    }
                }
            }
        }
        true
    }
}

/// `Biome.shouldFreeze(level, pos, false)` (block light is not computed during generation).
fn should_freeze(w: &World, biome: u8, p: Pos) -> bool {
    let info = w.gen.biomes.get(biome);
    if info.warm_enough_to_rain(p.x, p.y, p.z, w.sea_level) {
        return false;
    }
    if w.out_of_height(p.y) {
        return false;
    }
    let s = w.get(p);
    w.b.fluid(s) == Fluid::Water && w.b.is_block(s, w.b.known.water)
}

/// `Biome.shouldSnow(level, pos)`.
fn should_snow(w: &World, biome: u8, p: Pos) -> bool {
    let info = w.gen.biomes.get(biome);
    if !info.has_precipitation || !info.cold_enough_to_snow(p.x, p.y, p.z, w.sea_level) {
        return false;
    }
    if w.out_of_height(p.y) {
        return false;
    }
    let s = w.get(p);
    (w.b.is_air(s) || w.b.is_block(s, w.b.known.snow)) && w.b.can_survive(w, w.b.known.snow, p)
}

struct FreezeTopLayer;

impl Place for FreezeTopLayer {
    fn place(&self, w: &mut World, _r: &mut WorldgenRandom, pos: Pos) -> bool {
        for dx in 0..16 {
            for dz in 0..16 {
                let (x, z) = (pos.x + dx, pos.z + dz);
                let y = w.height_at(Heightmap::MotionBlocking, x, z);
                let top = Pos::new(x, y, z);
                let below = top.down();
                let biome = w.biome(top);
                if should_freeze(w, biome, below) {
                    w.set(below, w.b.known.ice);
                }
                if should_snow(w, biome, top) {
                    w.set(top, w.b.known.snow);
                    let under = w.get(below);
                    if w.b.has_prop(under, "snowy") {
                        let s = w.b.with_bool(under, "snowy", true);
                        w.set(below, s);
                    }
                }
            }
        }
        true
    }
}

const DUNGEON_MOBS: [&str; 4] = ["minecraft:skeleton", "minecraft:zombie", "minecraft:zombie", "minecraft:spider"];

struct MonsterRoom;

impl MonsterRoom {
    fn safe_set(w: &mut World, p: Pos, state: u32) {
        if !w.b.tags.features_cannot_replace.has(w.get(p)) {
            w.set(p, state);
        }
    }
}

/// `StructurePiece.reorient` for a chest.
pub(crate) fn reorient_chest(w: &World, p: Pos, chest: u32) -> u32 {
    let b = w.b;
    let mut solid: Option<Dir> = None;
    for d in Dir::HORIZONTAL {
        let n = w.get(p.rel(d));
        if b.is_block(n, b.known.chest) {
            return chest;
        }
        if b.is_solid_render(n) {
            if solid.is_some() {
                solid = None;
                break;
            }
            solid = Some(d);
        }
    }
    if let Some(d) = solid {
        return b.with(chest, "facing", d.opposite().name());
    }
    let mut lock = b.prop(chest, "facing").and_then(Dir::parse).unwrap_or(Dir::North);
    let mut lock_pos = p.rel(lock);
    if b.is_solid_render(w.get(lock_pos)) {
        lock = lock.opposite();
        lock_pos = p.rel(lock);
    }
    if b.is_solid_render(w.get(lock_pos)) {
        lock = lock.clockwise();
        lock_pos = p.rel(lock);
    }
    if b.is_solid_render(w.get(lock_pos)) {
        lock = lock.opposite();
    }
    b.with(chest, "facing", lock.name())
}

impl Place for MonsterRoom {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let xr = r.next_bounded(2) + 2;
        let (min_x, max_x) = (-xr - 1, xr + 1);
        let zr = r.next_bounded(2) + 2;
        let (min_z, max_z) = (-zr - 1, zr + 1);
        let mut holes = 0;
        for dx in min_x..=max_x {
            for dy in -1..=4 {
                for dz in min_z..=max_z {
                    let p = pos.add(dx, dy, dz);
                    let solid = w.b.is_solid(w.get(p));
                    if (dy == -1 || dy == 4) && !solid {
                        return false;
                    }
                    if (dx == min_x || dx == max_x || dz == min_z || dz == max_z) && dy == 0 && w.is_air(p) && w.is_air(p.up()) {
                        holes += 1;
                    }
                }
            }
        }
        if !(1..=5).contains(&holes) {
            return false;
        }
        let air = w.b.known.cave_air;
        for dx in min_x..=max_x {
            for dy in (-1..=3).rev() {
                for dz in min_z..=max_z {
                    let p = pos.add(dx, dy, dz);
                    let s = w.get(p);
                    if dx == min_x || dy == -1 || dz == min_z || dx == max_x || dy == 4 || dz == max_z {
                        if p.y >= w.min_y && !w.b.is_solid(w.get(p.down())) {
                            w.set(p, air);
                        } else if w.b.is_solid(s) && !w.b.is_block(s, w.b.known.chest) {
                            let block = if dy == -1 && r.next_bounded(4) != 0 { w.b.known.mossy_cobblestone } else { w.b.known.cobblestone };
                            Self::safe_set(w, p, block);
                        }
                    } else if !w.b.is_block(s, w.b.known.chest) && !w.b.is_block(s, w.b.known.spawner) {
                        Self::safe_set(w, p, air);
                    }
                }
            }
        }
        for _ in 0..2 {
            for _ in 0..3 {
                let x = pos.x + r.next_bounded(xr * 2 + 1) - xr;
                let z = pos.z + r.next_bounded(zr * 2 + 1) - zr;
                let p = Pos::new(x, pos.y, z);
                if !w.is_air(p) {
                    continue;
                }
                let walls = Dir::HORIZONTAL.iter().filter(|d| w.b.is_solid(w.get(p.rel(**d)))).count();
                if walls == 1 {
                    let chest = reorient_chest(w, p, w.b.known.chest);
                    Self::safe_set(w, p, chest);
                    let seed = r.next_i64();
                    w.add_block_entity(p, BlockEntity::Loot { table: "minecraft:chests/simple_dungeon".into(), seed });
                    break;
                }
            }
        }
        Self::safe_set(w, pos, w.b.known.spawner);
        let mob = DUNGEON_MOBS[r.next_bounded(DUNGEON_MOBS.len() as i32) as usize];
        w.add_block_entity(pos, BlockEntity::Spawner { entity: mob });
        true
    }
}

struct UnderwaterMagma {
    floor_search_range: i32,
    radius: i32,
    probability: f32,
}

impl UnderwaterMagma {
    fn water_or_air(w: &World, p: Pos) -> bool {
        let s = w.get(p);
        w.b.is_block(s, w.b.known.water) || w.b.is_air(s)
    }

    fn valid(w: &World, p: Pos) -> bool {
        if Self::water_or_air(w, p) || Self::water_or_air(w, p.down()) {
            return false;
        }
        !Dir::HORIZONTAL.iter().any(|d| Self::water_or_air(w, p.rel(*d)))
    }
}

impl Place for UnderwaterMagma {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let water = |w: &World, p: Pos| w.b.is_block(w.get(p), w.b.known.water);
        if !water(w, pos) {
            return false;
        }
        let mut p = pos;
        let mut i = 1;
        while i < self.floor_search_range && water(w, p) {
            p = p.down();
            i += 1;
        }
        if water(w, p) {
            return false;
        }
        let floor = pos.with_y(p.y);
        let r_ = self.radius;
        let mut count = 0;
        for dz in -r_..=r_ {
            for dy in -r_..=r_ {
                for dx in -r_..=r_ {
                    if r.next_f32() >= self.probability {
                        continue;
                    }
                    let t = floor.add(dx, dy, dz);
                    if Self::valid(w, t) {
                        w.set(t, w.b.known.magma_block);
                        count += 1;
                    }
                }
            }
        }
        count > 0
    }
}

struct Geode {
    filling: Arc<StateProvider>,
    inner: Arc<StateProvider>,
    alternate_inner: Arc<StateProvider>,
    middle: Arc<StateProvider>,
    outer: Arc<StateProvider>,
    inner_placements: Vec<u32>,
    cannot_replace: Arc<BlockSet>,
    invalid: Arc<BlockSet>,
    layers: [f64; 4],
    crack_chance: f64,
    base_crack_size: f64,
    crack_point_offset: i32,
    potential_placements_chance: f64,
    alternate_chance: f64,
    require_alternate: bool,
    outer_wall_distance: IntProvider,
    distribution_points: IntProvider,
    point_offset: IntProvider,
    min_offset: i32,
    max_offset: i32,
    noise_multiplier: f64,
    invalid_threshold: i32,
}

impl Geode {
    fn parse(p: &mut Parser, v: &Value) -> Result<Self> {
        let blocks = v.field("blocks")?;
        let empty = Value::Object(Default::default());
        let layers = v.get("layers").unwrap_or(&empty);
        let crack = v.get("crack").unwrap_or(&empty);
        let ip = |p: &Parser, k: &str, min: i32, max: i32| -> Result<IntProvider> {
            match v.get(k) {
                Some(x) => p.int_provider(x),
                None => Ok(IntProvider::Uniform(min, max)),
            }
        };
        Ok(Self {
            filling: p.provider(blocks.field("filling_provider")?)?,
            inner: p.provider(blocks.field("inner_layer_provider")?)?,
            alternate_inner: p.provider(blocks.field("alternate_inner_layer_provider")?)?,
            middle: p.provider(blocks.field("middle_layer_provider")?)?,
            outer: p.provider(blocks.field("outer_layer_provider")?)?,
            inner_placements: blocks.field("inner_placements")?.as_array().ok_or_else(|| anyhow!("inner_placements"))?.iter().map(|s| p.state(s)).collect::<Result<_>>()?,
            cannot_replace: p.block_set(blocks.field("cannot_replace")?)?,
            invalid: p.block_set(blocks.field("invalid_blocks")?)?,
            layers: [layers.f64_or("filling", 1.7), layers.f64_or("inner_layer", 2.2), layers.f64_or("middle_layer", 3.2), layers.f64_or("outer_layer", 4.2)],
            crack_chance: crack.f64_or("generate_crack_chance", 1.0),
            base_crack_size: crack.f64_or("base_crack_size", 2.0),
            crack_point_offset: crack.i64_or("crack_point_offset", 2) as i32,
            potential_placements_chance: v.f64_or("use_potential_placements_chance", 0.35),
            alternate_chance: v.f64_or("use_alternate_layer0_chance", 0.0),
            require_alternate: v.bool_or("placements_require_layer0_alternate", true),
            outer_wall_distance: ip(p, "outer_wall_distance", 4, 5)?,
            distribution_points: ip(p, "distribution_points", 3, 4)?,
            point_offset: ip(p, "point_offset", 1, 2)?,
            min_offset: v.i64_or("min_gen_offset", -16) as i32,
            max_offset: v.i64_or("max_gen_offset", 16) as i32,
            noise_multiplier: v.f64_or("noise_multiplier", 0.05),
            invalid_threshold: v.i64_or("invalid_blocks_threshold", 0) as i32,
        })
    }

    fn safe_set(&self, w: &mut World, p: Pos, state: u32) {
        if !self.cannot_replace.has(w.get(p)) {
            w.set(p, state);
        }
    }
}

impl Place for Geode {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let num_points = self.distribution_points.sample(r);
        let noise = NormalNoise::new(&mut RandomSource::Legacy(LegacyRandom::new(w.seed())), &NoiseParameters::new(-4, vec![1.0], None));
        let adjust = num_points as f64 / self.outer_wall_distance.max_value() as f64;
        let inner_air = 1.0 / self.layers[0].sqrt();
        let innermost = 1.0 / (self.layers[1] + adjust).sqrt();
        let inner_crust = 1.0 / (self.layers[2] + adjust).sqrt();
        let outer_crust = 1.0 / (self.layers[3] + adjust).sqrt();
        let crack_size = 1.0 / (self.base_crack_size + r.next_f64() / 2.0 + if num_points > 3 { adjust } else { 0.0 }).sqrt();
        let crack = (r.next_f32() as f64) < self.crack_chance;
        let mut invalid = 0;
        let mut points: Vec<(Pos, i32)> = Vec::new();
        for _ in 0..num_points {
            let x = self.outer_wall_distance.sample(r);
            let y = self.outer_wall_distance.sample(r);
            let z = self.outer_wall_distance.sample(r);
            let p = pos.add(x, y, z);
            let s = w.get(p);
            if w.b.is_air(s) || self.invalid.has(s) {
                invalid += 1;
                if invalid > self.invalid_threshold {
                    return false;
                }
            }
            points.push((p, self.point_offset.sample(r)));
        }
        let mut crack_points = Vec::new();
        if crack {
            let index = r.next_bounded(4);
            let off = num_points * 2 + 1;
            let (cx, cz) = match index {
                0 => (off, 0),
                1 => (0, off),
                2 => (off, off),
                _ => (0, 0),
            };
            for y in [7, 5, 1] {
                crack_points.push(pos.add(cx, y, cz));
            }
        }
        let mut potential = Vec::new();
        let (lo, hi) = (pos.add(self.min_offset, self.min_offset, self.min_offset), pos.add(self.max_offset, self.max_offset, self.max_offset));
        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                for x in lo.x..=hi.x {
                    let p = Pos::new(x, y, z);
                    let noise_offset = noise.get(x as f64, y as f64, z as f64) as f64 * self.noise_multiplier;
                    let mut shell = 0.0;
                    for (pt, off) in &points {
                        shell += 1.0 / (p.dist_sq(*pt) + *off as f64).sqrt() + noise_offset;
                    }
                    if shell < outer_crust {
                        continue;
                    }
                    if shell >= inner_air {
                        let s = self.filling.get(w, r, p);
                        self.safe_set(w, p, s);
                        continue;
                    }
                    let mut crack_sum = 0.0;
                    for pt in &crack_points {
                        crack_sum += 1.0 / (p.dist_sq(*pt) + self.crack_point_offset as f64).sqrt() + noise_offset;
                    }
                    if crack && crack_sum >= crack_size {
                        self.safe_set(w, p, w.b.known.air);
                    } else if shell >= innermost {
                        let alternate = (r.next_f32() as f64) < self.alternate_chance;
                        let s = if alternate { self.alternate_inner.get(w, r, p) } else { self.inner.get(w, r, p) };
                        self.safe_set(w, p, s);
                        if (!self.require_alternate || alternate) && (r.next_f32() as f64) < self.potential_placements_chance {
                            potential.push(p);
                        }
                    } else if shell >= inner_crust {
                        let s = self.middle.get(w, r, p);
                        self.safe_set(w, p, s);
                    } else {
                        let s = self.outer.get(w, r, p);
                        self.safe_set(w, p, s);
                    }
                }
            }
        }
        for crystal in potential {
            let mut state = self.inner_placements[r.next_bounded(self.inner_placements.len() as i32) as usize];
            for d in Dir::ALL {
                state = w.b.with(state, "facing", d.name());
                let place = crystal.rel(d);
                let there = w.get(place);
                if w.b.has_prop(state, "waterlogged") {
                    state = w.b.waterlogged(state, w.b.fluid(there) == Fluid::Water);
                }
                let can_grow = w.b.is_air(there) || (w.b.is_block(there, w.b.known.water) && fluid_amount_full(w, there));
                if can_grow {
                    self.safe_set(w, place, state);
                    break;
                }
            }
        }
        true
    }
}

/// `FluidState.getAmount() == 8` for a water block: sources and falling water.
fn fluid_amount_full(w: &World, s: u32) -> bool {
    match w.b.prop_int(s, "level") {
        Some(0) => true,
        Some(l) => l >= 8,
        None => w.b.fluid(s) == Fluid::Water,
    }
}
