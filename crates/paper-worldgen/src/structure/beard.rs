//! The terrain adaptation around structures (`Beardifier`): a density added to the
//! terrain's final density near the pieces of structures that bury themselves, sit on a
//! beard of terrain or are encapsulated.
//!
//! Derived from `pumpkin-world/src/generation/noise/router/density_function/beardifier.rs`
//! of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, checked against vanilla
//! 26.3's `Beardifier` (float arithmetic, kernel, affected box, junction filter).

use super::{BoundingBox, Junction, StructureStart, TerrainAdjustment};
use crate::noise::DensityVolume;
use std::sync::{Arc, OnceLock};

const KERNEL_RADIUS: i32 = 12;
const KERNEL_SIZE: i32 = 24;

fn kernel() -> &'static [f32] {
    static K: OnceLock<Vec<f32>> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = vec![0.0f32; (KERNEL_SIZE * KERNEL_SIZE * KERNEL_SIZE) as usize];
        for zi in 0..KERNEL_SIZE {
            for xi in 0..KERNEL_SIZE {
                for yi in 0..KERNEL_SIZE {
                    let (dx, dy, dz) = ((xi - KERNEL_RADIUS) as f64, (yi - KERNEL_RADIUS) as f64 + 0.5, (zi - KERNEL_RADIUS) as f64);
                    let d2 = dx * dx + dy * dy + dz * dz;
                    k[(zi * KERNEL_SIZE * KERNEL_SIZE + xi * KERNEL_SIZE + yi) as usize] = std::f64::consts::E.powf(-d2 / 16.0) as f32;
                }
            }
        }
        k
    })
}

/// `Mth.fastInvSqrt`.
fn fast_inv_sqrt(x: f64) -> f64 {
    let half = 0.5 * x;
    let i = 6_910_469_410_427_058_090_i64.wrapping_sub((x.to_bits() as i64) >> 1);
    let y = f64::from_bits(i as u64);
    y * (1.5 - half * y * y)
}

fn bury_contribution(dx: f32, dy: f32, dz: f32) -> f32 {
    let d2 = dx * dx + dy * dy + dz * dz;
    if d2 >= 36.0 {
        0.0
    } else {
        1.0 - d2.sqrt() / 6.0
    }
}

fn beard_contribution(dx: i32, dy: i32, dz: i32, y_to_ground: i32) -> f32 {
    let (xi, yi, zi) = (dx + KERNEL_RADIUS, dy + KERNEL_RADIUS, dz + KERNEL_RADIUS);
    if !(0..KERNEL_SIZE).contains(&xi) || !(0..KERNEL_SIZE).contains(&yi) || !(0..KERNEL_SIZE).contains(&zi) {
        return 0.0;
    }
    let dyo = y_to_ground as f32 + 0.5;
    let d2 = (dx as f32) * (dx as f32) + dyo * dyo + (dz as f32) * (dz as f32);
    let value = -dyo * (fast_inv_sqrt((d2 / 2.0) as f64) as f32) / 2.0;
    value * kernel()[(zi * KERNEL_SIZE * KERNEL_SIZE + xi * KERNEL_SIZE + yi) as usize]
}

/// `Beardifier.Rigid`.
#[derive(Clone, Copy, Debug)]
struct Rigid {
    bbox: BoundingBox,
    adjustment: TerrainAdjustment,
    ground_level_delta: i32,
}

/// The beardifier of one chunk.
#[derive(Clone, Debug)]
pub struct Beardifier {
    rigids: Vec<Rigid>,
    junctions: Vec<Junction>,
    affected: BoundingBox,
}

impl Beardifier {
    /// `forStructuresInChunk` over the chunk's terrain adapting structure starts.
    pub fn for_chunk(cx: i32, cz: i32, starts: &[(TerrainAdjustment, Arc<StructureStart>)]) -> Option<Self> {
        let (min_x, min_z) = (cx << 4, cz << 4);
        let mut rigids = Vec::new();
        let mut junctions = Vec::new();
        let mut any: Option<BoundingBox> = None;
        let include = |b: BoundingBox, any: &mut Option<BoundingBox>| match any {
            Some(a) => a.encapsulate(&b),
            None => *any = Some(b),
        };
        for (adjustment, start) in starts {
            for piece in &start.pieces {
                let base = piece.base();
                let bbox = piece.bbox();
                if !bbox.intersects_xz(min_x - 12, min_z - 12, min_x + 15 + 12, min_z + 15 + 12) {
                    continue;
                }
                let _ = base;
                match piece.jigsaw() {
                    Some(j) => {
                        if j.rigid {
                            rigids.push(Rigid { bbox, adjustment: *adjustment, ground_level_delta: j.ground_level_delta });
                            include(bbox, &mut any);
                        }
                        for junction in &j.junctions {
                            let (jx, jz) = (junction.source_x, junction.source_z);
                            if jx > min_x - 12 && jz > min_z - 12 && jx < min_x + 15 + 12 && jz < min_z + 15 + 12 {
                                junctions.push(*junction);
                                include(BoundingBox::new(jx, junction.source_ground_y, jz, jx, junction.source_ground_y, jz), &mut any);
                            }
                        }
                    }
                    None => {
                        rigids.push(Rigid { bbox, adjustment: *adjustment, ground_level_delta: 0 });
                        include(bbox, &mut any);
                    }
                }
            }
        }
        let affected = any?.inflated(24);
        Some(Self { rigids, junctions, affected })
    }

    /// `sampleValueUnchecked`.
    fn value(&self, x: i32, y: i32, z: i32) -> f32 {
        let mut v = 0.0f32;
        for r in &self.rigids {
            let b = &r.bbox;
            let dx = 0.max((b.min_x - x).max(x - b.max_x));
            let dz = 0.max((b.min_z - z).max(z - b.max_z));
            let ground_y = b.min_y + r.ground_level_delta;
            let dy_to_ground = y - ground_y;
            let dy = match r.adjustment {
                TerrainAdjustment::None => 0,
                TerrainAdjustment::Bury | TerrainAdjustment::BeardThin => dy_to_ground,
                TerrainAdjustment::BeardBox => 0.max((ground_y - y).max(y - b.max_y)),
                TerrainAdjustment::Encapsulate => 0.max((b.min_y - y).max(y - b.max_y)),
            };
            v += match r.adjustment {
                TerrainAdjustment::None => 0.0,
                TerrainAdjustment::Bury => bury_contribution(dx as f32, dy as f32 / 2.0, dz as f32),
                TerrainAdjustment::BeardThin | TerrainAdjustment::BeardBox => beard_contribution(dx, dy, dz, dy_to_ground) * 0.8,
                TerrainAdjustment::Encapsulate => bury_contribution(dx as f32 / 2.0, dy as f32 / 2.0, dz as f32 / 2.0) * 0.8,
            };
        }
        for j in &self.junctions {
            let (dx, dy, dz) = (x - j.source_x, y - j.source_ground_y, z - j.source_z);
            v += beard_contribution(dx, dy, dz, dy) * 0.4;
        }
        v
    }

    /// `sampleValue`.
    pub fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
        if !self.affected.contains(x, y, z) {
            return 0.0;
        }
        self.value(x, y, z)
    }

    /// `sampleVolume`.
    pub fn sample_volume(&self, buffer: &mut [f32], volume: &DensityVolume) {
        buffer.fill(0.0);
        let a = &self.affected;
        if a.min_x > volume.max_x() || a.max_x < volume.min_x || a.min_y > volume.max_y() || a.max_y < volume.min_y || a.min_z > volume.max_z() || a.max_z < volume.min_z {
            return;
        }
        let min_x = 0.max(a.min_x - volume.min_x).div_euclid(volume.step_x);
        let min_y = 0.max(a.min_y - volume.min_y).div_euclid(volume.step_y);
        let min_z = 0.max(a.min_z - volume.min_z).div_euclid(volume.step_z);
        let max_x = (volume.size_x as i32 - 1).min((a.max_x - volume.min_x).div_euclid(volume.step_x));
        let max_y = (volume.size_y as i32 - 1).min((a.max_y - volume.min_y).div_euclid(volume.step_y));
        let max_z = (volume.size_z as i32 - 1).min((a.max_z - volume.min_z).div_euclid(volume.step_z));
        for z in min_z..=max_z {
            let bz = volume.block_z(z as usize);
            for x in min_x..=max_x {
                let bx = volume.block_x(x as usize);
                for y in min_y..=max_y {
                    buffer[volume.index(x as usize, y as usize, z as usize)] = self.value(bx, volume.block_y(y as usize), bz);
                }
            }
        }
    }
}
