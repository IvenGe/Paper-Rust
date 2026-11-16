//! Paper-RS Boost: more threads for one player's connection, for clients with the Paper-RS
//! Boost Fabric mod (`client-mod/`), when the server allows it (`network.boost-mod` in
//! `config/paper-rs.toml`; off by default, and then every client is served as vanilla).
//!
//! A client with the mod says so during login (it answers a login query on `paper-rs:boost`
//! that other clients answer as unknown) and opens a few extra connections, "streams". The
//! chunks go over those: each chunk's packet is the frame already encoded and compressed once
//! for everyone, and each stream has its own task, so one player's chunk data is encrypted
//! and written by several threads at once instead of queueing behind everything else on one
//! connection, and the mod reads and decodes it on several threads too. A small marker on the
//! main connection (`paper-rs:boost/chunk` with the chunk's number) keeps each chunk in order
//! with the rest of what the server sends.
//!
//! Version 1 on the wire:
//! - Login query, channel `paper-rs:boost`: varint version, varint the streams offered,
//!   varint the compression threshold (-1 for none), the 16-byte session token.
//! - Its answer: varint version, varint the streams the client opens (0 for none). An answer
//!   without data (vanilla's) means no mod.
//! - A stream starts with `PRSB`, a version byte, the token's id (the first 8 bytes of
//!   SHA-256 of `paper-rs:boost id` and the token) and the stream's index; the server answers
//!   one byte, 1 to take it or 0 to refuse. From then on what the server sends on it is
//!   encrypted with AES-128-CFB8 keyed (and IV'd) by the first 16 bytes of SHA-256 of
//!   `paper-rs:boost stream`, the token and the index, and is a series of: varint length,
//!   varint chunk number, the chunk's frame as the main connection would carry it (varint
//!   length, then with compression a varint data length and the zlib data).
//! - The marker, a play custom payload on `paper-rs:boost/chunk`: varint chunk number. The
//!   client handles the chunk with that number when it gets there.

use paper_protocol::codec::WriteExt;
use paper_protocol::crypto::Cfb8;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

pub const CHANNEL: &str = "paper-rs:boost";
pub const MARKER: &str = "paper-rs:boost/chunk";
pub const VERSION: i32 = 1;
pub const MAGIC: &[u8; 4] = b"PRSB";
/// What a stream sends before it is taken: the magic, the version, the token's id, the index.
pub const STREAM_HEADER: usize = 4 + 1 + 8 + 1;
/// How long a login's streams may take to connect.
const ATTACH_WINDOW: Duration = Duration::from_secs(60);

/// The query's data.
pub fn hello(streams: u32, threshold: Option<u32>, token: &[u8; 16]) -> Vec<u8> {
    let mut w = Vec::new();
    w.put_varint(VERSION);
    w.put_varint(streams as i32);
    w.put_varint(threshold.map_or(-1, |t| t as i32));
    w.extend_from_slice(token);
    w
}

/// The streams a client with the mod opens, from its answer (`None`: no mod, or none).
pub fn streams_answered(data: Option<&[u8]>, offered: u32) -> Option<u32> {
    let mut r = paper_protocol::codec::Reader::new(data?);
    let version = r.varint().ok()?;
    let streams = r.varint().ok()?;
    (version == VERSION && streams > 0).then(|| (streams as u32).min(offered))
}

/// The id a stream gives for its session (the token itself never goes over a stream).
pub fn token_id(token: &[u8; 16]) -> u64 {
    let digest = Sha256::new().chain_update(b"paper-rs:boost id").chain_update(token).finalize();
    u64::from_be_bytes(digest[..8].try_into().unwrap())
}

/// A stream's key (and IV).
pub fn stream_key(token: &[u8; 16], index: u8) -> [u8; 16] {
    let digest = Sha256::new().chain_update(b"paper-rs:boost stream").chain_update(token).chain_update([index]).finalize();
    digest[..16].try_into().unwrap()
}

/// What a stream carries for one chunk.
pub fn stream_message(seq: u32, frame: &[u8], out: &mut Vec<u8>) {
    let mut head = Vec::with_capacity(5);
    head.put_varint(seq as i32);
    out.put_varint((head.len() + frame.len()) as i32);
    out.extend_from_slice(&head);
    out.extend_from_slice(frame);
}

/// The marker's payload (a play custom payload packet).
pub fn marker(seq: u32) -> Vec<u8> {
    let mut data = Vec::new();
    data.put_varint(seq as i32);
    paper_protocol::packets::encode_packet(&paper_protocol::packets::play::CustomPayload { channel: MARKER.into(), data: paper_protocol::codec::RemainingBytes(data) })
}

/// A player's streams, as the main thread's handle to the player sees them.
#[derive(Default)]
pub struct Streams {
    writers: Mutex<Vec<mpsc::UnboundedSender<(u32, Arc<Vec<u8>>)>>>,
    next: AtomicU32,
    /// The main connection's writer, told to close it when a stream breaks (its chunks would
    /// never come).
    kick: OnceLock<mpsc::WeakUnboundedSender<super::Outbound>>,
}

impl Streams {
    /// Sends a chunk's frame over the next stream; returns its number, or `None` while no
    /// stream is there (then it goes over the main connection as usual).
    pub fn send(&self, frame: &Arc<Vec<u8>>) -> Option<u32> {
        let writers = self.writers.lock();
        if writers.is_empty() {
            return None;
        }
        let seq = self.next.fetch_add(1, Ordering::Relaxed);
        writers[seq as usize % writers.len()].send((seq, frame.clone())).ok()?;
        Some(seq)
    }

    /// How many streams are taking chunks.
    pub fn count(&self) -> usize {
        self.writers.lock().len()
    }

    pub(super) fn set_kick(&self, tx: mpsc::WeakUnboundedSender<super::Outbound>) {
        let _ = self.kick.set(tx);
    }

    fn broken(&self) {
        self.writers.lock().clear();
        if let Some(tx) = self.kick.get().and_then(|t| t.upgrade()) {
            let reason = paper_protocol::text::Component::text("Paper-RS Boost: a stream connection was lost");
            let payload = paper_protocol::packets::encode_packet(&paper_protocol::packets::play::Disconnect { reason });
            let _ = tx.send(super::Outbound::Close(Some(payload.into())));
        }
    }
}

/// A login whose streams may still connect.
struct Waiting {
    token: [u8; 16],
    streams: u32,
    attached: Vec<u8>,
    handle: Arc<Streams>,
    until: Instant,
}

/// The logins waiting for streams, by token id.
#[derive(Default)]
pub struct Sessions {
    waiting: Mutex<HashMap<u64, Waiting>>,
}

impl Sessions {
    /// A client with the mod logged in: its streams may come.
    pub fn open(&self, token: [u8; 16], streams: u32) -> Arc<Streams> {
        let handle = Arc::new(Streams::default());
        let mut waiting = self.waiting.lock();
        let now = Instant::now();
        waiting.retain(|_, w| w.until > now);
        waiting.insert(token_id(&token), Waiting { token, streams, attached: Vec::new(), handle: handle.clone(), until: now + ATTACH_WINDOW });
        handle
    }

    /// A stream asks to be taken: its session's token and handle, if it may.
    fn take(&self, id: u64, index: u8) -> Option<([u8; 16], Arc<Streams>)> {
        let mut waiting = self.waiting.lock();
        let w = waiting.get_mut(&id).filter(|w| w.until > Instant::now() && (index as u32) < w.streams && !w.attached.contains(&index))?;
        w.attached.push(index);
        let taken = (w.token, w.handle.clone());
        if w.attached.len() as u32 >= w.streams {
            waiting.remove(&id);
        }
        Some(taken)
    }
}

/// Serves a stream connection after its header (`header`, [`STREAM_HEADER`] bytes): refuses
/// it, or writes the chunks its player's handle hands it until either side goes.
pub(super) async fn serve_stream(mut stream: TcpStream, header: &[u8], shared: &super::Shared) {
    let version = header[4];
    let id = u64::from_be_bytes(header[5..13].try_into().unwrap());
    let index = header[13];
    let taken = if version as i32 == VERSION { shared.boost.take(id, index) } else { None };
    let Some((token, handle)) = taken else {
        let _ = stream.write_all(&[0]).await;
        return;
    };
    if stream.write_all(&[1]).await.is_err() {
        handle.broken();
        return;
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<(u32, Arc<Vec<u8>>)>();
    handle.writers.lock().push(tx);
    // Only the handle keeps the stream going: when the player leaves, it goes.
    let weak = Arc::downgrade(&handle);
    drop(handle);
    let mut cipher = Cfb8::new(&stream_key(&token, index));
    let (mut read_half, mut write_half) = stream.into_split();
    let mut out = Vec::new();
    let mut scratch = [0u8; 64];
    loop {
        tokio::select! {
            message = rx.recv() => {
                let Some((seq, frame)) = message else { break };
                out.clear();
                stream_message(seq, &frame, &mut out);
                // Whatever else is waiting goes in the same write.
                while out.len() < 1 << 20 {
                    match rx.try_recv() {
                        Ok((seq, frame)) => stream_message(seq, &frame, &mut out),
                        Err(_) => break,
                    }
                }
                cipher.encrypt(&mut out);
                if write_half.write_all(&out).await.is_err() {
                    if let Some(h) = weak.upgrade() {
                        h.broken();
                    }
                    break;
                }
            }
            // The client sends nothing more; a read that ends means it went.
            gone = tokio::io::AsyncReadExt::read(&mut read_half, &mut scratch) => {
                if gone.map_or(true, |n| n == 0) {
                    if let Some(h) = weak.upgrade() {
                        h.broken();
                    }
                    break;
                }
            }
        }
    }
    let _ = write_half.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_and_messages() {
        assert_eq!(streams_answered(None, 4), None, "vanilla");
        let mut answer = Vec::new();
        answer.put_varint(VERSION);
        answer.put_varint(8);
        assert_eq!(streams_answered(Some(&answer), 4), Some(4), "no more than offered");
        let mut none = Vec::new();
        none.put_varint(VERSION);
        none.put_varint(0);
        assert_eq!(streams_answered(Some(&none), 4), None);
        let mut out = Vec::new();
        stream_message(300, &[9, 8, 7], &mut out);
        assert_eq!(out, vec![5, 0xAC, 0x02, 9, 8, 7]);
        let token = [7u8; 16];
        assert_ne!(stream_key(&token, 0), stream_key(&token, 1));
        let sessions = Sessions::default();
        let handle = sessions.open(token, 2);
        assert!(sessions.take(token_id(&token), 2).is_none(), "index out of range");
        assert!(sessions.take(token_id(&token), 0).is_some());
        assert!(sessions.take(token_id(&token), 0).is_none(), "taken already");
        assert!(sessions.take(token_id(&token), 1).is_some());
        assert!(sessions.take(token_id(&token), 1).is_none(), "all taken: the session is closed");
        assert_eq!(handle.count(), 0);
        assert_eq!(handle.send(&Arc::new(vec![1])), None, "no writer yet");
    }
}
