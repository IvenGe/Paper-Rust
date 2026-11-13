//! A regular grid of block positions that density functions are evaluated over.
//!
//! Derived from `pumpkin-util/src/noise/volume.rs` and
//! `pumpkin-world/src/generation/noise/router/density_volume.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0.

/// Buffers are indexed `y + (x + z * size_x) * size_y`: Y varies fastest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DensityVolume {
    pub size_x: usize,
    pub size_y: usize,
    pub size_z: usize,
    pub min_x: i32,
    pub min_y: i32,
    pub min_z: i32,
    pub step_x: i32,
    pub step_y: i32,
    pub step_z: i32,
}

impl DensityVolume {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(size_x: usize, size_y: usize, size_z: usize, min_x: i32, min_y: i32, min_z: i32, step_x: i32, step_y: i32, step_z: i32) -> Self {
        Self { size_x, size_y, size_z, min_x, min_y, min_z, step_x, step_y, step_z }
    }

    pub const fn blocks(size_x: usize, size_y: usize, size_z: usize, min_x: i32, min_y: i32, min_z: i32) -> Self {
        Self::new(size_x, size_y, size_z, min_x, min_y, min_z, 1, 1, 1)
    }

    #[inline]
    pub const fn index(&self, x: usize, y: usize, z: usize) -> usize {
        y + (x + z * self.size_x) * self.size_y
    }

    #[inline]
    pub const fn block_x(&self, x: usize) -> i32 {
        self.min_x + x as i32 * self.step_x
    }

    #[inline]
    pub const fn block_y(&self, y: usize) -> i32 {
        self.min_y + y as i32 * self.step_y
    }

    #[inline]
    pub const fn block_z(&self, z: usize) -> i32 {
        self.min_z + z as i32 * self.step_z
    }

    pub const fn max_x(&self) -> i32 {
        self.min_x + self.size_x as i32 * self.step_x - 1
    }

    pub const fn max_y(&self) -> i32 {
        self.min_y + self.size_y as i32 * self.step_y - 1
    }

    pub const fn max_z(&self) -> i32 {
        self.min_z + self.size_z as i32 * self.step_z - 1
    }

    #[inline]
    pub const fn size(&self) -> usize {
        self.size_x * self.size_y * self.size_z
    }

    pub const fn is_block_step(&self) -> bool {
        self.step_x == 1 && self.step_y == 1 && self.step_z == 1
    }

    /// Index of a block position that lies on the grid.
    pub fn index_of(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        let rx = x - self.min_x;
        let ry = y - self.min_y;
        let rz = z - self.min_z;
        if rx < 0 || ry < 0 || rz < 0 {
            return None;
        }
        if self.is_block_step() {
            if rx < self.size_x as i32 && ry < self.size_y as i32 && rz < self.size_z as i32 {
                return Some(self.index(rx as usize, ry as usize, rz as usize));
            }
            return None;
        }
        if rx < self.size_x as i32 * self.step_x
            && ry < self.size_y as i32 * self.step_y
            && rz < self.size_z as i32 * self.step_z
            && rx % self.step_x == 0
            && ry % self.step_y == 0
            && rz % self.step_z == 0
        {
            return Some(self.index((rx / self.step_x) as usize, (ry / self.step_y) as usize, (rz / self.step_z) as usize));
        }
        None
    }

    /// Fills `buffer` by calling `sample` for every grid position, in buffer order.
    pub fn fill_with(&self, buffer: &mut [f32], mut sample: impl FnMut(i32, i32, i32) -> f32) {
        debug_assert_eq!(buffer.len(), self.size());
        let mut index = 0;
        for z in 0..self.size_z {
            let bz = self.block_z(z);
            for x in 0..self.size_x {
                let bx = self.block_x(x);
                for y in 0..self.size_y {
                    buffer[index] = sample(bx, self.block_y(y), bz);
                    index += 1;
                }
            }
        }
    }
}
