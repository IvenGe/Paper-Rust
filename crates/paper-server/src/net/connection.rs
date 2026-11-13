//! The pre-play life of a connection: handshake, status, login and configuration.

use super::{auth, core_pack, play, Shared};
use anyhow::{anyhow, bail, Context, Result};
use paper_protocol::codec::{ByteArray, Decode, Encode, Reader, VarInt};
use paper_protocol::frame::{FrameDecoder, FrameEncoder};
use paper_protocol::packets::common::ClientSettings;
use paper_protocol::packets::handshake::{Intent, Intention};
use paper_protocol::packets::{configuration, encode_packet, login, status, Packet};
use paper_protocol::text::Component;
use paper_protocol::types::{GameProfile, JsonText};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A connection before it is split for the play state.
pub(super) struct Connection {
    pub stream: TcpStream,
    pub addr: SocketAddr,
    pub dec: FrameDecoder,
    pub enc: FrameEncoder,
    /// Encoded bytes waiting for the next flush.
    pub out: Vec<u8>,
    read_buf: Vec<u8>,
}

/// Everything learned about a player on the way to the play state.
pub(super) struct LoginResult {
    pub profile: GameProfile,
    pub settings: ClientSettings,
    pub brand: Option<String>,
    /// The address the client says it connected to.
    pub virtual_host: String,
}

/// Why the pre-play phase ended without a player.
enum Finished {
    /// Status ping answered, or the peer went away.
    Done,
}

impl Connection {
    fn new(stream: TcpStream, addr: SocketAddr, compression_level: u32) -> Self {
        Self { stream, addr, dec: FrameDecoder::new(), enc: FrameEncoder::new(compression_level), out: Vec::with_capacity(1024), read_buf: vec![0; 8192] }
    }

    /// The next payload (packet id + body), or `None` once the peer closed the connection.
    async fn read_frame(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(frame) = self.dec.next_frame()? {
                return Ok(Some(frame));
            }
            let n = self.stream.read(&mut self.read_buf).await?;
            if n == 0 {
                return Ok(None);
            }
            self.dec.push(&self.read_buf[..n]);
        }
    }

    async fn expect_frame(&mut self) -> Result<Vec<u8>> {
        self.read_frame().await?.ok_or_else(|| anyhow!("connection closed"))
    }

    fn queue<P: Packet + Encode>(&mut self, packet: &P) -> Result<()> {
        self.queue_payload(&encode_packet(packet))
    }

    fn queue_payload(&mut self, payload: &[u8]) -> Result<()> {
        self.enc.encode(payload, &mut self.out)?;
        Ok(())
    }

    async fn flush(&mut self) -> Result<()> {
        if !self.out.is_empty() {
            self.stream.write_all(&self.out).await?;
            self.out.clear();
        }
        Ok(())
    }

    async fn send<P: Packet + Encode>(&mut self, packet: &P) -> Result<()> {
        self.queue(packet)?;
        self.flush().await
    }

    /// Refuses a login with a reason and closes.
    async fn login_disconnect(&mut self, reason: Component) -> Result<()> {
        self.send(&login::LoginDisconnect { reason: JsonText(reason) }).await?;
        let _ = self.stream.shutdown().await;
        Ok(())
    }
}

/// Decodes a payload as packet `P`, checking the id and that nothing is left over.
fn decode_as<P: Packet + Decode>(payload: &[u8]) -> Result<P> {
    let mut r = Reader::new(payload);
    let id = r.varint()?;
    if id != P::ID {
        bail!("expected {} (id {}) but received packet id {id}", P::NAME, P::ID);
    }
    let packet = P::decode(&mut r)?;
    r.finish()?;
    Ok(packet)
}

fn packet_id(payload: &[u8]) -> Result<i32> {
    Ok(Reader::new(payload).varint()?)
}

/// Runs a new connection until it ends.
pub(super) async fn handle(stream: TcpStream, addr: SocketAddr, shared: Arc<Shared>) {
    let _ = stream.set_nodelay(true);
    let network = &shared.config.paper.network;
    let mut conn = Connection::new(stream, addr, network.compression_level);
    let timeout = Duration::from_secs(network.login_timeout_seconds.max(1));
    match tokio::time::timeout(timeout, pre_play(&mut conn, &shared)).await {
        Ok(Ok(Some(login))) => play::run(conn, login, shared).await,
        Ok(Ok(None)) => {}
        Ok(Err(e)) => log::debug!("{addr}: connection ended before play: {e:#}"),
        Err(_) => {
            // Mirrors vanilla's "Took too long to log in"; the packet only makes sense if
            // the client got as far as the login state, and is harmless otherwise.
            let _ = conn.login_disconnect(Component::translatable("multiplayer.disconnect.slow_login", vec![])).await;
            log::debug!("{addr}: took too long to log in");
        }
    }
}

async fn pre_play(conn: &mut Connection, shared: &Arc<Shared>) -> Result<Option<LoginResult>> {
    let Some(first) = conn.read_frame().await? else { return Ok(None) };
    let intention: Intention = decode_as(&first).context("handshake")?;
    match intention.intent {
        Intent::Status => {
            status_flow(conn, shared).await?;
            Ok(None)
        }
        Intent::Login | Intent::Transfer => login_flow(conn, shared, intention).await,
    }
}

async fn status_flow(conn: &mut Connection, shared: &Arc<Shared>) -> Result<Finished> {
    if !shared.config.properties.enable_status {
        return Ok(Finished::Done);
    }
    // A client sends a status request and then a ping; either may be omitted.
    while let Some(frame) = conn.read_frame().await? {
        match packet_id(&frame)? {
            id if id == status::StatusRequest::ID => {
                let json = shared.status.read().to_json();
                conn.send(&status::StatusResponse { json }).await?;
            }
            id if id == status::PingRequest::ID => {
                let ping: status::PingRequest = decode_as(&frame)?;
                conn.send(&status::PongResponse { payload: ping.payload }).await?;
                break;
            }
            other => bail!("unexpected packet id {other} in status state"),
        }
    }
    Ok(Finished::Done)
}

async fn login_flow(conn: &mut Connection, shared: &Arc<Shared>, intention: Intention) -> Result<Option<LoginResult>> {
    let props = &shared.config.properties;

    if intention.protocol_version != paper_data::PROTOCOL_VERSION {
        let key = if intention.protocol_version < paper_data::PROTOCOL_VERSION { "multiplayer.disconnect.outdated_client" } else { "multiplayer.disconnect.incompatible" };
        conn.login_disconnect(Component::translatable(key, vec![Component::text(paper_data::VERSION_NAME)])).await?;
        return Ok(None);
    }
    if intention.intent == Intent::Transfer && !props.accepts_transfers {
        conn.login_disconnect(Component::translatable("multiplayer.disconnect.transfers_disabled", vec![])).await?;
        return Ok(None);
    }
    if shared.stopping.load(Ordering::Relaxed) {
        conn.login_disconnect(Component::translatable("multiplayer.disconnect.server_shutdown", vec![])).await?;
        return Ok(None);
    }

    let hello: login::Hello = decode_as(&conn.expect_frame().await?).context("login start")?;
    if !auth::is_valid_player_name(&hello.name) {
        conn.login_disconnect(Component::text("Invalid characters in username")).await?;
        return Ok(None);
    }

    // --- identity ---
    let profile = if props.online_mode {
        match authenticate(conn, shared, &hello.name).await? {
            Some(profile) => profile,
            None => return Ok(None),
        }
    } else {
        GameProfile { uuid: auth::offline_uuid(&hello.name), name: hello.name.clone(), properties: Vec::new() }
    };

    // --- may this player join? ---
    let verdict = shared.access.read().check_login(&profile, conn.addr.ip(), shared.player_count.load(Ordering::Relaxed), props);
    if let Err(reason) = verdict {
        log::info!("Disconnecting {} ({}): {}", profile.name, conn.addr, reason.to_plain());
        conn.login_disconnect(reason).await?;
        return Ok(None);
    }

    // --- compression, then success ---
    if let Some(threshold) = shared.config.compression_threshold() {
        conn.send(&login::LoginCompression { threshold: VarInt(threshold as i32) }).await?;
        conn.enc.set_compression(Some(threshold));
        conn.dec.set_compression(Some(threshold));
    }
    conn.send(&login::LoginFinished { profile: profile.clone(), session_id: uuid::Uuid::new_v4() }).await?;

    // The client may answer cookie requests or plugin queries before acknowledging.
    loop {
        let frame = conn.expect_frame().await?;
        if packet_id(&frame)? == login::LoginAcknowledged::ID {
            break;
        }
    }

    configuration_flow(conn, shared, profile, intention.server_address).await
}

/// The online-mode encryption handshake and session check. Returns `None` if the player
/// was refused (and told why).
async fn authenticate(conn: &mut Connection, shared: &Arc<Shared>, name: &str) -> Result<Option<GameProfile>> {
    let keys = shared.keys.as_ref().ok_or_else(|| anyhow!("online mode without server keys"))?;
    let verify_token: [u8; 4] = rand::random();
    conn.send(&login::EncryptionRequest {
        server_id: String::new(),
        public_key: ByteArray(keys.public_der.clone()),
        verify_token: ByteArray(verify_token.to_vec()),
        should_authenticate: true,
    })
    .await?;

    let key: login::Key = decode_as(&conn.expect_frame().await?).context("encryption response")?;
    let secret = keys.decrypt(&key.shared_secret.0)?;
    let token = keys.decrypt(&key.verify_token.0)?;
    let secret: [u8; 16] = secret.as_slice().try_into().map_err(|_| anyhow!("shared secret is not 16 bytes"))?;
    if token != verify_token {
        bail!("verify token mismatch");
    }
    conn.dec.enable_encryption(&secret);
    conn.enc.enable_encryption(&secret);

    let hash = auth::server_hash("", &secret, &keys.public_der);
    let ip = shared.config.properties.prevent_proxy_connections.then(|| conn.addr.ip().to_string());
    let username = name.to_owned();
    let result = tokio::task::spawn_blocking(move || auth::has_joined(&username, &hash, ip.as_deref())).await?;
    match result {
        Ok(Some(profile)) => {
            log::info!("UUID of player {} is {}", profile.name, profile.uuid);
            Ok(Some(profile))
        }
        Ok(None) => {
            log::info!("Username '{name}' tried to join with an invalid session");
            conn.login_disconnect(Component::translatable("multiplayer.disconnect.unverified_username", vec![])).await?;
            Ok(None)
        }
        Err(e) => {
            log::warn!("Couldn't verify username '{name}': {e:#}");
            conn.login_disconnect(Component::translatable("multiplayer.disconnect.authservers_down", vec![])).await?;
            Ok(None)
        }
    }
}

async fn configuration_flow(conn: &mut Connection, shared: &Arc<Shared>, profile: GameProfile, virtual_host: String) -> Result<Option<LoginResult>> {
    let packets = &shared.config_packets;
    let mut settings = ClientSettings::default();
    let mut brand = None;

    conn.queue_payload(&packets.brand)?;
    conn.queue_payload(&packets.features)?;
    conn.queue_payload(&packets.known_packs)?;
    conn.flush().await?;

    let handle_common = |packet: configuration::Serverbound, settings: &mut ClientSettings, brand: &mut Option<String>| match packet {
        configuration::Serverbound::ClientInformation(info) => *settings = info.settings,
        configuration::Serverbound::CustomPayload(payload) if payload.channel == "minecraft:brand" => {
            *brand = Reader::new(&payload.data.0).string(256).ok();
        }
        _ => {}
    };

    // Wait for the client's known packs; registry data is elided only for packs it has.
    loop {
        match configuration::Serverbound::decode(&conn.expect_frame().await?)? {
            configuration::Serverbound::SelectKnownPacks(known) => {
                if !known.packs.contains(&core_pack()) {
                    let reason = Component::text(format!(
                        "This server requires the vanilla {} data pack on the client; your client did not report it.",
                        paper_data::VERSION_NAME
                    ));
                    conn.send(&configuration::Disconnect { reason }).await?;
                    return Ok(None);
                }
                break;
            }
            other => handle_common(other, &mut settings, &mut brand),
        }
    }

    for registry in &packets.registries {
        conn.queue_payload(registry)?;
    }
    conn.queue_payload(&packets.tags)?;
    conn.queue_payload(&packets.finish)?;
    conn.flush().await?;

    loop {
        match configuration::Serverbound::decode(&conn.expect_frame().await?)? {
            configuration::Serverbound::FinishConfiguration => break,
            other => handle_common(other, &mut settings, &mut brand),
        }
    }
    Ok(Some(LoginResult { profile, settings, brand, virtual_host }))
}
