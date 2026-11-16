//! Chunk sections as 26.1 to 26.3 encode them: paletted containers of block states and
//! biomes, with no length before their long arrays.

use crate::io::Result;
use paper_protocol::codec::{Reader, WriteExt};

/// The fewest bits that tell `count` values apart.
pub fn ceil_log2(count: usize) -> u8 {
    if count <= 1 {
        0
    } else {
        (usize::BITS - (count - 1).leading_zeros()) as u8
    }
}

fn longs_for(entries: usize, bits: u8) -> usize {
    let per_long = 64 / bits as usize;
    entries.div_ceil(per_long)
}

/// What a paletted container holds.
pub enum Palette {
    Single(u32),
    Indirect { bits: u8, palette: Vec<u32>, data: Vec<u64> },
    Direct { values: Vec<u32> },
}

/// Reads a paletted container of `entries` values. `max_indirect` is the largest palette bit
/// count (8 for blocks, 3 for biomes), `min_indirect` the smallest the client uses (4 for
/// blocks, 1 for biomes), `direct` the bits of a direct container.
pub fn read_palette(r: &mut Reader<'_>, entries: usize, min_indirect: u8, max_indirect: u8, direct: u8) -> Result<Palette> {
    let bits = r.u8()?;
    if bits == 0 {
        return Ok(Palette::Single(r.varint()? as u32));
    }
    if bits <= max_indirect {
        let bits = bits.max(min_indirect);
        let palette = r.list(1 << max_indirect, |r| Ok(r.varint()? as u32))?;
        let mut data = Vec::with_capacity(longs_for(entries, bits));
        for _ in 0..longs_for(entries, bits) {
            data.push(r.u64()?);
        }
        return Ok(Palette::Indirect { bits, palette, data });
    }
    let per_long = 64 / direct as usize;
    let mask = (1u64 << direct) - 1;
    let mut values = Vec::with_capacity(entries);
    for _ in 0..longs_for(entries, direct) {
        let long = r.u64()?;
        for k in 0..per_long {
            if values.len() < entries {
                values.push(((long >> (k * direct as usize)) & mask) as u32);
            }
        }
    }
    Ok(Palette::Direct { values })
}

/// Writes a paletted container (a direct one with `direct` bits).
pub fn write_palette(w: &mut Vec<u8>, palette: &Palette, direct: u8) {
    match palette {
        Palette::Single(v) => {
            w.put_u8(0);
            w.put_varint(*v as i32);
        }
        Palette::Indirect { bits, palette, data } => {
            w.put_u8(*bits);
            w.put_varint(palette.len() as i32);
            for v in palette {
                w.put_varint(*v as i32);
            }
            for long in data {
                w.put_u64(*long);
            }
        }
        Palette::Direct { values } => {
            w.put_u8(direct);
            let per_long = 64 / direct as usize;
            for chunk in values.chunks(per_long) {
                let mut long = 0u64;
                for (k, v) in chunk.iter().enumerate() {
                    long |= (*v as u64) << (k * direct as usize);
                }
                w.put_u64(long);
            }
        }
    }
}

impl Palette {
    /// Replaces every value by `f` of it.
    pub fn map(&mut self, f: impl Fn(u32) -> u32) {
        match self {
            Palette::Single(v) => *v = f(*v),
            Palette::Indirect { palette, .. } => palette.iter_mut().for_each(|v| *v = f(*v)),
            Palette::Direct { values } => values.iter_mut().for_each(|v| *v = f(*v)),
        }
    }

    /// The value at `index`.
    pub fn get(&self, index: usize) -> u32 {
        match self {
            Palette::Single(v) => *v,
            Palette::Indirect { bits, palette, data } => {
                let per_long = 64 / *bits as usize;
                let at = (data[index / per_long] >> ((index % per_long) * *bits as usize)) & ((1u64 << bits) - 1);
                palette.get(at as usize).copied().unwrap_or(0)
            }
            Palette::Direct { values } => values.get(index).copied().unwrap_or(0),
        }
    }

    /// The indices (0..4096, y-z-x order) whose value passes `test`.
    pub fn find(&self, test: impl Fn(u32) -> bool) -> Vec<usize> {
        match self {
            Palette::Single(v) if test(*v) => (0..4096).collect(),
            Palette::Single(_) => Vec::new(),
            Palette::Indirect { palette, .. } => {
                if !palette.iter().any(|v| test(*v)) {
                    return Vec::new();
                }
                (0..4096).filter(|i| test(self.get(*i))).collect()
            }
            Palette::Direct { values } => (0..4096).filter(|i| test(values[*i])).collect(),
        }
    }
}
