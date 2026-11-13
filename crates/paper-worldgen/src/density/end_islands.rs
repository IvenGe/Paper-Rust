//! The outer end islands density function.
//!
//! Derived from `pumpkin-world/src/generation/noise/router/density_function/misc.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::noise::simplex::SimplexNoise;
use crate::noise::DensityVolume;
use crate::random::{LegacyRandom, Random};

pub struct EndIslands {
    noise: SimplexNoise,
}

impl EndIslands {
    pub fn new(seed: i64) -> Self {
        let mut r = LegacyRandom::new(seed);
        r.consume(17292);
        Self { noise: SimplexNoise::new(&mut r) }
    }

    fn height(&self, x: i32, z: i32) -> f32 {
        let i = x / 2;
        let j = z / 2;
        let k = x % 2;
        let l = z % 2;
        let mut f = -100.0f32;
        for m in -12..=12 {
            for n in -12..=12 {
                let o = (i + m) as i64;
                let p = (j + n) as i64;
                if o * o + p * p > 4096 && (self.noise.get_2d(o as f64, p as f64) as f32) < -0.9 {
                    let size = ((o as f32).abs() * 3439.0 + (p as f32).abs() * 147.0) % 13.0 + 9.0;
                    let xd = (k - m * 2) as f32;
                    let zd = (l - n * 2) as f32;
                    let h = 100.0 - ((xd * xd + zd * zd) as f64).sqrt() as f32 * size;
                    f = f.max(h.clamp(-100.0, 80.0));
                }
            }
        }
        f
    }

    #[inline]
    pub fn sample(&self, x: i32, z: i32) -> f32 {
        (self.height(x / 8, z / 8) - 8.0) / 128.0
    }

    pub fn sample_volume(&self, buffer: &mut [f32], volume: &DensityVolume) {
        for z in 0..volume.size_z {
            let bz = volume.block_z(z);
            for x in 0..volume.size_x {
                let v = self.sample(volume.block_x(x), bz);
                let i = volume.index(x, 0, z);
                buffer[i..i + volume.size_y].fill(v);
            }
        }
    }
}
