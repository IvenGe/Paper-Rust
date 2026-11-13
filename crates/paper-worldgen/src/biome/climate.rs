//! Biome temperature at a position (`Biome.getHeightAdjustedTemperature`).
//!
//! Derived from `pumpkin-util/src/biome.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use super::TemperatureModifier;
use crate::noise::simplex::PerlinSimplexNoise;
use crate::random::LegacyRandom;
use std::sync::OnceLock;

fn temperature_noise() -> &'static PerlinSimplexNoise {
    static N: OnceLock<PerlinSimplexNoise> = OnceLock::new();
    N.get_or_init(|| PerlinSimplexNoise::new(&mut LegacyRandom::new(1234), &[0]))
}

fn frozen_temperature_noise() -> &'static PerlinSimplexNoise {
    static N: OnceLock<PerlinSimplexNoise> = OnceLock::new();
    N.get_or_init(|| PerlinSimplexNoise::new(&mut LegacyRandom::new(3456), &[-2, -1, 0]))
}

/// `Biome.BIOME_INFO_NOISE`.
pub fn biome_info_noise() -> &'static PerlinSimplexNoise {
    static N: OnceLock<PerlinSimplexNoise> = OnceLock::new();
    N.get_or_init(|| PerlinSimplexNoise::new(&mut LegacyRandom::new(2345), &[0]))
}

fn modified_temperature(modifier: TemperatureModifier, x: i32, z: i32, temperature: f32) -> f32 {
    match modifier {
        TemperatureModifier::None => temperature,
        TemperatureModifier::Frozen => {
            let (x, z) = (x as f64, z as f64);
            let d = frozen_temperature_noise().get_value(x * 0.05, z * 0.05, false) * 7.0;
            let e = biome_info_noise().get_value(x * 0.2, z * 0.2, false);
            if d + e < 0.3 {
                let g = biome_info_noise().get_value(x * 0.09, z * 0.09, false);
                if g < 0.8 {
                    return 0.2;
                }
            }
            temperature
        }
    }
}

pub fn height_adjusted_temperature(base: f32, modifier: TemperatureModifier, x: i32, y: i32, z: i32, sea_level: i32) -> f32 {
    let f = modified_temperature(modifier, x, z, base);
    let i = sea_level + 17;
    if y > i {
        let g = (temperature_noise().get_value((x as f32 / 8.0) as f64, (z as f32 / 8.0) as f64, false) * 8.0) as f32;
        f - (g + y as f32 - i as f32) * 0.05 / 40.0
    } else {
        f
    }
}
