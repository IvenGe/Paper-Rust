//! `BoundingBox`: an inclusive box of block positions.
//!
//! Written to vanilla 26.3's `BoundingBox`.

use crate::feature::world::{Dir, Pos};

/// An inclusive block box (`BoundingBox`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoundingBox {
    pub min_x: i32,
    pub min_y: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub max_z: i32,
}

impl BoundingBox {
    /// `new BoundingBox(...)`: the corners are sorted.
    pub fn new(x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32) -> Self {
        Self { min_x: x0.min(x1), min_y: y0.min(y1), min_z: z0.min(z1), max_x: x0.max(x1), max_y: y0.max(y1), max_z: z0.max(z1) }
    }

    pub fn single(p: Pos) -> Self {
        Self::new(p.x, p.y, p.z, p.x, p.y, p.z)
    }

    pub fn from_corners(a: Pos, b: Pos) -> Self {
        Self::new(a.x, a.y, a.z, b.x, b.y, b.z)
    }

    /// `BoundingBox.orientBox`.
    #[allow(clippy::too_many_arguments)]
    pub fn orient(foot_x: i32, foot_y: i32, foot_z: i32, off_x: i32, off_y: i32, off_z: i32, width: i32, height: i32, depth: i32, dir: Dir) -> Self {
        match dir {
            Dir::North => Self::new(foot_x + off_x, foot_y + off_y, foot_z - depth + 1 + off_z, foot_x + width - 1 + off_x, foot_y + height - 1 + off_y, foot_z + off_z),
            Dir::West => Self::new(foot_x - depth + 1 + off_z, foot_y + off_y, foot_z + off_x, foot_x + off_z, foot_y + height - 1 + off_y, foot_z + width - 1 + off_x),
            Dir::East => Self::new(foot_x + off_z, foot_y + off_y, foot_z + off_x, foot_x + depth - 1 + off_z, foot_y + height - 1 + off_y, foot_z + width - 1 + off_x),
            _ => Self::new(foot_x + off_x, foot_y + off_y, foot_z + off_z, foot_x + width - 1 + off_x, foot_y + height - 1 + off_y, foot_z + depth - 1 + off_z),
        }
    }

    /// `StructurePiece.makeBoundingBox`.
    pub fn make(x: i32, y: i32, z: i32, dir: Dir, width: i32, height: i32, depth: i32) -> Self {
        if matches!(dir, Dir::North | Dir::South) {
            Self::new(x, y, z, x + width - 1, y + height - 1, z + depth - 1)
        } else {
            Self::new(x, y, z, x + depth - 1, y + height - 1, z + width - 1)
        }
    }

    #[inline]
    pub fn intersects(&self, o: &BoundingBox) -> bool {
        self.max_x >= o.min_x && self.min_x <= o.max_x && self.max_z >= o.min_z && self.min_z <= o.max_z && self.max_y >= o.min_y && self.min_y <= o.max_y
    }

    /// `intersects(minX, minZ, maxX, maxZ)`: ignores y.
    #[inline]
    pub fn intersects_xz(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> bool {
        self.max_x >= min_x && self.min_x <= max_x && self.max_z >= min_z && self.min_z <= max_z
    }

    #[inline]
    pub fn contains(&self, x: i32, y: i32, z: i32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z && y >= self.min_y && y <= self.max_y
    }

    #[inline]
    pub fn is_inside(&self, p: Pos) -> bool {
        self.contains(p.x, p.y, p.z)
    }

    pub fn encapsulate(&mut self, o: &BoundingBox) {
        self.min_x = self.min_x.min(o.min_x);
        self.min_y = self.min_y.min(o.min_y);
        self.min_z = self.min_z.min(o.min_z);
        self.max_x = self.max_x.max(o.max_x);
        self.max_y = self.max_y.max(o.max_y);
        self.max_z = self.max_z.max(o.max_z);
    }

    pub fn encapsulate_pos(&mut self, p: Pos) {
        self.encapsulate(&Self::single(p));
    }

    /// `BoundingBox.encapsulatingBoxes`.
    pub fn encapsulating(boxes: impl IntoIterator<Item = BoundingBox>) -> Option<Self> {
        let mut it = boxes.into_iter();
        let mut b = it.next()?;
        for o in it {
            b.encapsulate(&o);
        }
        Some(b)
    }

    pub fn move_by(&mut self, dx: i32, dy: i32, dz: i32) {
        self.min_x += dx;
        self.min_y += dy;
        self.min_z += dz;
        self.max_x += dx;
        self.max_y += dy;
        self.max_z += dz;
    }

    pub fn moved(mut self, dx: i32, dy: i32, dz: i32) -> Self {
        self.move_by(dx, dy, dz);
        self
    }

    pub fn inflated(&self, n: i32) -> Self {
        self.inflated_xyz(n, n, n)
    }

    pub fn inflated_xyz(&self, x: i32, y: i32, z: i32) -> Self {
        Self::new(self.min_x - x, self.min_y - y, self.min_z - z, self.max_x + x, self.max_y + y, self.max_z + z)
    }

    /// `getCenter`.
    pub fn center(&self) -> Pos {
        Pos::new(self.min_x + (self.max_x - self.min_x + 1) / 2, self.min_y + (self.max_y - self.min_y + 1) / 2, self.min_z + (self.max_z - self.min_z + 1) / 2)
    }

    pub fn x_span(&self) -> i32 {
        self.max_x - self.min_x + 1
    }

    pub fn y_span(&self) -> i32 {
        self.max_y - self.min_y + 1
    }

    pub fn z_span(&self) -> i32 {
        self.max_z - self.min_z + 1
    }

    /// The box of the blocks of chunk `(cx, cz)` structures may write to (`getWritableArea`).
    pub fn writable_area(chunk_x: i32, chunk_z: i32, min_y: i32, max_y: i32) -> Self {
        Self::new(chunk_x << 4, min_y + 1, chunk_z << 4, (chunk_x << 4) + 15, max_y, (chunk_z << 4) + 15)
    }
}
