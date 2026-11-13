//! Small numeric helpers with vanilla's exact operation order.
//!
//! Derived from `pumpkin-util/src/math/mod.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use std::ops::{Add, Mul, Sub};

/// Floating point types the interpolation helpers work on.
pub trait Float: Copy + PartialOrd + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + std::ops::Div<Output = Self> {
    const ZERO: Self;
    const ONE: Self;
}

impl Float for f32 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
}

impl Float for f64 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
}

/// `Mth.lerp(delta, start, end)`.
#[inline]
pub fn lerp<T: Float>(delta: T, start: T, end: T) -> T {
    start + delta * (end - start)
}

/// `Mth.inverseLerp(value, start, end)`.
#[inline]
pub fn inverse_lerp<T: Float>(value: T, start: T, end: T) -> T {
    (value - start) / (end - start)
}

/// `Mth.clampedLerp(start, end, delta)`.
#[inline]
pub fn clamped_lerp<T: Float>(start: T, end: T, delta: T) -> T {
    if delta < T::ZERO {
        start
    } else if delta > T::ONE {
        end
    } else {
        lerp(delta, start, end)
    }
}

/// `Mth.clampedMap(value, fromStart, fromEnd, toStart, toEnd)`.
#[inline]
pub fn clamped_map<T: Float>(value: T, from_start: T, from_end: T, to_start: T, to_end: T) -> T {
    clamped_lerp(to_start, to_end, inverse_lerp(value, from_start, from_end))
}

/// `Mth.map(value, fromStart, fromEnd, toStart, toEnd)`.
#[inline]
pub fn map<T: Float>(value: T, from_start: T, from_end: T, to_start: T, to_end: T) -> T {
    lerp(inverse_lerp(value, from_start, from_end), to_start, to_end)
}

/// `Mth.lerp2`.
#[inline]
pub fn lerp2<T: Float>(dx: T, dy: T, x0y0: T, x1y0: T, x0y1: T, x1y1: T) -> T {
    lerp(dy, lerp(dx, x0y0, x1y0), lerp(dx, x0y1, x1y1))
}

/// `Mth.lerp3`.
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn lerp3<T: Float>(dx: T, dy: T, dz: T, v000: T, v100: T, v010: T, v110: T, v001: T, v101: T, v011: T, v111: T) -> T {
    lerp(dz, lerp2(dx, dy, v000, v100, v010, v110), lerp2(dx, dy, v001, v101, v011, v111))
}

/// `Mth.smoothstep` in single precision.
#[inline]
pub fn smoothstep(x: f32) -> f32 {
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// `Math.floorDiv`.
#[inline]
pub fn floor_div(a: i32, b: i32) -> i32 {
    let d = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) {
        d - 1
    } else {
        d
    }
}

/// `Math.floorMod`.
#[inline]
pub fn floor_mod(a: i32, b: i32) -> i32 {
    let m = a % b;
    if m != 0 && ((m < 0) != (b < 0)) {
        m + b
    } else {
        m
    }
}

/// `Mth.floor(double)`.
#[inline]
pub fn floor(v: f64) -> i32 {
    floor_i32(v)
}

/// `v.floor() as i32` without a libm call (the default x86-64 target has no `roundsd`).
/// Saturates like the cast, and maps NaN to 0.
#[inline(always)]
pub fn floor_i32(v: f64) -> i32 {
    let i = v as i32;
    if (i as f64) > v {
        i.saturating_sub(1)
    } else {
        i
    }
}

/// `Math.round(double)`: rounds half up.
#[inline]
pub fn java_round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

/// `Math.round(float)`.
#[inline]
pub fn java_round_f32(v: f32) -> i32 {
    (v + 0.5).floor() as i32
}

/// A fast non-cryptographic hasher for small integer keys (the "Fx" hash).
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const FX_SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(FX_SEED);
    }
}

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.add(*b as u64);
        }
    }
    #[inline]
    fn write_u32(&mut self, v: u32) {
        self.add(v as u64);
    }
    #[inline]
    fn write_i32(&mut self, v: i32) {
        self.add(v as u32 as u64);
    }
    #[inline]
    fn write_u64(&mut self, v: u64) {
        self.add(v);
    }
    #[inline]
    fn write_i64(&mut self, v: i64) {
        self.add(v as u64);
    }
    #[inline]
    fn write_usize(&mut self, v: usize) {
        self.add(v as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// A `HashMap` with [`FxHasher`].
pub type FxMap<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<FxHasher>>;

/// The sine table of `Mth.sin`.
fn sin_table() -> &'static [f32; 65536] {
    static TABLE: std::sync::OnceLock<Box<[f32; 65536]>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Box::new([0f32; 65536]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f64 * std::f64::consts::PI * 2.0 / 65536.0).sin() as f32;
        }
        t
    })
}

/// `Mth.sin(double)` (table based).
#[inline]
pub fn mth_sin(v: f64) -> f32 {
    sin_table()[((v * 10430.378350470453) as i64 & 65535) as usize]
}

/// `Mth.cos(double)` (table based).
#[inline]
pub fn mth_cos(v: f64) -> f32 {
    sin_table()[((v * 10430.378350470453 + 16384.0) as i64 & 65535) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_ops() {
        assert_eq!(floor_div(-1, 16), -1);
        assert_eq!(floor_div(-16, 16), -1);
        assert_eq!(floor_div(-17, 16), -2);
        assert_eq!(floor_mod(-1, 16), 15);
        assert_eq!(floor_mod(5, 16), 5);
        assert_eq!(java_round(-2.5), -2);
    }
}
