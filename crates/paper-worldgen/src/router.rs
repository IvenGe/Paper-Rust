//! The noise router of a dimension, seeded, and its per-chunk evaluators.
//!
//! Derived from `pumpkin-world/src/generation/noise/router/{chunk_noise_router,
//! surface_height_sampler,multi_noise_sampler}.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

use crate::density::chunk::{build_chunk_stack, sample, sample_volume, Buf, ChunkComp, InterpolationMode};
use crate::density::stack::{Dependent, ProtoKind};
use crate::density::SeededStack;
use crate::noise::DensityVolume;
use crate::math::FxMap;

/// A chunk-level evaluator over a seeded stack.
pub struct ChunkRouter<'a> {
    stack: Vec<ChunkComp<'a>>,
}

impl<'a> ChunkRouter<'a> {
    /// The terrain evaluator: cells are interpolated and the beardifier reports an infinite
    /// range, like vanilla's `NoiseChunk`.
    pub fn terrain(stack: &'a SeededStack) -> Self {
        Self { stack: build_chunk_stack(&stack.protos, InterpolationMode::Cells, true) }
    }

    /// An evaluator without cell interpolation (climate sampling, surface estimation).
    pub fn flat(stack: &'a SeededStack) -> Self {
        Self { stack: build_chunk_stack(&stack.protos, InterpolationMode::PassThrough, false) }
    }

    #[inline]
    pub fn sample(&mut self, index: usize, x: i32, y: i32, z: i32) -> f32 {
        sample(&mut self.stack[..=index], x, y, z)
    }

    pub fn sample_volume(&mut self, index: usize, buffer: &mut [f32], volume: &DensityVolume) {
        sample_volume(&mut self.stack[..=index], buffer, volume);
    }

    fn component(&self, index: usize) -> &ChunkComp<'a> {
        &self.stack[index]
    }
}

/// Estimates the terrain surface height per 4x4 column (`preliminary_surface_level`).
pub struct SurfaceEstimator<'a> {
    router: ChunkRouter<'a>,
    root: usize,
    min_y: i32,
    cache: FxMap<(i32, i32), i32>,
}

impl<'a> SurfaceEstimator<'a> {
    pub fn new(stack: &'a SeededStack, min_y: i32) -> Self {
        Self { router: ChunkRouter::flat(stack), root: stack.root(0), min_y, cache: FxMap::default() }
    }

    pub fn estimate_height(&mut self, x: i32, z: i32) -> i32 {
        let ax = (x >> 2) << 2;
        let az = (z >> 2) << 2;
        if let Some(h) = self.cache.get(&(ax, az)) {
            return *h;
        }
        let h = self.calculate(ax, az);
        self.cache.insert((ax, az), h);
        h
    }

    fn calculate(&mut self, x: i32, z: i32) -> i32 {
        let top = self.router.component(self.root);
        if let ChunkComp::Proto(p) = top {
            if let ProtoKind::Dependent(Dependent::FindTopSurface { density, upper_bound, cell_height, .. }) = &p.kind {
                let (density, upper_bound, cell_height) = (*density, *upper_bound, *cell_height);
                let upper = self.router.sample(upper_bound, x, 0, z);
                let mut y = (upper / cell_height as f32).floor() as i32 * cell_height;
                while y >= self.min_y {
                    if self.router.sample(density, x, y, z) > 0.0 {
                        return y;
                    }
                    y -= cell_height;
                }
                return self.min_y;
            }
        }
        self.router.sample(self.root, x, 0, z).floor() as i32
    }
}

/// Quantized climate parameters (`Climate.TargetPoint`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClimatePoint {
    pub temperature: i64,
    pub humidity: i64,
    pub continentalness: i64,
    pub erosion: i64,
    pub depth: i64,
    pub weirdness: i64,
}

impl ClimatePoint {
    pub fn to_array(self) -> [i64; 7] {
        [self.temperature, self.humidity, self.continentalness, self.erosion, self.depth, self.weirdness, 0]
    }
}

/// `Climate.quantizeCoord`.
#[inline]
pub fn quantize(v: f32) -> i64 {
    (v * 10000.0) as i64
}

enum Sampled {
    Columns([i64; 16]),
    Volume(Buf),
}

impl Sampled {
    #[inline]
    fn get(&self, index: usize, column: usize) -> i64 {
        match self {
            Sampled::Columns(c) => c[column],
            Sampled::Volume(b) => quantize(b[index]),
        }
    }
}

/// Samples the six climate parameters (the multi-noise biome inputs).
pub struct ClimateSampler<'a> {
    router: ChunkRouter<'a>,
    /// temperature, vegetation (humidity), continents, erosion, depth, ridges (weirdness)
    roots: [usize; 6],
    volume: Option<DensityVolume>,
    buffers: Option<[Sampled; 6]>,
}

impl<'a> ClimateSampler<'a> {
    pub fn new(stack: &'a SeededStack) -> Self {
        let roots = [stack.root(0), stack.root(1), stack.root(2), stack.root(3), stack.root(4), stack.root(5)];
        Self { router: ChunkRouter::flat(stack), roots, volume: None, buffers: None }
    }

    /// Precomputes the parameters over a volume of biome positions.
    pub fn fill_volume(&mut self, volume: DensityVolume) {
        let columns = volume.size_x * volume.size_z;
        let roots = self.roots;
        let buffers = roots.map(|index| {
            let y_slice = if columns <= 16 {
                match self.router.component(index) {
                    ChunkComp::Proto(p) => match &p.kind {
                        ProtoKind::Dependent(Dependent::Slice { axis: crate::density::ir::Axis::Y, coordinate, .. }) => Some(*coordinate),
                        ProtoKind::Independent(crate::density::stack::Independent::Constant(_)) => Some(volume.min_y),
                        _ => None,
                    },
                    _ => None,
                }
            } else {
                None
            };
            if let Some(y) = y_slice {
                let flat = DensityVolume::new(volume.size_x, 1, volume.size_z, volume.min_x, y, volume.min_z, volume.step_x, 1, volume.step_z);
                let mut raw = [0.0f32; 16];
                self.router.sample_volume(index, &mut raw[..columns], &flat);
                Sampled::Columns(std::array::from_fn(|i| quantize(raw[i])))
            } else {
                let mut b = Buf::for_volume(&volume);
                self.router.sample_volume(index, &mut b, &volume);
                Sampled::Volume(b)
            }
        });
        self.volume = Some(volume);
        self.buffers = Some(buffers);
    }

    /// The climate at a biome (quart) position.
    pub fn sample(&mut self, qx: i32, qy: i32, qz: i32) -> ClimatePoint {
        let (x, y, z) = (qx << 2, qy << 2, qz << 2);
        if let (Some(volume), Some(b)) = (&self.volume, &self.buffers) {
            if let Some(index) = volume.index_of(x, y, z) {
                let column = index / volume.size_y;
                return ClimatePoint {
                    temperature: b[0].get(index, column),
                    humidity: b[1].get(index, column),
                    continentalness: b[2].get(index, column),
                    erosion: b[3].get(index, column),
                    depth: b[4].get(index, column),
                    weirdness: b[5].get(index, column),
                };
            }
        }
        let r = self.roots;
        let t = self.router.sample(r[0], x, y, z);
        let h = self.router.sample(r[1], x, y, z);
        let c = self.router.sample(r[2], x, y, z);
        let e = self.router.sample(r[3], x, y, z);
        let d = self.router.sample(r[4], x, y, z);
        let w = self.router.sample(r[5], x, y, z);
        ClimatePoint { temperature: quantize(t), humidity: quantize(h), continentalness: quantize(c), erosion: quantize(e), depth: quantize(d), weirdness: quantize(w) }
    }

    /// The raw erosion value at a block position (the end biome source reads it).
    pub fn erosion(&mut self, x: i32, y: i32, z: i32) -> f32 {
        self.router.sample(self.roots[3], x, y, z)
    }

    /// Samples one raw parameter (0..6 in the order of [`ClimatePoint`]).
    pub fn raw(&mut self, which: usize, x: i32, y: i32, z: i32) -> f32 {
        self.router.sample(self.roots[which], x, y, z)
    }
}
