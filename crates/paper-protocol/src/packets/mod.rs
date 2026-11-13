//! Packet definitions for protocol 777, grouped by connection state.
//!
//! Every packet type implements [`Packet`] (its id and name) plus [`Encode`] and, where a
//! receiver exists in this code base (the server for serverbound packets, the test client
//! for clientbound ones), [`Decode`].

use crate::codec::Encode;
use crate::codec::WriteExt;

pub mod common;
pub mod configuration;
pub mod handshake;
pub mod login;
pub mod play;
pub mod status;

/// The protocol state of a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Handshake => "handshake",
            State::Status => "status",
            State::Login => "login",
            State::Configuration => "configuration",
            State::Play => "play",
        }
    }
}

/// Static identity of a packet type.
pub trait Packet {
    /// Numeric id within its state and direction.
    const ID: i32;
    /// Vanilla resource name, e.g. `minecraft:login`.
    const NAME: &'static str;
}

/// Encodes `packet` as a payload: VarInt id followed by the body.
pub fn encode_packet<P: Packet + Encode>(packet: &P) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.put_varint(P::ID);
    packet.encode(&mut out);
    out
}

/// Declares a packet whose body is its fields encoded in order.
macro_rules! packet {
    (
        $(#[$meta:meta])*
        $name:ident = $state:ident :: $dir:ident :: $id:ident {
            $( $(#[$fmeta:meta])* $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: $ty, )*
        }

        $crate::packets::packet_id!($name = $state :: $dir :: $id);

        impl $crate::codec::Encode for $name {
            #[allow(unused_variables)]
            fn encode(&self, w: &mut Vec<u8>) {
                $( $crate::codec::Encode::encode(&self.$field, w); )*
            }
        }

        impl $crate::codec::Decode for $name {
            #[allow(unused_variables)]
            fn decode(r: &mut $crate::codec::Reader<'_>) -> $crate::codec::Result<Self> {
                Ok(Self { $( $field: $crate::codec::Decode::decode(r)?, )* })
            }
        }
    };
}

/// Implements [`Packet`] for a type whose codec is written by hand.
macro_rules! packet_id {
    ($name:ident = $state:ident :: $dir:ident :: $id:ident) => {
        impl $crate::packets::Packet for $name {
            const ID: i32 = ::paper_data::packets::$state::$dir::$id;
            const NAME: &'static str = ::paper_data::packets::$state::$dir::NAMES[::paper_data::packets::$state::$dir::$id as usize];
        }
    };
}

pub(crate) use packet;
pub(crate) use packet_id;
