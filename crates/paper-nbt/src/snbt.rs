//! Stringified NBT output (the compact form vanilla prints in command feedback).

use crate::{Compound, Tag};
use std::fmt::{self, Write};

fn is_bare_key(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+'))
}

fn write_quoted(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    // Prefer double quotes; fall back to single quotes when that avoids escaping.
    let quote = if s.contains('"') && !s.contains('\'') { '\'' } else { '"' };
    f.write_char(quote)?;
    for ch in s.chars() {
        if ch == quote || ch == '\\' {
            f.write_char('\\')?;
        }
        f.write_char(ch)?;
    }
    f.write_char(quote)
}

pub(crate) fn write_compound(f: &mut fmt::Formatter<'_>, c: &Compound) -> fmt::Result {
    f.write_char('{')?;
    for (i, (key, value)) in c.iter().enumerate() {
        if i > 0 {
            f.write_char(',')?;
        }
        if is_bare_key(key) {
            f.write_str(key)?;
        } else {
            write_quoted(f, key)?;
        }
        f.write_char(':')?;
        write_tag(f, value)?;
    }
    f.write_char('}')
}

fn write_array<T: fmt::Display>(f: &mut fmt::Formatter<'_>, prefix: char, suffix: &str, items: impl Iterator<Item = T>) -> fmt::Result {
    write!(f, "[{prefix};")?;
    for (i, item) in items.enumerate() {
        if i > 0 {
            f.write_char(',')?;
        }
        write!(f, "{item}{suffix}")?;
    }
    f.write_char(']')
}

pub(crate) fn write_tag(f: &mut fmt::Formatter<'_>, tag: &Tag) -> fmt::Result {
    match tag {
        Tag::Byte(v) => write!(f, "{v}b"),
        Tag::Short(v) => write!(f, "{v}s"),
        Tag::Int(v) => write!(f, "{v}"),
        Tag::Long(v) => write!(f, "{v}L"),
        Tag::Float(v) => write!(f, "{v}f"),
        Tag::Double(v) => write!(f, "{v}d"),
        Tag::ByteArray(v) => write_array(f, 'B', "B", v.iter().map(|b| *b as i8)),
        Tag::String(s) => write_quoted(f, s),
        Tag::List(list) => {
            f.write_char('[')?;
            for (i, item) in list.iter().enumerate() {
                if i > 0 {
                    f.write_char(',')?;
                }
                write_tag(f, item)?;
            }
            f.write_char(']')
        }
        Tag::Compound(c) => write_compound(f, c),
        Tag::IntArray(v) => write_array(f, 'I', "", v.iter()),
        Tag::LongArray(v) => write_array(f, 'L', "L", v.iter()),
    }
}
