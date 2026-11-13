//! Density function trees as read from `worldgen/density_function` JSON, with the constant
//! folding and axis slicing vanilla 26.x applies before evaluation.
//!
//! Derived from `tools/pumpkin-codegen/src/noise_router.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0. Pumpkin runs this at build time to
//! generate Rust code; here it runs when a generator is created.

use crate::data::{DataReader, JsonExt};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    const fn bits(self) -> u8 {
        match self {
            Axis::X => AXIS_X,
            Axis::Y => AXIS_Y,
            Axis::Z => AXIS_Z,
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "x" | "X" => Ok(Axis::X),
            "y" | "Y" => Ok(Axis::Y),
            "z" | "Z" => Ok(Axis::Z),
            _ => Err(anyhow!("unknown axis {s}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tiling {
    ClampToEdge,
    Repeat,
    MirroredRepeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DistanceMetric {
    Euclidean,
    EuclideanSquared,
    Manhattan,
    Chebyshev,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RoundingOp {
    Floor,
    Round,
    Ceil,
    Truncate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Mul,
    Min,
    Max,
    Sub,
    Div,
    Pow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinearOp {
    Add,
    Mul,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Invert,
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Squeeze,
    Negate,
    Sqrt,
    Log,
    Sign,
}

impl UnaryOp {
    #[inline]
    pub fn apply(self, d: f32) -> f32 {
        match self {
            UnaryOp::Abs => d.abs(),
            UnaryOp::Square => d * d,
            UnaryOp::Cube => d * d * d,
            UnaryOp::HalfNegative => {
                if d > 0.0 {
                    d
                } else {
                    d * 0.5
                }
            }
            UnaryOp::QuarterNegative => {
                if d > 0.0 {
                    d
                } else {
                    d * 0.25
                }
            }
            UnaryOp::Squeeze => {
                let c = d.clamp(-1.0, 1.0);
                c / 2.0 - c * c * c / 24.0
            }
            UnaryOp::Invert => {
                if d == 0.0 {
                    f32::INFINITY
                } else {
                    1.0 / d
                }
            }
            UnaryOp::Negate => -d,
            UnaryOp::Sqrt => d.sqrt(),
            UnaryOp::Log => d.ln(),
            UnaryOp::Sign => {
                if d > 0.0 {
                    1.0
                } else if d < 0.0 {
                    -1.0
                } else {
                    0.0
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WrapperKind {
    Interpolated { cell_size_xz: i32, cell_size_y: i32 },
    Cache,
}

/// A density function node.
#[derive(Clone, Debug)]
pub enum Df {
    Beardifier,
    BlendAlpha,
    BlendOffset,
    BlendDensity(Box<Df>),
    FindTopSurface { density: Box<Df>, upper_bound: Box<Df>, lower_bound: i32, cell_height: i32 },
    EndIslands,
    Noise { noise: String, xz_scale: f64, y_scale: f64 },
    ShiftA(String),
    ShiftB(String),
    ShiftedNoise { shift_x: Box<Df>, shift_y: Box<Df>, shift_z: Box<Df>, noise: String, xz_scale: f64, y_scale: f64 },
    OldBlendedNoise { xz_scale: f64, y_scale: f64, xz_factor: f64, y_factor: f64, smear_scale_multiplier: f64 },
    IntervalSelect { input: Box<Df>, thresholds: Vec<f32>, functions: Vec<Df> },
    Wrapper { input: Box<Df>, kind: WrapperKind },
    Constant(f32),
    Gradient { axis: Axis, tiling: Tiling, from: i32, to: i32, from_value: f32, to_value: f32 },
    DistanceToPoint { point: [i32; 3], metric: DistanceMetric },
    Lerp { alpha: Box<Df>, first: Box<Df>, second: Box<Df> },
    Rounding { input: Box<Df>, multiple: Box<Df>, op: RoundingOp },
    Slice { axis: Axis, coordinate: i32, input: Box<Df> },
    Binary { op: BinaryOp, a: Box<Df>, b: Box<Df> },
    Linear { op: LinearOp, input: Box<Df>, argument: f32 },
    Unary { op: UnaryOp, input: Box<Df> },
    Clamp { input: Box<Df>, min: f32, max: f32 },
    RangeChoice { input: Box<Df>, min_inclusive: f32, max_exclusive: f32, when_in: Box<Df>, when_out: Box<Df> },
    Spline(Spline),
}

/// A cubic spline over a density function coordinate.
#[derive(Clone, Debug)]
pub enum Spline {
    Fixed(f32),
    Standard { coordinate: Box<Df>, locations: Vec<f32>, values: Vec<Spline>, derivatives: Vec<f32> },
}

const AXIS_X: u8 = 1;
const AXIS_Y: u8 = 2;
const AXIS_Z: u8 = 4;
const AXES_XZ: u8 = AXIS_X | AXIS_Z;
pub const AXES_ALL: u8 = AXIS_X | AXIS_Y | AXIS_Z;

fn noise_axes(xz_scale: f64, y_scale: f64) -> u8 {
    let mut axes = AXES_ALL;
    if y_scale == 0.0 {
        axes &= !AXIS_Y;
    }
    if xz_scale == 0.0 {
        axes &= !AXES_XZ;
    }
    axes
}

/// Parses density functions, following references to other `density_function` entries.
pub struct DfParser<'r, 'a> {
    data: &'r DataReader<'a>,
    depth: usize,
}

impl<'r, 'a> DfParser<'r, 'a> {
    pub fn new(data: &'r DataReader<'a>) -> Self {
        Self { data, depth: 0 }
    }

    pub fn parse(&mut self, v: &Value) -> Result<Df> {
        self.depth += 1;
        if self.depth > 512 {
            bail!("density function nesting too deep");
        }
        let r = self.parse_inner(v);
        self.depth -= 1;
        r
    }

    fn child(&mut self, obj: &Value, keys: &[&str]) -> Result<Df> {
        for k in keys {
            if let Some(v) = obj.get(*k) {
                return self.parse(v);
            }
        }
        Err(anyhow!("density function missing {:?}", keys))
    }

    fn parse_inner(&mut self, v: &Value) -> Result<Df> {
        match v {
            Value::Number(n) => Ok(Df::Constant(n.as_f64().unwrap_or(0.0) as f32)),
            Value::String(s) => {
                let json = self.data.worldgen("density_function", s)?;
                self.parse(&json)
            }
            Value::Object(_) => self.parse_object(v),
            _ => Err(anyhow!("unsupported density function {v}")),
        }
    }

    fn parse_object(&mut self, obj: &Value) -> Result<Df> {
        let ty = obj.str_field("type")?;
        let ty = ty.strip_prefix("minecraft:").unwrap_or(ty);
        Ok(match ty {
            "constant" => Df::Constant(obj.get("value").or_else(|| obj.get("argument")).and_then(Value::as_f64).unwrap_or(0.0) as f32),
            "y_clamped_gradient" => Df::Gradient {
                axis: Axis::Y,
                tiling: Tiling::ClampToEdge,
                from: obj.i64_or("from_y", -64) as i32,
                to: obj.i64_or("to_y", 320) as i32,
                from_value: obj.f64_or("from_value", 0.0) as f32,
                to_value: obj.f64_or("to_value", 0.0) as f32,
            },
            "gradient" => {
                let tiling = match obj.get("tiling").and_then(Value::as_str).unwrap_or("clamp_to_edge") {
                    "repeat" => Tiling::Repeat,
                    "mirrored_repeat" => Tiling::MirroredRepeat,
                    _ => Tiling::ClampToEdge,
                };
                let from = obj.get("from_coordinate").or_else(|| obj.get("from_y")).and_then(Value::as_i64).ok_or_else(|| anyhow!("gradient without from_coordinate"))?;
                let to = obj.get("to_coordinate").or_else(|| obj.get("to_y")).and_then(Value::as_i64).ok_or_else(|| anyhow!("gradient without to_coordinate"))?;
                Df::Gradient {
                    axis: Axis::parse(obj.get("axis").and_then(Value::as_str).unwrap_or("y"))?,
                    tiling,
                    from: from as i32,
                    to: to as i32,
                    from_value: obj.field("from_value")?.as_f64().unwrap_or(0.0) as f32,
                    to_value: obj.field("to_value")?.as_f64().unwrap_or(0.0) as f32,
                }
            }
            "distance_to_point" => {
                let p = obj.field("point")?.as_array().ok_or_else(|| anyhow!("bad point"))?;
                let c = |i: usize| p.get(i).and_then(Value::as_i64).unwrap_or(0) as i32;
                let metric = match obj.get("metric").and_then(Value::as_str).unwrap_or("euclidean") {
                    "euclidean" => DistanceMetric::Euclidean,
                    "euclidean_squared" => DistanceMetric::EuclideanSquared,
                    "manhattan" => DistanceMetric::Manhattan,
                    "chebyshev" => DistanceMetric::Chebyshev,
                    m => bail!("unknown distance metric {m}"),
                };
                Df::DistanceToPoint { point: [c(0), c(1), c(2)], metric }
            }
            "lerp" => Df::Lerp {
                alpha: Box::new(self.child(obj, &["alpha"])?),
                first: Box::new(self.child(obj, &["first"])?),
                second: Box::new(self.child(obj, &["second"])?),
            },
            "floor" | "round" | "ceil" | "truncate" => {
                let op = match ty {
                    "floor" => RoundingOp::Floor,
                    "round" => RoundingOp::Round,
                    "ceil" => RoundingOp::Ceil,
                    _ => RoundingOp::Truncate,
                };
                let input = self.child(obj, &["input", "argument"])?;
                let multiple = match obj.get("multiple") {
                    Some(m) => self.parse(m)?,
                    None => Df::Constant(1.0),
                };
                Df::Rounding { input: Box::new(input), multiple: Box::new(multiple), op }
            }
            "slice" => Df::Slice {
                axis: Axis::parse(obj.get("axis").and_then(Value::as_str).unwrap_or("y"))?,
                coordinate: obj.field("coordinate")?.as_i64().unwrap_or(0) as i32,
                input: Box::new(self.child(obj, &["input"])?),
            },
            "old_blended_noise" => Df::OldBlendedNoise {
                xz_scale: obj.f64_or("xz_scale", 0.25),
                y_scale: obj.f64_or("y_scale", 0.125),
                xz_factor: obj.f64_or("xz_factor", 80.0),
                y_factor: obj.f64_or("y_factor", 160.0),
                smear_scale_multiplier: obj.f64_or("smear_scale_multiplier", 8.0),
            },
            "add" | "mul" | "min" | "max" | "sub" | "div" | "pow" => {
                let a = self.child(obj, &["left", "argument1", "base"])?;
                let b = self.child(obj, &["right", "argument2", "exponent"])?;
                if ty == "add" || ty == "mul" {
                    let op = if ty == "add" { LinearOp::Add } else { LinearOp::Mul };
                    if let Df::Constant(c) = a {
                        return Ok(Df::Linear { op, input: Box::new(b), argument: c });
                    }
                    if let Df::Constant(c) = b {
                        return Ok(Df::Linear { op, input: Box::new(a), argument: c });
                    }
                }
                let op = match ty {
                    "add" => BinaryOp::Add,
                    "mul" => BinaryOp::Mul,
                    "min" => BinaryOp::Min,
                    "max" => BinaryOp::Max,
                    "sub" => BinaryOp::Sub,
                    "div" => BinaryOp::Div,
                    _ => BinaryOp::Pow,
                };
                Df::Binary { op, a: Box::new(a), b: Box::new(b) }
            }
            "abs" | "square" | "cube" | "half_negative" | "quarter_negative" | "squeeze" | "invert" | "reciprocal" | "negate" | "sqrt" | "log" | "sign" => {
                let op = match ty {
                    "abs" => UnaryOp::Abs,
                    "square" => UnaryOp::Square,
                    "cube" => UnaryOp::Cube,
                    "half_negative" => UnaryOp::HalfNegative,
                    "quarter_negative" => UnaryOp::QuarterNegative,
                    "squeeze" => UnaryOp::Squeeze,
                    "invert" | "reciprocal" => UnaryOp::Invert,
                    "negate" => UnaryOp::Negate,
                    "sqrt" => UnaryOp::Sqrt,
                    "log" => UnaryOp::Log,
                    _ => UnaryOp::Sign,
                };
                Df::Unary { op, input: Box::new(self.child(obj, &["argument", "input"])?) }
            }
            "clamp" => Df::Clamp {
                input: Box::new(self.child(obj, &["input", "argument"])?),
                min: obj.get("min").or_else(|| obj.get("min_value")).and_then(Value::as_f64).unwrap_or(f64::NEG_INFINITY) as f32,
                max: obj.get("max").or_else(|| obj.get("max_value")).and_then(Value::as_f64).unwrap_or(f64::INFINITY) as f32,
            },
            "range_choice" => Df::RangeChoice {
                input: Box::new(self.child(obj, &["input"])?),
                min_inclusive: obj.f64_or("min_inclusive", f64::NEG_INFINITY) as f32,
                max_exclusive: obj.f64_or("max_exclusive", f64::INFINITY) as f32,
                when_in: Box::new(self.child(obj, &["when_in_range"])?),
                when_out: Box::new(self.child(obj, &["when_out_of_range"])?),
            },
            "interval_select" => {
                let input = self.child(obj, &["input"])?;
                let thresholds = obj
                    .field("thresholds")?
                    .as_array()
                    .ok_or_else(|| anyhow!("bad thresholds"))?
                    .iter()
                    .map(|t| t.as_f64().unwrap_or(0.0) as f32)
                    .collect();
                let functions = obj
                    .field("functions")?
                    .as_array()
                    .ok_or_else(|| anyhow!("bad functions"))?
                    .iter()
                    .map(|f| self.parse(f))
                    .collect::<Result<Vec<_>>>()?;
                Df::IntervalSelect { input: Box::new(input), thresholds, functions }
            }
            "spline" => Df::Spline(self.parse_spline(obj.field("spline")?)?),
            "noise" | "shifted_noise" => {
                let noise = obj.str_field("noise")?.to_owned();
                let xz_scale = obj.f64_or("xz_scale", 1.0);
                let y_scale = obj.f64_or("y_scale", 1.0);
                if ty == "shifted_noise" || obj.get("shift_x").is_some() || obj.get("shift_y").is_some() || obj.get("shift_z").is_some() {
                    let mut shift = |k: &str| -> Result<Df> {
                        match obj.get(k) {
                            Some(v) => self.parse(v),
                            None => Ok(Df::Constant(0.0)),
                        }
                    };
                    let shift_x = shift("shift_x")?;
                    let shift_y = shift("shift_y")?;
                    let shift_z = shift("shift_z")?;
                    Df::ShiftedNoise { shift_x: Box::new(shift_x), shift_y: Box::new(shift_y), shift_z: Box::new(shift_z), noise, xz_scale, y_scale }
                } else {
                    Df::Noise { noise, xz_scale, y_scale }
                }
            }
            "shift_a" | "shift" => Df::ShiftA(obj.get("noise").or_else(|| obj.get("argument")).and_then(Value::as_str).ok_or_else(|| anyhow!("shift without noise"))?.to_owned()),
            "shift_b" => Df::ShiftB(obj.get("noise").or_else(|| obj.get("argument")).and_then(Value::as_str).ok_or_else(|| anyhow!("shift without noise"))?.to_owned()),
            "blend_alpha" => Df::BlendAlpha,
            "blend_offset" => Df::BlendOffset,
            "blend_density" => Df::BlendDensity(Box::new(self.child(obj, &["input", "argument"])?)),
            "end_islands" | "end_outer_islands" => Df::EndIslands,
            "beardifier" => Df::Beardifier,
            "cache" | "interpolated" | "flat_cache" | "cache_flat" | "cache_2d" | "cache_once" | "cache_all_in_cell" => {
                let input = self.child(obj, &["input", "argument"])?;
                let kind = if ty == "interpolated" {
                    WrapperKind::Interpolated {
                        cell_size_xz: obj.field("cell_size_xz")?.as_i64().unwrap_or(4) as i32,
                        cell_size_y: obj.field("cell_size_y")?.as_i64().unwrap_or(8) as i32,
                    }
                } else {
                    WrapperKind::Cache
                };
                Df::Wrapper { input: Box::new(input), kind }
            }
            "find_top_surface" | "weird_utility_density" => Df::FindTopSurface {
                density: Box::new(self.child(obj, &["density"])?),
                upper_bound: Box::new(self.child(obj, &["upper_bound"])?),
                lower_bound: obj.i64_or("lower_bound", -64) as i32,
                cell_height: obj.i64_or("cell_height", 8) as i32,
            },
            other => bail!("unknown density function type {other}"),
        })
    }

    fn parse_spline(&mut self, v: &Value) -> Result<Spline> {
        if let Some(n) = v.as_f64() {
            return Ok(Spline::Fixed(n as f32));
        }
        let obj = v.as_object().ok_or_else(|| anyhow!("bad spline {v}"))?;
        if obj.contains_key("value") && !obj.contains_key("points") {
            return self.parse_spline(&obj["value"]);
        }
        let coordinate = self.parse(v.field("coordinate")?)?;
        let points = v.field("points")?.as_array().ok_or_else(|| anyhow!("bad spline points"))?;
        let mut locations = Vec::with_capacity(points.len());
        let mut values = Vec::with_capacity(points.len());
        let mut derivatives = Vec::with_capacity(points.len());
        for p in points {
            locations.push(p.field("location")?.as_f64().unwrap_or(0.0) as f32);
            values.push(self.parse_spline(p.field("value")?)?);
            derivatives.push(p.field("derivative")?.as_f64().unwrap_or(0.0) as f32);
        }
        Ok(Spline::Standard { coordinate: Box::new(coordinate), locations, values, derivatives })
    }
}

impl Spline {
    fn for_each_df(&mut self, f: &mut dyn FnMut(&mut Df)) {
        if let Spline::Standard { coordinate, values, .. } = self {
            f(coordinate);
            for v in values {
                v.for_each_df(f);
            }
        }
    }

    fn axes(&self) -> u8 {
        match self {
            Spline::Fixed(_) => 0,
            Spline::Standard { coordinate, values, .. } => values.iter().fold(coordinate.axes(), |a, v| a | v.axes()),
        }
    }
}

impl Df {
    /// The axes the function's value depends on.
    pub fn axes(&self) -> u8 {
        match self {
            Df::Constant(_) => 0,
            Df::BlendAlpha | Df::BlendOffset | Df::EndIslands | Df::ShiftA(_) | Df::ShiftB(_) => AXES_XZ,
            Df::Beardifier | Df::OldBlendedNoise { .. } | Df::DistanceToPoint { .. } => AXES_ALL,
            Df::Gradient { axis, .. } => axis.bits(),
            Df::Noise { xz_scale, y_scale, .. } => noise_axes(*xz_scale, *y_scale),
            Df::ShiftedNoise { shift_x, shift_y, shift_z, xz_scale, y_scale, .. } => noise_axes(*xz_scale, *y_scale) | shift_x.axes() | shift_y.axes() | shift_z.axes(),
            Df::BlendDensity(input) | Df::Wrapper { input, .. } | Df::Linear { input, .. } | Df::Unary { input, .. } | Df::Clamp { input, .. } => input.axes(),
            Df::Slice { axis, input, .. } => input.axes() & !axis.bits(),
            Df::FindTopSurface { density, upper_bound, .. } => (density.axes() | upper_bound.axes()) & !AXIS_Y,
            Df::IntervalSelect { input, functions, .. } => functions.iter().fold(input.axes(), |a, f| a | f.axes()),
            Df::Lerp { alpha, first, second } => alpha.axes() | first.axes() | second.axes(),
            Df::Rounding { input, multiple, .. } => input.axes() | multiple.axes(),
            Df::Binary { a, b, .. } => a.axes() | b.axes(),
            Df::RangeChoice { input, when_in, when_out, .. } => input.axes() | when_in.axes() | when_out.axes(),
            Df::Spline(s) => s.axes(),
        }
    }

    fn for_each_child(&mut self, f: &mut dyn FnMut(&mut Df)) {
        match self {
            Df::Beardifier
            | Df::BlendAlpha
            | Df::BlendOffset
            | Df::EndIslands
            | Df::Noise { .. }
            | Df::ShiftA(_)
            | Df::ShiftB(_)
            | Df::OldBlendedNoise { .. }
            | Df::Constant(_)
            | Df::Gradient { .. }
            | Df::DistanceToPoint { .. } => {}
            Df::BlendDensity(input) | Df::Wrapper { input, .. } | Df::Linear { input, .. } | Df::Unary { input, .. } | Df::Clamp { input, .. } | Df::Slice { input, .. } => f(input),
            Df::FindTopSurface { density, upper_bound, .. } => {
                f(density);
                f(upper_bound);
            }
            Df::ShiftedNoise { shift_x, shift_y, shift_z, .. } => {
                f(shift_x);
                f(shift_y);
                f(shift_z);
            }
            Df::IntervalSelect { input, functions, .. } => {
                f(input);
                for x in functions {
                    f(x);
                }
            }
            Df::Lerp { alpha, first, second } => {
                f(alpha);
                f(first);
                f(second);
            }
            Df::Rounding { input, multiple, .. } => {
                f(input);
                f(multiple);
            }
            Df::Binary { a, b, .. } => {
                f(a);
                f(b);
            }
            Df::RangeChoice { input, when_in, when_out, .. } => {
                f(input);
                f(when_in);
                f(when_out);
            }
            Df::Spline(s) => s.for_each_df(f),
        }
    }

    fn existing_removed_axes(&self) -> u8 {
        let mut axes = 0;
        let mut f = self;
        while let Df::Slice { axis, input, .. } = f {
            axes |= axis.bits();
            f = input;
        }
        axes
    }

    fn remove_axes(&mut self, axes: u8) {
        let filtered = axes & !self.existing_removed_axes();
        for (bit, axis) in [(AXIS_X, Axis::X), (AXIS_Z, Axis::Z), (AXIS_Y, Axis::Y)] {
            if filtered & bit != 0 {
                let input = std::mem::replace(self, Df::Constant(0.0));
                *self = Df::Slice { axis, coordinate: 0, input: Box::new(input) };
            }
        }
    }

    /// Wraps sub-functions that do not depend on an axis their parent varies over in a slice
    /// at coordinate 0 of that axis.
    pub fn slice_uniform_axes(&mut self, parent_axes: u8) {
        if matches!(self, Df::Constant(_) | Df::Gradient { .. }) {
            return;
        }
        let axes = self.axes();
        if let Df::FindTopSurface { density, upper_bound, .. } = self {
            density.slice_uniform_axes(axes | AXIS_Y);
            upper_bound.slice_uniform_axes(axes);
        } else {
            self.for_each_child(&mut |c| c.slice_uniform_axes(axes));
        }
        if parent_axes != axes {
            self.remove_axes(parent_axes & !axes);
        }
    }

    /// Folds operations on constants (in single precision).
    pub fn optimize(&mut self) {
        match self {
            Df::BlendDensity(input) | Df::Slice { input, .. } | Df::Wrapper { input, .. } => input.optimize(),
            Df::FindTopSurface { density, upper_bound, .. } => {
                density.optimize();
                upper_bound.optimize();
            }
            Df::ShiftedNoise { shift_x, shift_y, shift_z, .. } => {
                shift_x.optimize();
                shift_y.optimize();
                shift_z.optimize();
            }
            Df::IntervalSelect { input, functions, .. } => {
                input.optimize();
                for f in functions {
                    f.optimize();
                }
            }
            Df::Lerp { alpha, first, second } => {
                alpha.optimize();
                first.optimize();
                second.optimize();
            }
            Df::Rounding { input, multiple, .. } => {
                input.optimize();
                multiple.optimize();
            }
            Df::RangeChoice { input, when_in, when_out, .. } => {
                input.optimize();
                when_in.optimize();
                when_out.optimize();
            }
            Df::Linear { op, input, argument } => {
                input.optimize();
                if let Df::Constant(v) = **input {
                    *self = Df::Constant(match op {
                        LinearOp::Add => v + *argument,
                        LinearOp::Mul => v * *argument,
                    });
                }
            }
            Df::Binary { op, a, b } => {
                a.optimize();
                b.optimize();
                if let (Df::Constant(x), Df::Constant(y)) = (&**a, &**b) {
                    let (x, y) = (*x, *y);
                    *self = Df::Constant(match op {
                        BinaryOp::Add => x + y,
                        BinaryOp::Mul => x * y,
                        BinaryOp::Min => x.min(y),
                        BinaryOp::Max => x.max(y),
                        BinaryOp::Sub => x - y,
                        BinaryOp::Div => {
                            if y == 0.0 {
                                0.0
                            } else {
                                x / y
                            }
                        }
                        BinaryOp::Pow => x.powf(y),
                    });
                }
            }
            Df::Unary { op, input } => {
                input.optimize();
                if let Df::Constant(v) = **input {
                    *self = Df::Constant(op.apply(v));
                }
            }
            Df::Clamp { input, min, max } => {
                input.optimize();
                if let Df::Constant(v) = **input {
                    *self = Df::Constant(v.clamp(*min, *max));
                }
            }
            _ => {}
        }
    }
}

/// Parses a router entry (absent entries are the constant 0), then folds and slices it.
pub fn prepare(parser: &mut DfParser, v: Option<&Value>) -> Result<Df> {
    let mut df = match v {
        Some(v) => parser.parse(v)?,
        None => Df::Constant(0.0),
    };
    df.optimize();
    df.slice_uniform_axes(AXES_ALL);
    Ok(df)
}
