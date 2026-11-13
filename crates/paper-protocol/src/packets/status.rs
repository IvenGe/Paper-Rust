//! Status state: the server list ping.

use super::packet;

packet! {
    /// Asks for the status JSON.
    StatusRequest = status::serverbound::STATUS_REQUEST {}
}

packet! {
    /// Latency probe; answered with [`PongResponse`] carrying the same payload.
    PingRequest = status::serverbound::PING_REQUEST {
        payload: i64,
    }
}

packet! {
    /// The status document (version, players, description, favicon) as a JSON string.
    StatusResponse = status::clientbound::STATUS_RESPONSE {
        json: String,
    }
}

packet! {
    PongResponse = status::clientbound::PONG_RESPONSE {
        payload: i64,
    }
}
