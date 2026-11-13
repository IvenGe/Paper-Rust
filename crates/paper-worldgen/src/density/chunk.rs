//! Evaluation of a seeded component stack with per-chunk state (caches and cell
//! interpolation), at single positions and over [`DensityVolume`]s.
//!
//! Derived from `pumpkin-world/src/generation/noise/router/{chunk_noise_router,
//! chunk_density_function,find_top_surface}.rs` and `density_function/{math,misc,noise,
//! spline}.rs` of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.
//!
//! The point and volume paths deliberately keep their different floating point operation
//! orders: vanilla evaluates some parts of the graph per block and some per volume, and
//! the results differ in the last bits.

use super::ir::{Axis, BinaryOp, DistanceMetric, LinearOp, RoundingOp, Tiling, WrapperKind};
use super::stack::{Dependent, Independent, Proto, ProtoKind, SplineEval};
use crate::math::lerp;
use crate::noise::{DensityVolume, Noise};
use std::cell::RefCell;

thread_local! {
    static POOL: RefCell<Vec<Vec<f32>>> = const { RefCell::new(Vec::new()) };
}

/// A temporary `f32` buffer borrowed from a thread-local pool.
pub struct Buf(Vec<f32>);

impl Buf {
    pub fn new(len: usize) -> Self {
        let mut v = POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default();
        v.clear();
        v.resize(len, 0.0);
        Buf(v)
    }

    pub fn for_volume(volume: &DensityVolume) -> Self {
        Self::new(volume.size())
    }
}

impl std::ops::Deref for Buf {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        &self.0
    }
}

impl std::ops::DerefMut for Buf {
    fn deref_mut(&mut self) -> &mut [f32] {
        &mut self.0
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        let v = std::mem::take(&mut self.0);
        if v.capacity() > 0 {
            POOL.with(|p| {
                let mut p = p.borrow_mut();
                if p.len() < 256 {
                    p.push(v);
                }
            });
        }
    }
}

/// Per-chunk state of a `cache` component.
pub struct CacheState {
    input: usize,
    volume: Option<DensityVolume>,
    buffer: Vec<f32>,
    key: Option<(i32, i32, i32)>,
    value: f32,
    min: f32,
    max: f32,
}

/// An `interpolated` component.
pub struct InterpState {
    input: usize,
    cell_xz: i32,
    cell_y: i32,
    inv_xz: f32,
    inv_y: f32,
    min: f32,
    max: f32,
}

/// A component of a chunk-level stack.
pub enum ChunkComp<'a> {
    Proto(&'a Proto),
    Cache(CacheState),
    Interpolated(InterpState),
    PassThrough { input: usize, min: f32, max: f32 },
    /// A constant zero with the given range (the beardifier).
    Zero { min: f32, max: f32 },
}

/// How `interpolated` wrappers behave in a chunk stack.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InterpolationMode {
    /// Interpolate between cell corners (noise generation).
    Cells,
    /// Pass the input through (surface estimation, climate sampling).
    PassThrough,
}

impl ChunkComp<'_> {
    #[inline]
    pub fn min(&self) -> f32 {
        match self {
            ChunkComp::Proto(p) => p.min,
            ChunkComp::Cache(c) => c.min,
            ChunkComp::Interpolated(i) => i.min,
            ChunkComp::PassThrough { min, .. } | ChunkComp::Zero { min, .. } => *min,
        }
    }

    #[inline]
    pub fn max(&self) -> f32 {
        match self {
            ChunkComp::Proto(p) => p.max,
            ChunkComp::Cache(c) => c.max,
            ChunkComp::Interpolated(i) => i.max,
            ChunkComp::PassThrough { max, .. } | ChunkComp::Zero { max, .. } => *max,
        }
    }
}

/// Builds the chunk stack for a seeded stack. `beardifier_range` is the range the zero
/// beardifier reports (vanilla's noise router uses an infinite one, the other routers a
/// plain constant zero).
pub fn build_chunk_stack(protos: &[Proto], mode: InterpolationMode, beardifier_infinite: bool) -> Vec<ChunkComp<'_>> {
    let mut stack: Vec<ChunkComp<'_>> = Vec::with_capacity(protos.len());
    for p in protos {
        let comp = match &p.kind {
            ProtoKind::Independent(_) | ProtoKind::Dependent(_) => ChunkComp::Proto(p),
            ProtoKind::PassThrough(input) => ChunkComp::PassThrough { input: *input, min: p.min, max: p.max },
            ProtoKind::Beardifier => {
                if beardifier_infinite {
                    ChunkComp::Zero { min: f32::NEG_INFINITY, max: f32::INFINITY }
                } else {
                    ChunkComp::Zero { min: 0.0, max: 0.0 }
                }
            }
            ProtoKind::Wrapper { input, kind } => {
                let min = stack[*input].min();
                let max = stack[*input].max();
                match (kind, mode) {
                    (WrapperKind::Cache, _) => ChunkComp::Cache(CacheState { input: *input, volume: None, buffer: Vec::new(), key: None, value: 0.0, min, max }),
                    (WrapperKind::Interpolated { .. }, InterpolationMode::PassThrough) => ChunkComp::PassThrough { input: *input, min, max },
                    (WrapperKind::Interpolated { cell_size_xz, cell_size_y }, InterpolationMode::Cells) => ChunkComp::Interpolated(InterpState {
                        input: *input,
                        cell_xz: *cell_size_xz,
                        cell_y: *cell_size_y,
                        inv_xz: 1.0 / *cell_size_xz as f32,
                        inv_y: 1.0 / *cell_size_y as f32,
                        min,
                        max,
                    }),
                }
            }
        };
        stack.push(comp);
    }
    stack
}

/// Samples the top component of `stack` at a block position.
#[inline]
pub fn sample(stack: &mut [ChunkComp], x: i32, y: i32, z: i32) -> f32 {
    let Some((top, rest)) = stack.split_last_mut() else {
        return 0.0;
    };
    match top {
        ChunkComp::Proto(p) => sample_proto(p, rest, x, y, z),
        ChunkComp::Cache(c) => {
            if c.key == Some((x, y, z)) {
                return c.value;
            }
            if let Some(volume) = &c.volume {
                if let Some(i) = volume.index_of(x, y, z) {
                    return c.buffer[i];
                }
            }
            let v = sample(&mut rest[..=c.input], x, y, z);
            c.key = Some((x, y, z));
            c.value = v;
            v
        }
        ChunkComp::Interpolated(i) => interpolated_sample(i, rest, x, y, z),
        ChunkComp::PassThrough { input, .. } => sample(&mut rest[..=*input], x, y, z),
        ChunkComp::Zero { .. } => 0.0,
    }
}

/// Samples the top component of `stack` over a volume.
pub fn sample_volume(stack: &mut [ChunkComp], buffer: &mut [f32], volume: &DensityVolume) {
    let Some((top, rest)) = stack.split_last_mut() else {
        return;
    };
    match top {
        ChunkComp::Proto(p) => sample_volume_proto(p, rest, buffer, volume),
        ChunkComp::Cache(c) => {
            if c.volume.as_ref() != Some(volume) {
                let mut cached = vec![0.0f32; volume.size()];
                sample_volume(&mut rest[..=c.input], &mut cached, volume);
                c.volume = Some(*volume);
                c.buffer = cached;
            }
            buffer.copy_from_slice(&c.buffer);
        }
        ChunkComp::Interpolated(i) => interpolated_volume(i, rest, buffer, volume),
        ChunkComp::PassThrough { input, .. } => sample_volume(&mut rest[..=*input], buffer, volume),
        ChunkComp::Zero { .. } => buffer.fill(0.0),
    }
}

#[inline]
fn gradient(tiling: Tiling, from: i32, to: i32, from_value: f32, to_value: f32, c: i32) -> f32 {
    let range = to - from;
    let factor = (to_value - from_value) / (range as f32);
    match tiling {
        Tiling::ClampToEdge => {
            let rel = c.clamp(from.min(to), from.max(to)) - from;
            from_value + rel as f32 * factor
        }
        Tiling::Repeat => from_value + (c - from).rem_euclid(range) as f32 * factor,
        Tiling::MirroredRepeat => {
            let rel = c - from;
            let tile = rel.div_euclid(range);
            let local = rel - tile * range;
            if tile & 1 == 0 {
                from_value + local as f32 * factor
            } else {
                from_value + (range - local) as f32 * factor
            }
        }
    }
}

fn sample_independent(ind: &Independent, x: i32, y: i32, z: i32) -> f32 {
    match ind {
        Independent::Constant(v) => *v,
        Independent::EndIslands(e) => e.sample(x, z),
        Independent::Noise { sampler, xz_scale, y_scale } => sampler.get(x as f64 * xz_scale, y as f64 * y_scale, z as f64 * xz_scale),
        Independent::ShiftA(s) => s.get(x as f64 * 0.25, y as f64 * 0.0, z as f64 * 0.25) * 4.0,
        Independent::ShiftB(s) => s.get(z as f64 * 0.25, x as f64 * 0.25, 0.0) * 4.0,
        Independent::OldBlended(b) => b.sample(x, y, z),
        Independent::Gradient { axis, tiling, from, to, from_value, to_value } => {
            let c = match axis {
                Axis::X => x,
                Axis::Y => y,
                Axis::Z => z,
            };
            gradient(*tiling, *from, *to, *from_value, *to_value, c)
        }
        Independent::DistanceToPoint { point, metric } => {
            let dx = (x - point[0]) as f32;
            let dy = (y - point[1]) as f32;
            let dz = (z - point[2]) as f32;
            match metric {
                DistanceMetric::Euclidean => (dx * dx + dy * dy + dz * dz).sqrt(),
                DistanceMetric::EuclideanSquared => dx * dx + dy * dy + dz * dz,
                DistanceMetric::Manhattan => dx.abs() + dy.abs() + dz.abs(),
                DistanceMetric::Chebyshev => dx.abs().max(dy.abs()).max(dz.abs()),
            }
        }
    }
}

fn sample_volume_independent(ind: &Independent, buffer: &mut [f32], volume: &DensityVolume) {
    match ind {
        Independent::Constant(v) => buffer.fill(*v),
        Independent::EndIslands(e) => e.sample_volume(buffer, volume),
        Independent::Noise { sampler, xz_scale, y_scale } => {
            buffer.fill(0.0);
            sampler.add_to_volume(buffer, volume, *xz_scale, *y_scale, 1.0);
        }
        Independent::ShiftA(s) => {
            buffer.fill(0.0);
            s.add_to_volume(buffer, volume, 0.25, 0.0, 1.0);
            for v in buffer.iter_mut() {
                *v *= 4.0;
            }
        }
        Independent::ShiftB(s) => {
            let transposed = DensityVolume::new(volume.size_z, volume.size_x, 1, volume.min_z, volume.min_x, 0, volume.step_z, volume.step_x, 1);
            let mut columns = Buf::for_volume(&transposed);
            columns.fill(0.0);
            s.add_to_volume(&mut columns, &transposed, 0.25, 0.25, 4.0);
            for z in 0..volume.size_z {
                for x in 0..volume.size_x {
                    let v = columns[transposed.index(z, x, 0)];
                    let i = volume.index(x, 0, z);
                    buffer[i..i + volume.size_y].fill(v);
                }
            }
        }
        Independent::OldBlended(b) => b.sample_volume(buffer, volume),
        Independent::Gradient { axis, tiling, from, to, from_value, to_value } => match axis {
            Axis::X => {
                for x in 0..volume.size_x {
                    let v = gradient(*tiling, *from, *to, *from_value, *to_value, volume.block_x(x));
                    for z in 0..volume.size_z {
                        let i = volume.index(x, 0, z);
                        buffer[i..i + volume.size_y].fill(v);
                    }
                }
            }
            Axis::Y => {
                for y in 0..volume.size_y {
                    let v = gradient(*tiling, *from, *to, *from_value, *to_value, volume.block_y(y));
                    for z in 0..volume.size_z {
                        for x in 0..volume.size_x {
                            buffer[volume.index(x, y, z)] = v;
                        }
                    }
                }
            }
            Axis::Z => {
                let slab = volume.size_x * volume.size_y;
                for z in 0..volume.size_z {
                    let v = gradient(*tiling, *from, *to, *from_value, *to_value, volume.block_z(z));
                    let i = volume.index(0, 0, z);
                    buffer[i..i + slab].fill(v);
                }
            }
        },
        Independent::DistanceToPoint { .. } => volume.fill_with(buffer, |x, y, z| sample_independent(ind, x, y, z)),
    }
}

#[inline]
fn apply_linear(op: LinearOp, argument: f32, v: f32) -> f32 {
    match op {
        LinearOp::Add => v + argument,
        LinearOp::Mul => v * argument,
    }
}

#[inline]
fn round_to_integer(v: f32, op: RoundingOp) -> f32 {
    match op {
        RoundingOp::Floor => v.floor(),
        RoundingOp::Round => (v + 0.5).floor(),
        RoundingOp::Ceil => v.ceil(),
        RoundingOp::Truncate => {
            if v > 0.0 {
                v.floor()
            } else {
                v.ceil()
            }
        }
    }
}

fn sample_proto(p: &Proto, rest: &mut [ChunkComp], x: i32, y: i32, z: i32) -> f32 {
    match &p.kind {
        ProtoKind::Independent(ind) => sample_independent(ind, x, y, z),
        ProtoKind::Dependent(dep) => sample_dependent(dep, rest, x, y, z),
        // Wrappers, pass-throughs and the beardifier are replaced in chunk stacks.
        ProtoKind::Wrapper { input, .. } | ProtoKind::PassThrough(input) => sample(&mut rest[..=*input], x, y, z),
        ProtoKind::Beardifier => 0.0,
    }
}

fn sample_volume_proto(p: &Proto, rest: &mut [ChunkComp], buffer: &mut [f32], volume: &DensityVolume) {
    match &p.kind {
        ProtoKind::Independent(ind) => sample_volume_independent(ind, buffer, volume),
        ProtoKind::Dependent(dep) => sample_volume_dependent(dep, rest, buffer, volume),
        ProtoKind::Wrapper { input, .. } | ProtoKind::PassThrough(input) => sample_volume(&mut rest[..=*input], buffer, volume),
        ProtoKind::Beardifier => buffer.fill(0.0),
    }
}

fn spline_point(s: &SplineEval, location_of: &mut impl FnMut(usize) -> f32) -> f32 {
    match s {
        SplineEval::Fixed(v) => *v,
        SplineEval::Standard { coordinate, points } => {
            let location = location_of(*coordinate);
            let n = points.len();
            let idx = points.partition_point(|p| location >= p.location);
            if idx == 0 {
                let p = &points[0];
                return spline_point(&p.value, location_of) + p.derivative * (location - p.location);
            }
            if idx == n {
                let p = &points[n - 1];
                return spline_point(&p.value, location_of) + p.derivative * (location - p.location);
            }
            let prev = &points[idx - 1];
            let cur = &points[idx];
            let (x0, x1) = (prev.location, cur.location);
            let v0 = spline_point(&prev.value, location_of);
            let v1 = spline_point(&cur.value, location_of);
            let t = (location - x0) / (x1 - x0);
            let h00 = (1.0 + 2.0 * t) * (1.0 - t) * (1.0 - t);
            let h10 = t * (1.0 - t) * (1.0 - t);
            let h01 = t * t * (3.0 - 2.0 * t);
            let h11 = t * t * (t - 1.0);
            h00.mul_add(v0, h10.mul_add(prev.derivative * (x1 - x0), h01.mul_add(v1, h11 * (cur.derivative * (x1 - x0)))))
        }
    }
}

fn find_surface_from(rest: &mut [ChunkComp], density: usize, lower_bound: i32, cell_height: i32, x: i32, z: i32, upper: f32) -> f32 {
    let top_y = (upper / cell_height as f32).floor() as i32 * cell_height;
    if top_y <= lower_bound {
        return lower_bound as f32;
    }
    let mut y = top_y;
    while y >= lower_bound {
        if sample(&mut rest[..=density], x, y, z) > 0.0 {
            return y as f32;
        }
        y -= cell_height;
    }
    lower_bound as f32
}

fn sample_dependent(dep: &Dependent, rest: &mut [ChunkComp], x: i32, y: i32, z: i32) -> f32 {
    match dep {
        Dependent::Linear { input, op, argument } => apply_linear(*op, *argument, sample(&mut rest[..=*input], x, y, z)),
        Dependent::Unary { input, op } => op.apply(sample(&mut rest[..=*input], x, y, z)),
        Dependent::Clamp { input, min, max } => sample(&mut rest[..=*input], x, y, z).clamp(*min, *max),
        Dependent::Binary { a, b, op } => {
            let va = sample(&mut rest[..=*a], x, y, z);
            match op {
                BinaryOp::Add => va + sample(&mut rest[..=*b], x, y, z),
                BinaryOp::Mul => {
                    if va == 0.0 {
                        0.0
                    } else {
                        va * sample(&mut rest[..=*b], x, y, z)
                    }
                }
                BinaryOp::Min => {
                    if va < rest[*b].min() {
                        va
                    } else {
                        va.min(sample(&mut rest[..=*b], x, y, z))
                    }
                }
                BinaryOp::Max => {
                    if va > rest[*b].max() {
                        va
                    } else {
                        va.max(sample(&mut rest[..=*b], x, y, z))
                    }
                }
                BinaryOp::Sub => va - sample(&mut rest[..=*b], x, y, z),
                BinaryOp::Div => {
                    if va == 0.0 {
                        0.0
                    } else {
                        va / sample(&mut rest[..=*b], x, y, z)
                    }
                }
                BinaryOp::Pow => (va as f64).powf(sample(&mut rest[..=*b], x, y, z) as f64) as f32,
            }
        }
        Dependent::ShiftedNoise { shift_x, shift_y, shift_z, sampler, xz_scale, y_scale } => {
            let sx = sample(&mut rest[..=*shift_x], x, y, z);
            let sy = sample(&mut rest[..=*shift_y], x, y, z);
            let sz = sample(&mut rest[..=*shift_z], x, y, z);
            sampler.get(x as f64 * xz_scale + sx as f64, y as f64 * y_scale + sy as f64, z as f64 * xz_scale + sz as f64)
        }
        Dependent::IntervalSelect { input, thresholds, functions } => {
            let v = sample(&mut rest[..=*input], x, y, z);
            let mut selected = thresholds.len();
            for (i, t) in thresholds.iter().enumerate() {
                if v < *t {
                    selected = i;
                    break;
                }
            }
            sample(&mut rest[..=functions[selected]], x, y, z)
        }
        Dependent::FindTopSurface { density, upper_bound, lower_bound, cell_height } => {
            let upper = sample(&mut rest[..=*upper_bound], x, 0, z);
            find_surface_from(rest, *density, *lower_bound, *cell_height, x, z, upper)
        }
        Dependent::RangeChoice { input, when_in, when_out, min_inclusive, max_exclusive } => {
            let v = sample(&mut rest[..=*input], x, y, z);
            if *min_inclusive <= v && v < *max_exclusive {
                sample(&mut rest[..=*when_in], x, y, z)
            } else {
                sample(&mut rest[..=*when_out], x, y, z)
            }
        }
        Dependent::Spline(s) => spline_point(s, &mut |i| sample(&mut rest[..=i], x, y, z)),
        Dependent::Lerp { alpha, first, second } => {
            let a = sample(&mut rest[..=*alpha], x, y, z);
            if a == 0.0 {
                return sample(&mut rest[..=*first], x, y, z);
            }
            if a == 1.0 {
                return sample(&mut rest[..=*second], x, y, z);
            }
            let f = sample(&mut rest[..=*first], x, y, z);
            let s = sample(&mut rest[..=*second], x, y, z);
            f + a * (s - f)
        }
        Dependent::Rounding { input, multiple, op } => {
            let v = sample(&mut rest[..=*input], x, y, z);
            let m = sample(&mut rest[..=*multiple], x, y, z);
            if m == 0.0 {
                v
            } else {
                round_to_integer(v / m, *op) * m
            }
        }
        Dependent::Slice { input, axis, coordinate } => {
            let (x, y, z) = match axis {
                Axis::X => (*coordinate, y, z),
                Axis::Y => (x, *coordinate, z),
                Axis::Z => (x, y, *coordinate),
            };
            sample(&mut rest[..=*input], x, y, z)
        }
    }
}

fn sample_volume_dependent(dep: &Dependent, rest: &mut [ChunkComp], buffer: &mut [f32], volume: &DensityVolume) {
    match dep {
        Dependent::Linear { input, op, argument } => {
            sample_volume(&mut rest[..=*input], buffer, volume);
            for v in buffer.iter_mut() {
                *v = apply_linear(*op, *argument, *v);
            }
        }
        Dependent::Unary { input, op } => {
            sample_volume(&mut rest[..=*input], buffer, volume);
            for v in buffer.iter_mut() {
                *v = op.apply(*v);
            }
        }
        Dependent::Clamp { input, min, max } => {
            sample_volume(&mut rest[..=*input], buffer, volume);
            for v in buffer.iter_mut() {
                *v = v.clamp(*min, *max);
            }
        }
        Dependent::Binary { a, b, op } => {
            sample_volume(&mut rest[..=*a], buffer, volume);
            match op {
                BinaryOp::Mul => {
                    if buffer.iter().all(|v| *v == 0.0) {
                        buffer.fill(0.0);
                        return;
                    }
                }
                BinaryOp::Min => {
                    let b_min = rest[*b].min();
                    if buffer.iter().all(|v| *v < b_min) {
                        return;
                    }
                }
                BinaryOp::Max => {
                    let b_max = rest[*b].max();
                    if buffer.iter().all(|v| *v > b_max) {
                        return;
                    }
                }
                _ => {}
            }
            let mut other = Buf::for_volume(volume);
            sample_volume(&mut rest[..=*b], &mut other, volume);
            let other = &other[..];
            match op {
                BinaryOp::Add => buffer.iter_mut().zip(other).for_each(|(v, o)| *v += *o),
                BinaryOp::Sub => buffer.iter_mut().zip(other).for_each(|(v, o)| *v -= *o),
                BinaryOp::Mul => buffer.iter_mut().zip(other).for_each(|(v, o)| *v *= *o),
                BinaryOp::Div => buffer.iter_mut().zip(other).for_each(|(v, o)| *v /= *o),
                BinaryOp::Min => buffer.iter_mut().zip(other).for_each(|(v, o)| *v = v.min(*o)),
                BinaryOp::Max => buffer.iter_mut().zip(other).for_each(|(v, o)| *v = v.max(*o)),
                BinaryOp::Pow => buffer.iter_mut().zip(other).for_each(|(v, o)| *v = (*v as f64).powf(*o as f64) as f32),
            }
        }
        Dependent::ShiftedNoise { shift_x, shift_y, shift_z, sampler, xz_scale, y_scale } => {
            sample_volume(&mut rest[..=*shift_x], buffer, volume);
            let mut ys = Buf::for_volume(volume);
            sample_volume(&mut rest[..=*shift_y], &mut ys, volume);
            let mut zs = Buf::for_volume(volume);
            sample_volume(&mut rest[..=*shift_z], &mut zs, volume);
            let mut index = 0;
            if *y_scale == 0.0 && volume.size_y > 1 {
                for z in 0..volume.size_z {
                    let bz = volume.block_z(z) as f64 * xz_scale;
                    for x in 0..volume.size_x {
                        let bx = volume.block_x(x) as f64 * xz_scale;
                        let (mut lx, mut ly, mut lz) = (f32::NAN, f32::NAN, f32::NAN);
                        let mut last = 0.0;
                        for _ in 0..volume.size_y {
                            let (sx, sy, sz) = (buffer[index], ys[index], zs[index]);
                            if sx.to_bits() != lx.to_bits() || sy.to_bits() != ly.to_bits() || sz.to_bits() != lz.to_bits() {
                                lx = sx;
                                ly = sy;
                                lz = sz;
                                last = sampler.get(bx + sx as f64, sy as f64, bz + sz as f64);
                            }
                            buffer[index] = last;
                            index += 1;
                        }
                    }
                }
                return;
            }
            for z in 0..volume.size_z {
                let bz = volume.block_z(z) as f64 * xz_scale;
                for x in 0..volume.size_x {
                    let bx = volume.block_x(x) as f64 * xz_scale;
                    for y in 0..volume.size_y {
                        let nx = bx + buffer[index] as f64;
                        let ny = volume.block_y(y) as f64 * y_scale + ys[index] as f64;
                        let nz = bz + zs[index] as f64;
                        buffer[index] = sampler.get(nx, ny, nz);
                        index += 1;
                    }
                }
            }
        }
        Dependent::IntervalSelect { input, thresholds, functions } => {
            sample_volume(&mut rest[..=*input], buffer, volume);
            let mut bufs = Vec::with_capacity(functions.len());
            for f in functions {
                let mut b = Buf::for_volume(volume);
                sample_volume(&mut rest[..=*f], &mut b, volume);
                bufs.push(b);
            }
            for (i, v) in buffer.iter_mut().enumerate() {
                let mut selected = thresholds.len();
                for (t, th) in thresholds.iter().enumerate() {
                    if *v < *th {
                        selected = t;
                        break;
                    }
                }
                *v = bufs[selected][i];
            }
        }
        Dependent::FindTopSurface { density, upper_bound, lower_bound, cell_height } => {
            if volume.size_y != 1 || volume.min_y != 0 {
                let columns = DensityVolume::new(volume.size_x, 1, volume.size_z, volume.min_x, 0, volume.min_z, volume.step_x, volume.step_y, volume.step_z);
                let mut col = Buf::for_volume(&columns);
                sample_volume_dependent(dep, rest, &mut col, &columns);
                for z in 0..volume.size_z {
                    for x in 0..volume.size_x {
                        let v = col[columns.index(x, 0, z)];
                        let i = volume.index(x, 0, z);
                        buffer[i..i + volume.size_y].fill(v);
                    }
                }
                return;
            }
            sample_volume(&mut rest[..=*upper_bound], buffer, volume);
            let mut index = 0;
            for z in 0..volume.size_z {
                let bz = volume.block_z(z);
                for x in 0..volume.size_x {
                    let bx = volume.block_x(x);
                    buffer[index] = find_surface_from(rest, *density, *lower_bound, *cell_height, bx, bz, buffer[index]);
                    index += 1;
                }
            }
        }
        Dependent::RangeChoice { input, when_in, when_out, min_inclusive, max_exclusive } => {
            sample_volume(&mut rest[..=*input], buffer, volume);
            let (lo, hi) = (*min_inclusive, *max_exclusive);
            let mut any_in = false;
            let mut any_out = false;
            for v in buffer.iter() {
                if lo <= *v && *v < hi {
                    any_in = true;
                } else {
                    any_out = true;
                }
                if any_in && any_out {
                    break;
                }
            }
            if !any_out {
                sample_volume(&mut rest[..=*when_in], buffer, volume);
                return;
            }
            if !any_in {
                sample_volume(&mut rest[..=*when_out], buffer, volume);
                return;
            }
            let mut a = Buf::for_volume(volume);
            sample_volume(&mut rest[..=*when_in], &mut a, volume);
            let mut b = Buf::for_volume(volume);
            sample_volume(&mut rest[..=*when_out], &mut b, volume);
            for ((v, a), b) in buffer.iter_mut().zip(a.iter()).zip(b.iter()) {
                let input = *v;
                *v = if lo <= input && input < hi { *a } else { *b };
            }
        }
        Dependent::Spline(s) => {
            let mut coords: Vec<(usize, Buf)> = Vec::with_capacity(4);
            let mut last = 0.0f32;
            for index in 0..buffer.len() {
                if index > 0 && coords.iter().all(|(_, c)| c[index].to_bits() == c[index - 1].to_bits()) {
                    buffer[index] = last;
                    continue;
                }
                last = spline_point(s, &mut |loc| {
                    let pos = match coords.iter().position(|(c, _)| *c == loc) {
                        Some(p) => p,
                        None => {
                            let mut c = Buf::for_volume(volume);
                            sample_volume(&mut rest[..=loc], &mut c, volume);
                            coords.push((loc, c));
                            coords.len() - 1
                        }
                    };
                    coords[pos].1[index]
                });
                buffer[index] = last;
            }
        }
        Dependent::Lerp { alpha, first, second } => {
            sample_volume(&mut rest[..=*alpha], buffer, volume);
            let mut f = Buf::for_volume(volume);
            sample_volume(&mut rest[..=*first], &mut f, volume);
            let mut s = Buf::for_volume(volume);
            sample_volume(&mut rest[..=*second], &mut s, volume);
            for ((v, f), s) in buffer.iter_mut().zip(f.iter()).zip(s.iter()) {
                let a = *v;
                *v = if a == 0.0 {
                    *f
                } else if a == 1.0 {
                    *s
                } else {
                    f + a * (s - f)
                };
            }
        }
        Dependent::Rounding { input, multiple, op } => {
            sample_volume(&mut rest[..=*input], buffer, volume);
            let mut m = Buf::for_volume(volume);
            sample_volume(&mut rest[..=*multiple], &mut m, volume);
            for (v, m) in buffer.iter_mut().zip(m.iter()) {
                if *m != 0.0 {
                    *v = round_to_integer(*v / m, *op) * m;
                }
            }
        }
        Dependent::Slice { input, axis, coordinate } => {
            let (input_volume, single) = match axis {
                Axis::X => (DensityVolume::new(1, volume.size_y, volume.size_z, *coordinate, volume.min_y, volume.min_z, 1, volume.step_y, volume.step_z), volume.size_x == 1),
                Axis::Y => (DensityVolume::new(volume.size_x, 1, volume.size_z, volume.min_x, *coordinate, volume.min_z, volume.step_x, 1, volume.step_z), volume.size_y == 1),
                Axis::Z => (DensityVolume::new(volume.size_x, volume.size_y, 1, volume.min_x, volume.min_y, *coordinate, volume.step_x, volume.step_y, 1), volume.size_z == 1),
            };
            if single {
                sample_volume(&mut rest[..=*input], buffer, &input_volume);
                return;
            }
            let mut inp = Buf::for_volume(&input_volume);
            sample_volume(&mut rest[..=*input], &mut inp, &input_volume);
            match axis {
                Axis::X => {
                    let mut index = 0;
                    for z in 0..volume.size_z {
                        for _ in 0..volume.size_x {
                            for y in 0..volume.size_y {
                                buffer[index] = inp[input_volume.index(0, y, z)];
                                index += 1;
                            }
                        }
                    }
                }
                Axis::Y => {
                    for z in 0..volume.size_z {
                        for x in 0..volume.size_x {
                            let v = inp[input_volume.index(x, 0, z)];
                            let i = volume.index(x, 0, z);
                            buffer[i..i + volume.size_y].fill(v);
                        }
                    }
                }
                Axis::Z => {
                    let slab = volume.size_x * volume.size_y;
                    for z in 0..volume.size_z {
                        let i = volume.index(0, 0, z);
                        buffer[i..i + slab].copy_from_slice(&inp[..slab]);
                    }
                }
            }
        }
    }
}

fn interpolated_sample(s: &InterpState, rest: &mut [ChunkComp], x: i32, y: i32, z: i32) -> f32 {
    let xi = x.rem_euclid(s.cell_xz);
    let yi = y.rem_euclid(s.cell_y);
    let zi = z.rem_euclid(s.cell_xz);
    if xi == 0 && yi == 0 && zi == 0 {
        return sample(&mut rest[..=s.input], x, y, z);
    }
    let cell = DensityVolume::new(2, 2, 2, x - xi, y - yi, z - zi, s.cell_xz, s.cell_y, s.cell_xz);
    let mut c = Buf::for_volume(&cell);
    sample_volume(&mut rest[..=s.input], &mut c, &cell);
    let corner = |x: usize, y: usize, z: usize| c[cell.index(x, y, z)];
    let dx = xi as f32 / s.cell_xz as f32;
    let dy = yi as f32 / s.cell_y as f32;
    let dz = zi as f32 / s.cell_xz as f32;
    lerp(
        dz,
        lerp(dy, lerp(dx, corner(0, 0, 0), corner(1, 0, 0)), lerp(dx, corner(0, 1, 0), corner(1, 1, 0))),
        lerp(dy, lerp(dx, corner(0, 0, 1), corner(1, 0, 1)), lerp(dx, corner(0, 1, 1), corner(1, 1, 1))),
    )
}

fn interpolated_volume(s: &InterpState, rest: &mut [ChunkComp], buffer: &mut [f32], volume: &DensityVolume) {
    let aligned = (volume.step_x == s.cell_xz || volume.size_x == 1)
        && (volume.step_y == s.cell_y || volume.size_y == 1)
        && (volume.step_z == s.cell_xz || volume.size_z == 1)
        && volume.min_x.rem_euclid(s.cell_xz) == 0
        && volume.min_y.rem_euclid(s.cell_y) == 0
        && volume.min_z.rem_euclid(s.cell_xz) == 0;
    if aligned {
        sample_volume(&mut rest[..=s.input], buffer, volume);
    } else if volume.is_block_step() {
        interpolate_blocks(s, rest, buffer, volume);
    } else {
        let blocks = DensityVolume::blocks(
            volume.size_x * volume.step_x as usize,
            volume.size_y * volume.step_y as usize,
            volume.size_z * volume.step_z as usize,
            volume.min_x,
            volume.min_y,
            volume.min_z,
        );
        let mut b = Buf::for_volume(&blocks);
        interpolate_blocks(s, rest, &mut b, &blocks);
        for z in 0..volume.size_z {
            for x in 0..volume.size_x {
                for y in 0..volume.size_y {
                    buffer[volume.index(x, y, z)] = b[blocks.index(x * volume.step_x as usize, y * volume.step_y as usize, z * volume.step_z as usize)];
                }
            }
        }
    }
}

fn interpolate_blocks(s: &InterpState, rest: &mut [ChunkComp], buffer: &mut [f32], volume: &DensityVolume) {
    let min_cx = volume.min_x.div_euclid(s.cell_xz);
    let min_cy = volume.min_y.div_euclid(s.cell_y);
    let min_cz = volume.min_z.div_euclid(s.cell_xz);
    let (max_x, max_y, max_z) = (volume.max_x(), volume.max_y(), volume.max_z());
    let cells_x = (max_x.div_euclid(s.cell_xz) - min_cx + 1) as usize;
    let cells_y = (max_y.div_euclid(s.cell_y) - min_cy + 1) as usize;
    let cells_z = (max_z.div_euclid(s.cell_xz) - min_cz + 1) as usize;
    let corners = |count: usize, max: i32, size: i32| if max.rem_euclid(size) == 0 { count } else { count + 1 };
    let cv = DensityVolume::new(
        corners(cells_x, max_x, s.cell_xz),
        corners(cells_y, max_y, s.cell_y),
        corners(cells_z, max_z, s.cell_xz),
        min_cx * s.cell_xz,
        min_cy * s.cell_y,
        min_cz * s.cell_xz,
        s.cell_xz,
        s.cell_y,
        s.cell_xz,
    );
    let mut c = Buf::for_volume(&cv);
    sample_volume(&mut rest[..=s.input], &mut c, &cv);
    for cz in 0..cells_z {
        let nz = (cz + 1).min(cv.size_z - 1);
        for cx in 0..cells_x {
            let nx = (cx + 1).min(cv.size_x - 1);
            for cy in 0..cells_y {
                let ny = (cy + 1).min(cv.size_y - 1);
                let corner = |x: usize, y: usize, z: usize| c[cv.index(x, y, z)];
                let v000 = corner(cx, cy, cz);
                let v100 = corner(nx, cy, cz);
                let v010 = corner(cx, ny, cz);
                let v110 = corner(nx, ny, cz);
                let v001 = corner(cx, cy, nz);
                let v101 = corner(nx, cy, nz);
                let v011 = corner(cx, ny, nz);
                let v111 = corner(nx, ny, nz);
                let out_x = cv.block_x(cx) - volume.min_x;
                let out_y = cv.block_y(cy) - volume.min_y;
                let out_z = cv.block_z(cz) - volume.min_z;
                let x0 = 0.max(-out_x);
                let y0 = 0.max(-out_y);
                let z0 = 0.max(-out_z);
                let x1 = s.cell_xz.min(volume.size_x as i32 - out_x) - 1;
                let y1 = s.cell_y.min(volume.size_y as i32 - out_y) - 1;
                let z1 = s.cell_xz.min(volume.size_z as i32 - out_z) - 1;
                for z in z0..=z1 {
                    let az = z as f32 * s.inv_xz;
                    let oz = (out_z + z) as usize;
                    let v00 = lerp(az, v000, v001);
                    let v01 = lerp(az, v010, v011);
                    let v10 = lerp(az, v100, v101);
                    let v11 = lerp(az, v110, v111);
                    for x in x0..=x1 {
                        let ax = x as f32 * s.inv_xz;
                        let ox = (out_x + x) as usize;
                        let v0 = lerp(ax, v00, v10);
                        let v1 = lerp(ax, v01, v11);
                        let step = (v1 - v0) * s.inv_y;
                        let mut value = v0 + step * y0 as f32;
                        let start = volume.index(ox, (out_y + y0) as usize, oz);
                        for slot in &mut buffer[start..start + (y1 - y0 + 1) as usize] {
                            *slot = value;
                            value += step;
                        }
                    }
                }
            }
        }
    }
}
