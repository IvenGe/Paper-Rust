//! Lets clients of older Minecraft versions join a Paper-RS server.
//!
//! The server speaks only 26.3 (protocol 777). A connection from an older client gets a
//! [`Translator`] for its version. The translator turns every packet the server sends into
//! what that client understands, and every packet the client sends into its 26.3 form,
//! on the connection's I/O thread, so the game never sees the difference. This is what
//! ViaVersion and its siblings (ViaBackwards, ViaRewind, ViaLegacy) do for Java servers; their
//! mapping data and their knowledge of each version's protocol are the references here
//! (see `data/compat/README.md`).
//!
//! A translator works on payloads (a packet id and its body) in every state after the
//! handshake. Besides answering in the other version it may answer for the client
//! ([`Outcome::to_server`]): a client too old for a step of the 26.3 conversation (the
//! configuration phase, teleport confirmations, ...) has that step done for it.

pub mod chunk;
pub mod components;
pub mod data;
pub mod io;
pub mod legacy;
pub mod modern;
pub mod particles;

pub use legacy::prenetty;

use anyhow::Result;

/// The version the server speaks.
pub const NATIVE_PROTOCOL: i32 = paper_data::PROTOCOL_VERSION;

/// How a version's packets are translated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// The versions just before 26.3: the same protocol with changes (`modern`).
    Modern,
    /// 1.8: from before the flattening (`legacy`).
    Legacy18,
    /// 1.7, through the 1.8 translator (`legacy::v1_7`).
    Legacy17,
    /// From before Netty (`legacy::prenetty`), through the 1.8 translator. Such clients are
    /// told apart by their first bytes, not by a handshake's protocol number (whose numbers
    /// Netty's restarted).
    PreNetty,
}

/// A client version the server can translate for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    /// The protocol number its handshake carries.
    pub protocol: i32,
    /// Its name (the first release with this protocol, or a range).
    pub name: &'static str,
    pub family: Family,
    /// The data file of `data/compat` with its ids (modern versions).
    data: &'static str,
}

/// Every version besides 26.3 that may join, newest first.
pub const VERSIONS: &[Version] = &[
    Version { protocol: 776, name: "26.2", family: Family::Modern, data: "26.2" },
    Version { protocol: 775, name: "26.1", family: Family::Modern, data: "26.1" },
    Version { protocol: 47, name: "1.8.x", family: Family::Legacy18, data: "" },
    Version { protocol: 5, name: "1.7.x", family: Family::Legacy17, data: "" },
    Version { protocol: 4, name: "1.7.x", family: Family::Legacy17, data: "" },
    Version { protocol: 78, name: "1.6.2-1.6.4", family: Family::PreNetty, data: "" },
    Version { protocol: 74, name: "1.6.2-1.6.4", family: Family::PreNetty, data: "" },
    Version { protocol: 61, name: "1.5.x", family: Family::PreNetty, data: "" },
    Version { protocol: 60, name: "1.5.x", family: Family::PreNetty, data: "" },
    Version { protocol: legacy::prenetty::PROTOCOL as i32, name: "1.4.6-1.4.7", family: Family::PreNetty, data: "" },
];

/// The version of a client's (Netty) handshake, if it can join.
pub fn version(protocol: i32) -> Option<&'static Version> {
    VERSIONS.iter().find(|v| v.protocol == protocol && v.family != Family::PreNetty)
}

/// Whether clients of `protocol` can join (26.3 itself included).
pub fn supported(protocol: i32) -> bool {
    protocol == NATIVE_PROTOCOL || version(protocol).is_some()
}

/// The protocol to put in a status response to a client of `protocol`: its own when it can
/// join, so its server list shows the server as compatible.
pub fn status_protocol(protocol: i32) -> i32 {
    if supported(protocol) {
        protocol
    } else {
        NATIVE_PROTOCOL
    }
}

/// The versions that can join, for the server list and messages: `1.4.6-1.4.7, 1.5.x,
/// 1.6.2-1.6.4, 1.7.x, 1.8.x, 26.1-26.3`.
pub fn version_range() -> String {
    let modern = VERSIONS.iter().rfind(|v| v.family == Family::Modern);
    let mut parts: Vec<String> = VERSIONS.iter().filter(|v| v.family != Family::Modern).rev().map(|v| v.name.to_owned()).collect();
    parts.dedup();
    parts.push(match modern {
        Some(oldest) => format!("{}-{}", oldest.name, paper_data::VERSION_NAME),
        None => paper_data::VERSION_NAME.to_owned(),
    });
    parts.join(", ")
}

/// What a translator makes of one packet.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Payloads for the client, in its version.
    pub to_client: Vec<Vec<u8>>,
    /// Payloads for the server, in 26.3, as if the client had sent them.
    pub to_server: Vec<Vec<u8>>,
}

impl Outcome {
    pub fn is_empty(&self) -> bool {
        self.to_client.is_empty() && self.to_server.is_empty()
    }

    pub fn clear(&mut self) {
        self.to_client.clear();
        self.to_server.clear();
    }
}

/// Translates one connection's packets (in the states after the handshake).
pub trait Translator: Send {
    fn version(&self) -> &'static Version;

    /// A packet of the server (26.3).
    fn clientbound(&mut self, payload: &[u8], out: &mut Outcome) -> Result<()>;

    /// A packet of the client (its version).
    fn serverbound(&mut self, payload: &[u8], out: &mut Outcome) -> Result<()>;

    /// Whether the client's version knows compressed packets (1.8 on): the connection does
    /// not turn compression on for one that does not.
    fn compression(&self) -> bool {
        true
    }
}

/// A translator for a client of `protocol`, or `None` if it speaks 26.3 or cannot join.
pub fn translator(protocol: i32) -> Option<Box<dyn Translator>> {
    let version = version(protocol)?;
    Some(match version.family {
        Family::Modern => Box::new(modern::Modern::new(version, data::load(version.data))),
        Family::Legacy18 => Box::new(legacy::Legacy18::new(version)),
        Family::Legacy17 => Box::new(legacy::v1_7::Legacy17::new(version)),
        // Found by their first bytes: see `pre_netty_translator`.
        Family::PreNetty => return None,
    })
}

/// A translator for a client from before Netty whose handshake carried `protocol`, logging in
/// as `name`; `None` if it cannot join.
pub fn pre_netty_translator(protocol: u8, name: &str) -> Option<Box<dyn Translator>> {
    let version = VERSIONS.iter().find(|v| v.family == Family::PreNetty && v.protocol == protocol as i32)?;
    Some(Box::new(legacy::prenetty::PreNetty::new(version, name)))
}
