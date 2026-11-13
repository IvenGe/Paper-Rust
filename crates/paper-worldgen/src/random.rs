//! Random sources with vanilla's exact bit streams.
//!
//! Derived from `pumpkin-util/src/random/{mod,xoroshiro128,legacy_rand,gaussian,worldgen_random}.rs`
//! of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.
//!
//! * [`Xoroshiro`] is `XoroshiroRandomSource` (Xoroshiro128++), used by modern world generation.
//! * [`LegacyRandom`] is `LegacyRandomSource`, the 48 bit LCG of `java.util.Random`.
//! * [`RandomSource`] is either of the two, as handed out by positional factories.
//! * [`WorldgenRandom`] is vanilla's `WorldgenRandom`: `java.util.Random` algorithms on top of
//!   the bits of a wrapped source. Feature and carver placement use it.
//! * [`PositionalRandom`] is a `PositionalRandomFactory` (`at`, `fromHashOf`, `fromSeed`).

use md5::{Digest, Md5};

/// Common interface of the random sources.
pub trait Random {
    fn next_i32(&mut self) -> i32;
    /// Uniform integer in `[0, bound)`; `bound` must be positive.
    fn next_bounded(&mut self, bound: i32) -> i32;
    fn next_i64(&mut self) -> i64;
    fn next_bool(&mut self) -> bool;
    fn next_f32(&mut self) -> f32;
    fn next_f64(&mut self) -> f64;
    fn next_gaussian(&mut self) -> f64;
    /// `consumeCount`.
    fn consume(&mut self, count: i32);

    /// `nextIntBetweenInclusive`.
    fn next_between(&mut self, min: i32, max: i32) -> i32 {
        self.next_bounded(max - min + 1) + min
    }

    /// `Mth.nextInt(random, min, max)`: inclusive, returns `min` for an empty range.
    fn next_int_range(&mut self, min: i32, max: i32) -> i32 {
        if min >= max {
            min
        } else {
            self.next_bounded(max - min + 1) + min
        }
    }

    /// `nextIntBetweenExclusive`.
    fn next_between_exclusive(&mut self, min: i32, max: i32) -> i32 {
        min + self.next_bounded(max - min)
    }

    fn next_triangle(&mut self, mode: f64, deviation: f64) -> f64 {
        mode + deviation * (self.next_f64() - self.next_f64())
    }

    /// `Mth.nextFloat(random, min, max)`.
    fn next_f32_range(&mut self, min: f32, max: f32) -> f32 {
        if min >= max {
            min
        } else {
            self.next_f32() * (max - min) + min
        }
    }
}

/// `RandomSupport.upgradeSeedTo128bit` plus the Stafford 13 mix.
fn upgrade_seed(seed: u64) -> (u64, u64) {
    let lo = seed ^ 0x6A09_E667_F3BC_C909;
    let hi = lo.wrapping_add(0x9E37_79B9_7F4A_7C15);
    (mix_stafford13(lo), mix_stafford13(hi))
}

const fn mix_stafford13(z: u64) -> u64 {
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `RandomSupport.seedFromHashOf`: the MD5 of the UTF-8 bytes, as two big-endian longs.
pub fn md5_seed(name: &str) -> (u64, u64) {
    let digest = Md5::digest(name.as_bytes());
    let lo = u64::from_be_bytes(digest[0..8].try_into().expect("md5 is 16 bytes"));
    let hi = u64::from_be_bytes(digest[8..16].try_into().expect("md5 is 16 bytes"));
    (lo, hi)
}

/// `Mth.getSeed(x, y, z)`. Note the 32 bit multiplication of `x`.
#[inline]
pub const fn position_seed(x: i32, y: i32, z: i32) -> i64 {
    let l = (x.wrapping_mul(3_129_871) as i64) ^ (z as i64).wrapping_mul(116_129_781) ^ (y as i64);
    let l = l.wrapping_mul(l).wrapping_mul(42_317_861).wrapping_add(l.wrapping_mul(11));
    l >> 16
}

/// `String.hashCode()`.
pub fn java_string_hash(s: &str) -> i32 {
    let mut h = 0i32;
    for unit in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(unit as i32);
    }
    h
}

#[derive(Clone, Debug, Default)]
struct Gaussian {
    next: Option<f64>,
}

impl Gaussian {
    /// `MarsagliaPolarGaussian.nextGaussian` drawing doubles from `next_f64`.
    fn sample(&mut self, mut next_f64: impl FnMut() -> f64) -> f64 {
        if let Some(v) = self.next.take() {
            return v;
        }
        loop {
            let d = 2.0 * next_f64() - 1.0;
            let e = 2.0 * next_f64() - 1.0;
            let f = d * d + e * e;
            if f < 1.0 && f != 0.0 {
                let g = (-2.0 * f.ln() / f).sqrt();
                self.next = Some(e * g);
                return d * g;
            }
        }
    }
}

/// `XoroshiroRandomSource`.
#[derive(Clone, Debug)]
pub struct Xoroshiro {
    lo: u64,
    hi: u64,
    gaussian: Gaussian,
}

impl Xoroshiro {
    /// `new XoroshiroRandomSource(seed)`.
    pub fn new(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed(seed as u64);
        Self::from_parts(lo, hi)
    }

    /// `new XoroshiroRandomSource(lo, hi)`.
    pub fn from_parts(lo: u64, hi: u64) -> Self {
        let (lo, hi) = if lo | hi == 0 { (0x9E37_79B9_7F4A_7C15, 0x6A09_E667_F3BC_C909) } else { (lo, hi) };
        Self { lo, hi, gaussian: Gaussian::default() }
    }

    pub fn set_seed(&mut self, seed: i64) {
        *self = Self::new(seed);
    }

    #[inline]
    pub fn next_long_raw(&mut self) -> u64 {
        let l = self.lo;
        let m = self.hi;
        let n = l.wrapping_add(m).rotate_left(17).wrapping_add(l);
        let m = m ^ l;
        self.lo = l.rotate_left(49) ^ m ^ (m << 21);
        self.hi = m.rotate_left(28);
        n
    }

    #[inline]
    fn next_bits(&mut self, bits: u32) -> u64 {
        self.next_long_raw() >> (64 - bits)
    }

    pub fn fork(&mut self) -> Self {
        let lo = self.next_long_raw();
        let hi = self.next_long_raw();
        Self::from_parts(lo, hi)
    }

    pub fn fork_positional(&mut self) -> PositionalRandom {
        let lo = self.next_long_raw();
        let hi = self.next_long_raw();
        PositionalRandom::Xoroshiro { lo, hi }
    }
}

impl Random for Xoroshiro {
    #[inline]
    fn next_i32(&mut self) -> i32 {
        self.next_long_raw() as i32
    }

    fn next_bounded(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0, "bound must be positive");
        let bound64 = bound as u32 as u64;
        let mut l = self.next_i32() as u32 as u64;
        let mut m = l.wrapping_mul(bound64);
        let mut n = m & 0xFFFF_FFFF;
        if n < bound64 {
            let i = ((!bound).wrapping_add(1) as u32 % bound as u32) as u64;
            while n < i {
                l = self.next_i32() as u32 as u64;
                m = l.wrapping_mul(bound64);
                n = m & 0xFFFF_FFFF;
            }
        }
        (m >> 32) as i32
    }

    #[inline]
    fn next_i64(&mut self) -> i64 {
        self.next_long_raw() as i64
    }

    #[inline]
    fn next_bool(&mut self) -> bool {
        self.next_long_raw() & 1 != 0
    }

    #[inline]
    fn next_f32(&mut self) -> f32 {
        self.next_bits(24) as f32 * 5.960_464_5E-8
    }

    #[inline]
    fn next_f64(&mut self) -> f64 {
        self.next_bits(53) as f64 * 1.110_223_024_625_156_5E-16
    }

    fn next_gaussian(&mut self) -> f64 {
        let mut g = std::mem::take(&mut self.gaussian);
        let v = g.sample(|| self.next_f64());
        self.gaussian = g;
        v
    }

    fn consume(&mut self, count: i32) {
        for _ in 0..count {
            self.next_long_raw();
        }
    }
}

/// `LegacyRandomSource`: the `java.util.Random` LCG.
#[derive(Clone, Debug)]
pub struct LegacyRandom {
    seed: u64,
    gaussian: Gaussian,
}

const LCG_MULTIPLIER: u64 = 0x5_DEEC_E66D;
const LCG_MASK: u64 = (1 << 48) - 1;

impl LegacyRandom {
    pub fn new(seed: i64) -> Self {
        Self { seed: (seed as u64 ^ LCG_MULTIPLIER) & LCG_MASK, gaussian: Gaussian::default() }
    }

    pub fn set_seed(&mut self, seed: i64) {
        *self = Self::new(seed);
    }

    /// `next(bits)`.
    #[inline]
    pub fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(LCG_MULTIPLIER).wrapping_add(11) & LCG_MASK;
        ((self.seed as i64) >> (48 - bits)) as i32
    }

    pub fn fork(&mut self) -> Self {
        Self::new(self.next_i64())
    }

    pub fn fork_positional(&mut self) -> PositionalRandom {
        PositionalRandom::Legacy { seed: self.next_i64() }
    }
}

/// `BitRandomSource.nextInt(bound)` on top of `next(bits)`.
#[inline]
fn bit_bounded(bound: i32, mut next: impl FnMut(u32) -> i32) -> i32 {
    debug_assert!(bound > 0, "bound must be positive");
    if bound & (bound - 1) == 0 {
        return ((bound as i64 * next(31) as i64) >> 31) as i32;
    }
    loop {
        let i = next(31);
        let j = i % bound;
        if i.wrapping_sub(j).wrapping_add(bound - 1) >= 0 {
            return j;
        }
    }
}

#[inline]
fn bit_f64(mut next: impl FnMut(u32) -> i32) -> f64 {
    let i = next(26) as i64;
    let j = next(27) as i64;
    ((i << 27) + j) as f64 * 1.110_223_024_625_156_5E-16
}

impl Random for LegacyRandom {
    #[inline]
    fn next_i32(&mut self) -> i32 {
        self.next(32)
    }

    fn next_bounded(&mut self, bound: i32) -> i32 {
        bit_bounded(bound, |bits| self.next(bits))
    }

    fn next_i64(&mut self) -> i64 {
        let i = self.next(32) as i64;
        let j = self.next(32) as i64;
        (i << 32).wrapping_add(j)
    }

    fn next_bool(&mut self) -> bool {
        self.next(1) != 0
    }

    fn next_f32(&mut self) -> f32 {
        self.next(24) as f32 * 5.960_464_5E-8
    }

    fn next_f64(&mut self) -> f64 {
        bit_f64(|bits| self.next(bits))
    }

    fn next_gaussian(&mut self) -> f64 {
        let mut g = std::mem::take(&mut self.gaussian);
        let v = g.sample(|| self.next_f64());
        self.gaussian = g;
        v
    }

    fn consume(&mut self, count: i32) {
        for _ in 0..count {
            self.next(32);
        }
    }
}

/// A random source of either kind.
#[derive(Clone, Debug)]
pub enum RandomSource {
    Xoroshiro(Xoroshiro),
    Legacy(LegacyRandom),
}

impl RandomSource {
    pub fn fork(&mut self) -> Self {
        match self {
            Self::Xoroshiro(r) => Self::Xoroshiro(r.fork()),
            Self::Legacy(r) => Self::Legacy(r.fork()),
        }
    }

    pub fn fork_positional(&mut self) -> PositionalRandom {
        match self {
            Self::Xoroshiro(r) => r.fork_positional(),
            Self::Legacy(r) => r.fork_positional(),
        }
    }

    pub fn set_seed(&mut self, seed: i64) {
        match self {
            Self::Xoroshiro(r) => r.set_seed(seed),
            Self::Legacy(r) => r.set_seed(seed),
        }
    }
}

macro_rules! delegate_random {
    ($ty:ty) => {
        impl Random for $ty {
            #[inline]
            fn next_i32(&mut self) -> i32 {
                match self {
                    Self::Xoroshiro(r) => r.next_i32(),
                    Self::Legacy(r) => r.next_i32(),
                }
            }
            #[inline]
            fn next_bounded(&mut self, bound: i32) -> i32 {
                match self {
                    Self::Xoroshiro(r) => r.next_bounded(bound),
                    Self::Legacy(r) => r.next_bounded(bound),
                }
            }
            #[inline]
            fn next_i64(&mut self) -> i64 {
                match self {
                    Self::Xoroshiro(r) => r.next_i64(),
                    Self::Legacy(r) => r.next_i64(),
                }
            }
            #[inline]
            fn next_bool(&mut self) -> bool {
                match self {
                    Self::Xoroshiro(r) => r.next_bool(),
                    Self::Legacy(r) => r.next_bool(),
                }
            }
            #[inline]
            fn next_f32(&mut self) -> f32 {
                match self {
                    Self::Xoroshiro(r) => r.next_f32(),
                    Self::Legacy(r) => r.next_f32(),
                }
            }
            #[inline]
            fn next_f64(&mut self) -> f64 {
                match self {
                    Self::Xoroshiro(r) => r.next_f64(),
                    Self::Legacy(r) => r.next_f64(),
                }
            }
            fn next_gaussian(&mut self) -> f64 {
                match self {
                    Self::Xoroshiro(r) => r.next_gaussian(),
                    Self::Legacy(r) => r.next_gaussian(),
                }
            }
            fn consume(&mut self, count: i32) {
                match self {
                    Self::Xoroshiro(r) => r.consume(count),
                    Self::Legacy(r) => r.consume(count),
                }
            }
        }
    };
}

delegate_random!(RandomSource);

/// `PositionalRandomFactory`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionalRandom {
    Xoroshiro { lo: u64, hi: u64 },
    Legacy { seed: i64 },
}

impl PositionalRandom {
    /// `at(x, y, z)`.
    #[inline]
    pub fn at(&self, x: i32, y: i32, z: i32) -> RandomSource {
        let seed = position_seed(x, y, z);
        match *self {
            Self::Xoroshiro { lo, hi } => RandomSource::Xoroshiro(Xoroshiro::from_parts(seed as u64 ^ lo, hi)),
            Self::Legacy { seed: base } => RandomSource::Legacy(LegacyRandom::new(seed ^ base)),
        }
    }

    /// `fromHashOf(name)`.
    pub fn from_hash_of(&self, name: &str) -> RandomSource {
        match *self {
            Self::Xoroshiro { lo, hi } => {
                let (l, h) = md5_seed(name);
                RandomSource::Xoroshiro(Xoroshiro::from_parts(l ^ lo, h ^ hi))
            }
            Self::Legacy { seed } => RandomSource::Legacy(LegacyRandom::new(java_string_hash(name) as i64 ^ seed)),
        }
    }

    /// `fromSeed(seed)`.
    pub fn from_seed(&self, seed: i64) -> RandomSource {
        match *self {
            Self::Xoroshiro { lo, hi } => RandomSource::Xoroshiro(Xoroshiro::from_parts(seed as u64 ^ lo, seed as u64 ^ hi)),
            Self::Legacy { .. } => RandomSource::Legacy(LegacyRandom::new(seed)),
        }
    }
}

/// `WorldgenRandom`: `java.util.Random` algorithms over the bits of a wrapped source.
#[derive(Clone, Debug)]
pub struct WorldgenRandom {
    source: RandomSource,
    gaussian: Gaussian,
}

impl WorldgenRandom {
    pub fn new(source: RandomSource) -> Self {
        Self { source, gaussian: Gaussian::default() }
    }

    /// `new WorldgenRandom(new XoroshiroRandomSource(seed))`.
    pub fn xoroshiro(seed: i64) -> Self {
        Self::new(RandomSource::Xoroshiro(Xoroshiro::new(seed)))
    }

    /// `new WorldgenRandom(new LegacyRandomSource(seed))`.
    pub fn legacy(seed: i64) -> Self {
        Self::new(RandomSource::Legacy(LegacyRandom::new(seed)))
    }

    #[inline]
    pub fn next(&mut self, bits: u32) -> i32 {
        match &mut self.source {
            RandomSource::Legacy(r) => r.next(bits),
            RandomSource::Xoroshiro(r) => (r.next_long_raw() >> (64 - bits)) as i32,
        }
    }

    /// `setSeed`: reseeds the wrapped source. Like vanilla, the cached second gaussian is kept.
    pub fn set_seed(&mut self, seed: i64) {
        self.source.set_seed(seed);
    }

    /// `setDecorationSeed`; returns the population seed.
    pub fn set_decoration_seed(&mut self, seed: i64, x: i32, z: i32) -> i64 {
        self.set_seed(seed);
        let l = self.next_i64() | 1;
        let m = self.next_i64() | 1;
        let n = (x as i64).wrapping_mul(l).wrapping_add((z as i64).wrapping_mul(m)) ^ seed;
        self.set_seed(n);
        n
    }

    /// `setFeatureSeed`.
    pub fn set_feature_seed(&mut self, seed: i64, index: i32, step: i32) {
        let l = seed.wrapping_add(index as i64).wrapping_add(10000i64 * step as i64);
        self.set_seed(l);
    }

    /// `setLargeFeatureSeed`.
    pub fn set_large_feature_seed(&mut self, seed: i64, x: i32, z: i32) {
        self.set_seed(seed);
        let l = self.next_i64();
        let m = self.next_i64();
        let n = (x as i64).wrapping_mul(l) ^ (z as i64).wrapping_mul(m) ^ seed;
        self.set_seed(n);
    }

    /// `setLargeFeatureWithSalt`.
    pub fn set_large_feature_with_salt(&mut self, seed: i64, x: i32, z: i32, salt: i32) {
        let l = (x as i64)
            .wrapping_mul(341_873_128_712)
            .wrapping_add((z as i64).wrapping_mul(132_897_987_541))
            .wrapping_add(seed)
            .wrapping_add(salt as i64);
        self.set_seed(l);
    }

    pub fn fork(&mut self) -> RandomSource {
        self.source.fork()
    }

    pub fn fork_positional(&mut self) -> PositionalRandom {
        self.source.fork_positional()
    }
}

impl Random for WorldgenRandom {
    #[inline]
    fn next_i32(&mut self) -> i32 {
        self.next(32)
    }

    fn next_bounded(&mut self, bound: i32) -> i32 {
        bit_bounded(bound, |bits| self.next(bits))
    }

    fn next_i64(&mut self) -> i64 {
        let i = self.next(32) as i64;
        let j = self.next(32) as i64;
        (i << 32).wrapping_add(j)
    }

    fn next_bool(&mut self) -> bool {
        self.next(1) != 0
    }

    fn next_f32(&mut self) -> f32 {
        self.next(24) as f32 * 5.960_464_5E-8
    }

    fn next_f64(&mut self) -> f64 {
        bit_f64(|bits| self.next(bits))
    }

    fn next_gaussian(&mut self) -> f64 {
        let mut g = std::mem::take(&mut self.gaussian);
        let v = g.sample(|| self.next_f64());
        self.gaussian = g;
        v
    }

    fn consume(&mut self, count: i32) {
        for _ in 0..count {
            self.next(32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values from Pumpkin's tests, which were checked against the Java sources.

    #[test]
    fn xoroshiro_sequences() {
        let mut r = Xoroshiro::new(0);
        let expected = [-160476802, 781697906, 653572596, 1337520923, -505875771, -47281585, 342195906, 1417498593, -1478887443, 1560080270];
        for v in expected {
            assert_eq!(r.next_i32(), v);
        }
        let mut r = Xoroshiro::new(0);
        for v in [9, 1, 1, 3, 8, 9, 0, 3, 6, 3] {
            assert_eq!(r.next_bounded(10), v);
        }
        for v in [9784805, 470346, 13560642, 7320226, 14949645, 13460529, 2824352, 10938308, 14146127, 4549185] {
            assert_eq!(r.next_bounded(0xFFFFFF), v);
        }
        let mut r = Xoroshiro::new(0);
        let doubles = [0.16474369376959186, 0.7997457290026366, 0.2511961888876212, 0.11712489470639631, 0.0997124786680137];
        for v in doubles {
            assert_eq!(r.next_f64(), v);
        }
    }

    #[test]
    fn legacy_sequences() {
        let mut r = LegacyRandom::new(0);
        for v in [-1155484576, -723955400, 1033096058, -1690734402, -1557280266, 1327362106, -1930858313, 502539523] {
            assert_eq!(r.next_i32(), v);
        }
    }

    #[test]
    fn position_seeds() {
        let values: [((i32, i32, i32), i64); 8] = [
            ((0, 0, 0), 0),
            ((1, 1, 1), 60311958971344),
            ((4, 4, 4), 120566413180880),
            ((25, 25, 25), 111753446486209),
            ((676, 676, 676), 75210837988243),
            ((458329, 458329, 458329), -43764888250),
            ((-387008604, -387008604, -387008604), 8437923733503),
            ((176771161, 176771161, 176771161), 18421337580760),
        ];
        for ((x, y, z), v) in values {
            assert_eq!(position_seed(x, y, z), v);
        }
    }
}
