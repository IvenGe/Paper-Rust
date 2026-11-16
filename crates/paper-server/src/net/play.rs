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

/// A connection's translator, shared by its reader and writer.
type SharedTranslator = Arc<parking_lot::Mutex<Box<dyn paper_compat::Translator>>>;

pub(super) async fn run(conn: Connection, login: LoginResult, shared: Arc<Shared>) {
    let Connection { stream, addr, dec, enc, out, translator, pending, .. } = conn;
    let translator: Option<SharedTranslator> = translator.map(|t| Arc::new(parking_lot::Mutex::new(t)));
    let (tx, rx) = mpsc::unbounded_channel();
    // The reader's answers to the client go through the writer; a weak sender does not keep
    // the writer alive once the main thread drops the connection.
    let answers = tx.downgrade();
    let queued = Arc::new(AtomicUsize::new(0));
    let entity_id = shared.next_entity_id.fetch_add(1, Ordering::Relaxed);
    let request = JoinRequest {
        entity_id,
        profile: login.profile,
        settings: login.settings,
        brand: login.brand,
        virtual_host: login.virtual_host,
        conn: PlayerConn::new(tx, addr, queued.clone(), shared.outgoing.clone()),
    };
    if shared.to_main.send(MainTask::Join(Box::new(request))).is_err() {
        return;
    }
    // What a translator made for the server before play (an old client's first packets).
    for payload in &pending {
        if forward(&shared, entity_id, payload).is_err() {
            return;
        }
    }

    let (read_half, write_half) = stream.into_split();
    let closed = Arc::new(Notify::new());
    let translation = translator.clone().map(|t| Translation { translator: t, entity_id, shared: shared.clone() });
    let writer = tokio::spawn(write_loop(write_half, enc, rx, out, queued, closed.clone(), translation));
    let read_timeout = Duration::from_secs(shared.config.paper.network.read_timeout_seconds.max(1));
    let reason = read_loop(read_half, dec, entity_id, &shared, read_timeout, closed, translator.map(|t| (t, answers))).await;
    let _ = shared.to_main.send(MainTask::Disconnected { entity_id, reason });
    // The writer ends when the main thread drops the player's connection handle.
    let _ = tokio::time::timeout(Duration::from_secs(10), writer).await;
}

/// Decodes inbound packets and forwards them to the main thread. Returns why it stopped.
///
/// With a translator the client's packets are first made 26.3 ones; what the translator
/// answers the client itself goes to the writer.
async fn read_loop(
    mut read_half: OwnedReadHalf,
    mut dec: FrameDecoder,
    entity_id: i32,
    shared: &Arc<Shared>,
    read_timeout: Duration,
    closed: Arc<Notify>,
    translator: Option<(SharedTranslator, mpsc::WeakUnboundedSender<Outbound>)>,
) -> String {
    let mut buf = vec![0u8; 16 * 1024];
    let mut window_start = tokio::time::Instant::now();
    let mut packets_in_window = 0u32;
    let mut outcome = paper_compat::Outcome::default();
    loop {
        loop {
            let frame = match dec.next_frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
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
            let Some((translator, answers)) = &translator else {
                match forward(shared, entity_id, &frame) {
                    Ok(()) => continue,
                    Err(reason) => return reason,
                }
            };
            outcome.clear();
            if let Err(e) = translator.lock().serverbound(&frame, &mut outcome) {
                return format!("Internal Exception: {e:#}");
            }
            if !outcome.to_client.is_empty() {
                if let Some(answers) = answers.upgrade() {
                    let _ = answers.send(Outbound::Translated(std::mem::take(&mut outcome.to_client)));
                }
            }
            for payload in &outcome.to_server {
                if let Err(reason) = forward(shared, entity_id, payload) {
                    return reason;
                }
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

/// Decodes a 26.3 payload from the client and hands it to the main thread.
fn forward(shared: &Shared, entity_id: i32, payload: &[u8]) -> Result<(), String> {
    let packet = Serverbound::decode(payload).map_err(|e| format!("Internal Exception: {e}"))?;
    shared.to_main.send(MainTask::Packet { entity_id, packet }).map_err(|_| "Server closed".to_owned())
}

/// What the writer of a translated connection needs: the translator, and the way to the
/// main thread for the packets the translator makes in the client's place.
struct Translation {
    translator: SharedTranslator,
    entity_id: i32,
    shared: Arc<Shared>,
}

impl Translation {
    /// Translates a payload of the server and frames what the client gets.
    fn encode(&self, payload: &[u8], enc: &mut FrameEncoder, out: &mut Vec<u8>) {
        let mut outcome = paper_compat::Outcome::default();
        if let Err(e) = self.translator.lock().clientbound(payload, &mut outcome) {
            let id = paper_protocol::codec::Reader::new(payload).varint().unwrap_or(-1);
            log::warn!("Could not translate packet {id} for a {} client: {e:#}", self.translator.lock().version().name);
            return;
        }
        for payload in &outcome.to_client {
            if let Err(e) = enc.encode(payload, out) {
                log::warn!("Dropping an unsendable packet: {e}");
            }
        }
        for payload in &outcome.to_server {
            if let Err(reason) = forward(&self.shared, self.entity_id, payload) {
                log::debug!("A translated packet for the server was dropped: {reason}");
            }
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
    translation: Option<Translation>,
) {
    /// Frames one message into `out`; returns whether it closes the connection.
    fn encode(message: Outbound, enc: &mut FrameEncoder, out: &mut Vec<u8>, queued: &AtomicUsize, translation: Option<&Translation>) -> bool {
        match message {
            Outbound::Packet(payload) => {
                queued.fetch_sub(payload.len().min(queued.load(Ordering::Relaxed)), Ordering::Relaxed);
                match translation {
                    Some(t) => t.encode(&payload, enc, out),
                    None => {
                        if let Err(e) = enc.encode(&payload, out) {
                            log::warn!("Dropping an unsendable packet: {e}");
                        }
                    }
                }
                false
            }
            Outbound::Framed(frame) => {
                queued.fetch_sub(frame.len().min(queued.load(Ordering::Relaxed)), Ordering::Relaxed);
                match translation {
                    // Shared frames are 26.3: the translator needs the packet itself.
                    Some(t) => match paper_protocol::frame::unframe(&frame, enc.compression()) {
                        Ok(payload) => t.encode(&payload, enc, out),
                        Err(e) => log::warn!("Dropping an unreadable shared frame: {e}"),
                    },
                    None => enc.encode_framed(&frame, out),
                }
                false
            }
            Outbound::Translated(payloads) => {
                for payload in payloads {
                    let _ = enc.encode(&payload, out);
                }
                false
            }
            Outbound::Close(last) => {
                if let Some(payload) = last {
                    match translation {
                        Some(t) => t.encode(&payload, enc, out),
                        None => {
                            let _ = enc.encode(&payload, out);
                        }
                    }
                }
                true
            }
            Outbound::Batch(messages) => {
                for message in messages {
                    if encode(message, enc, out, queued, translation) {
                        return true;
                    }
                }
                false
            }
        }
    }
    let mut closing = false;
    'outer: while let Some(first) = rx.recv().await {
        let mut next = Some(first);
        while let Some(message) = next {
            if encode(message, &mut enc, &mut out, &queued, translation.as_ref()) {
                closing = true;
                break;
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
