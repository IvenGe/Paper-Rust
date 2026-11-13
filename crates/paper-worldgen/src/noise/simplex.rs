//! Simplex noise (`SimplexNoise`) and its octave sum (`PerlinSimplexNoise`).
//!
//! Derived from `pumpkin-util/src/noise/simplex.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::random::{LegacyRandom, Random};

const GRADIENTS: [[f64; 3]; 16] = [
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

const SQRT_3: f64 = 1.7320508075688772;
const F2: f64 = 0.5 * (SQRT_3 - 1.0);
const G2: f64 = (3.0 - SQRT_3) / 6.0;

#[derive(Clone)]
pub struct SimplexNoise {
    p: [u8; 256],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

#[inline]
fn dot(g: &[f64; 3], x: f64, y: f64, z: f64) -> f64 {
    g[0] * x + g[1] * y + g[2] * z
}

#[inline]
fn corner(index: usize, x: f64, y: f64, z: f64, base: f64) -> f64 {
    let d = base - x * x - y * y - z * z;
    if d < 0.0 {
        0.0
    } else {
        let d = d * d;
        d * d * dot(&GRADIENTS[index], x, y, z)
    }
}

impl SimplexNoise {
    pub fn new(random: &mut impl Random) -> Self {
        let xo = random.next_f64() * 256.0;
        let yo = random.next_f64() * 256.0;
        let zo = random.next_f64() * 256.0;
        let mut p = [0u8; 256];
        for (i, v) in p.iter_mut().enumerate() {
            *v = i as u8;
        }
        for i in 0..256usize {
            let j = random.next_bounded(256 - i as i32) as usize;
            p.swap(i, i + j);
        }
        Self { p, xo, yo, zo }
    }

    #[inline]
    fn p(&self, i: i32) -> i32 {
        self.p[(i & 0xFF) as usize] as i32
    }

    pub fn get_2d(&self, x: f64, y: f64) -> f64 {
        let d = (x + y) * F2;
        let i = (x + d).floor() as i32;
        let j = (y + d).floor() as i32;
        let e = i.wrapping_add(j) as f64 * G2;
        let h = x - (i as f64 - e);
        let k = y - (j as f64 - e);
        let (l, m) = if h > k { (1, 0) } else { (0, 1) };
        let n = h - l as f64 + G2;
        let o = k - m as f64 + G2;
        let p = h - 1.0 + 2.0 * G2;
        let q = k - 1.0 + 2.0 * G2;
        let r = i & 0xFF;
        let s = j & 0xFF;
        let t = self.p(r + self.p(s)) % 12;
        let u = self.p(r + l + self.p(s + m)) % 12;
        let v = self.p(r + 1 + self.p(s + 1)) % 12;
        let w = corner(t as usize, h, k, 0.0, 0.5);
        let z = corner(u as usize, n, o, 0.0, 0.5);
        let aa = corner(v as usize, p, q, 0.0, 0.5);
        70.0 * (w + z + aa)
    }

    #[allow(clippy::many_single_char_names)]
    pub fn get_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        let e = (x + y + z) * 0.3333333333333333;
        let i = (x + e).floor() as i32;
        let j = (y + e).floor() as i32;
        let k = (z + e).floor() as i32;
        let g = i.wrapping_add(j).wrapping_add(k) as f64 * 0.16666666666666666;
        let n = x - (i as f64 - g);
        let o = y - (j as f64 - g);
        let p = z - (k as f64 - g);
        let (q, r, s, t, u, v) = if n >= o {
            if o >= p {
                (1, 0, 0, 1, 1, 0)
            } else if n >= p {
                (1, 0, 0, 1, 0, 1)
            } else {
                (0, 0, 1, 1, 0, 1)
            }
        } else if o < p {
            (0, 0, 1, 0, 1, 1)
        } else if n < p {
            (0, 1, 0, 0, 1, 1)
        } else {
            (0, 1, 0, 1, 1, 0)
        };
        let w = n - q as f64 + 0.16666666666666666;
        let aa = o - r as f64 + 0.16666666666666666;
        let ab = p - s as f64 + 0.16666666666666666;
        let ac = n - t as f64 + 0.3333333333333333;
        let ad = o - u as f64 + 0.3333333333333333;
        let ae = p - v as f64 + 0.3333333333333333;
        let af = n - 1.0 + 0.5;
        let ag = o - 1.0 + 0.5;
        let ah = p - 1.0 + 0.5;
        let ai = i & 0xFF;
        let aj = j & 0xFF;
        let ak = k & 0xFF;
        let al = self.p(ai + self.p(aj + self.p(ak))) % 12;
        let am = self.p(ai + q + self.p(aj + r + self.p(ak + s))) % 12;
        let an = self.p(ai + t + self.p(aj + u + self.p(ak + v))) % 12;
        let ao = self.p(ai + 1 + self.p(aj + 1 + self.p(ak + 1))) % 12;
        let ap = corner(al as usize, n, o, p, 0.6);
        let aq = corner(am as usize, w, aa, ab, 0.6);
        let ar = corner(an as usize, ac, ad, ae, 0.6);
        let az = corner(ao as usize, af, ag, ah, 0.6);
        32.0 * (ap + aq + ar + az)
    }
}

/// `PerlinSimplexNoise`: a sum of simplex octaves.
pub struct PerlinSimplexNoise {
    levels: Vec<Option<SimplexNoise>>,
    highest_freq_value_factor: f64,
    highest_freq_input_factor: f64,
}

impl PerlinSimplexNoise {
    pub fn new(random: &mut impl Random, octaves: &[i32]) -> Self {
        let mut octaves: Vec<i32> = octaves.to_vec();
        octaves.sort_unstable();
        let first = octaves[0];
        let last = *octaves.last().expect("at least one octave");
        let i = -first;
        let j = last;
        let k = i + j + 1;
        let noise = SimplexNoise::new(random);
        let mut levels: Vec<Option<SimplexNoise>> = (0..k).map(|_| None).collect();
        for m in (j + 1)..k {
            if m >= 0 && octaves.contains(&(j - m)) {
                levels[m as usize] = Some(SimplexNoise::new(random));
            } else {
                random.consume(262);
            }
        }
        if j > 0 {
            let seed = (noise.get_3d(noise.xo, noise.yo, noise.zo) * 9.223372E18f32 as f64) as i64;
            let mut r = LegacyRandom::new(seed);
            for o in (0..j).rev() {
                if o < k && octaves.contains(&(j - o)) {
                    levels[o as usize] = Some(SimplexNoise::new(&mut r));
                } else {
                    r.consume(262);
                }
            }
        }
        if j >= 0 && j < k && octaves.contains(&0) {
            levels[j as usize] = Some(noise);
        }
        Self { levels, highest_freq_value_factor: 1.0 / (2f64.powi(k) - 1.0), highest_freq_input_factor: 2f64.powi(j) }
    }

    pub fn get_value(&self, x: f64, y: f64, use_origin: bool) -> f64 {
        let mut d = 0.0;
        let mut e = self.highest_freq_input_factor;
        let mut f = self.highest_freq_value_factor;
        for level in &self.levels {
            if let Some(n) = level {
                d += n.get_2d(x * e + if use_origin { n.xo } else { 0.0 }, y * e + if use_origin { n.yo } else { 0.0 }) * f;
            }
            e /= 2.0;
            f *= 2.0;
        }
        d
    }
}
