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
use std::collections::VecDeque;
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
    /// Translates the packets of a client of an older version.
    pub translator: Option<Box<dyn paper_compat::Translator>>,
    /// Payloads the translator made for the server, read before the socket.
    pub pending: VecDeque<Vec<u8>>,
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
        Self {
            stream,
            addr,
            dec: FrameDecoder::new(),
            enc: FrameEncoder::new(compression_level),
            out: Vec::with_capacity(1024),
            read_buf: vec![0; 8192],
            translator: None,
            pending: VecDeque::new(),
        }
    }

    /// The next payload (packet id + body), or `None` once the peer closed the connection.
    async fn read_frame(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(payload) = self.pending.pop_front() {
                return Ok(Some(payload));
            }
            let frame = loop {
                if let Some(frame) = self.dec.next_frame()? {
                    break frame;
                }
                let n = self.stream.read(&mut self.read_buf).await?;
                if n == 0 {
                    return Ok(None);
                }
                self.dec.push(&self.read_buf[..n]);
            };
            let Some(translator) = &mut self.translator else { return Ok(Some(frame)) };
            let mut outcome = paper_compat::Outcome::default();
            translator.serverbound(&frame, &mut outcome)?;
            self.pending.extend(outcome.to_server);
            if !outcome.to_client.is_empty() {
                for payload in &outcome.to_client {
                    self.enc.encode(payload, &mut self.out)?;
                }
                self.flush().await?;
            }
        }
    }

    /// Reads until at least `n` bytes are buffered, or until `wait` is over if given. Returns
    /// whether any are (`false`: the peer closed without sending anything).
    async fn fill(&mut self, n: usize, wait: Option<Duration>) -> Result<bool> {
        let deadline = wait.map(|w| tokio::time::Instant::now() + w);
        while self.dec.buffered() < n {
            let read = self.stream.read(&mut self.read_buf);
            let got = match deadline {
                Some(deadline) => match tokio::time::timeout_at(deadline, read).await {
                    Ok(got) => got?,
                    Err(_) => break,
                },
                None => read.await?,
            };
            if got == 0 {
                break;
            }
            self.dec.push(&self.read_buf[..got]);
        }
        Ok(self.dec.buffered() > 0)
    }

    async fn expect_frame(&mut self) -> Result<Vec<u8>> {
        self.read_frame().await?.ok_or_else(|| anyhow!("connection closed"))
    }

    fn queue<P: Packet + Encode>(&mut self, packet: &P) -> Result<()> {
        self.queue_payload(&encode_packet(packet))
    }

    fn queue_payload(&mut self, payload: &[u8]) -> Result<()> {
        let Some(translator) = &mut self.translator else {
            self.enc.encode(payload, &mut self.out)?;
            return Ok(());
        };
        let mut outcome = paper_compat::Outcome::default();
        translator.clientbound(payload, &mut outcome)?;
        for payload in &outcome.to_client {
            self.enc.encode(payload, &mut self.out)?;
        }
        self.pending.extend(outcome.to_server);
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
        if self.enc.is_raw() && self.translator.is_none() {
            // A client from before 1.7, still in its own handshake.
            let kick = paper_compat::prenetty::kick(&paper_compat::legacy::text::legacy_string(&reason.0));
            self.enc.encode(&kick, &mut self.out)?;
            self.flush().await?;
        } else {
            self.send(&login::LoginDisconnect { reason: JsonText(reason) }).await?;
        }
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

/// What a client's first bytes say it is.
enum Opening {
    Netty,
    /// A server list ping from before 1.7: from 1.4 on (`0xFE 0x01`) or older (`0xFE`).
    LegacyPing { modern: bool },
    /// A login from before 1.7 (its handshake, `0x02`).
    PreNettyLogin,
    Closed,
}

async fn opening(conn: &mut Connection) -> Result<Opening> {
    if !conn.fill(1, None).await? {
        return Ok(Opening::Closed);
    }
    match conn.dec.peek()[0] {
        // A Netty frame two bytes long cannot be a handshake.
        0x02 => Ok(Opening::PreNettyLogin),
        0xFE => {
            // Pings from 1.4 and 1.5 are `0xFE 0x01`, then the client waits; 1.6 follows with
            // 0xFA. A Netty handshake 254 to 381 bytes long starts with 0xFE too, then its
            // length's second byte and the packet id 0.
            conn.fill(3, Some(Duration::from_millis(250))).await?;
            Ok(match conn.dec.peek() {
                [0xFE] => Opening::LegacyPing { modern: false },
                [0xFE, 0x01] | [0xFE, 0x01, 0xFA, ..] => Opening::LegacyPing { modern: true },
                _ => Opening::Netty,
            })
        }
        _ => Ok(Opening::Netty),
    }
}

async fn pre_play(conn: &mut Connection, shared: &Arc<Shared>) -> Result<Option<LoginResult>> {
    match opening(conn).await? {
        Opening::Netty => {}
        Opening::LegacyPing { modern } => {
            legacy_ping(conn, shared, modern).await?;
            return Ok(None);
        }
        Opening::PreNettyLogin => return pre_netty_login(conn, shared).await,
        Opening::Closed => return Ok(None),
    }
    let Some(first) = conn.read_frame().await? else { return Ok(None) };
    let intention: Intention = decode_as(&first).context("handshake")?;
    match intention.intent {
        Intent::Status => {
            status_flow(conn, shared, intention.protocol_version).await?;
            Ok(None)
        }
        Intent::Login | Intent::Transfer => login_flow(conn, shared, intention).await,
    }
}

/// A status response for a client of `protocol`: one that can join sees its own protocol
/// (so its server list shows the server as compatible) and the versions that can join,
/// unless a plugin chose others.
fn status_for(json: String, protocol: i32) -> String {
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(&json) else { return json };
    if doc["version"]["protocol"] == paper_data::PROTOCOL_VERSION {
        doc["version"]["protocol"] = paper_compat::status_protocol(protocol).into();
        if doc["version"]["name"] == format!("Paper-RS {}", paper_data::VERSION_NAME) {
            doc["version"]["name"] = format!("Paper-RS {}", paper_compat::version_range()).into();
        }
    }
    doc.to_string()
}

async fn status_flow(conn: &mut Connection, shared: &Arc<Shared>, protocol: i32) -> Result<Finished> {
    if !shared.config.properties.enable_status {
        return Ok(Finished::Done);
    }
    // A client sends a status request and then a ping; either may be omitted.
    while let Some(frame) = conn.read_frame().await? {
        match packet_id(&frame)? {
            id if id == status::StatusRequest::ID => {
                let snapshot = shared.status.read().clone();
                let host = shared.plugin_host.read().clone();
                let json = match host {
                    // PaperServerListPingEvent; the host call blocks, so it runs off the I/O threads.
                    Some(host) => {
                        let address = conn.addr.ip().to_string();
                        let snap = snapshot.clone();
                        let result = tokio::task::spawn_blocking(move || crate::plugins::events::on_server_list_ping(&host, &address, &snap)).await.ok().flatten();
                        match result {
                            Some(r) if r.bool("cancelled") == Some(true) => return Ok(Finished::Done),
                            Some(r) => crate::plugins::events::server_list_ping_json(&snapshot, &r),
                            None => snapshot.to_json(),
                        }
                    }
                    None => snapshot.to_json(),
                };
                conn.send(&status::StatusResponse { json: status_for(json, protocol) }).await?;
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

/// Answers the server list ping of a client from before 1.7, then closes.
async fn legacy_ping(conn: &mut Connection, shared: &Arc<Shared>, modern: bool) -> Result<()> {
    // 1.6 sends more after `0xFE 0x01 0xFA` (its address); read it, so closing with it unread
    // does not reset the connection before the client reads the answer.
    conn.fill(usize::MAX, Some(Duration::from_millis(50))).await?;
    if shared.config.properties.enable_status {
        let status = shared.status.read().clone();
        let motd = paper_compat::legacy::text::legacy_string(&status.motd.0);
        let version = format!("Paper-RS {}", paper_compat::version_range());
        conn.stream.write_all(&paper_compat::prenetty::ping_answer(modern, &version, &motd, status.online, status.max)).await?;
    }
    let _ = conn.stream.shutdown().await;
    Ok(())
}

/// The login of a client from before 1.7: its handshake, the encryption exchange it does even
/// with an offline server, then the usual login through its translator. Online mode cannot
/// have them: the session servers they knew are gone.
async fn pre_netty_login(conn: &mut Connection, shared: &Arc<Shared>) -> Result<Option<LoginResult>> {
    use paper_compat::prenetty;
    conn.dec.set_splitter(prenetty::split_serverbound);
    conn.enc.set_raw();
    // The protocol is the byte after 0x02 (from 1.3 on; older clients send a string there).
    conn.fill(2, Some(Duration::from_secs(5))).await?;
    let protocol = conn.dec.peek().get(1).copied().unwrap_or(0);
    if paper_compat::pre_netty_translator(protocol, "").is_none() {
        let which = if protocol > prenetty::PROTOCOL { "server" } else { "client" };
        conn.login_disconnect(Component::text(format!("Outdated {which}! This server takes {}", paper_compat::version_range()))).await?;
        return Ok(None);
    }
    let handshake = prenetty::Handshake::read(&conn.expect_frame().await?).context("handshake")?;
    if shared.config.properties.online_mode {
        conn.login_disconnect(Component::text("This server is in online mode, which clients from before 1.7 can no longer sign in to")).await?;
        return Ok(None);
    }

    let keys = match shared.legacy_keys.get() {
        Some(keys) => keys,
        None => {
            let keys = tokio::task::spawn_blocking(auth::ServerKeys::generate).await??;
            let _ = shared.legacy_keys.set(keys);
            shared.legacy_keys.get().expect("just set")
        }
    };
    let verify_token: [u8; 4] = rand::random();
    conn.enc.encode(&prenetty::server_auth_data("-", &keys.public_der, &verify_token), &mut conn.out)?;
    conn.flush().await?;
    let (secret, token) = prenetty::read_shared_key(&conn.expect_frame().await?).context("encryption answer")?;
    let secret = keys.decrypt(&secret)?;
    if keys.decrypt(&token)? != verify_token {
        bail!("verify token mismatch");
    }
    let secret: [u8; 16] = secret.as_slice().try_into().map_err(|_| anyhow!("shared secret is not 16 bytes"))?;
    // The client encrypts what follows its answer, and decrypts what follows the confirmation.
    conn.dec.enable_encryption(&secret);
    conn.enc.encode(&prenetty::SHARED_KEY_ACK, &mut conn.out)?;
    conn.flush().await?;
    conn.enc.enable_encryption(&secret);

    conn.translator = paper_compat::pre_netty_translator(handshake.protocol, &handshake.name);
    log::debug!("{}: a client from before Netty (protocol {}) as {}", conn.addr, handshake.protocol, handshake.name);
    let intention = Intention {
        protocol_version: handshake.protocol as i32,
        server_address: handshake.host,
        server_port: handshake.port.clamp(0, 65535) as u16,
        intent: Intent::Login,
    };
    login_flow(conn, shared, intention).await
}

async fn login_flow(conn: &mut Connection, shared: &Arc<Shared>, intention: Intention) -> Result<Option<LoginResult>> {
    let props = &shared.config.properties;

    if conn.translator.is_none() && intention.protocol_version != paper_data::PROTOCOL_VERSION {
        // An older client joins through a translator, if its version is one of those.
        conn.translator = paper_compat::translator(intention.protocol_version);
        match &conn.translator {
            Some(translator) => log::debug!("{}: a {} client (protocol {})", conn.addr, translator.version().name, intention.protocol_version),
            None => {
                let key = if intention.protocol_version < paper_data::PROTOCOL_VERSION { "multiplayer.disconnect.outdated_client" } else { "multiplayer.disconnect.incompatible" };
                conn.login_disconnect(Component::translatable(key, vec![Component::text(paper_compat::version_range())])).await?;
                return Ok(None);
            }
        }
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

    // --- plugins may refuse the player (AsyncPlayerPreLoginEvent) ---
    let host = shared.plugin_host.read().clone();
    if let Some(host) = host {
        let (name, uuid, address) = (profile.name.clone(), profile.uuid, conn.addr.ip().to_string());
        let refused = tokio::task::spawn_blocking(move || crate::plugins::events::on_pre_login(&host, &name, uuid, &address)).await.ok().flatten();
        if let Some(reason) = refused {
            log::info!("Disconnecting {} ({}): {}", profile.name, conn.addr, reason.to_plain());
            conn.login_disconnect(reason).await?;
            return Ok(None);
        }
    }

    // --- may this player join? (plugins may change the verdict: PlayerLoginEvent) ---
    let verdict = shared.access.read().check_login(&profile, conn.addr.ip(), shared.player_count.load(Ordering::Relaxed), props);
    let verdict = if shared.plugin_host.read().is_some() {
        let (to_main, profile, address, host) = (shared.to_main.clone(), profile.clone(), conn.addr.ip().to_string(), intention.server_address.clone());
        let decided = tokio::task::spawn_blocking(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let task = move |s: &mut crate::server::Server| {
                let _ = tx.send(crate::plugins::events::on_login(s, &profile, &address, &host, verdict));
            };
            to_main.send(crate::server::MainTask::Run(Box::new(task))).ok()?;
            rx.recv().ok()
        });
        decided.await.ok().flatten().unwrap_or(Ok(()))
    } else {
        verdict.map_err(|(_, reason)| reason)
    };
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
