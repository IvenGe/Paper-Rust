//! Named Binary Tag (NBT) support.
//!
//! Covers the two binary framings Minecraft uses:
//!
//! * the *file* framing (`read_named` / `write_named`): a root compound preceded by its
//!   tag type and a name, used for region files, `level.dat` and player data;
//! * the *network* framing (`read_network` / `write_network`): any root tag preceded only
//!   by its type byte, used on the wire since 1.20.2.
//!
//! Strings use Java's "modified UTF-8" ([`mutf8`]).

pub mod mutf8;
mod snbt;

use indexmap::IndexMap;
use std::fmt;

/// Tag type identifiers as they appear in the binary format.
pub mod id {
    pub const END: u8 = 0;
    pub const BYTE: u8 = 1;
    pub const SHORT: u8 = 2;
    pub const INT: u8 = 3;
    pub const LONG: u8 = 4;
    pub const FLOAT: u8 = 5;
    pub const DOUBLE: u8 = 6;
    pub const BYTE_ARRAY: u8 = 7;
    pub const STRING: u8 = 8;
    pub const LIST: u8 = 9;
    pub const COMPOUND: u8 = 10;
    pub const INT_ARRAY: u8 = 11;
    pub const LONG_ARRAY: u8 = 12;
}

/// Maximum nesting depth accepted when reading, matching vanilla.
pub const MAX_DEPTH: usize = 512;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NbtError {
    #[error("unexpected end of NBT data")]
    UnexpectedEof,
    #[error("unknown NBT tag type {0}")]
    UnknownTagType(u8),
    #[error("NBT root must be a compound, found tag type {0}")]
    RootNotCompound(u8),
    #[error("NBT nesting deeper than {MAX_DEPTH}")]
    TooDeep,
    #[error("NBT data larger than the permitted {0} bytes")]
    TooLarge(usize),
    #[error("negative length {0} in NBT data")]
    NegativeLength(i32),
    #[error("invalid modified UTF-8 in NBT string")]
    InvalidString,
    #[error("NBT list declares element type END but has {0} elements")]
    EndTypedList(i32),
}

pub type Result<T> = std::result::Result<T, NbtError>;

/// An ordered string-keyed map of tags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compound(pub IndexMap<String, Tag>);

/// A single NBT value.
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Tag>),
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    pub fn type_id(&self) -> u8 {
        match self {
            Tag::Byte(_) => id::BYTE,
            Tag::Short(_) => id::SHORT,
            Tag::Int(_) => id::INT,
            Tag::Long(_) => id::LONG,
            Tag::Float(_) => id::FLOAT,
            Tag::Double(_) => id::DOUBLE,
            Tag::ByteArray(_) => id::BYTE_ARRAY,
            Tag::String(_) => id::STRING,
            Tag::List(_) => id::LIST,
            Tag::Compound(_) => id::COMPOUND,
            Tag::IntArray(_) => id::INT_ARRAY,
            Tag::LongArray(_) => id::LONG_ARRAY,
        }
    }

    pub fn as_compound(&self) -> Option<&Compound> {
        match self {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }

    pub fn as_compound_mut(&mut self) -> Option<&mut Compound> {
        match self {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }

    pub fn into_compound(self) -> Option<Compound> {
        match self {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Tag::List(l) => Some(l),
            _ => None,
        }
    }

    /// Any integral or floating tag as an `i64` (floats truncate), like vanilla's
    /// `NumericTag#longValue`.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Tag::Byte(v) => Some(v as i64),
            Tag::Short(v) => Some(v as i64),
            Tag::Int(v) => Some(v as i64),
            Tag::Long(v) => Some(v),
            Tag::Float(v) => Some(v as i64),
            Tag::Double(v) => Some(v as i64),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().map(|v| v as i32)
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Tag::Byte(v) => Some(v as f64),
            Tag::Short(v) => Some(v as f64),
            Tag::Int(v) => Some(v as f64),
            Tag::Long(v) => Some(v as f64),
            Tag::Float(v) => Some(v as f64),
            Tag::Double(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        self.as_i64().map(|v| v != 0)
    }

    pub fn as_long_array(&self) -> Option<&[i64]> {
        match self {
            Tag::LongArray(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_int_array(&self) -> Option<&[i32]> {
        match self {
            Tag::IntArray(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_byte_array(&self) -> Option<&[u8]> {
        match self {
            Tag::ByteArray(v) => Some(v),
            _ => None,
        }
    }
}

macro_rules! tag_from {
    ($($t:ty => $v:ident),* $(,)?) => {$(
        impl From<$t> for Tag {
            fn from(v: $t) -> Tag { Tag::$v(v) }
        }
    )*};
}
tag_from!(i8 => Byte, i16 => Short, i32 => Int, i64 => Long, f32 => Float, f64 => Double,
          String => String, Compound => Compound, Vec<Tag> => List,
          Vec<i32> => IntArray, Vec<i64> => LongArray);

impl From<bool> for Tag {
    fn from(v: bool) -> Tag {
        Tag::Byte(v as i8)
    }
}

impl From<&str> for Tag {
    fn from(v: &str) -> Tag {
        Tag::String(v.to_owned())
    }
}

impl Compound {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style insert.
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Tag>) -> Self {
        self.0.insert(key.into(), value.into());
        self
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Tag>) -> Option<Tag> {
        self.0.insert(key.into(), value.into())
    }

    pub fn get(&self, key: &str) -> Option<&Tag> {
        self.0.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Tag> {
        self.0.get_mut(key)
    }

    pub fn remove(&mut self, key: &str) -> Option<Tag> {
        self.0.shift_remove(key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Tag)> {
        self.0.iter()
    }

    pub fn compound(&self, key: &str) -> Option<&Compound> {
        self.get(key).and_then(Tag::as_compound)
    }

    pub fn list(&self, key: &str) -> Option<&[Tag]> {
        self.get(key).and_then(Tag::as_list)
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Tag::as_str)
    }

    pub fn i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Tag::as_i64)
    }

    pub fn i32(&self, key: &str) -> Option<i32> {
        self.get(key).and_then(Tag::as_i32)
    }

    pub fn f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Tag::as_f64)
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Tag::as_bool)
    }

    pub fn long_array(&self, key: &str) -> Option<&[i64]> {
        self.get(key).and_then(Tag::as_long_array)
    }

    pub fn byte_array(&self, key: &str) -> Option<&[u8]> {
        self.get(key).and_then(Tag::as_byte_array)
    }
}

impl FromIterator<(String, Tag)> for Compound {
    fn from_iter<I: IntoIterator<Item = (String, Tag)>>(iter: I) -> Self {
        Compound(iter.into_iter().collect())
    }
}

/// Builds a [`Compound`]: `compound! { "key" => 1i32, "other" => "text" }`.
#[macro_export]
macro_rules! compound {
    ($($key:expr => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut c = $crate::Compound::new();
        $( c.insert($key, $value); )*
        c
    }};
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        snbt::write_tag(f, self)
    }
}

impl fmt::Display for Compound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        snbt::write_compound(f, self)
    }
}

// ---------------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------------

struct Reader<'a, 'b> {
    buf: &'b mut &'a [u8],
    /// Remaining allocation budget in bytes (vanilla's `NbtAccounter`).
    budget: usize,
    limit: usize,
}

impl<'a, 'b> Reader<'a, 'b> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.buf.len() < n {
            return Err(NbtError::UnexpectedEof);
        }
        let (head, tail) = self.buf.split_at(n);
        *self.buf = tail;
        Ok(head)
    }

    fn account(&mut self, bytes: usize) -> Result<()> {
        if bytes > self.budget {
            return Err(NbtError::TooLarge(self.limit));
        }
        self.budget -= bytes;
        Ok(())
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn len(&mut self) -> Result<usize> {
        let n = self.i32()?;
        if n < 0 {
            return Err(NbtError::NegativeLength(n));
        }
        Ok(n as usize)
    }

    fn string(&mut self) -> Result<String> {
        let n = u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as usize;
        let raw = self.take(n)?;
        self.account(n + 2)?;
        mutf8::decode(raw).ok_or(NbtError::InvalidString)
    }

    fn payload(&mut self, ty: u8, depth: usize) -> Result<Tag> {
        if depth > MAX_DEPTH {
            return Err(NbtError::TooDeep);
        }
        Ok(match ty {
            id::BYTE => Tag::Byte(self.u8()? as i8),
            id::SHORT => Tag::Short(self.i16()?),
            id::INT => Tag::Int(self.i32()?),
            id::LONG => Tag::Long(self.i64()?),
            id::FLOAT => Tag::Float(f32::from_bits(self.i32()? as u32)),
            id::DOUBLE => Tag::Double(f64::from_bits(self.i64()? as u64)),
            id::BYTE_ARRAY => {
                let n = self.len()?;
                self.account(n)?;
                Tag::ByteArray(self.take(n)?.to_vec())
            }
            id::STRING => Tag::String(self.string()?),
            id::LIST => {
                let elem = self.u8()?;
                let n = self.i32()?;
                if n <= 0 {
                    return Ok(Tag::List(Vec::new()));
                }
                if elem == id::END {
                    return Err(NbtError::EndTypedList(n));
                }
                let n = n as usize;
                // Each element costs at least a pointer-sized slot; refuse absurd counts early.
                self.account(n.saturating_mul(4))?;
                if self.buf.len() < n {
                    return Err(NbtError::UnexpectedEof);
                }
                let mut out = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    out.push(self.payload(elem, depth + 1)?);
                }
                Tag::List(out)
            }
            id::COMPOUND => {
                let mut map = IndexMap::new();
                loop {
                    let ty = self.u8()?;
                    if ty == id::END {
                        break;
                    }
                    let key = self.string()?;
                    self.account(16)?;
                    let value = self.payload(ty, depth + 1)?;
                    map.insert(key, value);
                }
                Tag::Compound(Compound(map))
            }
            id::INT_ARRAY => {
                let n = self.len()?;
                self.account(n.saturating_mul(4))?;
                let raw = self.take(n.checked_mul(4).ok_or(NbtError::UnexpectedEof)?)?;
                Tag::IntArray(raw.chunks_exact(4).map(|c| i32::from_be_bytes(c.try_into().unwrap())).collect())
            }
            id::LONG_ARRAY => {
                let n = self.len()?;
                self.account(n.saturating_mul(8))?;
                let raw = self.take(n.checked_mul(8).ok_or(NbtError::UnexpectedEof)?)?;
                Tag::LongArray(raw.chunks_exact(8).map(|c| i64::from_be_bytes(c.try_into().unwrap())).collect())
            }
            other => return Err(NbtError::UnknownTagType(other)),
        })
    }
}

/// Reads the file framing: `type(10) name payload`. Returns the root name and compound.
/// `limit` bounds the decoded size in bytes (`usize::MAX` for trusted on-disk data).
pub fn read_named(buf: &mut &[u8], limit: usize) -> Result<(String, Compound)> {
    let mut r = Reader { buf, budget: limit, limit };
    let ty = r.u8()?;
    if ty != id::COMPOUND {
        return Err(NbtError::RootNotCompound(ty));
    }
    let name = r.string()?;
    match r.payload(ty, 0)? {
        Tag::Compound(c) => Ok((name, c)),
        _ => unreachable!(),
    }
}

/// Reads the network framing: `type payload` with no root name. A lone END tag (a single
/// zero byte) decodes to `None`.
pub fn read_network(buf: &mut &[u8], limit: usize) -> Result<Option<Tag>> {
    let mut r = Reader { buf, budget: limit, limit };
    let ty = r.u8()?;
    if ty == id::END {
        return Ok(None);
    }
    r.payload(ty, 0).map(Some)
}

// ---------------------------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------------------------

fn write_string(out: &mut Vec<u8>, s: &str) {
    let start = out.len();
    out.extend_from_slice(&[0, 0]);
    mutf8::encode(s, out);
    // Strings longer than u16::MAX bytes cannot be represented; truncate at the limit the
    // same way DataOutputStream would refuse them, but without failing the whole write.
    let len = (out.len() - start - 2).min(u16::MAX as usize);
    out.truncate(start + 2 + len);
    out[start..start + 2].copy_from_slice(&(len as u16).to_be_bytes());
}

/// Element type a list is written with. Mixed lists are written as compounds, with every
/// non-compound element wrapped as `{"": element}` — the convention vanilla adopted in 1.21.5.
fn list_element_type(list: &[Tag]) -> u8 {
    match list.first() {
        None => id::END,
        Some(first) => {
            let ty = first.type_id();
            if list.iter().all(|t| t.type_id() == ty) {
                ty
            } else {
                id::COMPOUND
            }
        }
    }
}

fn write_payload(out: &mut Vec<u8>, tag: &Tag) {
    match tag {
        Tag::Byte(v) => out.push(*v as u8),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::ByteArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.extend_from_slice(v);
        }
        Tag::String(s) => write_string(out, s),
        Tag::List(list) => {
            let ty = list_element_type(list);
            out.push(ty);
            out.extend_from_slice(&(list.len() as i32).to_be_bytes());
            for item in list {
                if ty == id::COMPOUND && item.type_id() != id::COMPOUND {
                    // Heterogeneous list: wrap.
                    out.push(item.type_id());
                    write_string(out, "");
                    write_payload(out, item);
                    out.push(id::END);
                } else {
                    write_payload(out, item);
                }
            }
        }
        Tag::Compound(c) => write_compound_payload(out, c),
        Tag::IntArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.reserve(v.len() * 4);
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        Tag::LongArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.reserve(v.len() * 8);
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
    }
}

fn write_compound_payload(out: &mut Vec<u8>, c: &Compound) {
    for (key, value) in &c.0 {
        out.push(value.type_id());
        write_string(out, key);
        write_payload(out, value);
    }
    out.push(id::END);
}

/// Writes the file framing with the given root name (vanilla uses the empty string).
pub fn write_named(out: &mut Vec<u8>, name: &str, root: &Compound) {
    out.push(id::COMPOUND);
    write_string(out, name);
    write_compound_payload(out, root);
}

/// Writes the network framing for any tag.
pub fn write_network(out: &mut Vec<u8>, tag: &Tag) {
    out.push(tag.type_id());
    write_payload(out, tag);
}

/// Writes the network framing for a compound root without cloning it into a [`Tag`].
pub fn write_network_compound(out: &mut Vec<u8>, root: &Compound) {
    out.push(id::COMPOUND);
    write_compound_payload(out, root);
}

/// Writes the network encoding of "no tag" (a single END byte).
pub fn write_network_end(out: &mut Vec<u8>) {
    out.push(id::END);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Compound {
        compound! {
            "byte" => 7i8,
            "short" => -2i16,
            "int" => 123456i32,
            "long" => i64::MIN,
            "float" => 1.5f32,
            "double" => -0.25f64,
            "bytes" => Tag::ByteArray(vec![1, 2, 255]),
            "string" => "héllo \u{1F600} \u{0}",
            "list" => vec![Tag::Int(1), Tag::Int(2)],
            "empty" => Vec::<Tag>::new(),
            "nested" => compound! { "flag" => true, "ints" => vec![1i32, -1], "longs" => vec![5i64] },
        }
    }

    #[test]
    fn named_round_trip() {
        let c = sample();
        let mut out = Vec::new();
        write_named(&mut out, "", &c);
        let mut slice = &out[..];
        let (name, back) = read_named(&mut slice, usize::MAX).unwrap();
        assert_eq!(name, "");
        assert_eq!(back, c);
        assert!(slice.is_empty());
    }

    #[test]
    fn network_round_trip() {
        let c = Tag::Compound(sample());
        let mut out = Vec::new();
        write_network(&mut out, &c);
        let mut slice = &out[..];
        assert_eq!(read_network(&mut slice, usize::MAX).unwrap(), Some(c));

        let mut out = Vec::new();
        write_network(&mut out, &Tag::String("plain".into()));
        assert_eq!(out, [8, 0, 5, b'p', b'l', b'a', b'i', b'n']);
        let mut slice = &out[..];
        assert_eq!(read_network(&mut slice, 64).unwrap(), Some(Tag::String("plain".into())));

        let mut slice = &[0u8][..];
        assert_eq!(read_network(&mut slice, 64).unwrap(), None);
    }

    #[test]
    fn known_bytes() {
        // {"a": 1b} with an empty root name, as written by vanilla.
        let c = compound! { "a" => 1i8 };
        let mut out = Vec::new();
        write_named(&mut out, "", &c);
        assert_eq!(out, [10, 0, 0, 1, 0, 1, b'a', 1, 0]);
    }

    #[test]
    fn mixed_list_is_wrapped() {
        let list = Tag::List(vec![Tag::String("x".into()), Tag::Compound(compound! { "k" => 1i32 })]);
        let mut out = Vec::new();
        write_network(&mut out, &list);
        let mut slice = &out[..];
        let back = read_network(&mut slice, usize::MAX).unwrap().unwrap();
        let items = back.as_list().unwrap();
        assert_eq!(items[0], Tag::Compound(compound! { "" => "x" }));
        assert_eq!(items[1], Tag::Compound(compound! { "k" => 1i32 }));
    }

    #[test]
    fn rejects_garbage() {
        // Root compound containing one entry of an unknown tag type (99) with an empty key.
        let mut slice = &[10u8, 0, 0, 99, 0, 0][..];
        assert_eq!(read_named(&mut slice, 1024), Err(NbtError::UnknownTagType(99)));
        let mut slice = &[9u8, 0, 0, 3, 127, 255, 255, 255][..];
        assert!(read_network(&mut slice, 1024).is_err());
        // Truncated compound.
        let mut slice = &[10u8, 1, 0, 1][..];
        assert_eq!(read_network(&mut slice, 1024), Err(NbtError::UnexpectedEof));
    }

    #[test]
    fn depth_limit() {
        let mut data = vec![id::COMPOUND];
        for _ in 0..(MAX_DEPTH + 10) {
            data.extend_from_slice(&[id::COMPOUND, 0, 1, b'a']);
        }
        let mut slice = &data[..];
        assert_eq!(read_network(&mut slice, usize::MAX), Err(NbtError::TooDeep));
    }

    #[test]
    fn size_limit() {
        let c = Tag::Compound(compound! { "data" => Tag::ByteArray(vec![0; 4096]) });
        let mut out = Vec::new();
        write_network(&mut out, &c);
        let mut slice = &out[..];
        assert_eq!(read_network(&mut slice, 1024), Err(NbtError::TooLarge(1024)));
    }

    #[test]
    fn display_snbt() {
        let c = compound! { "a" => 1i8, "name" => "x y", "list" => vec![Tag::Int(1), Tag::Int(2)] };
        assert_eq!(c.to_string(), r#"{a:1b,name:"x y",list:[1,2]}"#);
    }
}
