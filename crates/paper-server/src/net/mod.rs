//! Networking.
//!
//! Every connection is a task on the multi-threaded I/O runtime. The task drives the
//! handshake, status, login and configuration states by itself; once a player reaches the
//! play state it splits into a reader, which decodes packets and forwards them to the main
//! thread, and a writer, which frames, compresses and encrypts whatever the main thread
//! queues for the player. Game state never lives on these threads.

pub mod auth;
mod connection;
mod play;

use crate::access::AccessLists;
use crate::config::Config;
use crate::server::MainTask;
use bytes::Bytes;
use paper_protocol::codec::Encode;
use paper_protocol::packets::{configuration, encode_packet, Packet};
use paper_protocol::text::Component;
use paper_protocol::types::KnownPack;
use parking_lot::RwLock;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// What the server list shows; rebuilt by the main thread when it changes.
#[derive(Clone, Debug)]
pub struct StatusSnapshot {
    pub motd: Component,
    pub online: usize,
    pub max: i32,
    /// `(name, uuid)` of some online players, unless hidden by configuration.
    pub sample: Vec<(String, uuid::Uuid)>,
    /// `data:image/png;base64,...` of `server-icon.png`, if present.
    pub favicon: Option<String>,
}

impl StatusSnapshot {
    /// The status response document.
    pub fn to_json(&self) -> String {
        let mut players = serde_json::json!({ "max": self.max, "online": self.online });
        if !self.sample.is_empty() {
            players["sample"] = self.sample.iter().map(|(name, id)| serde_json::json!({ "name": name, "id": id.to_string() })).collect();
        }
        let mut doc = serde_json::json!({
            "version": { "name": format!("Paper-RS {}", paper_data::VERSION_NAME), "protocol": paper_data::PROTOCOL_VERSION },
            "players": players,
            "description": self.motd.0,
            "enforcesSecureChat": false,
        });
        if let Some(icon) = &self.favicon {
            doc["favicon"] = serde_json::Value::String(icon.clone());
        }
        doc.to_string()
    }
}

/// Payloads sent to every client during configuration, encoded once at startup.
pub struct ConfigPackets {
    pub brand: Bytes,
    pub features: Bytes,
    pub known_packs: Bytes,
    pub registries: Vec<Bytes>,
    pub tags: Bytes,
    pub finish: Bytes,
}

/// The brand shown on the client's debug screen.
pub const BRAND: &str = "Paper-RS";

impl ConfigPackets {
    pub fn build() -> Self {
        use paper_protocol::codec::{RemainingBytes, WriteExt};
        let mut brand = Vec::new();
        brand.put_string(BRAND);
        let registries = paper_data::synced::SYNCED_REGISTRIES
            .iter()
            .map(|(registry, entries)| {
                // Entries carry no data: the client takes it from its own copy of the
                // `minecraft:core` pack, which the known-packs exchange confirmed it has.
                let packet = configuration::RegistryData { registry: (*registry).to_owned(), entries: entries.iter().map(|e| ((*e).to_owned(), None)).collect() };
                Bytes::from(encode_packet(&packet))
            })
            .collect();
        let tags = configuration::UpdateTags {
            registries: paper_data::synced::TAGS
                .iter()
                .map(|(registry, tags)| paper_protocol::packets::common::RegistryTags {
                    registry: (*registry).to_owned(),
                    tags: tags.iter().map(|(tag, ids)| ((*tag).to_owned(), ids.to_vec())).collect(),
                })
                .collect(),
        };
        Self {
            brand: encode_packet(&configuration::CustomPayload { channel: "minecraft:brand".into(), data: RemainingBytes(brand) }).into(),
            features: encode_packet(&configuration::UpdateEnabledFeatures { features: paper_data::synced::FEATURE_FLAGS.iter().map(|f| (*f).to_owned()).collect() }).into(),
            known_packs: encode_packet(&configuration::SelectKnownPacks { packs: vec![core_pack()] }).into(),
            registries,
            tags: encode_packet(&tags).into(),
            finish: encode_packet(&configuration::FinishConfiguration {}).into(),
        }
    }
}

/// The vanilla data pack both sides are expected to share.
pub fn core_pack() -> KnownPack {
    KnownPack { namespace: "minecraft".into(), id: "core".into(), version: paper_data::VERSION_NAME.into() }
}

/// State shared between the I/O tasks and the main thread.
pub struct Shared {
    pub config: Arc<Config>,
    pub to_main: crossbeam_channel::Sender<MainTask>,
    pub status: RwLock<Arc<StatusSnapshot>>,
    pub next_entity_id: AtomicI32,
    /// Present in online mode.
    pub keys: Option<auth::ServerKeys>,
    pub config_packets: ConfigPackets,
    pub access: RwLock<AccessLists>,
    pub player_count: AtomicUsize,
    /// Set while the server shuts down; new logins are refused.
    pub stopping: AtomicBool,
    /// The port the listener is actually bound to (differs from the configured one when
    /// that is 0).
    pub bound_port: AtomicI32,
}

/// A message for a player's writer task.
pub enum Outbound {
    /// A payload (packet id + body) to frame on this connection.
    Packet(Bytes),
    /// A frame produced by `frame_packet` with the server-wide compression threshold.
    Framed(Arc<Vec<u8>>),
    /// Optionally send a final payload (a disconnect packet), then close the connection.
    Close(Option<Bytes>),
}

/// The main thread's handle to a player's connection.
#[derive(Clone)]
pub struct PlayerConn {
    tx: mpsc::UnboundedSender<Outbound>,
    pub addr: SocketAddr,
    /// Bytes queued but not yet written, to detect clients that cannot keep up.
    queued: Arc<AtomicUsize>,
}

/// A connection is dropped once this much data is waiting to be written to it.
const MAX_QUEUED_BYTES: usize = 256 * 1024 * 1024;

impl PlayerConn {
    pub(crate) fn new(tx: mpsc::UnboundedSender<Outbound>, addr: SocketAddr, queued: Arc<AtomicUsize>) -> Self {
        Self { tx, addr, queued }
    }

    /// A connection that discards everything, for tests of main-thread logic.
    pub fn detached() -> (Self, mpsc::UnboundedReceiver<Outbound>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, addr: SocketAddr::from(([127, 0, 0, 1], 0)), queued: Arc::new(AtomicUsize::new(0)) }, rx)
    }

    fn push(&self, message: Outbound, size: usize) {
        if self.queued.fetch_add(size, Ordering::Relaxed) > MAX_QUEUED_BYTES {
            // The client is not reading; closing is the only way to bound memory.
            let _ = self.tx.send(Outbound::Close(None));
            return;
        }
        let _ = self.tx.send(message);
    }

    /// Queues a packet.
    pub fn send<P: Packet + Encode>(&self, packet: &P) {
        let payload = encode_packet(packet);
        let size = payload.len();
        self.push(Outbound::Packet(payload.into()), size);
    }

    /// Queues an already encoded payload (packet id + body), e.g. one shared by many players.
    pub fn send_payload(&self, payload: Bytes) {
        let size = payload.len();
        self.push(Outbound::Packet(payload), size);
    }

    /// Queues a pre-framed packet.
    pub fn send_framed(&self, frame: Arc<Vec<u8>>) {
        let size = frame.len();
        self.push(Outbound::Framed(frame), size);
    }

    /// Sends a disconnect packet with `reason` and closes the connection.
    pub fn disconnect(&self, reason: &Component) {
        let payload = encode_packet(&paper_protocol::packets::play::Disconnect { reason: reason.clone() });
        let _ = self.tx.send(Outbound::Close(Some(payload.into())));
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Accepts connections until the server stops.
pub async fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                if shared.stopping.load(Ordering::Relaxed) {
                    continue;
                }
                let shared = shared.clone();
                tokio::spawn(async move {
                    connection::handle(stream, addr, shared).await;
                });
            }
            Err(e) => {
                // Typically resource exhaustion (EMFILE); back off instead of spinning.
                log::warn!("Failed to accept a connection: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json() {
        let status = StatusSnapshot {
            motd: Component::text("Hello"),
            online: 1,
            max: 20,
            sample: vec![("Steve".into(), uuid::Uuid::from_u128(1))],
            favicon: None,
        };
        let doc: serde_json::Value = serde_json::from_str(&status.to_json()).unwrap();
        assert_eq!(doc["version"]["protocol"], 777);
        assert_eq!(doc["players"]["online"], 1);
        assert_eq!(doc["players"]["sample"][0]["name"], "Steve");
        assert_eq!(doc["description"]["text"], "Hello");
        assert_eq!(doc["enforcesSecureChat"], false);
        assert!(doc.get("favicon").is_none());
    }

    #[test]
    fn config_packets_cover_every_synced_registry() {
        let packets = ConfigPackets::build();
        assert_eq!(packets.registries.len(), paper_data::synced::SYNCED_REGISTRIES.len());
        assert!(packets.registries.len() >= 20);
        assert!(packets.tags.len() > 1000);
    }
}
