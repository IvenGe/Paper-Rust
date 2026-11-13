//! The play state of a connection: a reader task and a writer task per player.

use super::connection::{Connection, LoginResult};
use super::{Outbound, PlayerConn, Shared};
use crate::server::{JoinRequest, MainTask};
use paper_protocol::frame::{FrameDecoder, FrameEncoder};
use paper_protocol::packets::play::Serverbound;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, Notify};

/// Packets per second above which a client is considered to be flooding the server.
const MAX_PACKETS_PER_SECOND: u32 = 2000;

pub(super) async fn run(conn: Connection, login: LoginResult, shared: Arc<Shared>) {
    let Connection { stream, addr, dec, enc, out, .. } = conn;
    let (tx, rx) = mpsc::unbounded_channel();
    let queued = Arc::new(AtomicUsize::new(0));
    let entity_id = shared.next_entity_id.fetch_add(1, Ordering::Relaxed);
    let request = JoinRequest {
        entity_id,
        profile: login.profile,
        settings: login.settings,
        brand: login.brand,
        virtual_host: login.virtual_host,
        conn: PlayerConn::new(tx, addr, queued.clone()),
    };
    if shared.to_main.send(MainTask::Join(Box::new(request))).is_err() {
        return;
    }

    let (read_half, write_half) = stream.into_split();
    let closed = Arc::new(Notify::new());
    let writer = tokio::spawn(write_loop(write_half, enc, rx, out, queued, closed.clone()));
    let read_timeout = Duration::from_secs(shared.config.paper.network.read_timeout_seconds.max(1));
    let reason = read_loop(read_half, dec, entity_id, &shared, read_timeout, closed).await;
    let _ = shared.to_main.send(MainTask::Disconnected { entity_id, reason });
    // The writer ends when the main thread drops the player's connection handle.
    let _ = tokio::time::timeout(Duration::from_secs(10), writer).await;
}

/// Decodes inbound packets and forwards them to the main thread. Returns why it stopped.
async fn read_loop(mut read_half: OwnedReadHalf, mut dec: FrameDecoder, entity_id: i32, shared: &Arc<Shared>, read_timeout: Duration, closed: Arc<Notify>) -> String {
    let mut buf = vec![0u8; 16 * 1024];
    let mut window_start = tokio::time::Instant::now();
    let mut packets_in_window = 0u32;
    loop {
        loop {
            let frame = match dec.next_frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(e) => return format!("Internal Exception: {e}"),
            };
            let packet = match Serverbound::decode(&frame) {
                Ok(packet) => packet,
                Err(e) => return format!("Internal Exception: {e}"),
            };
            packets_in_window += 1;
            if packets_in_window > MAX_PACKETS_PER_SECOND {
                if window_start.elapsed() < Duration::from_secs(1) {
                    return "Sent too many packets".to_owned();
                }
                window_start = tokio::time::Instant::now();
                packets_in_window = 0;
            }
            if shared.to_main.send(MainTask::Packet { entity_id, packet }).is_err() {
                return "Server closed".to_owned();
            }
        }
        tokio::select! {
            result = tokio::time::timeout(read_timeout, read_half.read(&mut buf)) => match result {
                Ok(Ok(0)) => return "Disconnected".to_owned(),
                Ok(Ok(n)) => dec.push(&buf[..n]),
                Ok(Err(e)) => return format!("Internal Exception: {e}"),
                Err(_) => return "Timed out".to_owned(),
            },
            _ = closed.notified() => return "Disconnected".to_owned(),
        }
    }
}

/// Frames and writes everything queued for the player, batching whatever is available.
async fn write_loop(
    mut write_half: OwnedWriteHalf,
    mut enc: FrameEncoder,
    mut rx: mpsc::UnboundedReceiver<Outbound>,
    mut out: Vec<u8>,
    queued: Arc<AtomicUsize>,
    closed: Arc<Notify>,
) {
    let mut closing = false;
    'outer: while let Some(first) = rx.recv().await {
        let mut next = Some(first);
        while let Some(message) = next {
            match message {
                Outbound::Packet(payload) => {
                    queued.fetch_sub(payload.len().min(queued.load(Ordering::Relaxed)), Ordering::Relaxed);
                    if let Err(e) = enc.encode(&payload, &mut out) {
                        log::warn!("Dropping an unsendable packet: {e}");
                    }
                }
                Outbound::Framed(frame) => {
                    queued.fetch_sub(frame.len().min(queued.load(Ordering::Relaxed)), Ordering::Relaxed);
                    enc.encode_framed(&frame, &mut out);
                }
                Outbound::Close(last) => {
                    if let Some(payload) = last {
                        let _ = enc.encode(&payload, &mut out);
                    }
                    closing = true;
                    break;
                }
            }
            // Keep batching while more is immediately available, within reason.
            next = if out.len() < 1 << 20 { rx.try_recv().ok() } else { None };
        }
        if write_half.write_all(&out).await.is_err() {
            break 'outer;
        }
        out.clear();
        if closing {
            break;
        }
    }
    let _ = write_half.shutdown().await;
    // Wake the reader so the connection is torn down even if the peer keeps its side open.
    closed.notify_one();
}
