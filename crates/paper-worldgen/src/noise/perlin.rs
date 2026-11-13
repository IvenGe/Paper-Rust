//! Improved (Perlin) noise, its octave stacks and the double ("normal") noise, in the single
//! precision form vanilla 26.x evaluates them in.
//!
//! Derived from `pumpkin-util/src/noise/perlin.rs` and
//! `pumpkin-world/src/generation/noise/perlin.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use super::volume::DensityVolume;
use crate::math::{lerp3, smoothstep};
use crate::random::{PositionalRandom, Random, RandomSource};

const GRADIENTS: [[f32; 3]; 16] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 1.0, 0.0],
    [0.0, -1.0, 1.0],
    [-1.0, 1.0, 0.0],
    [0.0, -1.0, -1.0],
];

const ROUND_OFF: f64 = 33_554_432.0;
const HALF_ROUND_OFF: f64 = f64::from_bits(0x416F_FFFF_FFFF_FFFF);
const SHIFT_UP_EPSILON: f64 = 1.0E-7f32 as f64;
const STANDARD_DEVIATION: f64 = 0.270_224_783_124_521_1;
const TARGET_DEVIATION: f64 = 0.333_333_333_333_333_3;
const INPUT_FACTOR: f64 = 1.018_126_888_217_522_7;

/// A noise that can be sampled at a point or added onto a [`DensityVolume`] buffer.
pub trait Noise {
    fn get(&self, x: f64, y: f64, z: f64) -> f32;
    fn add_to_volume(&self, buffer: &mut [f32], volume: &DensityVolume, xz_scale: f64, y_scale: f64, amplitude: f32);
}

#[inline]
fn grad_dot(hash: i32, x: f32, y: f32, z: f32) -> f32 {
    let g = &GRADIENTS[(hash & 15) as usize];
    g[0] * x + g[1] * y + g[2] * z
}

#[inline]
fn grad_dot_xz(g: &[f32; 3], x: f32, z: f32) -> f32 {
    g[0] * x + g[2] * z
}

/// `PerlinNoise.wrap`.
#[inline]
pub fn wrap(x: f64) -> f64 {
    if (-HALF_ROUND_OFF..HALF_ROUND_OFF).contains(&x) {
        x
    } else {
        x - (x / ROUND_OFF + 0.5).floor() * ROUND_OFF
    }
}

/// `ImprovedNoise`.
#[derive(Clone)]
pub struct PerlinNoise {
    perms: [u8; 256],
    pub offset_x: f64,
    pub offset_y: f64,
    pub offset_z: f64,
}

#[derive(Default)]
struct Corners {
    d000: f32,
    d100: f32,
    d010: f32,
    d110: f32,
    d001: f32,
    d101: f32,
    d011: f32,
    d111: f32,
    g000: f32,
    g100: f32,
    g010: f32,
    g110: f32,
    g001: f32,
    g101: f32,
    g011: f32,
    g111: f32,
}

impl Corners {
    #[inline]
    fn lerp(&self, ax: f32, ay: f32, az: f32, ry: f32) -> f32 {
        lerp3(
            ax,
            ay,
            az,
            self.d000 + self.g000 * ry,
            self.d100 + self.g100 * ry,
            self.d010 + self.g010 * (ry - 1.0),
            self.d110 + self.g110 * (ry - 1.0),
            self.d001 + self.g001 * ry,
            self.d101 + self.g101 * ry,
            self.d011 + self.g011 * (ry - 1.0),
            self.d111 + self.g111 * (ry - 1.0),
        )
    }
}

impl PerlinNoise {
    pub fn new(random: &mut impl Random) -> Self {
        let offset_x = random.next_f64() * 256.0;
        let offset_y = random.next_f64() * 256.0;
        let offset_z = random.next_f64() * 256.0;
        let mut perms = [0u8; 256];
        for (i, p) in perms.iter_mut().enumerate() {
            *p = i as u8;
        }
        for i in 0..256 {
            let j = random.next_bounded(256 - i as i32) as usize;
            perms.swap(i, i + j);
        }
        Self { perms, offset_x, offset_y, offset_z }
    }

    #[inline]
    fn p(&self, x: i32) -> i32 {
        self.perms[(x & 0xFF) as usize] as i32
    }

    #[inline]
    fn grad(&self, x: i32) -> &'static [f32; 3] {
        &GRADIENTS[(self.p(x) & 15) as usize]
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(&self, x: i32, y: i32, z: i32, rx: f32, ry: f32, rz: f32, original_ry: f32) -> f32 {
        let x0 = self.p(x);
        let x1 = self.p(x + 1);
        let xy00 = self.p(x0 + y);
        let xy01 = self.p(x0 + y + 1);
        let xy10 = self.p(x1 + y);
        let xy11 = self.p(x1 + y + 1);
        let d000 = grad_dot(self.p(xy00 + z), rx, ry, rz);
        let d100 = grad_dot(self.p(xy10 + z), rx - 1.0, ry, rz);
        let d010 = grad_dot(self.p(xy01 + z), rx, ry - 1.0, rz);
        let d110 = grad_dot(self.p(xy11 + z), rx - 1.0, ry - 1.0, rz);
        let d001 = grad_dot(self.p(xy00 + z + 1), rx, ry, rz - 1.0);
        let d101 = grad_dot(self.p(xy10 + z + 1), rx - 1.0, ry, rz - 1.0);
        let d011 = grad_dot(self.p(xy01 + z + 1), rx, ry - 1.0, rz - 1.0);
        let d111 = grad_dot(self.p(xy11 + z + 1), rx - 1.0, ry - 1.0, rz - 1.0);
        lerp3(smoothstep(rx), smoothstep(original_ry), smoothstep(rz), d000, d100, d010, d110, d001, d101, d011, d111)
    }

    #[allow(clippy::too_many_arguments)]
    fn corners(&self, x0: i32, x1: i32, fy: i32, fz: i32, rx: f32, rz: f32, c: &mut Corners) {
        let xy00 = self.p(x0 + fy);
        let xy01 = self.p(x0 + fy + 1);
        let xy10 = self.p(x1 + fy);
        let xy11 = self.p(x1 + fy + 1);
        let g = self.grad(xy00 + fz);
        c.d000 = grad_dot_xz(g, rx, rz);
        c.g000 = g[1];
        let g = self.grad(xy10 + fz);
        c.d100 = grad_dot_xz(g, rx - 1.0, rz);
        c.g100 = g[1];
        let g = self.grad(xy01 + fz);
        c.d010 = grad_dot_xz(g, rx, rz);
        c.g010 = g[1];
        let g = self.grad(xy11 + fz);
        c.d110 = grad_dot_xz(g, rx - 1.0, rz);
        c.g110 = g[1];
        let g = self.grad(xy00 + fz + 1);
        c.d001 = grad_dot_xz(g, rx, rz - 1.0);
        c.g001 = g[1];
        let g = self.grad(xy10 + fz + 1);
        c.d101 = grad_dot_xz(g, rx - 1.0, rz - 1.0);
        c.g101 = g[1];
        let g = self.grad(xy01 + fz + 1);
        c.d011 = grad_dot_xz(g, rx, rz - 1.0);
        c.g011 = g[1];
        let g = self.grad(xy11 + fz + 1);
        c.d111 = grad_dot_xz(g, rx - 1.0, rz - 1.0);
        c.g111 = g[1];
    }
}

impl Noise for PerlinNoise {
    fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        let x = wrap(x) + self.offset_x;
        let y = wrap(y) + self.offset_y;
        let z = wrap(z) + self.offset_z;
        let fx = x.floor() as i32;
        let fy = y.floor() as i32;
        let fz = z.floor() as i32;
        let rx = (x - fx as f64) as f32;
        let ry = (y - fy as f64) as f32;
        let rz = (z - fz as f64) as f32;
        self.sample_and_lerp(fx, fy, fz, rx, ry, rz, ry)
    }

    fn add_to_volume(&self, buffer: &mut [f32], volume: &DensityVolume, xz_scale: f64, y_scale: f64, amplitude: f32) {
        let mut c = Corners::default();
        if y_scale == 0.0 && volume.size_y > 1 {
            let y = self.offset_y;
            let fy = y.floor() as i32;
            let ry = (y - fy as f64) as f32;
            let ay = smoothstep(ry);
            let mut index = 0;
            for iz in 0..volume.size_z {
                let z = wrap(volume.block_z(iz) as f64 * xz_scale) + self.offset_z;
                let fz = z.floor() as i32;
                let rz = (z - fz as f64) as f32;
                let az = smoothstep(rz);
                for ix in 0..volume.size_x {
                    let x = wrap(volume.block_x(ix) as f64 * xz_scale) + self.offset_x;
                    let fx = x.floor() as i32;
                    let rx = (x - fx as f64) as f32;
                    let x0 = self.p(fx);
                    let x1 = self.p(fx + 1);
                    let ax = smoothstep(rx);
                    self.corners(x0, x1, fy, fz, rx, rz, &mut c);
                    let v = amplitude * c.lerp(ax, ay, az, ry);
                    for out in &mut buffer[index..index + volume.size_y] {
                        *out += v;
                    }
                    index += volume.size_y;
                }
            }
            return;
        }
        let mut index = 0;
        for iz in 0..volume.size_z {
            let z = wrap(volume.block_z(iz) as f64 * xz_scale) + self.offset_z;
            let fz = z.floor() as i32;
            let rz = (z - fz as f64) as f32;
            let az = smoothstep(rz);
            for ix in 0..volume.size_x {
                let x = wrap(volume.block_x(ix) as f64 * xz_scale) + self.offset_x;
                let fx = x.floor() as i32;
                let rx = (x - fx as f64) as f32;
                let x0 = self.p(fx);
                let x1 = self.p(fx + 1);
                let ax = smoothstep(rx);
                let mut last_fy = i32::MIN;
                for iy in 0..volume.size_y {
                    let y = wrap(volume.block_y(iy) as f64 * y_scale) + self.offset_y;
                    let fy = y.floor() as i32;
                    let ry = (y - fy as f64) as f32;
                    let ay = smoothstep(ry);
                    if last_fy != fy {
                        self.corners(x0, x1, fy, fz, rx, rz, &mut c);
                        last_fy = fy;
                    }
                    buffer[index] += amplitude * c.lerp(ax, ay, az, ry);
                    index += 1;
                }
            }
        }
    }
}

/// `ImprovedNoise` sampled with the Y smearing used by the blended ("old") noise.
#[derive(Clone)]
pub struct SmearedPerlinNoise {
    noise: PerlinNoise,
    fudge_y_scale: f64,
}

impl SmearedPerlinNoise {
    pub fn new(random: &mut impl Random, fudge_y_scale: f64) -> Self {
        Self { noise: PerlinNoise::new(random), fudge_y_scale }
    }

    #[inline]
    fn fudge_y(&self, original_y: f64, relative_y: f64) -> f64 {
        let limit = if original_y >= 0.0 && original_y < relative_y { original_y } else { relative_y };
        (limit / self.fudge_y_scale + SHIFT_UP_EPSILON).floor() * self.fudge_y_scale
    }
}

impl Noise for SmearedPerlinNoise {
    fn get(&self, x: f64, original_y: f64, z: f64) -> f32 {
        let x = wrap(x) + self.noise.offset_x;
        let y = wrap(original_y) + self.noise.offset_y;
        let z = wrap(z) + self.noise.offset_z;
        let fx = x.floor() as i32;
        let fy = y.floor() as i32;
        let fz = z.floor() as i32;
        let rx = (x - fx as f64) as f32;
        let ry = y - fy as f64;
        let rz = (z - fz as f64) as f32;
        let fudged = (ry - self.fudge_y(original_y, ry)) as f32;
        self.noise.sample_and_lerp(fx, fy, fz, rx, fudged, rz, ry as f32)
    }

    fn add_to_volume(&self, buffer: &mut [f32], volume: &DensityVolume, xz_scale: f64, y_scale: f64, amplitude: f32) {
        let mut c = Corners::default();
        let mut index = 0;
        for iz in 0..volume.size_z {
            let z = wrap(volume.block_z(iz) as f64 * xz_scale) + self.noise.offset_z;
            let fz = z.floor() as i32;
            let rz = (z - fz as f64) as f32;
            let az = smoothstep(rz);
            for ix in 0..volume.size_x {
                let x = wrap(volume.block_x(ix) as f64 * xz_scale) + self.noise.offset_x;
                let fx = x.floor() as i32;
                let rx = (x - fx as f64) as f32;
                let x0 = self.noise.p(fx);
                let x1 = self.noise.p(fx + 1);
                let ax = smoothstep(rx);
                let mut last_fy = i32::MIN;
                for iy in 0..volume.size_y {
                    let original_y = volume.block_y(iy) as f64 * y_scale;
                    let y = wrap(original_y) + self.noise.offset_y;
                    let fy = y.floor() as i32;
                    let ry = y - fy as f64;
                    let ay = smoothstep(ry as f32);
                    if last_fy != fy {
                        self.noise.corners(x0, x1, fy, fz, rx, rz, &mut c);
                        last_fy = fy;
                    }
                    let fudged = (ry - self.fudge_y(original_y, ry)) as f32;
                    buffer[index] += amplitude * c.lerp(ax, ay, az, fudged);
                    index += 1;
                }
            }
        }
    }
}

pub struct Layer<N> {
    pub noise: N,
    pub frequency: f64,
    pub amplitude: f32,
}

/// A sum of scaled noises (an octave stack).
pub struct NoiseStack<N> {
    pub layers: Vec<Layer<N>>,
}

impl<N: Noise> Noise for NoiseStack<N> {
    #[inline]
    fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut value = 0.0f32;
        for layer in &self.layers {
            let f = layer.frequency;
            value += layer.amplitude * layer.noise.get(x * f, y * f, z * f);
        }
        value
    }

    fn add_to_volume(&self, buffer: &mut [f32], volume: &DensityVolume, xz_scale: f64, y_scale: f64, amplitude: f32) {
        for layer in &self.layers {
            let f = layer.frequency;
            layer.noise.add_to_volume(buffer, volume, xz_scale * f, y_scale * f, amplitude * layer.amplitude);
        }
    }
}

struct Octave {
    index: i32,
    frequency: f64,
    amplitude: f64,
}

fn modifier(modifiers: &[f64], i: usize) -> f64 {
    if modifiers.is_empty() {
        1.0
    } else {
        modifiers[i]
    }
}

fn build_octaves(base_octave: i32, base_amplitude: f64, count: usize, normalize: bool, modifiers: &[f64]) -> Vec<Octave> {
    let mut frequency = 2f64.powi(base_octave);
    let mut amplitude = if normalize {
        base_amplitude * (0.5f64.powi(-(count as i32 - 1)) / (0.5f64.powi(-(count as i32)) - 1.0))
    } else {
        base_amplitude
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let m = modifier(modifiers, i);
        if m != 0.0 {
            out.push(Octave { index: base_octave + i as i32, frequency, amplitude: amplitude * m });
        }
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    out
}

fn deviation(octaves: &[Octave]) -> f64 {
    let mut variance = 0.0;
    for o in octaves {
        let d = STANDARD_DEVIATION * o.amplitude.abs();
        variance += d * d;
    }
    variance.sqrt()
}

fn normalization_factor(target_amplitude: f64, octaves: &[Octave]) -> f64 {
    let d = deviation(octaves);
    if d == 0.0 {
        0.0
    } else {
        (target_amplitude * TARGET_DEVIATION) / (d * 2f64.sqrt())
    }
}

fn parity_normalization_factor(base_amplitude: f64, count: usize, modifiers: &[f64]) -> f64 {
    let mut min_octave = i32::MAX;
    let mut max_octave = i32::MIN;
    for i in 0..count {
        if modifier(modifiers, i) != 0.0 {
            min_octave = min_octave.min(i as i32);
            max_octave = max_octave.max(i as i32);
        }
    }
    let expected = 0.1 * (1.0 + 1.0 / ((max_octave - min_octave) + 1) as f64);
    base_amplitude * 0.5 * TARGET_DEVIATION / expected
}

/// The amplitude the noise JSON calls `base_amplitude`, when it is not given.
pub fn default_base_amplitude(base_octave: i32, count: usize, modifiers: &[f64]) -> f64 {
    let octaves = build_octaves(base_octave, 1.0, count, true, modifiers);
    let target: f64 = octaves.iter().map(|o| o.amplitude.abs()).sum();
    let factor = normalization_factor(target, &octaves);
    if factor == 0.0 {
        1.0
    } else {
        parity_normalization_factor(1.0, count, modifiers) / factor
    }
}

/// `PerlinNoise.createLegacyForLegacyNetherBiome`-style octaves from one legacy random.
fn legacy_fbm(random: &mut impl Random, first_octave: i32, amplitudes: &[f64]) -> NoiseStack<PerlinNoise> {
    let octaves = amplitudes.len();
    let zero_index = -first_octave;
    let mut levels: Vec<Option<PerlinNoise>> = (0..octaves).map(|_| None).collect();
    let zero = PerlinNoise::new(random);
    if zero_index >= 0 && zero_index < octaves as i32 && amplitudes[zero_index as usize] != 0.0 {
        levels[zero_index as usize] = Some(zero);
    }
    for i in (0..zero_index).rev() {
        if i < octaves as i32 {
            if amplitudes[i as usize] == 0.0 {
                random.consume(262);
            } else {
                levels[i as usize] = Some(PerlinNoise::new(random));
            }
        } else {
            random.consume(262);
        }
    }
    let mut factor = 2f64.powi(-zero_index);
    let mut value_factor = 2f64.powi(octaves as i32 - 1) / (2f64.powi(octaves as i32) - 1.0);
    let mut layers = Vec::with_capacity(octaves);
    for (i, noise) in levels.into_iter().enumerate() {
        if let Some(noise) = noise {
            layers.push(Layer { noise, frequency: factor, amplitude: (value_factor * amplitudes[i]) as f32 });
        }
        factor *= 2.0;
        value_factor /= 2.0;
    }
    NoiseStack { layers }
}

/// Parameters of a `worldgen/noise` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseParameters {
    pub base_octave: i32,
    pub amplitudes: Vec<f64>,
    pub base_amplitude: f64,
}

impl NoiseParameters {
    pub fn new(base_octave: i32, amplitudes: Vec<f64>, base_amplitude: Option<f64>) -> Self {
        let base_amplitude = base_amplitude.unwrap_or_else(|| default_base_amplitude(base_octave, amplitudes.len(), &amplitudes));
        Self { base_octave, amplitudes, base_amplitude }
    }

    /// Parses the 26.x JSON form (`base_octave`, `octave_count`, `amplitude_modifiers`,
    /// `base_amplitude`) as well as the older `firstOctave`/`amplitudes` form.
    pub fn from_json(v: &serde_json::Value) -> anyhow::Result<Self> {
        let base_octave = v
            .get("base_octave")
            .or_else(|| v.get("firstOctave"))
            .and_then(|o| o.as_i64())
            .ok_or_else(|| anyhow::anyhow!("noise without base_octave"))? as i32;
        let modifiers: Option<Vec<f64>> = v
            .get("amplitude_modifiers")
            .or_else(|| v.get("amplitudes"))
            .and_then(|a| a.as_array())
            .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect());
        let amplitudes = match modifiers {
            Some(m) => m,
            None => {
                let n = v.get("octave_count").and_then(|c| c.as_u64()).unwrap_or(1).max(1) as usize;
                vec![1.0; n]
            }
        };
        let base_amplitude = v.get("base_amplitude").and_then(|a| a.as_f64());
        Ok(Self::new(base_octave, amplitudes, base_amplitude))
    }
}

/// `NormalNoise`: two octave stacks, the second one sampled at slightly scaled coordinates.
pub struct NormalNoise {
    stack: NoiseStack<PerlinNoise>,
    max_value: f32,
}

impl NormalNoise {
    /// The modern construction: octaves seeded from `fromHashOf("octave_<n>")` of two forked
    /// positional factories.
    pub fn new(random: &mut RandomSource, params: &NoiseParameters) -> Self {
        let count = params.amplitudes.len();
        let octaves = build_octaves(params.base_octave, params.base_amplitude, count, true, &params.amplitudes);
        let target: f64 = octaves.iter().map(|o| o.amplitude.abs()).sum();
        let factor = normalization_factor(target, &octaves);
        let first: PositionalRandom = random.fork_positional();
        let second: PositionalRandom = random.fork_positional();
        let mut layers = Vec::with_capacity(octaves.len() * 2);
        for o in &octaves {
            let name = format!("octave_{}", o.index);
            let value_factor = (factor * o.amplitude) as f32;
            layers.push(Layer { noise: PerlinNoise::new(&mut first.from_hash_of(&name)), frequency: o.frequency, amplitude: value_factor });
            layers.push(Layer { noise: PerlinNoise::new(&mut second.from_hash_of(&name)), frequency: o.frequency * INPUT_FACTOR, amplitude: value_factor });
        }
        Self { stack: NoiseStack { layers }, max_value: (target * TARGET_DEVIATION * 6.0) as f32 }
    }

    /// The legacy nether biome construction (`createLegacyNetherBiome`).
    pub fn new_legacy_nether(random: &mut impl Random, params: &NoiseParameters) -> Self {
        let count = params.amplitudes.len();
        let amplitudes = &params.amplitudes;
        let first_octave = params.base_octave;
        let octaves = build_octaves(first_octave, 1.0, count, true, amplitudes);
        let mut target: f64 = octaves.iter().map(|o| o.amplitude.abs()).sum();
        let mut factor = normalization_factor(target, &octaves);
        if factor != 0.0 {
            let parity = parity_normalization_factor(1.0, count, amplitudes);
            target *= parity / factor;
            factor = parity;
        }
        let first = legacy_fbm(random, first_octave, amplitudes);
        let second = legacy_fbm(random, first_octave, amplitudes);
        let value_factor = factor as f32;
        let mut layers = Vec::new();
        for l in first.layers {
            layers.push(Layer { noise: l.noise, frequency: l.frequency, amplitude: l.amplitude * value_factor });
        }
        for l in second.layers {
            layers.push(Layer { noise: l.noise, frequency: l.frequency * INPUT_FACTOR, amplitude: l.amplitude * value_factor });
        }
        Self { stack: NoiseStack { layers }, max_value: (target * TARGET_DEVIATION * 6.0) as f32 }
    }

    pub fn max_value(&self) -> f32 {
        self.max_value
    }
}

impl Noise for NormalNoise {
    #[inline]
    fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        self.stack.get(x, y, z)
    }

    #[inline]
    fn add_to_volume(&self, buffer: &mut [f32], volume: &DensityVolume, xz_scale: f64, y_scale: f64, amplitude: f32) {
        self.stack.add_to_volume(buffer, volume, xz_scale, y_scale, amplitude);
    }
}
