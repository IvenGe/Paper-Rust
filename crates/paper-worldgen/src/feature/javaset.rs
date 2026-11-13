//! A set of block positions that iterates in the order of `java.util.HashSet<BlockPos>`.
//!
//! Derived from `JavaBlockPosHashSet` in
//! `pumpkin-world/src/generation/feature/features/mod.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0; it also resizes on long bins the way
//! `HashMap.treeifyBin` does for small tables.

use super::world::Pos;

#[derive(Clone, Debug)]
pub struct JavaPosSet {
    buckets: Vec<Vec<(i32, Pos)>>,
    len: usize,
    threshold: usize,
}

impl Default for JavaPosSet {
    fn default() -> Self {
        Self::new()
    }
}

impl JavaPosSet {
    pub fn new() -> Self {
        Self { buckets: vec![Vec::new(); 16], len: 0, threshold: 12 }
    }

    #[inline]
    fn hash(p: Pos) -> i32 {
        let h = p.java_hash();
        h ^ ((h as u32) >> 16) as i32
    }

    pub fn insert(&mut self, p: Pos) -> bool {
        let h = Self::hash(p);
        let idx = (h as u32 as usize) & (self.buckets.len() - 1);
        if self.buckets[idx].iter().any(|(_, q)| *q == p) {
            return false;
        }
        self.buckets[idx].push((h, p));
        // `treeifyBin` resizes instead while the table is smaller than 64.
        if self.buckets[idx].len() >= 9 && self.buckets.len() < 64 {
            self.resize();
        }
        self.len += 1;
        if self.len > self.threshold {
            self.resize();
        }
        true
    }

    pub fn contains(&self, p: Pos) -> bool {
        let idx = (Self::hash(p) as u32 as usize) & (self.buckets.len() - 1);
        self.buckets[idx].iter().any(|(_, q)| *q == p)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Removes and returns the first element in iteration order.
    pub fn pop_first(&mut self) -> Option<Pos> {
        for b in &mut self.buckets {
            if !b.is_empty() {
                self.len -= 1;
                return Some(b.remove(0).1);
            }
        }
        None
    }

    pub fn iter(&self) -> impl Iterator<Item = Pos> + '_ {
        self.buckets.iter().flat_map(|b| b.iter().map(|(_, p)| *p))
    }

    pub fn to_vec(&self) -> Vec<Pos> {
        self.iter().collect()
    }

    fn resize(&mut self) {
        let old = self.buckets.len();
        let mut next: Vec<Vec<(i32, Pos)>> = vec![Vec::new(); old * 2];
        for (j, b) in self.buckets.drain(..).enumerate() {
            for (h, p) in b {
                if (h as u32 as usize) & old == 0 {
                    next[j].push((h, p));
                } else {
                    next[j + old].push((h, p));
                }
            }
        }
        self.buckets = next;
        self.threshold *= 2;
    }
}

impl FromIterator<Pos> for JavaPosSet {
    fn from_iter<T: IntoIterator<Item = Pos>>(iter: T) -> Self {
        let mut s = Self::new();
        for p in iter {
            s.insert(p);
        }
        s
    }
}
