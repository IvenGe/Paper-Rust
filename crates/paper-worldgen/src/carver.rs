//! Carvers: caves, canyons and nether caves (`worldgen/configured_carver` entries).
//!
//! Carvers of the 17x17 chunks around a chunk mark the blocks they remove in a carving
//! mask; the mask is then applied column by column, asking the aquifer what replaces each
//! carved block.
//!
//! Derived from `pumpkin-world/src/generation/carver/{mod,cave,canyon,mask}.rs` and
//! `tools/pumpkin-codegen/src/carver.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::aquifer::Aquifer;
use crate::chunk::ProtoChunk;
use crate::data::{DataReader, JsonExt};
use crate::math::{mth_cos, mth_sin};
use crate::providers::{FloatProvider, GenContext, HeightProvider, IntProvider};
use crate::random::{LegacyRandom, Random};
use crate::router::{ChunkRouter, SurfaceEstimator};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::f32::consts::PI;

pub enum CarverKind {
    Cave {
        count: IntProvider,
        horizontal_radius_multiplier: FloatProvider,
        vertical_radius_multiplier: FloatProvider,
        floor_level: FloatProvider,
        room_vertical_radius_multiplier: FloatProvider,
        start_vertical_radius_multiplier: FloatProvider,
        thickness: FloatProvider,
        weird_thickness_bias: bool,
    },
    Canyon {
        vertical_rotation: FloatProvider,
        distance_factor: FloatProvider,
        thickness: FloatProvider,
        width_smoothness: i32,
        horizontal_radius_factor: FloatProvider,
        vertical_radius_default_factor: f32,
        vertical_radius_center_factor: f32,
        y_scale: FloatProvider,
    },
}

pub struct Carver {
    probability: f32,
    y: HeightProvider,
    kind: CarverKind,
}

impl Carver {
    pub fn parse(v: &Value) -> Result<Self> {
        let ty = v.str_field("type")?;
        let fp = |v: &Value, k: &str, default: f32| -> Result<FloatProvider> {
            match v.get(k) {
                Some(x) => FloatProvider::parse(x),
                None => Ok(FloatProvider::Constant(default)),
            }
        };
        let kind = match ty.strip_prefix("minecraft:").unwrap_or(ty) {
            "cave" | "nether_cave" => CarverKind::Cave {
                count: match v.get("count") {
                    Some(c) => IntProvider::parse(c)?,
                    None => IntProvider::VeryBiasedToBottom(0, 14),
                },
                horizontal_radius_multiplier: fp(v, "horizontal_radius_multiplier", 1.0)?,
                vertical_radius_multiplier: fp(v, "vertical_radius_multiplier", 1.0)?,
                floor_level: fp(v, "floor_level", -0.7)?,
                room_vertical_radius_multiplier: fp(v, "room_vertical_radius_multiplier", 0.5)?,
                start_vertical_radius_multiplier: fp(v, "start_vertical_radius_multiplier", 1.0)?,
                thickness: fp(v, "thickness", 1.0)?,
                weird_thickness_bias: v.bool_or("weird_thickness_bias", false),
            },
            "canyon" => {
                let shape = v.field("shape")?;
                CarverKind::Canyon {
                    vertical_rotation: fp(v, "vertical_rotation", 0.0)?,
                    distance_factor: fp(shape, "distance_factor", 1.0)?,
                    thickness: fp(shape, "thickness", 1.0)?,
                    width_smoothness: shape.i64_or("width_smoothness", 3) as i32,
                    horizontal_radius_factor: fp(shape, "horizontal_radius_factor", 1.0)?,
                    vertical_radius_default_factor: shape.f64_or("vertical_radius_default_factor", 1.0) as f32,
                    vertical_radius_center_factor: shape.f64_or("vertical_radius_center_factor", 0.0) as f32,
                    y_scale: fp(shape, "y_scale", 3.0)?,
                }
            }
            other => bail!("unknown carver type {other}"),
        };
        Ok(Self { probability: v.f64_or("probability", 0.0) as f32, y: HeightProvider::parse(v.field("y")?)?, kind })
    }
}

/// The carvers of a dimension and which biome uses which.
pub struct Carvers {
    pub carvers: Vec<Carver>,
    /// Carver indices per biome network id.
    pub by_biome: Vec<Vec<usize>>,
    /// The list shared by every biome the dimension can produce, if they all agree.
    pub uniform: Option<Vec<usize>>,
}

impl Carvers {
    pub fn load(data: &DataReader, biomes: &crate::biome::Biomes, possible: &[u8]) -> Result<Self> {
        let mut ids: Vec<String> = Vec::new();
        let mut carvers = Vec::new();
        let mut by_biome = Vec::with_capacity(biomes.list.len());
        for b in &biomes.list {
            let mut list = Vec::new();
            for id in &b.carvers {
                let id = crate::data::full_id(id);
                let index = match ids.iter().position(|x| *x == id) {
                    Some(i) => i,
                    None => {
                        carvers.push(Carver::parse(&*data.worldgen("carver", &id)?).map_err(|e| anyhow!("carver {id}: {e}"))?);
                        ids.push(id);
                        ids.len() - 1
                    }
                };
                list.push(index);
            }
            by_biome.push(list);
        }
        let first = possible.first().map(|b| by_biome[*b as usize].clone());
        let uniform = first.filter(|f| possible.iter().all(|b| by_biome[*b as usize] == *f));
        Ok(Self { carvers, by_biome, uniform })
    }
}

/// The blocks carved in one chunk.
struct Mask {
    min_y: i32,
    max_y: i32,
    height: usize,
    bits: Vec<u64>,
}

impl Mask {
    fn new(min_y: i32, max_y: i32) -> Self {
        let height = (max_y - min_y + 1).max(0) as usize;
        Self { min_y, max_y, height, bits: vec![0; (256 * height).div_ceil(64)] }
    }

    #[inline]
    fn set(&mut self, x: usize, y: i32, z: usize) {
        if y < self.min_y || y > self.max_y || x >= 16 || z >= 16 {
            return;
        }
        let i = (y - self.min_y) as usize + (z + (x << 4)) * self.height;
        self.bits[i / 64] |= 1 << (i % 64);
    }

    fn is_set(&self, i: usize) -> bool {
        self.bits[i / 64] >> (i % 64) & 1 != 0
    }

    /// Calls `f(x, z, bottom, top)` for each run of carved blocks, in index order.
    fn visit(&self, mut f: impl FnMut(usize, usize, i32, i32)) {
        let total = 256 * self.height;
        let mut i = 0;
        while i < total {
            if self.bits[i / 64] == 0 && i % 64 == 0 {
                i += 64;
                continue;
            }
            if !self.is_set(i) {
                i += 1;
                continue;
            }
            let start = i;
            while i < total && self.is_set(i) {
                i += 1;
            }
            let end = i - 1;
            let (first, last) = (start / self.height, end / self.height);
            for column in first..=last {
                let base = column * self.height;
                let bottom = (start.saturating_sub(base) as i32).max(0) + self.min_y;
                let top = ((end - base) as i32).min(self.height as i32 - 1) + self.min_y;
                f((column >> 4) & 15, column & 15, bottom, top);
            }
        }
    }
}

fn can_reach(cx: i32, cz: i32, x: f64, z: f64, step: i32, total: i32, thickness: f32) -> bool {
    let xd = x - ((cx << 4) as f64 + 8.0);
    let zd = z - ((cz << 4) as f64 + 8.0);
    let remaining = (total - step) as f64;
    let rr = (thickness + 2.0 + 16.0) as f64;
    xd * xd + zd * zd - remaining * remaining <= rr * rr
}

#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid(cx: i32, cz: i32, x: f64, y: f64, z: f64, hr: f64, vr: f64, mask: &mut Mask, skip: impl Fn(f64, f64, f64, i32) -> bool) {
    let max_delta = 16.0 + hr * 2.0;
    if (x - ((cx << 4) as f64 + 8.0)).abs() > max_delta || (z - ((cz << 4) as f64 + 8.0)).abs() > max_delta {
        return;
    }
    let (min_bx, min_bz) = (cx << 4, cz << 4);
    let min_xi = ((x - hr).floor() as i32 - min_bx - 1).max(0);
    let max_xi = ((x + hr).floor() as i32 - min_bx).min(15);
    let min_y = ((y - vr).floor() as i32 - 1).max(mask.min_y);
    let max_y = ((y + vr).floor() as i32 + 1).min(mask.max_y);
    let min_zi = ((z - hr).floor() as i32 - min_bz - 1).max(0);
    let max_zi = ((z + hr).floor() as i32 - min_bz).min(15);
    for xi in min_xi..=max_xi {
        let xd = ((min_bx + xi) as f64 + 0.5 - x) / hr;
        for zi in min_zi..=max_zi {
            let zd = ((min_bz + zi) as f64 + 0.5 - z) / hr;
            if xd * xd + zd * zd < 1.0 {
                let mut wy = max_y;
                while wy > min_y {
                    let yd = (wy as f64 - 0.5 - y) / vr;
                    if !skip(xd, yd, zd, wy) {
                        mask.set(xi as usize, wy, zi as usize);
                    }
                    wy -= 1;
                }
            }
        }
    }
}

struct CaveParams {
    hr_mult: f64,
    vr_mult: f64,
    floor_level: f64,
}

#[allow(clippy::too_many_arguments)]
fn cave_tunnel(cx: i32, cz: i32, mask: &mut Mask, seed: i64, mut x: f64, mut y: f64, mut z: f64, p: &CaveParams, thickness: f32, mut yaw: f32, mut pitch: f32, step: i32, dist: i32, y_scale: f64) {
    let mut r = LegacyRandom::new(seed);
    let split = r.next_bounded(dist / 2) + dist / 4;
    let steep = r.next_bounded(6) == 0;
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;
    for i in step..dist {
        let hr = 1.5 + (mth_sin((PI * i as f32 / dist as f32) as f64) * thickness) as f64;
        let vr = hr * y_scale;
        let c = mth_cos(pitch as f64);
        x += (mth_cos(yaw as f64) * c) as f64;
        y += mth_sin(pitch as f64) as f64;
        z += (mth_sin(yaw as f64) * c) as f64;
        pitch = (pitch * if steep { 0.92 } else { 0.7 }) + (x_rota * 0.1);
        yaw += y_rota * 0.1;
        x_rota = (x_rota * 0.9) + ((r.next_f32() - r.next_f32()) * r.next_f32() * 2.0);
        y_rota = (y_rota * 0.75) + ((r.next_f32() - r.next_f32()) * r.next_f32() * 4.0);
        if i == split && thickness > 1.0 {
            let s1 = r.next_i64();
            let t1 = r.next_f32() * 0.5 + 0.5;
            cave_tunnel(cx, cz, mask, s1, x, y, z, p, t1, yaw - PI / 2.0, pitch / 3.0, i, dist, 1.0);
            let s2 = r.next_i64();
            let t2 = r.next_f32() * 0.5 + 0.5;
            cave_tunnel(cx, cz, mask, s2, x, y, z, p, t2, yaw + PI / 2.0, pitch / 3.0, i, dist, 1.0);
            return;
        }
        if r.next_bounded(4) != 0 {
            if !can_reach(cx, cz, x, z, i, dist, thickness) {
                return;
            }
            let floor = p.floor_level;
            carve_ellipsoid(cx, cz, x, y, z, hr * p.hr_mult, vr * p.vr_mult, mask, |xd, yd, zd, _| yd <= floor || xd * xd + yd * yd + zd * zd >= 1.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn canyon_tunnel(
    cx: i32,
    cz: i32,
    mask: &mut Mask,
    ctx: &GenContext,
    kind: &CarverKind,
    seed: i64,
    mut x: f64,
    mut y: f64,
    mut z: f64,
    thickness: f32,
    mut yaw: f32,
    mut pitch: f32,
    dist: i32,
    y_scale: f64,
) {
    let CarverKind::Canyon { width_smoothness, horizontal_radius_factor, vertical_radius_default_factor, vertical_radius_center_factor, .. } = kind else {
        return;
    };
    let mut r = LegacyRandom::new(seed);
    let mut widths = vec![0.0f32; ctx.height as usize];
    let mut w = 1.0f32;
    for (i, slot) in widths.iter_mut().enumerate() {
        if i == 0 || r.next_bounded(*width_smoothness) == 0 {
            w = 1.0 + r.next_f32() * r.next_f32();
        }
        *slot = w * w;
    }
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;
    for i in 0..dist {
        let mut hr = 1.5 + (mth_sin((i as f32 * PI / dist as f32) as f64) * thickness) as f64;
        let mut vr = hr * y_scale;
        hr *= horizontal_radius_factor.sample(&mut r) as f64;
        let mult = 1.0 - (0.5 - i as f32 / dist as f32).abs() * 2.0;
        let factor = vertical_radius_default_factor + vertical_radius_center_factor * mult;
        vr = factor as f64 * vr * (r.next_f32() * (1.0 - 0.75) + 0.75) as f64;
        let c = mth_cos(pitch as f64);
        let s = mth_sin(pitch as f64);
        x += (mth_cos(yaw as f64) * c) as f64;
        y += s as f64;
        z += (mth_sin(yaw as f64) * c) as f64;
        pitch = (pitch * 0.7) + (x_rota * 0.05);
        yaw += y_rota * 0.05;
        x_rota = (x_rota * 0.8) + ((r.next_f32() - r.next_f32()) * r.next_f32() * 2.0);
        y_rota = (y_rota * 0.5) + ((r.next_f32() - r.next_f32()) * r.next_f32() * 4.0);
        if r.next_bounded(4) != 0 {
            if !can_reach(cx, cz, x, z, i, dist, thickness) {
                return;
            }
            let min_y = ctx.min_y;
            carve_ellipsoid(cx, cz, x, y, z, hr, vr, mask, |xd, yd, zd, wy| {
                let index = (wy - min_y) as usize;
                if index == 0 {
                    return true;
                }
                (xd * xd + zd * zd) * widths[index - 1] as f64 + yd * yd / 6.0 >= 1.0
            });
        }
    }
}

impl Carver {
    fn carve(&self, r: &mut LegacyRandom, cx: i32, cz: i32, origin_x: i32, origin_z: i32, mask: &mut Mask, ctx: &GenContext) {
        match &self.kind {
            CarverKind::Cave {
                count,
                horizontal_radius_multiplier,
                vertical_radius_multiplier,
                floor_level,
                room_vertical_radius_multiplier,
                start_vertical_radius_multiplier,
                thickness,
                weird_thickness_bias,
            } => {
                let max_distance = (4 * 2 - 1) << 4;
                let n = count.sample(r);
                for _ in 0..n {
                    let x = (origin_x << 4) + r.next_bounded(16);
                    let y = self.y.sample(r, ctx) as f64;
                    let z = (origin_z << 4) + r.next_bounded(16);
                    let hr_mult = horizontal_radius_multiplier.sample(r) as f64;
                    let vr_mult = vertical_radius_multiplier.sample(r) as f64;
                    let start_mult = start_vertical_radius_multiplier.sample(r) as f64;
                    let floor = floor_level.sample(r) as f64;
                    let params = CaveParams { hr_mult, vr_mult, floor_level: floor };
                    let tunnels = if r.next_bounded(4) == 0 {
                        let room_mult = room_vertical_radius_multiplier.sample(r) as f64;
                        let t = 1.0 + r.next_f32() * 6.0;
                        let hr = 1.5 + (mth_sin(std::f32::consts::FRAC_PI_2 as f64) * t) as f64;
                        let vr = hr * room_mult;
                        carve_ellipsoid(cx, cz, x as f64 + 1.0, y, z as f64, hr, vr, mask, |xd, yd, zd, _| yd <= floor || xd * xd + yd * yd + zd * zd >= 1.0);
                        1 + r.next_bounded(4)
                    } else {
                        1
                    };
                    for _ in 0..tunnels {
                        let yaw = r.next_f32() * PI * 2.0;
                        let pitch = (r.next_f32() - 0.5) / 4.0;
                        let mut t = thickness.sample(r);
                        if *weird_thickness_bias && r.next_bounded(10) == 0 {
                            t *= r.next_f32() * r.next_f32() * 3.0 + 1.0;
                        }
                        let dist = max_distance - r.next_bounded(max_distance / 4);
                        let seed = r.next_i64();
                        cave_tunnel(cx, cz, mask, seed, x as f64, y, z as f64, &params, t, yaw, pitch, 0, dist, start_mult);
                    }
                }
            }
            CarverKind::Canyon { vertical_rotation, distance_factor, thickness, y_scale, .. } => {
                let max_distance = (4 * 2 - 1) * 16;
                let x = (origin_x << 4) + r.next_bounded(16);
                let y = self.y.sample(r, ctx);
                let z = (origin_z << 4) + r.next_bounded(16);
                let yaw = r.next_f32() * PI * 2.0;
                let pitch = vertical_rotation.sample(r);
                let ys = y_scale.sample(r) as f64;
                let t = thickness.sample(r);
                let dist = (max_distance as f32 * distance_factor.sample(r)) as i32;
                let seed = r.next_i64();
                canyon_tunnel(cx, cz, mask, ctx, &self.kind, seed, x as f64, y as f64, z as f64, t, yaw, pitch, dist, ys);
            }
        }
    }
}

/// `WorldgenRandom.setLargeFeatureSeed` with a legacy random.
fn large_feature_seed(seed: i64, x: i32, z: i32) -> i64 {
    let mut r = LegacyRandom::new(seed);
    let a = r.next_i64();
    let b = r.next_i64();
    (x as i64).wrapping_mul(a) ^ (z as i64).wrapping_mul(b) ^ seed
}

/// Runs the carver stage on a chunk whose surface stage is done.
pub fn carve(gen: &crate::WorldGenerator, chunk: &mut ProtoChunk, aquifer: Option<&mut Aquifer>, estimator: &mut SurfaceEstimator) {
    let carvers = &gen.carvers;
    if carvers.uniform.as_ref().is_some_and(|u| u.is_empty()) {
        return;
    }
    let ctx = GenContext { min_y: gen.gen_min_y, height: gen.gen_height, sea_level: gen.sea_level };
    let mut mask = Mask::new(gen.gen_min_y + 1, gen.gen_min_y + gen.gen_height - 1 - 7);
    let (cx, cz) = (chunk.chunk_x, chunk.chunk_z);
    for dx in -8..=8 {
        for dz in -8..=8 {
            let (ox, oz) = (cx + dx, cz + dz);
            let list: &[usize] = match &carvers.uniform {
                Some(u) => u,
                None => &carvers.by_biome[gen.biome_at_quart(ox << 2, 0, oz << 2) as usize],
            };
            for (index, &c) in list.iter().enumerate() {
                let carver = &carvers.carvers[c];
                let mut r = LegacyRandom::new(large_feature_seed(gen.seed.wrapping_add(index as i64), ox, oz));
                if r.next_f32() <= carver.probability {
                    carver.carve(&mut r, cx, cz, ox, oz, &mut mask, &ctx);
                }
            }
        }
    }
    apply_mask(gen, chunk, &mask, aquifer, estimator);
}

fn apply_mask(gen: &crate::WorldGenerator, chunk: &mut ProtoChunk, mask: &Mask, mut aquifer: Option<&mut Aquifer>, estimator: &mut SurfaceEstimator) {
    let flags = &gen.flags;
    let bedrock = crate::blocks::state("minecraft:bedrock");
    let grass = crate::blocks::state("minecraft:grass_block");
    let mycelium = crate::blocks::state("minecraft:mycelium");
    let dirt = crate::blocks::state("minecraft:dirt");
    let mut router = aquifer.is_some().then(|| ChunkRouter::terrain(&gen.terrain));
    let mut rules: Option<crate::surface::RuleContext> = None;
    let (sx, sz) = (chunk.start_x(), chunk.start_z());
    mask.visit(|x, z, bottom, top| {
        let mut has_grass = false;
        let (wx, wz) = (sx + x as i32, sz + z as i32);
        let mut y = top;
        while y >= bottom {
            let current = chunk.get(wx, y, wz);
            if current == bedrock {
                y -= 1;
                continue;
            }
            if current == grass || current == mycelium {
                has_grass = true;
            }
            let state = match (&mut aquifer, &mut router) {
                (Some(a), Some(router)) => a.compute(router, estimator, wx, y, wz, 0.0).0,
                _ => Some(if y < gen.sea_level { gen.default_fluid } else { paper_data::blocks::AIR }),
            };
            if let Some(state) = state {
                chunk.set(wx, y, wz, state, flags);
                if aquifer.is_some() && has_grass && chunk.get(wx, y - 1, wz) == dirt {
                    let ctx = rules.get_or_insert_with(|| crate::surface::RuleContext::for_carver(gen, &gen.surface, chunk));
                    if let Some(top_state) = ctx.top_material(chunk, estimator, wx, y - 1, wz, flags.has_fluid(state)) {
                        chunk.set(wx, y - 1, wz, top_state, flags);
                    }
                }
            }
            y -= 1;
        }
    });
}
