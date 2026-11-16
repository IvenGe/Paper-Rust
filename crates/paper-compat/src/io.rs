//! Rewriting a packet field by field: what is read is either copied to the new packet as it
//! is or replaced.

use paper_protocol::codec::{Reader, WriteExt};

pub type Result<T> = anyhow::Result<T>;

/// A packet being rewritten: the old body being read, the new payload being written.
pub struct Rw<'a> {
    pub r: Reader<'a>,
    pub w: Vec<u8>,
}

impl<'a> Rw<'a> {
    /// Rewrites `body` into a payload with packet id `id`.
    pub fn new(body: &'a [u8], id: i32) -> Rw<'a> {
        let mut w = Vec::with_capacity(body.len() + 8);
        w.put_varint(id);
        Rw { r: Reader::new(body), w }
    }

    /// Reads with `f` and copies the bytes it read.
    pub fn copy<T>(&mut self, f: impl FnOnce(&mut Reader<'a>) -> paper_protocol::codec::Result<T>) -> Result<T> {
        let before = self.r.peek();
        let value = f(&mut self.r)?;
        let used = before.len() - self.r.peek().len();
        self.w.extend_from_slice(&before[..used]);
        Ok(value)
    }

    pub fn varint(&mut self) -> Result<i32> {
        self.copy(|r| r.varint())
    }

    pub fn varlong(&mut self) -> Result<i64> {
        self.copy(|r| r.varlong())
    }

    pub fn bool(&mut self) -> Result<bool> {
        self.copy(|r| r.bool())
    }

    pub fn u8(&mut self) -> Result<u8> {
        self.copy(|r| r.u8())
    }

    pub fn i8(&mut self) -> Result<i8> {
        self.copy(|r| r.i8())
    }

    pub fn i16(&mut self) -> Result<i16> {
        self.copy(|r| r.i16())
    }

    pub fn i32(&mut self) -> Result<i32> {
        self.copy(|r| r.i32())
    }

    pub fn i64(&mut self) -> Result<i64> {
        self.copy(|r| r.i64())
    }

    pub fn f32(&mut self) -> Result<f32> {
        self.copy(|r| r.f32())
    }

    pub fn f64(&mut self) -> Result<f64> {
        self.copy(|r| r.f64())
    }

    pub fn string(&mut self) -> Result<String> {
        self.copy(|r| r.string(1 << 20))
    }

    pub fn uuid(&mut self) -> Result<uuid::Uuid> {
        self.copy(|r| r.uuid())
    }

    pub fn bytes(&mut self, n: usize) -> Result<()> {
        self.copy(|r| r.take(n).map(|_| ()))
    }

    /// A network NBT tag (or the end tag).
    pub fn nbt(&mut self) -> Result<()> {
        self.copy(|r| r.nbt().map(|_| ()))
    }

    /// A VarInt, replaced by `f` of it.
    pub fn map_varint(&mut self, f: impl FnOnce(i32) -> i32) -> Result<i32> {
        let v = self.r.varint()?;
        self.w.put_varint(f(v));
        Ok(v)
    }

    /// The rest of the body, as it is.
    pub fn rest(&mut self) {
        let rest = self.r.rest();
        self.w.extend_from_slice(rest);
    }

    /// The new payload, with whatever is left of the body copied.
    pub fn finish(mut self) -> Vec<u8> {
        self.rest();
        self.w
    }
}

/// The packet id of a payload and its body.
pub fn split(payload: &[u8]) -> Result<(i32, &[u8])> {
    let mut r = Reader::new(payload);
    let id = r.varint()?;
    Ok((id, r.rest()))
}

/// A payload with its packet id replaced.
pub fn with_id(id: i32, body: &[u8]) -> Vec<u8> {
    let mut w = Vec::with_capacity(body.len() + 5);
    w.put_varint(id);
    w.extend_from_slice(body);
    w
}

/// How many bytes a VarInt takes.
pub fn varint_len(v: i32) -> usize {
    let mut v = v as u32;
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}
