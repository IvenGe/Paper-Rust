//! Wire format of the separate-mode plugin bridge.
//!
//! The Rust server and the JVM plugin host exchange length-prefixed frames over a local
//! stream socket. Everything the two sides say to each other is one of five frame kinds
//! carrying dynamically typed [`Value`]s, so new API calls and events need no schema change.
//! The Java mirror of this file is `plugin-host/.../bridge/BridgeCodec.java`; the two share
//! the golden test vector in [`tests::golden_vector`].
//!
//! ```text
//! frame   := u32 length (big endian, counts the bytes after it) | u8 kind | body
//! HELLO   (1, host -> server) := value
//! WELCOME (2, server -> host) := value
//! CALL    (3, either way)     := uvarint call_id | u8 flags | string method | value args
//! RETURN  (4, either way)     := uvarint call_id | u8 status (0 ok, 1 error) | value
//! NOTIFY  (5, either way)     := string method | value args
//!
//! value   := 0                          null
//!          | 1 | 2                      false | true
//!          | 3 zigzag-varint            64-bit integer
//!          | 4 f64 (big endian)         double
//!          | 5 uvarint len, utf-8       string
//!          | 6 uvarint len, bytes       byte string
//!          | 7 uvarint n, value * n     list
//!          | 8 uvarint n, (string value) * n   map with string keys
//! string  := uvarint len, utf-8
//! ```
//!
//! Call ids are allocated by the caller; a RETURN travelling in one direction always answers
//! a CALL that travelled in the other, so the two id spaces never collide.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};

/// Version of the bridge protocol, exchanged in HELLO/WELCOME. Bump on incompatible change.
pub const PROTOCOL_VERSION: i64 = 1;

/// Largest frame either side will accept.
pub const MAX_FRAME_LEN: usize = 64 * 1024 * 1024;

/// CALL flag: the caller holds the main-thread baton. The callee must run the call on its
/// own main thread, re-entrantly if that thread is itself waiting on a call.
pub const FLAG_SYNC: u8 = 1;

/// A dynamically typed bridge value.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().and_then(|v| i32::try_from(v).ok())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Looks up `key` if this is a map. Missing keys and non-maps both yield `Null`.
    pub fn get(&self, key: &str) -> &Value {
        const NULL: Value = Value::Null;
        match self {
            Value::Map(m) => m.get(key).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    /// Element `index` if this is a list, `Null` otherwise.
    pub fn at(&self, index: usize) -> &Value {
        const NULL: Value = Value::Null;
        match self {
            Value::List(l) => l.get(index).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).as_str()
    }
    pub fn i64(&self, key: &str) -> Option<i64> {
        self.get(key).as_i64()
    }
    pub fn i32(&self, key: &str) -> Option<i32> {
        self.get(key).as_i32()
    }
    pub fn f64(&self, key: &str) -> Option<f64> {
        self.get(key).as_f64()
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).as_bool()
    }
    pub fn list(&self, key: &str) -> Option<&[Value]> {
        self.get(key).as_list()
    }

    /// Inserts into a map value (turning `Null` into an empty map first). Builder style.
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Value {
        self.set(key, value);
        self
    }

    /// Inserts into a map value (turning `Null` into an empty map first).
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        if !matches!(self, Value::Map(_)) {
            *self = Value::Map(BTreeMap::new());
        }
        if let Value::Map(m) = self {
            m.insert(key.to_owned(), value.into());
        }
    }

    /// An empty map, ready for [`Value::with`].
    pub fn map() -> Value {
        Value::Map(BTreeMap::new())
    }
}

macro_rules! value_from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Value { Value::Int(v as i64) }
        }
    )*};
}
value_from_int!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);

impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Value {
        Value::Float(v as f64)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Float(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::Str(v.to_owned())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::Str(v)
    }
}
impl From<&String> for Value {
    fn from(v: &String) -> Value {
        Value::Str(v.clone())
    }
}
impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Value {
        Value::Bytes(v)
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Value {
        Value::List(v)
    }
}
impl From<Vec<String>> for Value {
    fn from(v: Vec<String>) -> Value {
        Value::List(v.into_iter().map(Value::Str).collect())
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Value {
        v.map_or(Value::Null, Into::into)
    }
}

/// Builds a map value: `bmap! { "name" => "Steve", "x" => 1.5 }`.
#[macro_export]
macro_rules! bmap {
    ($($key:expr => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut m = ::std::collections::BTreeMap::<String, $crate::Value>::new();
        $( m.insert(($key).to_string(), $crate::Value::from($value)); )*
        $crate::Value::Map(m)
    }};
}

/// Builds a list value: `blist![1, "two", 3.0]`.
#[macro_export]
macro_rules! blist {
    ($($value:expr),* $(,)?) => {
        $crate::Value::List(vec![$($crate::Value::from($value)),*])
    };
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x:?}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Bytes(b) => write!(f, "<{} bytes>", b.len()),
            Value::List(l) => {
                f.write_str("[")?;
                for (i, v) in l.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str("]")
            }
            Value::Map(m) => {
                f.write_str("{")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                f.write_str("}")
            }
        }
    }
}

/// One message on the bridge.
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Hello(Value),
    Welcome(Value),
    Call { id: u64, flags: u8, method: String, args: Value },
    Return { id: u64, ok: bool, value: Value },
    Notify { method: String, args: Value },
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("bridge i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("malformed bridge frame: {0}")]
    Malformed(&'static str),
    #[error("bridge frame of {0} bytes exceeds the limit")]
    TooLarge(usize),
}

mod kind {
    pub const HELLO: u8 = 1;
    pub const WELCOME: u8 = 2;
    pub const CALL: u8 = 3;
    pub const RETURN: u8 = 4;
    pub const NOTIFY: u8 = 5;
}

mod tag {
    pub const NULL: u8 = 0;
    pub const FALSE: u8 = 1;
    pub const TRUE: u8 = 2;
    pub const INT: u8 = 3;
    pub const FLOAT: u8 = 4;
    pub const STR: u8 = 5;
    pub const BYTES: u8 = 6;
    pub const LIST: u8 = 7;
    pub const MAP: u8 = 8;
}

fn put_uvarint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_uvarint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

/// Appends the encoding of `value`.
pub fn encode_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => out.push(tag::NULL),
        Value::Bool(false) => out.push(tag::FALSE),
        Value::Bool(true) => out.push(tag::TRUE),
        Value::Int(i) => {
            out.push(tag::INT);
            put_uvarint(out, ((i << 1) ^ (i >> 63)) as u64);
        }
        Value::Float(f) => {
            out.push(tag::FLOAT);
            out.extend_from_slice(&f.to_bits().to_be_bytes());
        }
        Value::Str(s) => {
            out.push(tag::STR);
            put_str(out, s);
        }
        Value::Bytes(b) => {
            out.push(tag::BYTES);
            put_uvarint(out, b.len() as u64);
            out.extend_from_slice(b);
        }
        Value::List(l) => {
            out.push(tag::LIST);
            put_uvarint(out, l.len() as u64);
            for v in l {
                encode_value(out, v);
            }
        }
        Value::Map(m) => {
            out.push(tag::MAP);
            put_uvarint(out, m.len() as u64);
            for (k, v) in m {
                put_str(out, k);
                encode_value(out, v);
            }
        }
    }
}

struct Cursor<'a> {
    buf: &'a [u8],
    depth: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], BridgeError> {
        if self.buf.len() < n {
            return Err(BridgeError::Malformed("truncated"));
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, BridgeError> {
        Ok(self.take(1)?[0])
    }

    fn uvarint(&mut self) -> Result<u64, BridgeError> {
        let mut v = 0u64;
        for shift in (0..70).step_by(7) {
            let b = self.u8()?;
            v |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(BridgeError::Malformed("varint too long"))
    }

    /// A length that must fit in the bytes that are left (each element is at least a byte).
    fn len(&mut self) -> Result<usize, BridgeError> {
        let n = self.uvarint()?;
        if n > self.buf.len() as u64 {
            return Err(BridgeError::Malformed("length exceeds frame"));
        }
        Ok(n as usize)
    }

    fn str(&mut self) -> Result<String, BridgeError> {
        let n = self.len()?;
        std::str::from_utf8(self.take(n)?).map(str::to_owned).map_err(|_| BridgeError::Malformed("invalid utf-8"))
    }

    fn value(&mut self) -> Result<Value, BridgeError> {
        if self.depth > 128 {
            return Err(BridgeError::Malformed("value nested too deeply"));
        }
        Ok(match self.u8()? {
            tag::NULL => Value::Null,
            tag::FALSE => Value::Bool(false),
            tag::TRUE => Value::Bool(true),
            tag::INT => {
                let z = self.uvarint()?;
                Value::Int(((z >> 1) as i64) ^ -((z & 1) as i64))
            }
            tag::FLOAT => Value::Float(f64::from_bits(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))),
            tag::STR => Value::Str(self.str()?),
            tag::BYTES => {
                let n = self.len()?;
                Value::Bytes(self.take(n)?.to_vec())
            }
            tag::LIST => {
                let n = self.len()?;
                self.depth += 1;
                let mut out = Vec::with_capacity(n);
                for _ in 0..n {
                    out.push(self.value()?);
                }
                self.depth -= 1;
                Value::List(out)
            }
            tag::MAP => {
                let n = self.len()?;
                self.depth += 1;
                let mut out = BTreeMap::new();
                for _ in 0..n {
                    let k = self.str()?;
                    out.insert(k, self.value()?);
                }
                self.depth -= 1;
                Value::Map(out)
            }
            _ => return Err(BridgeError::Malformed("unknown value tag")),
        })
    }
}

/// Decodes one value and requires that it uses up all of `buf`.
pub fn decode_value(buf: &[u8]) -> Result<Value, BridgeError> {
    let mut c = Cursor { buf, depth: 0 };
    let v = c.value()?;
    if !c.buf.is_empty() {
        return Err(BridgeError::Malformed("trailing bytes"));
    }
    Ok(v)
}

/// Appends the complete frame, including its length prefix.
pub fn encode_frame(out: &mut Vec<u8>, frame: &Frame) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    match frame {
        Frame::Hello(v) => {
            out.push(kind::HELLO);
            encode_value(out, v);
        }
        Frame::Welcome(v) => {
            out.push(kind::WELCOME);
            encode_value(out, v);
        }
        Frame::Call { id, flags, method, args } => {
            out.push(kind::CALL);
            put_uvarint(out, *id);
            out.push(*flags);
            put_str(out, method);
            encode_value(out, args);
        }
        Frame::Return { id, ok, value } => {
            out.push(kind::RETURN);
            put_uvarint(out, *id);
            out.push(!*ok as u8);
            encode_value(out, value);
        }
        Frame::Notify { method, args } => {
            out.push(kind::NOTIFY);
            put_str(out, method);
            encode_value(out, args);
        }
    }
    let len = (out.len() - start - 4) as u32;
    out[start..start + 4].copy_from_slice(&len.to_be_bytes());
}

/// Decodes a frame body (the bytes after the length prefix).
pub fn decode_frame(body: &[u8]) -> Result<Frame, BridgeError> {
    let mut c = Cursor { buf: body, depth: 0 };
    let frame = match c.u8()? {
        kind::HELLO => Frame::Hello(c.value()?),
        kind::WELCOME => Frame::Welcome(c.value()?),
        kind::CALL => {
            let id = c.uvarint()?;
            let flags = c.u8()?;
            let method = c.str()?;
            Frame::Call { id, flags, method, args: c.value()? }
        }
        kind::RETURN => {
            let id = c.uvarint()?;
            let status = c.u8()?;
            Frame::Return { id, ok: status == 0, value: c.value()? }
        }
        kind::NOTIFY => {
            let method = c.str()?;
            Frame::Notify { method, args: c.value()? }
        }
        _ => return Err(BridgeError::Malformed("unknown frame kind")),
    };
    if !c.buf.is_empty() {
        return Err(BridgeError::Malformed("trailing bytes"));
    }
    Ok(frame)
}

/// Blocking read of one frame. Returns `Ok(None)` on a clean end of stream.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Frame>, BridgeError> {
    let mut header = [0u8; 4];
    let mut filled = 0;
    while filled < 4 {
        match reader.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(BridgeError::Malformed("stream ended inside a frame header")),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_LEN {
        return Err(BridgeError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    decode_frame(&body).map(Some)
}

/// Blocking write of one frame (a single `write_all`, so frames from different threads do
/// not interleave as long as callers serialise access to the writer).
pub fn write_frame(writer: &mut impl Write, frame: &Frame) -> Result<(), BridgeError> {
    let mut buf = Vec::with_capacity(64);
    encode_frame(&mut buf, frame);
    writer.write_all(&buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(frame: Frame) {
        let mut buf = Vec::new();
        encode_frame(&mut buf, &frame);
        let len = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
        assert_eq!(len, buf.len() - 4);
        assert_eq!(decode_frame(&buf[4..]).unwrap(), frame);
        let mut cursor = io::Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor).unwrap(), Some(frame));
        assert_eq!(read_frame(&mut cursor).unwrap(), None);
    }

    fn sample() -> Value {
        bmap! {
            "null" => Value::Null,
            "yes" => true,
            "no" => false,
            "int" => -1234567890123i64,
            "small" => 5,
            "float" => 1.5,
            "text" => "héllo \u{1F600}",
            "bytes" => vec![0u8, 1, 255],
            "list" => blist![1, "two", 3.0, Value::Null],
            "nested" => bmap! { "k" => blist![] },
        }
    }

    #[test]
    fn frames_round_trip() {
        round(Frame::Hello(sample()));
        round(Frame::Welcome(Value::Null));
        round(Frame::Call { id: 0, flags: FLAG_SYNC, method: "event.fire".into(), args: sample() });
        round(Frame::Call { id: u64::MAX, flags: 0, method: String::new(), args: Value::Null });
        round(Frame::Return { id: 300, ok: true, value: blist![1, 2, 3] });
        round(Frame::Return { id: 7, ok: false, value: "boom".into() });
        round(Frame::Notify { method: "log".into(), args: sample() });
    }

    #[test]
    fn integers_zigzag() {
        for v in [0i64, 1, -1, 63, -64, 64, i64::MAX, i64::MIN] {
            let mut buf = Vec::new();
            encode_value(&mut buf, &Value::Int(v));
            assert_eq!(decode_value(&buf).unwrap(), Value::Int(v));
        }
        let mut buf = Vec::new();
        encode_value(&mut buf, &Value::Int(-1));
        assert_eq!(buf, [3, 1]);
        buf.clear();
        encode_value(&mut buf, &Value::Int(1));
        assert_eq!(buf, [3, 2]);
    }

    /// The same bytes are asserted by the Java codec test, pinning both implementations.
    #[test]
    fn golden_vector() {
        let frame = Frame::Call {
            id: 300,
            flags: FLAG_SYNC,
            method: "player.sendMessage".into(),
            args: bmap! { "uuid" => "00000000-0000-0000-0000-000000000001", "n" => -3, "ok" => true, "xs" => blist![1.5, Value::Null] },
        };
        let mut buf = Vec::new();
        encode_frame(&mut buf, &frame);
        let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            concat!(
                "0000005b",                                 // length (91 bytes follow)
                "03",                                       // CALL
                "ac02",                                     // id 300
                "01",                                       // flags
                "12", "706c617965722e73656e644d657373616765", // "player.sendMessage"
                "08", "04",                                 // map, 4 entries (sorted by key)
                "01", "6e", "03", "05",                     // "n" => -3
                "02", "6f6b", "02",                         // "ok" => true
                "04", "75756964", "05", "24",
                "30303030303030302d303030302d303030302d303030302d303030303030303030303031",
                "02", "7873", "07", "02", "04", "3ff8000000000000", "00"
            )
        );
        assert_eq!(decode_frame(&buf[4..]).unwrap(), frame);
    }

    #[test]
    fn rejects_malformed() {
        assert!(decode_frame(&[]).is_err());
        assert!(decode_frame(&[9, 0]).is_err());
        assert!(decode_frame(&[1, 0, 0]).is_err()); // trailing byte
        assert!(decode_value(&[5, 5, b'a']).is_err()); // string longer than data
        assert!(decode_value(&[7, 0xFF, 0xFF, 0xFF, 0x7F]).is_err()); // absurd list count
        assert!(decode_value(&[99]).is_err());
        // Deep nesting is refused rather than overflowing the stack.
        let mut deep = vec![7u8, 1].repeat(1000);
        deep.push(0);
        assert!(decode_value(&deep).is_err());
        // Oversized frame header.
        let mut cursor = io::Cursor::new(vec![0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(matches!(read_frame(&mut cursor), Err(BridgeError::TooLarge(_))));
        // Stream cut inside a frame.
        let mut cursor = io::Cursor::new(vec![0, 0, 0, 5, 1]);
        assert!(read_frame(&mut cursor).is_err());
    }

    #[test]
    fn accessors() {
        let v = sample();
        assert_eq!(v.str("text"), Some("héllo \u{1F600}"));
        assert_eq!(v.i64("int"), Some(-1234567890123));
        assert_eq!(v.f64("small"), Some(5.0));
        assert_eq!(v.bool("yes"), Some(true));
        assert_eq!(v.get("list").at(1).as_str(), Some("two"));
        assert!(v.get("missing").is_null());
        assert!(v.get("nested").get("k").as_list().unwrap().is_empty());
        let built = Value::map().with("a", 1).with("b", "x");
        assert_eq!(built, bmap! { "a" => 1, "b" => "x" });
        assert_eq!(Value::from(Some(3)), Value::Int(3));
        assert_eq!(Value::from(None::<i32>), Value::Null);
    }
}
