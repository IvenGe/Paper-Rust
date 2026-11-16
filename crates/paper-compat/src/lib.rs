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

pub mod components;
pub mod data;
pub mod io;
pub mod modern;

use anyhow::Result;

/// The version the server speaks.
pub const NATIVE_PROTOCOL: i32 = paper_data::PROTOCOL_VERSION;

/// A client version the server can translate for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    /// The protocol number its handshake carries.
    pub protocol: i32,
    /// Its name (the first release with this protocol, or a range).
    pub name: &'static str,
    /// The data file of `data/compat` with its ids.
    data: &'static str,
}

/// Every version besides 26.3 that may join, newest first.
pub const VERSIONS: &[Version] = &[Version { protocol: 776, name: "26.2", data: "26.2" }, Version { protocol: 775, name: "26.1", data: "26.1" }];

/// The version of a client's handshake, if it can join.
pub fn version(protocol: i32) -> Option<&'static Version> {
    VERSIONS.iter().find(|v| v.protocol == protocol)
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

/// The versions that can join, for the server list and messages: `26.1-26.3`.
pub fn version_range() -> String {
    match VERSIONS.last() {
        Some(oldest) => format!("{}-{}", oldest.name, paper_data::VERSION_NAME),
        None => paper_data::VERSION_NAME.to_owned(),
    }
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
}

/// A translator for a client of `protocol`, or `None` if it speaks 26.3 or cannot join.
pub fn translator(protocol: i32) -> Option<Box<dyn Translator>> {
    let version = version(protocol)?;
    Some(Box::new(modern::Modern::new(version, data::load(version.data))))
}
