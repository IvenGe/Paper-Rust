//! Packet framing: length prefixes, zlib compression and encryption.
//!
//! A *payload* is `VarInt packet id` followed by the packet body. On the wire it becomes
//!
//! ```text
//! without compression:  VarInt len | payload
//! with compression:     VarInt len | VarInt uncompressed_len (0 = stored) | payload or zlib(payload)
//! ```
//!
//! and, once encryption is enabled, the whole byte stream is passed through AES/CFB8.

use crate::codec::{varint_len, WriteExt};
use crate::crypto::Cfb8;
use bytes::{Buf, BytesMut};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};

/// Largest frame that fits a 3-byte length prefix.
pub const MAX_FRAME_LEN: usize = 2_097_151;
/// Largest uncompressed payload the vanilla server accepts from a client.
pub const MAX_UNCOMPRESSED_LEN: usize = 8_388_608;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("packet length prefix is wider than 21 bits")]
    LengthTooLong,
    #[error("empty packet")]
    Empty,
    #[error("packet of {0} bytes is larger than the protocol maximum of {MAX_FRAME_LEN}")]
    TooLarge(usize),
    #[error("badly compressed packet - size of {0} is below server threshold of {1}")]
    BelowThreshold(usize, u32),
    #[error("badly compressed packet - size of {0} is larger than protocol maximum of {MAX_UNCOMPRESSED_LEN}")]
    UncompressedTooLarge(usize),
    #[error("badly compressed packet - declared {declared} bytes but the stream holds {actual}")]
    SizeMismatch { declared: usize, actual: usize },
    #[error("badly compressed packet - {0}")]
    Zlib(String),
}

/// Reads a VarInt of at most 3 bytes from the front of `buf` without consuming it.
/// Returns `(value, encoded length)`, or `None` if more bytes are needed.
fn peek_frame_len(buf: &[u8]) -> Result<Option<(usize, usize)>, FrameError> {
    let mut value = 0usize;
    for i in 0..3 {
        let Some(&b) = buf.get(i) else { return Ok(None) };
        value |= ((b & 0x7F) as usize) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(Some((value, i + 1)));
        }
    }
    Err(FrameError::LengthTooLong)
}

/// Incremental decoder for the inbound byte stream of one connection.
pub struct FrameDecoder {
    buf: BytesMut,
    threshold: Option<u32>,
    cipher: Option<Cfb8>,
    inflater: Decompress,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buf: BytesMut::with_capacity(4096), threshold: None, cipher: None, inflater: Decompress::new(true) }
    }

    /// Enables (`Some(threshold)`) or disables (`None`) the compressed packet format.
    pub fn set_compression(&mut self, threshold: Option<u32>) {
        self.threshold = threshold;
    }

    /// Starts decrypting. Bytes that were already buffered are decrypted too, since the peer
    /// encrypts everything it sends after its Encryption Response.
    pub fn enable_encryption(&mut self, secret: &[u8; 16]) {
        let mut cipher = Cfb8::new(secret);
        cipher.decrypt(&mut self.buf);
        self.cipher = Some(cipher);
    }

    /// Feeds bytes read from the socket.
    pub fn push(&mut self, data: &[u8]) {
        let start = self.buf.len();
        self.buf.extend_from_slice(data);
        if let Some(cipher) = &mut self.cipher {
            cipher.decrypt(&mut self.buf[start..]);
        }
    }

    /// Number of buffered bytes not yet returned as frames.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Returns the next complete payload (packet id + body), or `None` if more bytes are needed.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        let Some((len, prefix)) = peek_frame_len(&self.buf)? else { return Ok(None) };
        if len == 0 {
            return Err(FrameError::Empty);
        }
        if self.buf.len() < prefix + len {
            self.buf.reserve(prefix + len - self.buf.len());
            return Ok(None);
        }
        self.buf.advance(prefix);
        let frame = self.buf.split_to(len);

        let Some(threshold) = self.threshold else { return Ok(Some(frame.to_vec())) };

        // Compressed format: a VarInt uncompressed length, 0 meaning "stored".
        let mut data_len = 0u32;
        let mut consumed = 0;
        loop {
            let Some(&b) = frame.get(consumed) else { return Err(FrameError::Empty) };
            if consumed >= 5 {
                return Err(FrameError::LengthTooLong);
            }
            data_len |= ((b & 0x7F) as u32) << (7 * consumed);
            consumed += 1;
            if b & 0x80 == 0 {
                break;
            }
        }
        let body = &frame[consumed..];
        if data_len == 0 {
            if body.is_empty() {
                return Err(FrameError::Empty);
            }
            return Ok(Some(body.to_vec()));
        }
        let data_len = data_len as usize;
        if data_len < threshold as usize {
            return Err(FrameError::BelowThreshold(data_len, threshold));
        }
        if data_len > MAX_UNCOMPRESSED_LEN {
            return Err(FrameError::UncompressedTooLarge(data_len));
        }
        inflate(&mut self.inflater, body, data_len).map(Some)
    }
}

fn inflate(inflater: &mut Decompress, input: &[u8], expected: usize) -> Result<Vec<u8>, FrameError> {
    inflater.reset(true);
    let mut out = Vec::with_capacity(expected);
    let status = inflater.decompress_vec(input, &mut out, FlushDecompress::Finish).map_err(|e| FrameError::Zlib(e.to_string()))?;
    if status != Status::StreamEnd || out.len() != expected {
        // Either the stream ended early or it holds more than was declared.
        return Err(FrameError::SizeMismatch { declared: expected, actual: out.len() });
    }
    Ok(out)
}

fn deflate(deflater: &mut Compress, input: &[u8], out: &mut Vec<u8>) {
    deflater.reset();
    out.reserve(input.len() / 2 + 64);
    let mut consumed = 0usize;
    loop {
        let before = deflater.total_in();
        let status = deflater
            .compress_vec(&input[consumed..], out, FlushCompress::Finish)
            .expect("deflating an in-memory buffer cannot fail");
        consumed += (deflater.total_in() - before) as usize;
        if status == Status::StreamEnd {
            return;
        }
        out.reserve(out.capacity().max(4096));
    }
}

/// Appends the frame for `payload` to `out` (no encryption).
fn write_frame(deflater: &mut Compress, threshold: Option<u32>, payload: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
    match threshold {
        None => {
            if payload.len() > MAX_FRAME_LEN {
                return Err(FrameError::TooLarge(payload.len()));
            }
            out.put_varint(payload.len() as i32);
            out.extend_from_slice(payload);
        }
        Some(threshold) if payload.len() < threshold as usize => {
            if payload.len() + 1 > MAX_FRAME_LEN {
                return Err(FrameError::TooLarge(payload.len() + 1));
            }
            out.put_varint(payload.len() as i32 + 1);
            out.push(0);
            out.extend_from_slice(payload);
        }
        Some(_) => {
            // Reserve the widest possible length prefix, compress after it, then close the gap.
            let start = out.len();
            out.extend_from_slice(&[0; 3]);
            out.put_varint(payload.len() as i32);
            deflate(deflater, payload, out);
            let frame_len = out.len() - start - 3;
            if frame_len > MAX_FRAME_LEN {
                out.truncate(start);
                return Err(FrameError::TooLarge(frame_len));
            }
            let prefix = varint_len(frame_len as i32);
            let mut header = Vec::with_capacity(3);
            header.put_varint(frame_len as i32);
            out.copy_within(start + 3.., start + prefix);
            out.truncate(out.len() - (3 - prefix));
            out[start..start + prefix].copy_from_slice(&header);
        }
    }
    Ok(())
}

/// Frames `payload` once so the identical bytes can be sent to every connection that uses
/// the same compression threshold (encryption, if any, is still applied per connection).
pub fn frame_packet(payload: &[u8], threshold: Option<u32>, level: u32) -> Result<Vec<u8>, FrameError> {
    let mut deflater = Compress::new(Compression::new(level), true);
    let mut out = Vec::with_capacity(payload.len().min(1 << 16) + 8);
    write_frame(&mut deflater, threshold, payload, &mut out)?;
    Ok(out)
}

/// Encoder for the outbound byte stream of one connection.
pub struct FrameEncoder {
    threshold: Option<u32>,
    cipher: Option<Cfb8>,
    deflater: Compress,
}

impl FrameEncoder {
    /// `level` is the zlib compression level (0-9).
    pub fn new(level: u32) -> Self {
        Self { threshold: None, cipher: None, deflater: Compress::new(Compression::new(level), true) }
    }

    pub fn set_compression(&mut self, threshold: Option<u32>) {
        self.threshold = threshold;
    }

    pub fn compression(&self) -> Option<u32> {
        self.threshold
    }

    pub fn enable_encryption(&mut self, secret: &[u8; 16]) {
        self.cipher = Some(Cfb8::new(secret));
    }

    /// Frames (and encrypts) `payload`, appending the wire bytes to `out`.
    pub fn encode(&mut self, payload: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
        let start = out.len();
        write_frame(&mut self.deflater, self.threshold, payload, out)?;
        if let Some(cipher) = &mut self.cipher {
            cipher.encrypt(&mut out[start..]);
        }
        Ok(())
    }

    /// Appends bytes produced by [`frame_packet`] with this connection's threshold,
    /// encrypting them if needed.
    pub fn encode_framed(&mut self, framed: &[u8], out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(framed);
        if let Some(cipher) = &mut self.cipher {
            cipher.encrypt(&mut out[start..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn uncompressed_round_trip() {
        let mut enc = FrameEncoder::new(6);
        let mut dec = FrameDecoder::new();
        let mut wire = Vec::new();
        for len in [1, 2, 127, 128, 300, 70_000] {
            enc.encode(&payload(len), &mut wire).unwrap();
        }
        // Deliver in awkward pieces.
        let mut got = Vec::new();
        for chunk in wire.chunks(113) {
            dec.push(chunk);
            while let Some(frame) = dec.next_frame().unwrap() {
                got.push(frame);
            }
        }
        assert_eq!(got.iter().map(Vec::len).collect::<Vec<_>>(), [1, 2, 127, 128, 300, 70_000]);
        assert_eq!(got[4], payload(300));
        assert_eq!(dec.buffered(), 0);
    }

    #[test]
    fn exact_bytes_without_compression() {
        let mut enc = FrameEncoder::new(6);
        let mut wire = Vec::new();
        enc.encode(&[0x00, 0xAB], &mut wire).unwrap();
        assert_eq!(wire, [2, 0x00, 0xAB]);
    }

    #[test]
    fn compressed_round_trip() {
        let mut enc = FrameEncoder::new(6);
        let mut dec = FrameDecoder::new();
        enc.set_compression(Some(256));
        dec.set_compression(Some(256));
        let mut wire = Vec::new();
        // Below the threshold: stored with a zero data length.
        enc.encode(&payload(10), &mut wire).unwrap();
        assert_eq!(wire[..2], [11, 0]);
        assert_eq!(wire.len(), 12);
        // At and above the threshold: deflated.
        for len in [256, 257, 5000, 200_000, 3_000_000] {
            enc.encode(&payload(len), &mut wire).unwrap();
        }
        dec.push(&wire);
        let mut lens = Vec::new();
        while let Some(frame) = dec.next_frame().unwrap() {
            assert_eq!(frame, payload(frame.len()));
            lens.push(frame.len());
        }
        assert_eq!(lens, [10, 256, 257, 5000, 200_000, 3_000_000]);
    }

    #[test]
    fn shared_frames_match_per_connection_frames() {
        let p = payload(10_000);
        for threshold in [None, Some(256), Some(1_000_000)] {
            let shared = frame_packet(&p, threshold, 6).unwrap();
            let mut enc = FrameEncoder::new(6);
            enc.set_compression(threshold);
            let mut direct = Vec::new();
            enc.encode(&p, &mut direct).unwrap();
            assert_eq!(shared, direct);

            let mut dec = FrameDecoder::new();
            dec.set_compression(threshold);
            dec.push(&shared);
            assert_eq!(dec.next_frame().unwrap().unwrap(), p);
        }
    }

    #[test]
    fn encrypted_round_trip() {
        let secret = *b"0123456789abcdef";
        let mut enc = FrameEncoder::new(6);
        let mut dec = FrameDecoder::new();
        enc.set_compression(Some(64));
        dec.set_compression(Some(64));
        enc.enable_encryption(&secret);
        dec.enable_encryption(&secret);
        let mut wire = Vec::new();
        enc.encode(&payload(40), &mut wire).unwrap();
        enc.encode(&payload(4000), &mut wire).unwrap();
        let shared = frame_packet(&payload(900), Some(64), 6).unwrap();
        enc.encode_framed(&shared, &mut wire);
        for chunk in wire.chunks(31) {
            dec.push(chunk);
        }
        assert_eq!(dec.next_frame().unwrap().unwrap(), payload(40));
        assert_eq!(dec.next_frame().unwrap().unwrap(), payload(4000));
        assert_eq!(dec.next_frame().unwrap().unwrap(), payload(900));
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn rejects_malformed_frames() {
        // Four-byte length prefix.
        let mut dec = FrameDecoder::new();
        dec.push(&[0x80, 0x80, 0x80, 0x01]);
        assert!(matches!(dec.next_frame(), Err(FrameError::LengthTooLong)));

        // Zero-length frame.
        let mut dec = FrameDecoder::new();
        dec.push(&[0]);
        assert!(matches!(dec.next_frame(), Err(FrameError::Empty)));

        // Compressed frame whose declared size is below the threshold.
        let mut dec = FrameDecoder::new();
        dec.set_compression(Some(256));
        let small = frame_packet(&payload(100), Some(50), 6).unwrap();
        dec.push(&small);
        assert!(matches!(dec.next_frame(), Err(FrameError::BelowThreshold(100, 256))));

        // Declared size disagrees with the stream.
        let mut framed = frame_packet(&payload(1000), Some(64), 6).unwrap();
        // Frame is [len:2][data_len:2][zlib...]; bump the declared data length (1000 = E8 07 -> E9 07).
        assert_eq!(framed[2..4], [0xE8, 0x07]);
        framed[2] = 0xE9;
        let mut dec = FrameDecoder::new();
        dec.set_compression(Some(64));
        dec.push(&framed);
        assert!(matches!(dec.next_frame(), Err(FrameError::SizeMismatch { .. })));

        // Garbage instead of a zlib stream.
        let mut dec = FrameDecoder::new();
        dec.set_compression(Some(1));
        dec.push(&[4, 100, 1, 2, 3]);
        assert!(dec.next_frame().is_err());
    }

    #[test]
    fn partial_length_prefix_waits() {
        let mut dec = FrameDecoder::new();
        dec.push(&[0x80]);
        assert!(dec.next_frame().unwrap().is_none());
        dec.push(&[0x01]);
        assert!(dec.next_frame().unwrap().is_none());
        dec.push(&vec![7; 128]);
        assert_eq!(dec.next_frame().unwrap().unwrap(), vec![7; 128]);
    }
}
