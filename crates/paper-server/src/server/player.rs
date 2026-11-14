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
    /// Where the last `PlayerMoveEvent` left the player (Paper only fires the event once the
    /// player moved a pixel or turned ten degrees from there).
    pub last_move_event: Location,
    /// The player is being removed; events and kicks must not start a second removal.
    pub leaving: bool,
    // --- windows ---
    /// The window open besides the player's own inventory.
    pub open_menu: Option<super::containers::OpenMenu>,
    pub drag: super::menu::Drag,
    pub ender_items: Vec<Slot>,
    pub next_window_id: i32,
    // --- survival ---
    pub exhaustion: f32,
    /// Ticks towards the next regeneration or starvation step.
    pub food_timer: i32,
    pub fall_distance: f64,
    pub fire_ticks: i32,
    /// Breath in ticks (300 when full).
    pub air: i32,
    pub invulnerable_time: i32,
    pub last_hurt: f32,
    pub dead: bool,
    /// Eating or drinking: (hand, ticks left).
    pub using_item: Option<(u8, i32)>,
    /// Tick of the last attack swing (for the attack cooldown).
    pub last_swing: u64,
    pub xp_level: i32,
    pub xp_progress: f32,
    pub xp_total: i32,
    /// Where the player respawns, if not at the world spawn: (world, block position).
    pub respawn: Option<(usize, paper_protocol::codec::BlockPos)>,
    /// Who hurt this player last and when (tick), for death messages.
    pub last_attacker: Option<(i32, u64)>,
    /// The last damage taken, for death messages.
    pub last_damage: Option<super::survival::Damage>,
    /// Fall distance of the last landing.
    pub last_fall: f64,
    /// Whether a movement packet moved the player since the last tick.
    pub moved_this_tick: bool,
    /// Health, food and "saturation is zero" as last sent.
    pub sent_health: Option<(f32, i32, bool)>,
    /// Ticks until the next experience orb can be picked up.
    pub xp_pickup_delay: i32,
    /// Ticks spent in a Nether portal.
    pub portal_time: i32,
    /// After arriving through a portal: ticks out of portals until one works again.
    pub portal_cooldown: i32,
    /// A portal trip waiting for the destination to load.
    pub travel: Option<super::portals::Travel>,
    /// Has seen the end credits (vanilla's `seenCredits`).
    pub seen_credits: bool,
    /// Left the End and waits for the client to ask to respawn.
    pub won_game: bool,
    /// Status effects.
    pub effects: Vec<super::effects::ActiveEffect>,
    /// Extra hearts from the absorption effect.
    pub absorption: f32,
    /// The block being broken and the tick it was started.
    pub digging: Option<(paper_protocol::codec::BlockPos, u64)>,
    /// A block reported broken early, broken when its time is up.
    pub delayed_break: Option<(paper_protocol::codec::BlockPos, u64)>,
    /// Crack stage last shown to others (-1 none).
    pub crack_stage: i32,
    /// The head of the bed the player sleeps in.
    pub sleeping: Option<paper_protocol::codec::BlockPos>,
    /// The entity the player rides (in the player's world).
    pub vehicle: Option<i32>,
    /// Item cooldown groups (`minecraft:ender_pearl`) and the tick they end.
    pub cooldowns: std::collections::HashMap<String, u64>,
    /// Plugins' persistent data (Bukkit's `BukkitValues`).
    pub pdc: Option<paper_nbt::Compound>,
    /// Statistics (`stats/<uuid>.json`).
    pub stats: super::stats::Stats,
    /// The mob the player last hit, and when (wolves join in).
    pub last_hurt_mob: Option<(i32, u64)>,
    pub sleep_ticks: i32,
    /// Seed of the enchanting table offers (`XpSeed`); changes with every enchantment.
    pub enchantment_seed: i32,
    /// Max health, speed, armour, reach, ... with the modifiers of equipment and effects.
    pub attributes: crate::world::attributes::Attributes,
    /// The client's last answer to a resource pack offer (0 loaded ... 7 discarded).
    pub resource_pack_status: Option<i32>,
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

    /// The `max_health` attribute.
    pub fn max_health(&self) -> f32 {
        self.attributes.value("max_health") as f32
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
