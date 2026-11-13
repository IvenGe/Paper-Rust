//! Per-player state owned by the main thread.

use crate::config::GameMode;
use crate::net::PlayerConn;
use crate::world::chunk::ChunkPos;
use paper_protocol::packets::common::ClientSettings;
use paper_protocol::packets::play::abilities;
use paper_protocol::text::Component;
use paper_protocol::types::{GameProfile, Slot};
use std::collections::HashSet;
use std::time::Instant;
use uuid::Uuid;

/// Number of slots in the player inventory window: crafting result (0), crafting grid
/// (1-4), armour (5-8), main inventory (9-35), hotbar (36-44), off hand (45).
pub const INVENTORY_SLOTS: usize = 46;
pub const HOTBAR_START: usize = 36;
pub const OFFHAND_SLOT: usize = 45;

/// A position with rotation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Location {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

impl Location {
    pub fn chunk(&self) -> ChunkPos {
        ChunkPos::new((self.x.floor() as i32) >> 4, (self.z.floor() as i32) >> 4)
    }

    pub fn distance_squared(&self, other: &Location) -> f64 {
        let (dx, dy, dz) = (self.x - other.x, self.y - other.y, self.z - other.z);
        dx * dx + dy * dy + dz * dz
    }
}

/// Which chunks a client has been sent and which it is still waiting for.
#[derive(Debug)]
pub struct ChunkView {
    pub center: ChunkPos,
    pub radius: i32,
    /// Chunks the player holds a ticket for.
    pub tracked: HashSet<ChunkPos>,
    /// Chunks already sent to the client.
    pub sent: HashSet<ChunkPos>,
    /// Tracked chunks not sent yet, nearest last (so the next one pops off the end).
    pub pending: Vec<ChunkPos>,
    /// Chunks the client says it can take per tick.
    pub desired_per_tick: f32,
    pub batch_quota: f32,
    pub unacknowledged_batches: u32,
    pub max_unacknowledged_batches: u32,
}

impl ChunkView {
    pub fn new(center: ChunkPos, radius: i32) -> Self {
        Self {
            center,
            radius,
            tracked: HashSet::new(),
            sent: HashSet::new(),
            pending: Vec::new(),
            // Vanilla's starting values: 9 chunks per tick, one unacknowledged batch.
            desired_per_tick: 9.0,
            batch_quota: 0.0,
            unacknowledged_batches: 0,
            max_unacknowledged_batches: 1,
        }
    }

    /// Vanilla's view shape: a square of `radius` with the corners rounded off.
    pub fn contains(center: ChunkPos, radius: i32, pos: ChunkPos) -> bool {
        let dx = ((pos.x - center.x).abs() - 1).max(0) as i64;
        let dz = ((pos.z - center.z).abs() - 1).max(0) as i64;
        dx * dx + dz * dz < (radius as i64) * (radius as i64)
    }

    /// Every chunk in view around `center`.
    pub fn area(center: ChunkPos, radius: i32) -> impl Iterator<Item = ChunkPos> {
        let r = radius + 1;
        (-r..=r).flat_map(move |dz| (-r..=r).map(move |dx| ChunkPos::new(center.x + dx, center.z + dz))).filter(move |p| Self::contains(center, radius, *p))
    }
}

/// Liveness tracking for one connection.
#[derive(Debug)]
pub struct KeepAlive {
    pub last_sent: Instant,
    /// Id of the probe awaiting an answer.
    pub pending: Option<i64>,
    /// Smoothed round-trip time in milliseconds.
    pub latency: i32,
}

pub struct Player {
    pub entity_id: i32,
    pub profile: GameProfile,
    pub conn: PlayerConn,
    pub settings: ClientSettings,
    pub brand: Option<String>,
    /// Index of the world the player is in.
    pub world: usize,
    pub loc: Location,
    pub on_ground: bool,
    /// Position and rotation last broadcast to other players.
    pub tracked_loc: Location,
    pub tracked_on_ground: bool,
    /// The next tracker update must send an absolute position.
    pub force_position_sync: bool,
    pub ticks_since_position_sync: u32,
    /// Entity ids of the players this player currently sees.
    pub visible: HashSet<i32>,
    /// Entity ids of the players who currently see this player.
    pub seen_by: HashSet<i32>,
    pub game_mode: GameMode,
    pub previous_game_mode: Option<GameMode>,
    pub flying: bool,
    pub allow_flight: bool,
    pub sneaking: bool,
    pub sprinting: bool,
    /// Metadata changed and must be broadcast.
    pub metadata_dirty: bool,
    pub equipment_dirty: bool,
    pub inventory: Vec<Slot>,
    /// Selected hotbar slot, 0-8.
    pub held_slot: u8,
    /// Item on the cursor while a window is open.
    pub carried: Slot,
    /// Counter for inventory state ids.
    pub inventory_state: i32,
    pub view: ChunkView,
    /// Id and target of a teleport the client has not confirmed yet.
    pub pending_teleport: Option<(i32, Location)>,
    pub next_teleport_id: i32,
    pub keep_alive: KeepAlive,
    /// The client finished loading terrain after joining or respawning.
    pub loaded: bool,
    pub op_level: u8,
    pub display_name: Option<Component>,
    pub list_name: Option<Component>,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub joined_at: Instant,
    pub last_action: Instant,
    /// The address the client connected to (as it claims).
    pub virtual_host: String,
}

impl Player {
    pub fn name(&self) -> &str {
        &self.profile.name
    }

    pub fn uuid(&self) -> Uuid {
        self.profile.uuid
    }

    /// The name as shown in chat: the display name if set, the profile name otherwise.
    pub fn chat_name(&self) -> Component {
        self.display_name.clone().unwrap_or_else(|| Component::text(self.profile.name.clone()))
    }

    /// Effective view distance: the server's, capped by what the client asked for.
    pub fn view_distance(&self, server: i32) -> i32 {
        let client = self.settings.view_distance as i32;
        if client < 2 {
            server
        } else {
            server.min(client)
        }
    }

    /// The flags byte of the abilities packet for the current game mode and state.
    pub fn ability_flags(&self) -> i8 {
        let mut flags = 0;
        if matches!(self.game_mode, GameMode::Creative | GameMode::Spectator) {
            flags |= abilities::INVULNERABLE;
        }
        if self.flying {
            flags |= abilities::FLYING;
        }
        if self.allow_flight {
            flags |= abilities::ALLOW_FLYING;
        }
        if self.game_mode == GameMode::Creative {
            flags |= abilities::INSTANT_BREAK;
        }
        flags
    }

    /// Applies the flight rules of the current game mode.
    pub fn apply_game_mode_abilities(&mut self) {
        match self.game_mode {
            GameMode::Creative => self.allow_flight = true,
            GameMode::Spectator => {
                self.allow_flight = true;
                self.flying = true;
            }
            GameMode::Survival | GameMode::Adventure => {
                self.allow_flight = false;
                self.flying = false;
            }
        }
    }

    /// The item in the main hand.
    pub fn held_item(&self) -> &Slot {
        &self.inventory[HOTBAR_START + self.held_slot as usize]
    }

    pub fn eye_height(&self) -> f64 {
        if self.sneaking {
            1.27
        } else {
            1.62
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_shape_matches_vanilla() {
        let c = ChunkPos::new(0, 0);
        // Radius 2: a 5x5 square plus one more ring with the far corners cut.
        assert!(ChunkView::contains(c, 2, ChunkPos::new(2, 2)));
        assert!(ChunkView::contains(c, 2, ChunkPos::new(2, -2)));
        assert!(!ChunkView::contains(c, 2, ChunkPos::new(3, 0)));
        assert!(!ChunkView::contains(c, 2, ChunkPos::new(3, 3)));
        assert!(ChunkView::contains(c, 3, ChunkPos::new(3, 3)));
        assert!(!ChunkView::contains(c, 3, ChunkPos::new(4, 4)));
        let count = ChunkView::area(c, 10).count();
        // Vanilla loads a rounded square of roughly (2r+1)^2 chunks.
        assert!(count > 380 && count <= 441, "{count}");
        assert!(ChunkView::area(ChunkPos::new(5, -7), 4).all(|p| ChunkView::contains(ChunkPos::new(5, -7), 4, p)));
    }

    #[test]
    fn location_chunks() {
        let loc = Location { x: -0.5, y: 64.0, z: 31.9, yaw: 0.0, pitch: 0.0 };
        assert_eq!(loc.chunk(), ChunkPos::new(-1, 1));
    }
}
