//! Aquifers: which fluid (or air) fills the space where the terrain density is not solid.
//!
//! Derived from `pumpkin-world/src/generation/noise/aquifer_sampler.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::math::{clamped_map, floor_div, map};
use crate::random::{PositionalRandom, Random};
use crate::router::{ChunkRouter, SurfaceEstimator};

/// `DimensionType.WAY_BELOW_MIN_Y`.
pub const WAY_BELOW_MIN_Y: i32 = -2032 << 4;

/// A fluid that fills everything below `max_y` (exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluidLevel {
    pub max_y: i32,
    /// Default state of the fluid block (water, lava, or air).
    pub state: u32,
}

impl FluidLevel {
    #[inline]
    pub fn at(&self, y: i32) -> u32 {
        if y < self.max_y {
            self.state
        } else {
            paper_data::blocks::AIR
        }
    }
}

/// The global fluid levels: the dimension's default fluid up to sea level and lava below
/// y = -54.
#[derive(Clone, Copy, Debug)]
pub struct FluidPicker {
    top: FluidLevel,
    bottom: FluidLevel,
    bottom_y: i32,
}

impl FluidPicker {
    pub fn new(sea_level: i32, default_fluid: u32, lava: u32) -> Self {
        let top = FluidLevel { max_y: sea_level, state: default_fluid };
        let bottom = FluidLevel { max_y: -54, state: lava };
        Self { top, bottom, bottom_y: top.max_y.min(bottom.max_y) }
    }

    #[inline]
    pub fn level(&self, y: i32) -> FluidLevel {
        if y < self.bottom_y {
            self.bottom
        } else {
            self.top
        }
    }
}

/// Indices of the aquifer functions in the noise router stack.
#[derive(Clone, Copy, Debug)]
pub struct AquiferFunctions {
    pub barrier: usize,
    pub floodedness: usize,
    pub spread: usize,
    pub lava: usize,
    /// Positive where aquifers are excluded (the deep dark).
    pub exclusion: usize,
}

/// The noise-based aquifer of one chunk.
pub struct Aquifer {
    picker: FluidPicker,
    funcs: AquiferFunctions,
    water: u32,
    lava: u32,
    start_x: i32,
    start_y: i32,
    start_z: i32,
    size_y: usize,
    size_z: usize,
    levels: Vec<Option<FluidLevel>>,
    positions: Vec<(i32, i32, i32)>,
    sample_min_x: i32,
    sample_min_z: i32,
    sample_max_x: i32,
    sample_max_z: i32,
    skip_above_y: Option<i32>,
}

const OFFSETS: [(i32, i32); 13] = [(0, 0), (-2, -1), (-1, -1), (0, -1), (1, -1), (-3, 0), (-2, 0), (-1, 0), (1, 0), (-2, 1), (-1, 1), (0, 1), (1, 1)];

impl Aquifer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(chunk_x: i32, chunk_z: i32, random: &PositionalRandom, min_y: i32, height: i32, picker: FluidPicker, funcs: AquiferFunctions, water: u32, lava: u32) -> Self {
        let start_x = floor_div(chunk_x << 4, 16) - 1;
        let end_x = floor_div((chunk_x << 4) + 15, 16) + 1;
        let size_x = (end_x - start_x) as usize + 1;
        let start_y = floor_div(min_y, 12) - 1;
        let end_y = floor_div(min_y + height, 12) + 1;
        let size_y = (end_y - start_y) as usize + 1;
        let start_z = floor_div(chunk_z << 4, 16) - 1;
        let end_z = floor_div((chunk_z << 4) + 15, 16) + 1;
        let size_z = (end_z - start_z) as usize + 1;
        let size = size_x * size_y * size_z;
        let mut positions = vec![(0, 0, 0); size];
        for ox in 0..size_x {
            for oy in 0..size_y {
                for oz in 0..size_z {
                    let x = start_x + ox as i32;
                    let y = start_y + oy as i32;
                    let z = start_z + oz as i32;
                    let mut r = random.at(x, y, z);
                    let rx = x * 16 + r.next_bounded(10);
                    let ry = y * 12 + r.next_bounded(9);
                    let rz = z * 16 + r.next_bounded(10);
                    positions[(ox * size_z + oz) * size_y + oy] = (rx, ry, rz);
                }
            }
        }
        Self {
            picker,
            funcs,
            water,
            lava,
            start_x,
            start_y,
            start_z,
            size_y,
            size_z,
            levels: vec![None; size],
            positions,
            sample_min_x: start_x << 4,
            sample_min_z: start_z << 4,
            sample_max_x: (end_x << 4) + 9,
            sample_max_z: (end_z << 4) + 9,
            skip_above_y: None,
        }
    }

    fn skip_above_y(&mut self, estimator: &mut SurfaceEstimator) -> i32 {
        if let Some(y) = self.skip_above_y {
            return y;
        }
        let mut max = i32::MIN;
        let mut z = self.sample_min_z;
        while z <= self.sample_max_z {
            let mut x = self.sample_min_x;
            while x <= self.sample_max_x {
                max = max.max(estimator.estimate_height(x, z));
                x += 4;
            }
            z += 4;
        }
        let adjusted = max + 8;
        let grid_y = floor_div(adjusted + 12, 12) + 1;
        let y = grid_y * 12 + 11 - 1;
        self.skip_above_y = Some(y);
        y
    }

    fn index(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        let lx = usize::try_from(x - self.start_x).ok()?;
        let ly = usize::try_from(y - self.start_y).ok()?;
        let lz = usize::try_from(z - self.start_z).ok()?;
        if ly >= self.size_y || lz >= self.size_z {
            return None;
        }
        let i = (lx * self.size_z + lz) * self.size_y + ly;
        (i < self.positions.len()).then_some(i)
    }

    fn random_positions(&self, x: i32, y: i32, z: i32) -> Option<[(i32, i32, i32); 12]> {
        let sy = self.size_y;
        let syz = self.size_y * self.size_z;
        let i00 = self.index(x, y - 1, z)?;
        let i01 = i00 + sy;
        let i10 = i00 + syz;
        let i11 = i10 + sy;
        if i11 + 2 >= self.positions.len() {
            return None;
        }
        let p = &self.positions;
        Some([p[i11 + 2], p[i10 + 2], p[i01 + 2], p[i00 + 2], p[i11 + 1], p[i10 + 1], p[i01 + 1], p[i00 + 1], p[i11], p[i10], p[i01], p[i00]])
    }

    #[inline]
    fn similarity(a: i32, b: i32) -> f32 {
        1.0 - ((b - a).abs() as f32) / 25.0
    }

    #[allow(clippy::too_many_arguments)]
    fn pressure(&self, barrier: &mut Option<f32>, router: &mut ChunkRouter, x: i32, y: i32, z: i32, l1: &FluidLevel, l2: &FluidLevel) -> f32 {
        let s1 = l1.at(y);
        let s2 = l2.at(y);
        if (s1 == self.lava && s2 == self.water) || (s1 == self.water && s2 == self.lava) {
            return 2.0;
        }
        let diff = (l1.max_y - l2.max_y).abs();
        if diff == 0 {
            return 0.0;
        }
        let avg = 0.5 * (l1.max_y + l2.max_y) as f32;
        let scaled = y as f32 + 0.5 - avg;
        let half = diff as f32 / 2.0;
        let o = half - scaled.abs();
        let q = if scaled > 0.0 {
            if o > 0.0 {
                o / 1.5
            } else {
                o / 2.5
            }
        } else {
            let p = 3.0 + o;
            if p > 0.0 {
                p / 3.0
            } else {
                p / 10.0
            }
        };
        let r = if (-2.0..=2.0).contains(&q) {
            *barrier.get_or_insert_with(|| router.sample(self.funcs.barrier, x, y, z))
        } else {
            0.0
        };
        2.0 * (r + q)
    }

    fn water_level(&mut self, pos: (i32, i32, i32), router: &mut ChunkRouter, estimator: &mut SurfaceEstimator) -> FluidLevel {
        let (x, y, z) = pos;
        let Some(i) = self.index(floor_div(x, 16), floor_div(y, 12), floor_div(z, 16)) else {
            return self.compute_level(x, y, z, router, estimator);
        };
        if let Some(l) = self.levels[i] {
            return l;
        }
        let l = self.compute_level(x, y, z, router, estimator);
        self.levels[i] = Some(l);
        l
    }

    fn compute_level(&self, x: i32, y: i32, z: i32, router: &mut ChunkRouter, estimator: &mut SurfaceEstimator) -> FluidLevel {
        let level = self.picker.level(y);
        let j = y + 12;
        let k = y - 12;
        let mut near_surface = false;
        let mut min_surface = i32::MAX;
        for (ox, oz) in OFFSETS {
            let sx = x + (ox << 4);
            let sz = z + (oz << 4);
            let n = estimator.estimate_height(sx, sz);
            let o = n + 8;
            let center = ox == 0 && oz == 0;
            if center && k > o {
                return level;
            }
            let above = j > o;
            if above || center {
                let l = self.picker.level(o);
                if !paper_data::blocks::is_air(l.at(o)) {
                    if center {
                        near_surface = true;
                    }
                    if above {
                        return l;
                    }
                }
            }
            min_surface = min_surface.min(n);
        }
        let p = self.fluid_y(x, y, z, &level, min_surface, near_surface, router);
        FluidLevel { max_y: p, state: self.fluid_state(x, y, z, &level, p, router) }
    }

    #[allow(clippy::too_many_arguments)]
    fn fluid_y(&self, x: i32, y: i32, z: i32, default: &FluidLevel, surface: i32, near_surface: bool, router: &mut ChunkRouter) -> i32 {
        let excluded = router.sample(self.funcs.exclusion, x, y, z) > 0.0;
        let (d, e) = if excluded {
            (-1.0f32, -1.0f32)
        } else {
            let top = surface + 8 - y;
            let f = if near_surface { clamped_map(top as f32, 0.0, 64.0, 1.0, 0.0) } else { 0.0 };
            let g = router.sample(self.funcs.floodedness, x, y, z).clamp(-1.0, 1.0);
            let h = map(f, 1.0, 0.0, -0.3, 0.8);
            let k = map(f, 1.0, 0.0, -0.8, 0.4);
            (g - k, g - h)
        };
        if e > 0.0 {
            default.max_y
        } else if d > 0.0 {
            let gx = floor_div(x, 16);
            let gy = floor_div(y, 40);
            let gz = floor_div(z, 16);
            let local_y = gy * 40 + 20;
            let sample = router.sample(self.funcs.spread, gx, gy, gz) * 10.0;
            let to_three = (sample / 3.0).floor() as i32 * 3;
            surface.min(to_three + local_y)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    fn fluid_state(&self, x: i32, y: i32, z: i32, default: &FluidLevel, level: i32, router: &mut ChunkRouter) -> u32 {
        if level <= -10 && level != WAY_BELOW_MIN_Y && default.state != self.lava {
            let sample = router.sample(self.funcs.lava, floor_div(x, 64), floor_div(y, 40), floor_div(z, 64));
            if sample.abs() > 0.3 {
                return self.lava;
            }
        }
        default.state
    }

    /// The block for a non-solid position, or `None` if the position stays solid. The flag
    /// tells whether vanilla would schedule a fluid update there.
    pub fn compute(&mut self, router: &mut ChunkRouter, estimator: &mut SurfaceEstimator, x: i32, y: i32, z: i32, density: f32) -> (Option<u32>, bool) {
        if density > 0.0 {
            return (None, false);
        }
        let level = self.picker.level(y);
        if y > self.skip_above_y(estimator) {
            return (Some(level.at(y)), false);
        }
        if level.at(y) == self.lava {
            return (Some(self.lava), false);
        }
        let Some(positions) = self.random_positions(floor_div(x - 5, 16), floor_div(y + 1, 12), floor_div(z - 5, 16)) else {
            return (Some(level.at(y)), false);
        };
        let mut nearest = [((0, 0, 0), i32::MAX); 4];
        for p in positions {
            let dx = p.0 - x;
            let dy = p.1 - y;
            let dz = p.2 - z;
            let h = dx * dx + dy * dy + dz * dz;
            if nearest[3].1 > h {
                nearest[3] = (p, h);
                if nearest[2].1 > h {
                    nearest[3] = nearest[2];
                    nearest[2] = (p, h);
                }
                if nearest[1].1 > h {
                    nearest[2] = nearest[1];
                    nearest[1] = (p, h);
                }
                if nearest[0].1 > h {
                    nearest[1] = nearest[0];
                    nearest[0] = (p, h);
                }
            }
        }
        let l2 = self.water_level(nearest[0].0, router, estimator);
        let state = l2.at(y);
        let sim12 = Self::similarity(nearest[0].1, nearest[1].1);
        if sim12 <= 0.0 {
            let schedule = if sim12 >= -0.12 {
                let l3 = self.water_level(nearest[1].0, router, estimator);
                l2 != l3
            } else {
                false
            };
            return (Some(state), schedule);
        }
        if state == self.water && self.picker.level(y - 1).at(y - 1) == self.lava {
            return (Some(state), true);
        }
        let mut barrier = None;
        let l3 = self.water_level(nearest[1].0, router, estimator);
        let b12 = sim12 * self.pressure(&mut barrier, router, x, y, z, &l2, &l3);
        if density + b12 > 0.0 {
            return (None, false);
        }
        let l4 = self.water_level(nearest[2].0, router, estimator);
        let sim13 = Self::similarity(nearest[0].1, nearest[2].1);
        if sim13 > 0.0 {
            let b13 = sim12 * sim13 * self.pressure(&mut barrier, router, x, y, z, &l2, &l4);
            if density + b13 > 0.0 {
                return (None, false);
            }
        }
        let sim23 = Self::similarity(nearest[1].1, nearest[2].1);
        if sim23 > 0.0 {
            let b23 = sim12 * sim23 * self.pressure(&mut barrier, router, x, y, z, &l3, &l4);
            if density + b23 > 0.0 {
                return (None, false);
            }
        }
        let flow12 = l2 != l3;
        let flow23 = sim23 >= -0.12 && l3 != l4;
        let flow13 = sim13 >= -0.12 && l2 != l4;
        let schedule = if flow12 || flow23 || flow13 {
            true
        } else {
            let l5 = self.water_level(nearest[3].0, router, estimator);
            sim13 >= -0.12 && Self::similarity(nearest[0].1, nearest[3].1) >= -0.12 && l2 != l5
        };
        (Some(state), schedule)
    }
}
