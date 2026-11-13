//! Primitive wire encodings shared by every packet.
//!
//! Encoding is infallible and appends to a `Vec<u8>`; decoding consumes from a [`Reader`]
//! and reports malformed input as a [`DecodeError`].

use crate::text::Component;
use paper_nbt::{NbtError, Tag};
use uuid::Uuid;

/// Limit applied to NBT received from clients, mirroring vanilla's default `NbtAccounter`.
pub const NBT_NETWORK_LIMIT: usize = 2 * 1024 * 1024;
/// Longest string the protocol permits, in UTF-16 code units.
pub const MAX_STRING: usize = 32767;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("unexpected end of packet data")]
    Eof,
    #[error("VarInt is longer than 5 bytes")]
    VarIntTooLong,
    #[error("VarLong is longer than 10 bytes")]
    VarLongTooLong,
    #[error("string of {len} bytes exceeds the limit of {max} characters")]
    StringTooLong { len: usize, max: usize },
    #[error("string is not valid UTF-8")]
    InvalidUtf8,
    #[error("invalid value {value} for {what}")]
    InvalidEnum { what: &'static str, value: i64 },
    #[error("array of {len} elements exceeds the limit of {max}")]
    ArrayTooLong { len: usize, max: usize },
    #[error("negative length {0}")]
    NegativeLength(i32),
    #[error("NBT: {0}")]
    Nbt(#[from] NbtError),
    #[error("{0} unread bytes at the end of the packet")]
    Trailing(usize),
    #[error("unknown packet id {id:#04x} in state {state}")]
    UnknownPacket { state: &'static str, id: i32 },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DecodeError>;

/// A type with a fixed wire encoding.
pub trait Encode {
    fn encode(&self, w: &mut Vec<u8>);
}

/// A type that can be read from the wire.
pub trait Decode: Sized {
    fn decode(r: &mut Reader<'_>) -> Result<Self>;
}

/// A block position (packed into 64 bits on the wire).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn pack(self) -> i64 {
        ((self.x as i64 & 0x3FF_FFFF) << 38) | ((self.z as i64 & 0x3FF_FFFF) << 12) | (self.y as i64 & 0xFFF)
    }

    pub fn unpack(v: i64) -> Self {
        Self { x: (v >> 38) as i32, y: (v << 52 >> 52) as i32, z: (v << 26 >> 38) as i32 }
    }

    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self { x: self.x + dx, y: self.y + dy, z: self.z + dz }
    }
}

/// Number of bytes `value` occupies as a VarInt.
pub const fn varint_len(value: i32) -> usize {
    let v = value as u32;
    if v < 1 << 7 {
        1
    } else if v < 1 << 14 {
        2
    } else if v < 1 << 21 {
        3
    } else if v < 1 << 28 {
        4
    } else {
        5
    }
}

/// Append-only encoders for the protocol's primitive types.
pub trait WriteExt {
    fn put_u8(&mut self, v: u8);
    fn put_slice(&mut self, v: &[u8]);

    fn put_bool(&mut self, v: bool) {
        self.put_u8(v as u8);
    }
    fn put_i8(&mut self, v: i8) {
        self.put_u8(v as u8);
    }
    fn put_u16(&mut self, v: u16) {
        self.put_slice(&v.to_be_bytes());
    }
    fn put_i16(&mut self, v: i16) {
        self.put_slice(&v.to_be_bytes());
    }
    fn put_i32(&mut self, v: i32) {
        self.put_slice(&v.to_be_bytes());
    }
    fn put_i64(&mut self, v: i64) {
        self.put_slice(&v.to_be_bytes());
    }
    fn put_u64(&mut self, v: u64) {
        self.put_slice(&v.to_be_bytes());
    }
    fn put_f32(&mut self, v: f32) {
        self.put_slice(&v.to_bits().to_be_bytes());
    }
    fn put_f64(&mut self, v: f64) {
        self.put_slice(&v.to_bits().to_be_bytes());
    }
    fn put_varint(&mut self, v: i32) {
        let mut v = v as u32;
        loop {
            if v & !0x7F == 0 {
                self.put_u8(v as u8);
                return;
            }
            self.put_u8((v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
    }
    fn put_varlong(&mut self, v: i64) {
        let mut v = v as u64;
        loop {
            if v & !0x7F == 0 {
                self.put_u8(v as u8);
                return;
            }
            self.put_u8((v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
    }
    /// VarInt byte length followed by UTF-8.
    fn put_string(&mut self, v: &str) {
        self.put_varint(v.len() as i32);
        self.put_slice(v.as_bytes());
    }
    fn put_uuid(&mut self, v: Uuid) {
        self.put_slice(v.as_bytes());
    }
    fn put_position(&mut self, v: BlockPos) {
        self.put_i64(v.pack());
    }
    /// A rotation in degrees as 1/256ths of a turn.
    fn put_angle(&mut self, degrees: f32) {
        self.put_u8((degrees.rem_euclid(360.0) * 256.0 / 360.0) as i32 as u8);
    }
    /// VarInt length followed by the raw bytes.
    fn put_byte_array(&mut self, v: &[u8]) {
        self.put_varint(v.len() as i32);
        self.put_slice(v);
    }
    /// `BitSet`: VarInt count of longs, then the longs.
    fn put_bitset(&mut self, longs: &[u64]) {
        self.put_varint(longs.len() as i32);
        for l in longs {
            self.put_u64(*l);
        }
    }
    /// Boolean presence flag followed by the value when present.
    fn put_option<T>(&mut self, v: Option<&T>, f: impl FnOnce(&mut Self, &T)) {
        match v {
            Some(v) => {
                self.put_bool(true);
                f(self, v);
            }
            None => self.put_bool(false),
        }
    }
    /// VarInt count followed by each element.
    fn put_list<T>(&mut self, items: &[T], mut f: impl FnMut(&mut Self, &T)) {
        self.put_varint(items.len() as i32);
        for item in items {
            f(self, item);
        }
    }
    /// Nameless-root NBT (the network framing).
    fn put_nbt(&mut self, tag: &Tag);
    /// A text component in its NBT form.
    fn put_component(&mut self, c: &Component) {
        self.put_nbt(&c.to_nbt());
    }
    /// Low-precision vector, see `LpVec3` in the protocol documentation.
    fn put_lp_vec3(&mut self, x: f64, y: f64, z: f64) {
        const MAX_QUANTIZED: f64 = 32766.0;
        fn sanitize(v: f64) -> f64 {
            if v.is_nan() {
                0.0
            } else {
                v.clamp(-1.7179869183e10, 1.7179869183e10)
            }
        }
        fn pack(v: f64) -> u64 {
            ((v * 0.5 + 0.5) * MAX_QUANTIZED).round() as u64
        }
        let (x, y, z) = (sanitize(x), sanitize(y), sanitize(z));
        let max = x.abs().max(y.abs()).max(z.abs());
        if max < 1.0 / MAX_QUANTIZED {
            self.put_u8(0);
            return;
        }
        let scale = max.ceil() as u64;
        let need_continuation = scale & 3 != scale;
        let packed_scale = if need_continuation { (scale & 3) | 4 } else { scale };
        let s = scale as f64;
        let packed = (pack(z / s) << 33) | (pack(y / s) << 18) | (pack(x / s) << 3) | packed_scale;
        self.put_u8(packed as u8);
        self.put_u8((packed >> 8) as u8);
        self.put_i32((packed >> 16) as u32 as i32);
        if need_continuation {
            self.put_varint((scale >> 2) as i32);
        }
    }
}

impl WriteExt for Vec<u8> {
    #[inline]
    fn put_u8(&mut self, v: u8) {
        self.push(v);
    }
    #[inline]
    fn put_slice(&mut self, v: &[u8]) {
        self.extend_from_slice(v);
    }
    fn put_nbt(&mut self, tag: &Tag) {
        paper_nbt::write_network(self, tag);
    }
}

/// A cursor over the bytes of one packet.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Fails if any bytes are left unread.
    pub fn finish(&self) -> Result<()> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::Trailing(self.buf.len()))
        }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.buf.len() < n {
            return Err(DecodeError::Eof);
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    /// The unread bytes, without consuming them.
    pub fn peek(&self) -> &'a [u8] {
        self.buf
    }

    /// Everything that is left.
    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.buf)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.i32()? as u32))
    }
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.i64()? as u64))
    }

    pub fn varint(&mut self) -> Result<i32> {
        let mut value = 0u32;
        for i in 0..5 {
            let b = self.u8()?;
            value |= ((b & 0x7F) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(DecodeError::VarIntTooLong)
    }

    pub fn varlong(&mut self) -> Result<i64> {
        let mut value = 0u64;
        for i in 0..10 {
            let b = self.u8()?;
            value |= ((b & 0x7F) as u64) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as i64);
            }
        }
        Err(DecodeError::VarLongTooLong)
    }

    /// A VarInt used as a length or count: must be non-negative and at most `max`.
    pub fn len(&mut self, max: usize) -> Result<usize> {
        let n = self.varint()?;
        if n < 0 {
            return Err(DecodeError::NegativeLength(n));
        }
        let n = n as usize;
        if n > max {
            return Err(DecodeError::ArrayTooLong { len: n, max });
        }
        Ok(n)
    }

    /// A string of at most `max` UTF-16 code units.
    pub fn string(&mut self, max: usize) -> Result<String> {
        let n = self.varint()?;
        if n < 0 {
            return Err(DecodeError::NegativeLength(n));
        }
        let n = n as usize;
        if n > max * 3 {
            return Err(DecodeError::StringTooLong { len: n, max });
        }
        let s = std::str::from_utf8(self.take(n)?).map_err(|_| DecodeError::InvalidUtf8)?;
        if s.len() > max && s.encode_utf16().count() > max {
            return Err(DecodeError::StringTooLong { len: n, max });
        }
        Ok(s.to_owned())
    }

    pub fn identifier(&mut self) -> Result<String> {
        self.string(MAX_STRING)
    }

    pub fn uuid(&mut self) -> Result<Uuid> {
        Ok(Uuid::from_bytes(self.take(16)?.try_into().unwrap()))
    }

    pub fn position(&mut self) -> Result<BlockPos> {
        Ok(BlockPos::unpack(self.i64()?))
    }

    /// A rotation in degrees decoded from 1/256ths of a turn.
    pub fn angle(&mut self) -> Result<f32> {
        Ok(self.u8()? as f32 * 360.0 / 256.0)
    }

    pub fn byte_array(&mut self, max: usize) -> Result<&'a [u8]> {
        let n = self.len(max)?;
        self.take(n)
    }

    pub fn bitset(&mut self) -> Result<Vec<u64>> {
        let n = self.len(self.remaining() / 8)?;
        (0..n).map(|_| self.u64()).collect()
    }

    pub fn option<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if self.bool()? {
            f(self).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn list<T>(&mut self, max: usize, mut f: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        // Every element needs at least one byte, which bounds the allocation by the packet size.
        let n = self.len(max.min(self.remaining()))?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(f(self)?);
        }
        Ok(out)
    }

    /// Nameless-root NBT; `None` for a lone END tag.
    pub fn nbt(&mut self) -> Result<Option<Tag>> {
        Ok(paper_nbt::read_network(&mut self.buf, NBT_NETWORK_LIMIT)?)
    }

    pub fn component(&mut self) -> Result<Component> {
        let tag = self.nbt()?.ok_or_else(|| DecodeError::Other("missing text component".into()))?;
        Ok(Component::from_nbt(&tag))
    }

    pub fn lp_vec3(&mut self) -> Result<(f64, f64, f64)> {
        fn unpack(v: u64) -> f64 {
            ((v & 32767) as f64).min(32766.0) * 2.0 / 32766.0 - 1.0
        }
        let b1 = self.u8()? as u64;
        if b1 == 0 {
            return Ok((0.0, 0.0, 0.0));
        }
        let b2 = self.u8()? as u64;
        let rest = self.i32()? as u32 as u64;
        let packed = (rest << 16) | (b2 << 8) | b1;
        let mut scale = b1 & 3;
        if b1 & 4 != 0 {
            scale |= (self.varint()? as u32 as u64) << 2;
        }
        let s = scale as f64;
        Ok((unpack(packed >> 3) * s, unpack(packed >> 18) * s, unpack(packed >> 33) * s))
    }
}

// --- Encode/Decode for plain types -------------------------------------------------------------

/// A 32-bit integer in the variable-length encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VarInt(pub i32);

/// A 64-bit integer in the variable-length encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VarLong(pub i64);

impl From<i32> for VarInt {
    fn from(v: i32) -> Self {
        VarInt(v)
    }
}

impl Encode for VarInt {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.0);
    }
}
impl Decode for VarInt {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        r.varint().map(VarInt)
    }
}
impl Encode for VarLong {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varlong(self.0);
    }
}
impl Decode for VarLong {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        r.varlong().map(VarLong)
    }
}

macro_rules! plain_codec {
    ($($t:ty => $put:ident, $get:ident);* $(;)?) => {$(
        impl Encode for $t {
            fn encode(&self, w: &mut Vec<u8>) { w.$put(*self); }
        }
        impl Decode for $t {
            fn decode(r: &mut Reader<'_>) -> Result<Self> { r.$get() }
        }
    )*};
}
plain_codec! {
    bool => put_bool, bool;
    u8 => put_u8, u8;
    i8 => put_i8, i8;
    u16 => put_u16, u16;
    i16 => put_i16, i16;
    i32 => put_i32, i32;
    i64 => put_i64, i64;
    f32 => put_f32, f32;
    f64 => put_f64, f64;
    Uuid => put_uuid, uuid;
    BlockPos => put_position, position;
}

impl Encode for String {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(self);
    }
}
impl Decode for String {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        r.string(MAX_STRING)
    }
}

impl<T: Encode> Encode for Option<T> {
    fn encode(&self, w: &mut Vec<u8>) {
        match self {
            Some(v) => {
                w.put_bool(true);
                v.encode(w);
            }
            None => w.put_bool(false),
        }
    }
}
impl<T: Decode> Decode for Option<T> {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        r.option(T::decode)
    }
}

impl<T: Encode> Encode for Vec<T> {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_varint(self.len() as i32);
        for item in self {
            item.encode(w);
        }
    }
}
impl<T: Decode> Decode for Vec<T> {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        r.list(usize::MAX, T::decode)
    }
}

impl Encode for Component {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_component(self);
    }
}
impl Decode for Component {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        r.component()
    }
}

/// Raw bytes running to the end of the packet (no length prefix).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemainingBytes(pub Vec<u8>);

impl Encode for RemainingBytes {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_slice(&self.0);
    }
}
impl Decode for RemainingBytes {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(RemainingBytes(r.rest().to_vec()))
    }
}

/// Bytes with a VarInt length prefix.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ByteArray(pub Vec<u8>);

impl Encode for ByteArray {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_byte_array(&self.0);
    }
}
impl Decode for ByteArray {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(ByteArray(r.byte_array(usize::MAX)?.to_vec()))
    }
}

/// An optional NBT tag where absence is encoded as a single END byte.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nbt(pub Option<Tag>);

impl Encode for Nbt {
    fn encode(&self, w: &mut Vec<u8>) {
        match &self.0 {
            Some(tag) => w.put_nbt(tag),
            None => paper_nbt::write_network_end(w),
        }
    }
}
impl Decode for Nbt {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        r.nbt().map(Nbt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint_bytes(v: i32) -> Vec<u8> {
        let mut out = Vec::new();
        out.put_varint(v);
        out
    }

    #[test]
    fn varint_vectors() {
        // Sample values from the protocol documentation.
        let cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (2, &[0x02]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (255, &[0xff, 0x01]),
            (25565, &[0xdd, 0xc7, 0x01]),
            (2097151, &[0xff, 0xff, 0x7f]),
            (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
            (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];
        for (value, bytes) in cases {
            assert_eq!(varint_bytes(*value), *bytes, "encode {value}");
            assert_eq!(varint_len(*value), bytes.len());
            let mut r = Reader::new(bytes);
            assert_eq!(r.varint().unwrap(), *value, "decode {value}");
            assert!(r.is_empty());
        }
        let mut r = Reader::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert!(matches!(r.varint(), Err(DecodeError::VarIntTooLong)));
    }

    #[test]
    fn varlong_vectors() {
        let cases: &[(i64, &[u8])] = &[
            (0, &[0x00]),
            (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (9223372036854775807, &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]),
            (-9223372036854775808, &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
        ];
        for (value, bytes) in cases {
            let mut out = Vec::new();
            out.put_varlong(*value);
            assert_eq!(out, *bytes);
            assert_eq!(Reader::new(bytes).varlong().unwrap(), *value);
        }
    }

    #[test]
    fn position_vector() {
        // Example from the protocol documentation.
        let v = 0b01000110000001110110001100_10110000010101101101001000_001100111111u64 as i64;
        let pos = BlockPos::unpack(v);
        assert_eq!(pos, BlockPos::new(18357644, 831, -20882616));
        assert_eq!(pos.pack(), v);
        for pos in [BlockPos::new(0, 0, 0), BlockPos::new(-1, -64, -1), BlockPos::new(30_000_000, 319, -30_000_000)] {
            assert_eq!(BlockPos::unpack(pos.pack()), pos);
        }
    }

    #[test]
    fn lp_vec3_vectors() {
        // Samples from the protocol documentation.
        let cases: &[((f64, f64, f64), &[u8])] = &[
            ((0.0, 0.0, 0.0), &[0x00]),
            ((1.0, 0.0, -1.0), &[0xF1, 0xFF, 0x00, 0x00, 0xFF, 0xFF]),
            ((10.0, 0.2, -5.0), &[0xF6, 0xFF, 0x40, 0x01, 0x05, 0x1F, 0x02]),
            ((123457.0, 15.071, 0.0), &[0xF5, 0xFF, 0x7F, 0xFF, 0x00, 0x07, 0x90, 0xF1, 0x01]),
        ];
        for ((x, y, z), bytes) in cases {
            let mut out = Vec::new();
            out.put_lp_vec3(*x, *y, *z);
            assert_eq!(out, *bytes, "encode ({x}, {y}, {z})");
            let mut r = Reader::new(bytes);
            let (dx, dy, dz) = r.lp_vec3().unwrap();
            assert!(r.is_empty());
            let tol = x.abs().max(y.abs()).max(z.abs()).max(1.0) / 16383.0;
            assert!((dx - x).abs() <= tol && (dy - y).abs() <= tol && (dz - z).abs() <= tol, "decode ({dx}, {dy}, {dz})");
        }
    }

    #[test]
    fn strings() {
        let mut out = Vec::new();
        out.put_string("héllo");
        assert_eq!(out, [6, b'h', 0xC3, 0xA9, b'l', b'l', b'o']);
        assert_eq!(Reader::new(&out).string(16).unwrap(), "héllo");
        assert!(matches!(Reader::new(&out).string(4), Err(DecodeError::StringTooLong { .. })));
        // Length prefix larger than the data.
        assert!(matches!(Reader::new(&[5, b'a']).string(16), Err(DecodeError::Eof)));
        assert!(matches!(Reader::new(&[2, 0xC3, 0x28]).string(16), Err(DecodeError::InvalidUtf8)));
    }

    #[test]
    fn list_count_is_bounded_by_packet_size() {
        // Claims 1,000,000 elements but carries none: must fail without allocating.
        let mut data = Vec::new();
        data.put_varint(1_000_000);
        assert!(Reader::new(&data).list(usize::MAX, |r| r.u8()).is_err());
    }

    #[test]
    fn angle_round_trip() {
        let mut out = Vec::new();
        out.put_angle(90.0);
        out.put_angle(-90.0);
        out.put_angle(360.0);
        assert_eq!(out, [64, 192, 0]);
    }
}
