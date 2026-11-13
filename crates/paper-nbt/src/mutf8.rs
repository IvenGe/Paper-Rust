//! Java "modified UTF-8", as written by `DataOutput#writeUTF`.
//!
//! Differences from standard UTF-8: U+0000 is encoded as the two bytes `C0 80`, and code
//! points above U+FFFF are encoded as a UTF-16 surrogate pair with each surrogate written as
//! its own three-byte sequence (CESU-8 style).

/// Appends the modified UTF-8 encoding of `s` to `out`.
pub fn encode(s: &str, out: &mut Vec<u8>) {
    // Fast path: plain ASCII without NUL is identical in both encodings.
    if s.bytes().all(|b| b != 0 && b < 0x80) {
        out.extend_from_slice(s.as_bytes());
        return;
    }
    for ch in s.chars() {
        let c = ch as u32;
        match c {
            0 => out.extend_from_slice(&[0xC0, 0x80]),
            0x01..=0x7F => out.push(c as u8),
            0x80..=0x7FF => out.extend_from_slice(&[0xC0 | (c >> 6) as u8, 0x80 | (c & 0x3F) as u8]),
            0x800..=0xFFFF => push3(out, c),
            _ => {
                let v = c - 0x10000;
                push3(out, 0xD800 + (v >> 10));
                push3(out, 0xDC00 + (v & 0x3FF));
            }
        }
    }
}

fn push3(out: &mut Vec<u8>, c: u32) {
    out.extend_from_slice(&[0xE0 | (c >> 12) as u8, 0x80 | ((c >> 6) & 0x3F) as u8, 0x80 | (c & 0x3F) as u8]);
}

/// Decodes modified UTF-8. Returns `None` on malformed input. Unpaired surrogates, which
/// Java strings may contain but Rust strings may not, are replaced with U+FFFD.
pub fn decode(raw: &[u8]) -> Option<String> {
    if raw.is_ascii() {
        // SAFETY-free fast path: ASCII is valid UTF-8.
        return Some(String::from_utf8_lossy(raw).into_owned());
    }
    let mut units: Vec<u16> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let b0 = raw[i];
        if b0 < 0x80 {
            units.push(b0 as u16);
            i += 1;
        } else if b0 & 0xE0 == 0xC0 {
            let b1 = *raw.get(i + 1)?;
            if b1 & 0xC0 != 0x80 {
                return None;
            }
            units.push((((b0 & 0x1F) as u16) << 6) | (b1 & 0x3F) as u16);
            i += 2;
        } else if b0 & 0xF0 == 0xE0 {
            let b1 = *raw.get(i + 1)?;
            let b2 = *raw.get(i + 2)?;
            if b1 & 0xC0 != 0x80 || b2 & 0xC0 != 0x80 {
                return None;
            }
            units.push((((b0 & 0x0F) as u16) << 12) | (((b1 & 0x3F) as u16) << 6) | (b2 & 0x3F) as u16);
            i += 3;
        } else {
            return None;
        }
    }
    Some(char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        encode(s, &mut out);
        assert_eq!(decode(&out).as_deref(), Some(s));
        out
    }

    #[test]
    fn ascii() {
        assert_eq!(round("hello"), b"hello");
    }

    #[test]
    fn nul_is_two_bytes() {
        assert_eq!(round("a\0b"), [b'a', 0xC0, 0x80, b'b']);
    }

    #[test]
    fn bmp() {
        assert_eq!(round("é"), [0xC3, 0xA9]);
        assert_eq!(round("€"), [0xE2, 0x82, 0xAC]);
    }

    #[test]
    fn supplementary_uses_surrogates() {
        // U+1F600 -> D83D DE00
        assert_eq!(round("\u{1F600}"), [0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]);
    }

    #[test]
    fn malformed() {
        assert_eq!(decode(&[0xC3]), None);
        assert_eq!(decode(&[0xF0, 0x9F, 0x98, 0x80]), None);
        assert_eq!(decode(&[0xE2, 0x28, 0xA1]), None);
    }
}
