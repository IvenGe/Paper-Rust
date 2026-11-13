//! Flattening of density function trees into a component stack (identical sub-trees share one
//! component), and seeding of the stack into immutable "proto" components.
//!
//! Derived from `tools/pumpkin-codegen/src/noise_router.rs` and
//! `pumpkin-world/src/generation/noise/router/proto_noise_router.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use super::blended::OldBlendedNoise;
use super::end_islands::EndIslands;
use super::ir::{Axis, BinaryOp, Df, DistanceMetric, LinearOp, RoundingOp, Spline, Tiling, UnaryOp, WrapperKind};
use crate::noise::NormalNoise;
use std::collections::HashMap;
use std::sync::Arc;

/// `f32` compared and hashed by bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct B32(pub u32);

impl B32 {
    pub fn new(v: f32) -> Self {
        Self(v.to_bits())
    }
    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}

/// `f64` compared and hashed by bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct B64(pub u64);

impl B64 {
    pub fn new(v: f64) -> Self {
        Self(v.to_bits())
    }
    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

/// A spline whose coordinate functions are stack indices.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BaseSpline {
    Fixed(B32),
    Standard { coordinate: usize, points: Vec<(B32, BaseSpline, B32)> },
}

/// One flattened density function component; children are stack indices.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Base {
    Beardifier,
    BlendAlpha,
    BlendOffset,
    BlendDensity(usize),
    FindTopSurface { density: usize, upper_bound: usize, lower_bound: i32, cell_height: i32 },
    EndIslands,
    Noise { noise: String, xz_scale: B64, y_scale: B64 },
    ShiftA(String),
    ShiftB(String),
    ShiftedNoise { shift_x: usize, shift_y: usize, shift_z: usize, noise: String, xz_scale: B64, y_scale: B64 },
    OldBlendedNoise { xz_scale: B64, y_scale: B64, xz_factor: B64, y_factor: B64, smear: B64 },
    IntervalSelect { input: usize, thresholds: Vec<B32>, functions: Vec<usize> },
    Wrapper { input: usize, kind: WrapperKind },
    Constant(B32),
    Gradient { axis: Axis, tiling: Tiling, from: i32, to: i32, from_value: B32, to_value: B32 },
    DistanceToPoint { point: [i32; 3], metric: DistanceMetric },
    Lerp { alpha: usize, first: usize, second: usize },
    Rounding { input: usize, multiple: usize, op: RoundingOp },
    Slice { axis: Axis, coordinate: i32, input: usize },
    Binary { op: BinaryOp, a: usize, b: usize },
    Linear { op: LinearOp, input: usize, argument: B32 },
    Unary { op: UnaryOp, input: usize },
    Clamp { input: usize, min: B32, max: B32 },
    RangeChoice { input: usize, min_inclusive: B32, max_exclusive: B32, when_in: usize, when_out: usize },
    Spline(BaseSpline),
}

/// Builds a deduplicated component stack from density function trees.
#[derive(Default)]
pub struct StackBuilder {
    pub components: Vec<Base>,
    index: HashMap<Base, usize>,
}

impl StackBuilder {
    fn push(&mut self, base: Base) -> usize {
        if let Some(i) = self.index.get(&base) {
            return *i;
        }
        let i = self.components.len();
        self.components.push(base.clone());
        self.index.insert(base, i);
        i
    }

    fn spline(&mut self, s: &Spline) -> BaseSpline {
        match s {
            Spline::Fixed(v) => BaseSpline::Fixed(B32::new(*v)),
            Spline::Standard { coordinate, locations, values, derivatives } => {
                let coordinate = self.add(coordinate);
                let points = locations
                    .iter()
                    .zip(values)
                    .zip(derivatives)
                    .map(|((l, v), d)| (B32::new(*l), self.spline(v), B32::new(*d)))
                    .collect();
                BaseSpline::Standard { coordinate, points }
            }
        }
    }

    /// Adds a tree and returns the stack index of its root.
    pub fn add(&mut self, df: &Df) -> usize {
        let base = match df {
            Df::Beardifier => Base::Beardifier,
            Df::BlendAlpha => Base::BlendAlpha,
            Df::BlendOffset => Base::BlendOffset,
            Df::BlendDensity(input) => Base::BlendDensity(self.add(input)),
            Df::FindTopSurface { density, upper_bound, lower_bound, cell_height } => {
                let density = self.add(density);
                let upper_bound = self.add(upper_bound);
                Base::FindTopSurface { density, upper_bound, lower_bound: *lower_bound, cell_height: *cell_height }
            }
            Df::EndIslands => Base::EndIslands,
            Df::Noise { noise, xz_scale, y_scale } => Base::Noise { noise: noise.clone(), xz_scale: B64::new(*xz_scale), y_scale: B64::new(*y_scale) },
            Df::ShiftA(n) => Base::ShiftA(n.clone()),
            Df::ShiftB(n) => Base::ShiftB(n.clone()),
            Df::ShiftedNoise { shift_x, shift_y, shift_z, noise, xz_scale, y_scale } => {
                let shift_x = self.add(shift_x);
                let shift_y = self.add(shift_y);
                let shift_z = self.add(shift_z);
                Base::ShiftedNoise { shift_x, shift_y, shift_z, noise: noise.clone(), xz_scale: B64::new(*xz_scale), y_scale: B64::new(*y_scale) }
            }
            Df::OldBlendedNoise { xz_scale, y_scale, xz_factor, y_factor, smear_scale_multiplier } => Base::OldBlendedNoise {
                xz_scale: B64::new(*xz_scale),
                y_scale: B64::new(*y_scale),
                xz_factor: B64::new(*xz_factor),
                y_factor: B64::new(*y_factor),
                smear: B64::new(*smear_scale_multiplier),
            },
            Df::IntervalSelect { input, thresholds, functions } => {
                let input = self.add(input);
                let functions = functions.iter().map(|f| self.add(f)).collect();
                Base::IntervalSelect { input, thresholds: thresholds.iter().map(|t| B32::new(*t)).collect(), functions }
            }
            Df::Wrapper { input, kind } => Base::Wrapper { input: self.add(input), kind: *kind },
            Df::Constant(v) => Base::Constant(B32::new(*v)),
            Df::Gradient { axis, tiling, from, to, from_value, to_value } => Base::Gradient {
                axis: *axis,
                tiling: *tiling,
                from: *from,
                to: *to,
                from_value: B32::new(*from_value),
                to_value: B32::new(*to_value),
            },
            Df::DistanceToPoint { point, metric } => Base::DistanceToPoint { point: *point, metric: *metric },
            Df::Lerp { alpha, first, second } => {
                let alpha = self.add(alpha);
                let first = self.add(first);
                let second = self.add(second);
                Base::Lerp { alpha, first, second }
            }
            Df::Rounding { input, multiple, op } => {
                let input = self.add(input);
                let multiple = self.add(multiple);
                Base::Rounding { input, multiple, op: *op }
            }
            Df::Slice { axis, coordinate, input } => Base::Slice { axis: *axis, coordinate: *coordinate, input: self.add(input) },
            Df::Binary { op, a, b } => {
                let a = self.add(a);
                let b = self.add(b);
                Base::Binary { op: *op, a, b }
            }
            Df::Linear { op, input, argument } => Base::Linear { op: *op, input: self.add(input), argument: B32::new(*argument) },
            Df::Unary { op, input } => Base::Unary { op: *op, input: self.add(input) },
            Df::Clamp { input, min, max } => Base::Clamp { input: self.add(input), min: B32::new(*min), max: B32::new(*max) },
            Df::RangeChoice { input, min_inclusive, max_exclusive, when_in, when_out } => {
                let input = self.add(input);
                let when_in = self.add(when_in);
                let when_out = self.add(when_out);
                Base::RangeChoice { input, min_inclusive: B32::new(*min_inclusive), max_exclusive: B32::new(*max_exclusive), when_in, when_out }
            }
            Df::Spline(s) => Base::Spline(self.spline(s)),
        };
        self.push(base)
    }
}

/// A spline over stack indices, as evaluated.
#[derive(Debug)]
pub enum SplineEval {
    Fixed(f32),
    Standard { coordinate: usize, points: Vec<SplinePoint> },
}

#[derive(Debug)]
pub struct SplinePoint {
    pub location: f32,
    pub value: SplineEval,
    pub derivative: f32,
}

impl SplineEval {
    fn from_base(b: &BaseSpline) -> Self {
        match b {
            BaseSpline::Fixed(v) => SplineEval::Fixed(v.get()),
            BaseSpline::Standard { coordinate, points } => SplineEval::Standard {
                coordinate: *coordinate,
                points: points.iter().map(|(l, v, d)| SplinePoint { location: l.get(), value: SplineEval::from_base(v), derivative: d.get() }).collect(),
            },
        }
    }
}

/// Components that do not read other components.
pub enum Independent {
    Constant(f32),
    EndIslands(Arc<EndIslands>),
    Noise { sampler: Arc<NormalNoise>, xz_scale: f64, y_scale: f64 },
    ShiftA(Arc<NormalNoise>),
    ShiftB(Arc<NormalNoise>),
    OldBlended(Box<OldBlendedNoise>),
    Gradient { axis: Axis, tiling: Tiling, from: i32, to: i32, from_value: f32, to_value: f32 },
    DistanceToPoint { point: [i32; 3], metric: DistanceMetric },
}

/// Components computed from other components.
pub enum Dependent {
    Linear { input: usize, op: LinearOp, argument: f32 },
    Unary { input: usize, op: UnaryOp },
    Binary { a: usize, b: usize, op: BinaryOp },
    ShiftedNoise { shift_x: usize, shift_y: usize, shift_z: usize, sampler: Arc<NormalNoise>, xz_scale: f64, y_scale: f64 },
    IntervalSelect { input: usize, thresholds: Vec<f32>, functions: Vec<usize> },
    FindTopSurface { density: usize, upper_bound: usize, lower_bound: i32, cell_height: i32 },
    Clamp { input: usize, min: f32, max: f32 },
    RangeChoice { input: usize, when_in: usize, when_out: usize, min_inclusive: f32, max_exclusive: f32 },
    Spline(SplineEval),
    Lerp { alpha: usize, first: usize, second: usize },
    Rounding { input: usize, multiple: usize, op: RoundingOp },
    Slice { input: usize, axis: Axis, coordinate: i32 },
}

pub enum ProtoKind {
    Independent(Independent),
    Dependent(Dependent),
    Wrapper { input: usize, kind: WrapperKind },
    /// Structures are not generated, so the beardifier is always zero.
    Beardifier,
    PassThrough(usize),
}

/// A seeded, immutable component with its value range.
pub struct Proto {
    pub kind: ProtoKind,
    pub min: f32,
    pub max: f32,
}

/// Creates the noise samplers referenced by density functions.
pub trait NoiseFactory {
    /// The sampler for a `worldgen/noise` id (memoized by the implementation).
    fn noise(&mut self, id: &str) -> anyhow::Result<Arc<NormalNoise>>;
    fn end_islands(&mut self) -> Arc<EndIslands>;
    fn old_blended(&mut self, xz_scale: f64, y_scale: f64, xz_factor: f64, y_factor: f64, smear: f64) -> OldBlendedNoise;
}

fn spline_range(s: &SplineEval, stack: &[Proto]) -> (f32, f32) {
    fn outside(p: &SplinePoint, at: f32, last: f32) -> f32 {
        if p.derivative == 0.0 {
            last
        } else {
            p.derivative * (at - p.location) + last
        }
    }
    match s {
        SplineEval::Fixed(v) => (*v, *v),
        SplineEval::Standard { coordinate, points } => {
            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;
            let input = &stack[*coordinate];
            let (input_min, input_max) = (input.min, input.max);
            let Some(first) = points.first() else {
                return (0.0, 0.0);
            };
            if input_min < first.location {
                let (pmin, pmax) = spline_range(&first.value, stack);
                let a = outside(first, input_min, pmin);
                let b = outside(first, input_min, pmax);
                min = min.min(a.min(b));
                max = max.max(a.max(b));
            }
            let last = points.last().expect("non-empty");
            if input_max > last.location {
                let (pmin, pmax) = spline_range(&last.value, stack);
                let a = outside(last, input_max, pmin);
                let b = outside(last, input_max, pmax);
                min = min.min(a.min(b));
                max = max.max(a.max(b));
            }
            for p in points {
                let (pmin, pmax) = spline_range(&p.value, stack);
                min = min.min(pmin);
                max = max.max(pmax);
            }
            for w in points.windows(2) {
                let (p1, p2) = (&w[0], &w[1]);
                if p1.derivative != 0.0 || p2.derivative != 0.0 {
                    let delta = p2.location - p1.location;
                    let (p1min, p1max) = spline_range(&p1.value, stack);
                    let (p2min, p2max) = spline_range(&p2.value, stack);
                    let p1partial = p1.derivative * delta;
                    let p2partial = p2.derivative * delta;
                    let points_min = p1min.min(p2min);
                    let points_max = p1max.max(p2max);
                    let z = p1partial - p2max + p1min;
                    let aa = p1partial - p2min + p1max;
                    let ab = -p2partial + p2min - p1max;
                    let ac = -p2partial + p2max - p1max;
                    min = min.min(points_min + 0.25 * z.min(ab));
                    max = max.max(points_max + 0.25 * aa.max(ac));
                }
            }
            (min, max)
        }
    }
}

fn mul_range(a_min: f32, a_max: f32, b_min: f32, b_max: f32) -> (f32, f32) {
    let min = if a_min > 0.0 && b_min > 0.0 {
        a_min * b_min
    } else if a_max < 0.0 && b_max < 0.0 {
        a_max * b_max
    } else {
        (a_min * b_max).min(a_max * b_min)
    };
    let max = if a_min > 0.0 && b_min > 0.0 {
        a_max * b_max
    } else if a_max < 0.0 && b_max < 0.0 {
        a_min * b_min
    } else {
        (a_min * b_min).max(a_max * b_max)
    };
    (min, max)
}

/// Seeds a flattened stack.
pub fn seed_stack(base: &[Base], factory: &mut dyn NoiseFactory) -> anyhow::Result<Vec<Proto>> {
    let mut stack: Vec<Proto> = Vec::with_capacity(base.len());
    for b in base {
        let range = |i: usize| (stack[i].min, stack[i].max);
        let proto = match b {
            Base::Spline(s) => {
                let s = SplineEval::from_base(s);
                let (min, max) = spline_range(&s, &stack);
                Proto { kind: ProtoKind::Dependent(Dependent::Spline(s)), min, max }
            }
            Base::FindTopSurface { density, upper_bound, lower_bound, cell_height } => {
                let min = *lower_bound as f32;
                let max = stack[*upper_bound].max.max(min);
                Proto {
                    kind: ProtoKind::Dependent(Dependent::FindTopSurface { density: *density, upper_bound: *upper_bound, lower_bound: *lower_bound, cell_height: *cell_height }),
                    min,
                    max,
                }
            }
            Base::EndIslands => Proto { kind: ProtoKind::Independent(Independent::EndIslands(factory.end_islands())), min: -0.84375, max: 0.5625 },
            Base::Noise { noise, xz_scale, y_scale } => {
                let sampler = factory.noise(noise)?;
                let max = sampler.max_value();
                Proto { kind: ProtoKind::Independent(Independent::Noise { sampler, xz_scale: xz_scale.get(), y_scale: y_scale.get() }), min: -max, max }
            }
            Base::ShiftA(noise) => {
                let sampler = factory.noise(noise)?;
                let max = sampler.max_value() * 4.0;
                Proto { kind: ProtoKind::Independent(Independent::ShiftA(sampler)), min: -max, max }
            }
            Base::ShiftB(noise) => {
                let sampler = factory.noise(noise)?;
                let max = sampler.max_value() * 4.0;
                Proto { kind: ProtoKind::Independent(Independent::ShiftB(sampler)), min: -max, max }
            }
            Base::BlendDensity(input) => {
                let (min, max) = range(*input);
                Proto { kind: ProtoKind::PassThrough(*input), min, max }
            }
            Base::BlendAlpha => Proto { kind: ProtoKind::Independent(Independent::Constant(1.0)), min: 1.0, max: 1.0 },
            Base::BlendOffset => Proto { kind: ProtoKind::Independent(Independent::Constant(0.0)), min: 0.0, max: 0.0 },
            Base::Beardifier => Proto { kind: ProtoKind::Beardifier, min: f32::NEG_INFINITY, max: f32::INFINITY },
            Base::ShiftedNoise { shift_x, shift_y, shift_z, noise, xz_scale, y_scale } => {
                let sampler = factory.noise(noise)?;
                let max = sampler.max_value();
                Proto {
                    kind: ProtoKind::Dependent(Dependent::ShiftedNoise {
                        shift_x: *shift_x,
                        shift_y: *shift_y,
                        shift_z: *shift_z,
                        sampler,
                        xz_scale: xz_scale.get(),
                        y_scale: y_scale.get(),
                    }),
                    min: -max,
                    max,
                }
            }
            Base::RangeChoice { input, min_inclusive, max_exclusive, when_in, when_out } => {
                let min = stack[*when_in].min.min(stack[*when_out].min);
                let max = stack[*when_in].max.max(stack[*when_out].max);
                Proto {
                    kind: ProtoKind::Dependent(Dependent::RangeChoice {
                        input: *input,
                        when_in: *when_in,
                        when_out: *when_out,
                        min_inclusive: min_inclusive.get(),
                        max_exclusive: max_exclusive.get(),
                    }),
                    min,
                    max,
                }
            }
            Base::Binary { op, a, b } => {
                let (a_min, a_max) = range(*a);
                let (b_min, b_max) = range(*b);
                let (min, max) = match op {
                    BinaryOp::Add => (a_min + b_min, a_max + b_max),
                    BinaryOp::Mul => mul_range(a_min, a_max, b_min, b_max),
                    BinaryOp::Min => (a_min.min(b_min), a_max.min(b_max)),
                    BinaryOp::Max => (a_min.max(b_min), a_max.max(b_max)),
                    BinaryOp::Sub => (a_min - b_max, a_max - b_min),
                    BinaryOp::Div => (f32::NEG_INFINITY, f32::INFINITY),
                    BinaryOp::Pow => (0.0, f32::INFINITY),
                };
                Proto { kind: ProtoKind::Dependent(Dependent::Binary { a: *a, b: *b, op: *op }), min, max }
            }
            Base::Gradient { axis, tiling, from, to, from_value, to_value } => {
                let (fv, tv) = (from_value.get(), to_value.get());
                Proto {
                    kind: ProtoKind::Independent(Independent::Gradient { axis: *axis, tiling: *tiling, from: *from, to: *to, from_value: fv, to_value: tv }),
                    min: fv.min(tv),
                    max: fv.max(tv),
                }
            }
            Base::DistanceToPoint { point, metric } => {
                Proto { kind: ProtoKind::Independent(Independent::DistanceToPoint { point: *point, metric: *metric }), min: 0.0, max: f32::INFINITY }
            }
            Base::Lerp { alpha, first, second } => {
                let min = stack[*first].min.min(stack[*second].min);
                let max = stack[*first].max.max(stack[*second].max);
                Proto { kind: ProtoKind::Dependent(Dependent::Lerp { alpha: *alpha, first: *first, second: *second }), min, max }
            }
            Base::Rounding { input, multiple, op } => {
                let (min, max) = range(*input);
                Proto { kind: ProtoKind::Dependent(Dependent::Rounding { input: *input, multiple: *multiple, op: *op }), min, max }
            }
            Base::Slice { axis, coordinate, input } => {
                let (min, max) = range(*input);
                Proto { kind: ProtoKind::Dependent(Dependent::Slice { input: *input, axis: *axis, coordinate: *coordinate }), min, max }
            }
            Base::Constant(v) => {
                let v = v.get();
                Proto { kind: ProtoKind::Independent(Independent::Constant(v)), min: v, max: v }
            }
            Base::Wrapper { input, kind } => {
                let (min, max) = range(*input);
                Proto { kind: ProtoKind::Wrapper { input: *input, kind: *kind }, min, max }
            }
            Base::Linear { op, input, argument } => {
                let (a_min, a_max) = range(*input);
                let arg = argument.get();
                let (min, max) = match op {
                    LinearOp::Add => (a_min + arg, a_max + arg),
                    LinearOp::Mul => mul_range(a_min, a_max, arg, arg),
                };
                Proto { kind: ProtoKind::Dependent(Dependent::Linear { input: *input, op: *op, argument: arg }), min, max }
            }
            Base::Clamp { input, min, max } => {
                let (min, max) = (min.get(), max.get());
                Proto { kind: ProtoKind::Dependent(Dependent::Clamp { input: *input, min, max }), min, max }
            }
            Base::Unary { op, input } => {
                let (a_min, a_max) = range(*input);
                let am = op.apply(a_min);
                let ax = op.apply(a_max);
                let (min, max) = match op {
                    UnaryOp::Abs | UnaryOp::Square => (a_min.max(0.0), am.max(ax)),
                    UnaryOp::Squeeze | UnaryOp::Cube | UnaryOp::QuarterNegative | UnaryOp::HalfNegative => (am, ax),
                    UnaryOp::Invert => {
                        if a_min < 0.0 && a_max > 0.0 {
                            (f32::NEG_INFINITY, f32::INFINITY)
                        } else {
                            (ax, am)
                        }
                    }
                    UnaryOp::Negate => (-a_max, -a_min),
                    UnaryOp::Sqrt => (0.0, ax.max(0.0)),
                    UnaryOp::Log => (f32::NEG_INFINITY, ax),
                    UnaryOp::Sign => (-1.0, 1.0),
                };
                Proto { kind: ProtoKind::Dependent(Dependent::Unary { input: *input, op: *op }), min, max }
            }
            Base::IntervalSelect { input, thresholds, functions } => {
                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;
                for &f in functions {
                    if stack[f].min < min {
                        min = stack[f].min;
                    }
                    if stack[f].max > max {
                        max = stack[f].max;
                    }
                }
                Proto {
                    kind: ProtoKind::Dependent(Dependent::IntervalSelect { input: *input, thresholds: thresholds.iter().map(|t| t.get()).collect(), functions: functions.clone() }),
                    min,
                    max,
                }
            }
            Base::OldBlendedNoise { xz_scale, y_scale, xz_factor, y_factor, smear } => {
                let n = factory.old_blended(xz_scale.get(), y_scale.get(), xz_factor.get(), y_factor.get(), smear.get());
                let max = n.max_value();
                Proto { kind: ProtoKind::Independent(Independent::OldBlended(Box::new(n))), min: -max, max }
            }
        };
        stack.push(proto);
    }
    Ok(stack)
}
