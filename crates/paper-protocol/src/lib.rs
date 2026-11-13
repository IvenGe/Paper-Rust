//! Minecraft Java Edition wire protocol, version 777 (Minecraft 26.3).
//!
//! * [`codec`] — primitive encodings (`VarInt`, strings, positions, NBT, ...)
//! * [`frame`] — length framing, zlib compression and AES/CFB8 encryption
//! * [`text`] — text components
//! * [`crypto`] — the CFB8 stream cipher
//! * [`types`] — composite wire types (profiles, item stacks, light data)
//! * [`packets`] — the packets of every connection state

pub mod codec;
pub mod crypto;
pub mod frame;
pub mod packets;
pub mod text;
pub mod types;

pub use codec::{BlockPos, Decode, DecodeError, Encode, Reader, VarInt, VarLong, WriteExt};
pub use text::Component;
