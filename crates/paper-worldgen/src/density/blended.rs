//! The legacy "blended" terrain noise (`old_blended_noise`) used by the nether and the end.
//!
//! Derived from `pumpkin-world/src/generation/noise/router/density_function/noise.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::math::lerp;
use crate::noise::perlin::{Layer, Noise, NoiseStack, SmearedPerlinNoise};
use crate::noise::DensityVolume;
use crate::random::Random;

const BASE_SCALE: f64 = 684.412;
const LIMIT_FACTOR: f64 = 0.999_984_74f32 as f64;
const MAIN_FACTOR: f64 = 12.75;
const LIMIT_FIRST_OCTAVE: i32 = -15;
const MAIN_FIRST_OCTAVE: i32 = -7;

fn create_fbm(random: &mut impl Random, first_octave: i32, smear_scale_y: f64, value_factor: f64) -> NoiseStack<SmearedPerlinNoise> {
    let octaves = -first_octave + 1;
    let mut factor = 1.0;
    let mut vf = value_factor / (2f64.powi(octaves) - 1.0);
    let mut layers = Vec::with_capacity(octaves as usize);
    for _ in 0..octaves {
        layers.push(Layer { noise: SmearedPerlinNoise::new(random, smear_scale_y * factor), frequency: factor, amplitude: vf as f32 });
        factor /= 2.0;
        vf *= 2.0;
    }
    NoiseStack { layers }
}

fn fbm_range_max(first_octave: i32, smear_scale_y: f64, value_factor: f64) -> f32 {
    let octaves = -first_octave + 1;
    let mut factor = 1.0;
    let mut vf = value_factor / (2f64.powi(octaves) - 1.0);
    let mut range = 0.0f32;
    for _ in 0..octaves {
        let layer = ((smear_scale_y * factor).abs() + 2.0) as f32;
        range += layer * vf as f32;
        factor /= 2.0;
        vf *= 2.0;
    }
    range
}

pub struct OldBlendedNoise {
    min_limit: NoiseStack<SmearedPerlinNoise>,
    max_limit: NoiseStack<SmearedPerlinNoise>,
    main: NoiseStack<SmearedPerlinNoise>,
    xz_multiplier: f64,
    y_multiplier: f64,
    main_xz_multiplier: f64,
    main_y_multiplier: f64,
    max_value: f32,
}

impl OldBlendedNoise {
    pub fn new(random: &mut impl Random, xz_scale: f64, y_scale: f64, xz_factor: f64, y_factor: f64, smear: f64) -> Self {
        let xz_multiplier = BASE_SCALE * xz_scale;
        let y_multiplier = BASE_SCALE * y_scale;
        let limit_smear = y_multiplier * smear;
        let main_smear = limit_smear / y_factor;
        let min_limit = create_fbm(random, LIMIT_FIRST_OCTAVE, limit_smear, LIMIT_FACTOR);
        let max_limit = create_fbm(random, LIMIT_FIRST_OCTAVE, limit_smear, LIMIT_FACTOR);
        let main = create_fbm(random, MAIN_FIRST_OCTAVE, main_smear, MAIN_FACTOR);
        Self {
            min_limit,
            max_limit,
            main,
            xz_multiplier,
            y_multiplier,
            main_xz_multiplier: xz_multiplier / xz_factor,
            main_y_multiplier: y_multiplier / y_factor,
            max_value: fbm_range_max(LIMIT_FIRST_OCTAVE, limit_smear, LIMIT_FACTOR),
        }
    }

    pub fn max_value(&self) -> f32 {
        self.max_value
    }

    #[inline]
    fn choice(main: f32) -> f32 {
        (main + 0.5).clamp(0.0, 1.0)
    }

    pub fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
        let (x, y, z) = (x as f64, y as f64, z as f64);
        let alpha = Self::choice(self.main.get(x * self.main_xz_multiplier, y * self.main_y_multiplier, z * self.main_xz_multiplier));
        let (lx, ly, lz) = (x * self.xz_multiplier, y * self.y_multiplier, z * self.xz_multiplier);
        if alpha == 0.0 {
            return self.min_limit.get(lx, ly, lz);
        }
        if alpha == 1.0 {
            return self.max_limit.get(lx, ly, lz);
        }
        lerp(alpha, self.min_limit.get(lx, ly, lz), self.max_limit.get(lx, ly, lz))
    }

    pub fn sample_volume(&self, buffer: &mut [f32], volume: &DensityVolume) {
        buffer.fill(0.0);
        self.main.add_to_volume(buffer, volume, self.main_xz_multiplier, self.main_y_multiplier, 1.0);
        for v in buffer.iter_mut() {
            *v = Self::choice(*v);
        }
        let mut first = vec![0.0f32; volume.size()];
        self.min_limit.add_to_volume(&mut first, volume, self.xz_multiplier, self.y_multiplier, 1.0);
        let mut second = vec![0.0f32; volume.size()];
        self.max_limit.add_to_volume(&mut second, volume, self.xz_multiplier, self.y_multiplier, 1.0);
        for ((v, f), s) in buffer.iter_mut().zip(first.iter()).zip(second.iter()) {
            let alpha = *v;
            *v = if alpha == 0.0 {
                *f
            } else if alpha == 1.0 {
                *s
            } else {
                lerp(alpha, *f, *s)
            };
        }
    }
}
