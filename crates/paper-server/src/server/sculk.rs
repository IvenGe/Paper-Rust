//! Vibrations and what hears them (`GameEvent`, `GameEventDispatcher`, `VibrationSystem`).
//! What happens in a world (steps, blocks placed and broken, doors, eating, deaths, ...) reaches
//! the sculk sensors, calibrated sensors and shriekers in range as a vibration that travels a
//! block a tick, unless wool is in the way. A sensor that hears one gives a redstone signal for
//! a while ([`crate::world::block_updates::sculk`]); a shrieker that hears a sensor click shrieks
//! and warns the players near it, calling the warden at the fourth warning
//! (`WardenSpawnTracker`). A catalyst blooms when something dies near it, and the nearest takes
//! the experience to spread sculk with (`SculkCatalystBlockEntity`, `SculkSpreader`).
//!
//! A listener's vibrations are kept in its block entity as vanilla keeps them (`listener`: the
//! one on its way and the one chosen this tick), and a catalyst's charge as its `cursors`; the
//! world notes which blocks have some (`World::sculk_active`) so only those are ticked.

use super::effects::ActiveEffect;
use super::mobs::SpawnReason;
use super::Server;
use crate::config::GameMode;
use crate::world::block_updates::sculk::{self as block, Cursor, CALIBRATED, CATALYST, SENSOR, SHRIEKER};
use crate::world::block_updates::Level;
use crate::world::chunk::ChunkPos;
use crate::world::entity::{EntityKind, Vec3};
use paper_data::blocks;
use paper_nbt::{Compound, Tag};
use paper_protocol::codec::{BlockPos, Encode, VarInt, WriteExt};
use paper_protocol::packets::play;
use rand::Rng;
use uuid::Uuid;

/// `WardenSpawnTracker`: how close a player is to calling the warden. Shriekers raise the
/// warning level (at most once every 10 seconds); it falls by one every 10 minutes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WardenTracker {
    pub ticks_since_last_warning: i32,
    pub warning_level: i32,
    pub cooldown_ticks: i32,
}

impl WardenTracker {
    pub fn tick(&mut self) {
        if self.ticks_since_last_warning >= 12000 {
            self.set_warning_level(self.warning_level - 1);
            self.ticks_since_last_warning = 0;
        } else {
            self.ticks_since_last_warning += 1;
        }
        if self.cooldown_ticks > 0 {
            self.cooldown_ticks -= 1;
        }
    }

    pub fn on_cooldown(&self) -> bool {
        self.cooldown_ticks > 0
    }

    /// `increaseWarningLevel`.
    pub fn increase(&mut self) {
        if !self.on_cooldown() {
            self.ticks_since_last_warning = 0;
            self.cooldown_ticks = 200;
            self.set_warning_level(self.warning_level + 1);
        }
    }

    pub fn set_warning_level(&mut self, level: i32) {
        self.warning_level = level.clamp(0, 4);
    }

    /// A player's saved `warden_spawn_tracker`.
    pub fn load(c: Option<&Compound>) -> WardenTracker {
        let Some(c) = c else { return WardenTracker::default() };
        WardenTracker {
            ticks_since_last_warning: c.i32("ticks_since_last_warning").unwrap_or(0),
            warning_level: c.i32("warning_level").unwrap_or(0).clamp(0, 4),
            cooldown_ticks: c.i32("cooldown_ticks").unwrap_or(0),
        }
    }

    pub fn save(&self) -> Compound {
        Compound::new()
            .with("ticks_since_last_warning", Tag::Int(self.ticks_since_last_warning))
            .with("warning_level", Tag::Int(self.warning_level))
            .with("cooldown_ticks", Tag::Int(self.cooldown_ticks))
    }
}

/// `VibrationSystem.getGameEventFrequency`: what a calibrated sensor can be tuned to and
/// what a sensor gives comparators. `bounce` is newer than the rest and not in the data; it is
/// taken to land like `hit_ground`.
pub fn frequency(event: &str) -> i32 {
    match event.strip_prefix("minecraft:").unwrap_or(event) {
        "step" | "swim" | "flap" => 1,
        "projectile_land" | "hit_ground" | "splash" | "bounce" => 2,
        "item_interact_finish" | "projectile_shoot" | "instrument_play" => 3,
        "entity_action" | "elytra_glide" | "unequip" => 4,
        "entity_dismount" | "equip" => 5,
        "entity_interact" | "shear" | "entity_mount" => 6,
        "entity_damage" => 7,
        "drink" | "eat" => 8,
        "container_close" | "block_close" | "block_deactivate" | "block_detach" => 9,
        "container_open" | "block_open" | "block_activate" | "block_attach" | "prime_fuse" | "note_block_play" => 10,
        "block_change" => 11,
        "block_destroy" | "fluid_pickup" => 12,
        "block_place" | "fluid_place" => 13,
        "entity_place" | "lightning_strike" | "teleport" => 14,
        "entity_die" | "explode" => 15,
        e => e.strip_prefix("resonate_").and_then(|n| n.parse().ok()).unwrap_or(0),
    }
}

fn event_in(tag: &str, event: &str) -> bool {
    let Some(id) = paper_data::registries::id("minecraft:game_event", event) else { return false };
    paper_data::synced::tag("minecraft:game_event", tag).is_some_and(|ids| ids.contains(&(id as i32)))
}

fn block_in(tag: &str, state: u32) -> bool {
    paper_data::synced::tag("minecraft:block", tag).is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(state) as i32)))
}

/// A vibration (`VibrationInfo`): what happened where, how far from the listener, and who
/// did it (and who shot it, for a projectile).
#[derive(Clone, Debug, PartialEq)]
struct Vibration {
    event: String,
    pos: Vec3,
    distance: f32,
    source: Option<Uuid>,
    projectile_owner: Option<Uuid>,
}

fn uuid_tag(u: Uuid) -> Tag {
    let v = u.as_u128();
    Tag::IntArray(vec![(v >> 96) as i32, (v >> 64) as i32, (v >> 32) as i32, v as i32])
}

impl Vibration {
    fn to_nbt(&self) -> Compound {
        let mut c = Compound::new()
            .with("game_event", Tag::String(self.event.clone()))
            .with("pos", Tag::List(vec![Tag::Double(self.pos.x), Tag::Double(self.pos.y), Tag::Double(self.pos.z)]))
            .with("distance", Tag::Float(self.distance));
        if let Some(u) = self.source {
            c.insert("source", uuid_tag(u));
        }
        if let Some(u) = self.projectile_owner {
            c.insert("projectile_owner", uuid_tag(u));
        }
        c
    }

    fn from_nbt(c: &Compound) -> Option<Vibration> {
        let pos = c.list("pos")?;
        let at = |i: usize| pos.get(i).and_then(Tag::as_f64);
        Some(Vibration {
            event: c.string("game_event")?.to_owned(),
            pos: Vec3::new(at(0)?, at(1)?, at(2)?),
            distance: c.f64("distance").unwrap_or(0.0) as f32,
            source: crate::world::entity_nbt::uuid_from(c.get("source")),
            projectile_owner: crate::world::entity_nbt::uuid_from(c.get("projectile_owner")),
        })
    }
}

/// A listener's state (`VibrationSystem.Data`): the vibration on its way and the ticks left,
/// and the one chosen this tick (`VibrationSelector`) with the game time it was heard.
#[derive(Clone, Debug, Default, PartialEq)]
struct ListenerData {
    current: Option<Vibration>,
    delay: i32,
    selected: Option<(Vibration, i64)>,
}

impl ListenerData {
    fn load(c: Option<&Compound>) -> ListenerData {
        let Some(c) = c else { return ListenerData::default() };
        let selected = c.compound("selector").and_then(|s| Some((Vibration::from_nbt(s.compound("event")?)?, s.i64("tick").unwrap_or(-1))));
        ListenerData { current: c.compound("event").and_then(Vibration::from_nbt), delay: c.i32("event_delay").unwrap_or(0), selected }
    }

    fn save(&self) -> Compound {
        let mut selector = Compound::new().with("tick", Tag::Long(self.selected.as_ref().map_or(-1, |s| s.1)));
        if let Some((v, _)) = &self.selected {
            selector.insert("event", Tag::Compound(v.to_nbt()));
        }
        let mut c = Compound::new().with("event_delay", Tag::Int(self.delay)).with("selector", Tag::Compound(selector));
        if let Some(v) = &self.current {
            c.insert("event", Tag::Compound(v.to_nbt()));
        }
        c
    }

    fn busy(&self) -> bool {
        self.current.is_some() || self.selected.is_some()
    }
}

/// Who did something, as vibrations and shriekers need to know.
struct Source {
    spectator: bool,
    sneaking: bool,
    /// An item entity holding wool (`dampensVibrations`).
    dampens: bool,
    uuid: Uuid,
    /// A projectile's or item's owner, by id (players and entities).
    owner: Option<i32>,
    /// The player riding it and steering.
    rider: Option<i32>,
}

impl Server {
    fn source_info(&self, world: usize, id: i32) -> Option<Source> {
        if let Some(p) = self.players.get(&id) {
            return Some(Source { spectator: p.game_mode == GameMode::Spectator, sneaking: p.sneaking, dampens: false, uuid: p.uuid(), owner: None, rider: None });
        }
        let e = self.worlds[world].entities.get(&id)?;
        let (owner, dampens) = match &e.kind {
            EntityKind::Arrow { owner, .. } | EntityKind::Thrown { owner, .. } => (*owner, false),
            EntityKind::Firework(f) => (f.owner, false),
            EntityKind::Fishing(h) => (Some(h.owner), false),
            EntityKind::Item { stack, thrower, .. } => {
                let wool = stack.0.as_ref().is_some_and(|s| paper_data::synced::tag("minecraft:item", "minecraft:dampens_vibrations").is_some_and(|ids| ids.contains(&s.item)));
                (thrower.and_then(|u| self.player_by_uuid(u)).map(|p| p.entity_id), wool)
            }
            // The warden's own doings make no vibrations.
            _ => (None, e.info.name == "minecraft:warden"),
        };
        let rider = e.passengers.first().copied().filter(|r| self.players.contains_key(r));
        Some(Source { spectator: false, sneaking: false, dampens, uuid: e.uuid, owner, rider })
    }

    /// An entity or player of this world by UUID.
    fn by_uuid(&self, world: usize, uuid: Uuid) -> Option<i32> {
        if let Some(p) = self.player_by_uuid(uuid).filter(|p| p.world == world) {
            return Some(p.entity_id);
        }
        self.worlds[world].entities.values().find(|e| e.uuid == uuid && !e.removed).map(|e| e.id)
    }

    /// `SculkShriekerBlockEntity.tryGetPlayer`: the player behind something (itself, its
    /// rider, a projectile's or an item's owner).
    fn player_behind(&self, world: usize, id: Option<i32>) -> Option<i32> {
        let id = id?;
        if self.players.get(&id).is_some_and(|p| p.world == world) {
            return Some(id);
        }
        let s = self.source_info(world, id)?;
        s.rider.or(s.owner.filter(|o| self.players.contains_key(o)))
    }

    /// The sculk listeners whose range reaches `at` (within `radius` blocks, the event's own
    /// reach), nearest first: their block, block name and range.
    fn sculk_listeners(&self, world: usize, at: Vec3, radius: i32) -> Vec<(BlockPos, &'static str, f64)> {
        let w = &self.worlds[world];
        let (bx, bz) = (at.x.floor() as i32, at.z.floor() as i32);
        let mut out = Vec::new();
        for cx in (bx - radius) >> 4..=(bx + radius) >> 4 {
            for cz in (bz - radius) >> 4..=(bz + radius) >> 4 {
                let Some(chunk) = w.chunks.get(&ChunkPos::new(cx, cz)) else { continue };
                for ((y, z, x), nbt) in &chunk.block_entities {
                    let (name, range) = match nbt.string("id") {
                        Some(SENSOR) => (SENSOR, 8.0),
                        Some(CALIBRATED) => (CALIBRATED, 16.0),
                        Some(SHRIEKER) => (SHRIEKER, 8.0),
                        Some(CATALYST) => (CATALYST, 8.0),
                        _ => continue,
                    };
                    let pos = BlockPos::new(cx * 16 + *x as i32, *y, cz * 16 + *z as i32);
                    let d2 = Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5).distance_squared(at);
                    if d2 <= range * range && (pos.y - at.y.floor() as i32).abs() <= radius + 16 {
                        out.push((pos, name, d2));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.2.total_cmp(&b.2));
        out.into_iter().map(|(p, n, d)| (p, n, d.sqrt())).collect()
    }

    /// `Level.gameEvent`: something happened at `at`, done by `source` (a player or entity), to
    /// the block state `affected` if any. Vibration listeners in range hear it.
    pub fn game_event(&mut self, world: usize, event: &str, at: Vec3, source: Option<i32>, affected: Option<u32>) {
        self.post_game_event(world, event, at, source, affected, None);
    }

    /// `entity_die` for a mob or player that would drop `xp`: catalysts in range bloom, and the
    /// nearest takes the experience to spread sculk with. Returns the experience left to drop.
    pub fn death_game_event(&mut self, world: usize, id: i32, at: Vec3, xp: i32) -> i32 {
        let mut left = Some(xp);
        self.post_game_event(world, "minecraft:entity_die", at, Some(id), None, Some(&mut left));
        left.unwrap_or(0)
    }

    fn post_game_event(&mut self, world: usize, event: &str, at: Vec3, source: Option<i32>, affected: Option<u32>, mut death_xp: Option<&mut Option<i32>>) {
        if world >= self.worlds.len() {
            return;
        }
        let event = if event.contains(':') { event.to_owned() } else { format!("minecraft:{event}") };
        // Plugins may stop it, or change how far it reaches (`GenericGameEvent`).
        let Some(radius) = crate::plugins::events::on_generic_game_event(self, world, &event, at, source, 16) else { return };
        let listeners = self.sculk_listeners(world, at, radius);
        if listeners.is_empty() {
            return;
        }
        let info = source.and_then(|s| self.source_info(world, s));
        for (pos, name, distance) in listeners {
            if name == CATALYST {
                if event == "minecraft:entity_die" {
                    let charge = death_xp.as_deref_mut().and_then(Option::take);
                    self.catalyst_hears_death(world, pos, at, charge);
                }
                continue;
            }
            // `VibrationSystem.Listener.handleGameEvent`.
            let data = ListenerData::load(self.block_entity_data(world, pos).and_then(|c| c.compound("listener")));
            if data.current.is_some() {
                continue;
            }
            let tag = if name == SHRIEKER { "minecraft:shrieker_can_listen" } else { "minecraft:vibrations" };
            if !event_in(tag, &event) {
                continue;
            }
            if let Some(s) = &info {
                if s.spectator || s.sneaking && event_in("minecraft:ignore_vibrations_sneaking", &event) || s.dampens {
                    continue;
                }
            }
            if affected.is_some_and(|a| block_in("minecraft:dampens_vibrations", a)) {
                continue;
            }
            let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { continue };
            let from = BlockPos::new(at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32);
            let mut can = match name {
                SHRIEKER => !blocks::property(state, "shrieking").is_some_and(|v| v == "true") && self.player_behind(world, source).is_some(),
                _ => {
                    let own = from == pos && matches!(event.as_str(), "minecraft:block_destroy" | "minecraft:block_place");
                    let tuned = name != CALIBRATED || {
                        let back = self.calibration(world, pos, state);
                        back == 0 || frequency(&event) == back
                    };
                    !own && tuned && block::can_activate(state)
                }
            };
            // Plugins may decide otherwise (`BlockReceiveGameEvent`).
            can = crate::plugins::events::on_block_receive_game_event(self, world, pos, &event, source, !can);
            if !can || self.vibration_occluded(world, at, pos) {
                continue;
            }
            // `VibrationSelector.addCandidate`: the nearest of the tick, the higher frequency
            // between equals.
            let now = self.worlds[world].age;
            let candidate = Vibration {
                event: event.clone(),
                pos: at,
                distance: distance as f32,
                source: info.as_ref().map(|s| s.uuid),
                projectile_owner: info.as_ref().and_then(|s| s.owner).and_then(|o| self.uuid_of(world, o)),
            };
            let replace = match &data.selected {
                None => true,
                Some((old, tick)) if *tick == now => candidate.distance < old.distance || candidate.distance == old.distance && frequency(&candidate.event) > frequency(&old.event),
                Some(_) => false,
            };
            if replace {
                let data = ListenerData { selected: Some((candidate, now)), ..data };
                self.set_block_entity_value(world, pos, "listener", Tag::Compound(data.save()));
                self.worlds[world].sculk_active.insert((pos.x, pos.y, pos.z));
            }
        }
    }

    /// `projectile_shoot`, heard where the shooter is.
    pub(super) fn projectile_shot(&mut self, world: usize, projectile: i32) {
        let Some(info) = self.source_info(world, projectile) else { return };
        let shooter = info.owner;
        let at = match shooter {
            Some(s) => match self.players.get(&s) {
                Some(p) => Some(Vec3::new(p.loc.x, p.loc.y, p.loc.z)),
                None => self.worlds[world].entities.get(&s).map(|e| e.pos),
            },
            None => None,
        };
        let Some(at) = at.or_else(|| self.worlds[world].entities.get(&projectile).map(|e| e.pos)) else { return };
        self.game_event(world, "minecraft:projectile_shoot", at, shooter.or(Some(projectile)), None);
    }

    fn uuid_of(&self, world: usize, id: i32) -> Option<Uuid> {
        if let Some(p) = self.players.get(&id) {
            return Some(p.uuid());
        }
        self.worlds[world].entities.get(&id).map(|e| e.uuid)
    }

    /// A calibrated sensor's tuning: the signal into its back (`getBackSignal`).
    fn calibration(&mut self, world: usize, pos: BlockPos, state: u32) -> i32 {
        use crate::world::block_updates::state::{get_dir, PosExt};
        let facing = get_dir(state, "facing").unwrap_or(crate::world::block_updates::Dir::North);
        let back = facing.opposite();
        Level::new(&mut self.worlds[world]).get_signal(pos.rel(back), back) as i32
    }

    /// `VibrationSystem.Listener.isOccluded`: blocked when each of six rays (from just off the
    /// centre of the source's block towards the centre of the listener) meets wool.
    fn vibration_occluded(&self, world: usize, from: Vec3, to: BlockPos) -> bool {
        let start = Vec3::new(from.x.floor() + 0.5, from.y.floor() + 0.5, from.z.floor() + 0.5);
        let end = Vec3::new(to.x as f64 + 0.5, to.y as f64 + 0.5, to.z as f64 + 0.5);
        let w = &self.worlds[world];
        for (dx, dy, dz) in [(0.0, -1.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, -1.0), (0.0, 0.0, 1.0), (-1.0, 0.0, 0.0), (1.0, 0.0, 0.0)] {
            let s = Vec3::new(start.x + dx * 1.0e-5, start.y + dy * 1.0e-5, start.z + dz * 1.0e-5);
            let blocked = blocks_on_line(s, end).into_iter().any(|p| w.block_at(p.x, p.y, p.z).is_some_and(|b| block_in("minecraft:occludes_vibration_signals", b)));
            if !blocked {
                return false;
            }
        }
        true
    }

    /// The listeners and catalysts with something going on, every tick: vibrations chosen
    /// last tick set off, those on their way arrive, charge spreads.
    pub(super) fn tick_sculk(&mut self) {
        for world in 0..self.worlds.len() {
            if self.worlds[world].sculk_active.is_empty() {
                continue;
            }
            let active: Vec<BlockPos> = self.worlds[world].sculk_active.iter().map(|(x, y, z)| BlockPos::new(*x, *y, *z)).collect();
            for pos in active {
                let chunk = ChunkPos::of_block(pos.x, pos.z);
                if !self.worlds[world].is_ready(chunk) {
                    continue;
                }
                let still = match self.block_entity_data(world, pos).and_then(|c| c.string("id")) {
                    Some(CATALYST) => self.tick_catalyst(world, pos),
                    Some(SENSOR | CALIBRATED | SHRIEKER) => self.tick_listener(world, pos),
                    _ => false,
                };
                if !still {
                    self.worlds[world].sculk_active.remove(&(pos.x, pos.y, pos.z));
                }
            }
            self.publish_block_updates(world);
        }
    }

    /// `VibrationSystem.Ticker.tick` for one listener. Returns whether it still has one.
    fn tick_listener(&mut self, world: usize, pos: BlockPos) -> bool {
        let mut data = ListenerData::load(self.block_entity_data(world, pos).and_then(|c| c.compound("listener")));
        let now = self.worlds[world].age;
        if data.current.is_none() {
            if let Some((v, tick)) = data.selected.clone() {
                if tick < now {
                    // Off it goes, shown as a particle flying to the listener.
                    data.delay = v.distance.floor() as i32;
                    self.vibration_particle(world, v.pos, pos, data.delay);
                    data.current = Some(v);
                    data.selected = None;
                }
            }
        }
        if let Some(v) = data.current.clone() {
            data.delay = (data.delay - 1).max(0);
            if data.delay <= 0 {
                data.current = None;
                self.set_block_entity_value(world, pos, "listener", Tag::Compound(data.save()));
                self.receive_vibration(world, pos, &v);
                return ListenerData::load(self.block_entity_data(world, pos).and_then(|c| c.compound("listener"))).busy();
            }
        }
        self.set_block_entity_value(world, pos, "listener", Tag::Compound(data.save()));
        data.busy()
    }

    fn vibration_particle(&self, world: usize, from: Vec3, to: BlockPos, ticks: i32) {
        let Some(id) = paper_data::registries::id("minecraft:particle_type", "minecraft:vibration") else { return };
        let mut particle = Vec::new();
        VarInt(id as i32).encode(&mut particle);
        particle.put_varint(0);
        particle.put_position(to);
        particle.put_varint(ticks);
        let packet = play::LevelParticles { particle, override_limiter: false, always_show: false, x: from.x, y: from.y, z: from.z, offset: (0.0, 0.0, 0.0), max_speed: (0.0, 0.0, 0.0), count: 1, randomization: 0 };
        for p in self.players.values().filter(|p| p.world == world) {
            if Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(from) < 32.0 * 32.0 {
                p.conn.send(&packet);
            }
        }
    }

    /// A vibration arrived (`onReceiveVibration`).
    fn receive_vibration(&mut self, world: usize, pos: BlockPos, v: &Vibration) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let source = v.source.and_then(|u| self.by_uuid(world, u));
        let owner = v.projectile_owner.and_then(|u| self.by_uuid(world, u));
        if blocks::block_of_state(state).name == SHRIEKER {
            let player = self.player_behind(world, owner.or(source));
            self.try_shriek(world, pos, player);
            return;
        }
        if !block::can_activate(state) {
            return;
        }
        // The power falls off with the distance between the two blocks.
        let from = BlockPos::new(v.pos.x.floor() as i32, v.pos.y.floor() as i32, v.pos.z.floor() as i32);
        let distance = (((from.x - pos.x) as f64).powi(2) + ((from.y - pos.y) as f64).powi(2) + ((from.z - pos.z) as f64).powi(2)).sqrt();
        let range = if blocks::block_of_state(state).name == CALIBRATED { 16.0 } else { 8.0 };
        let power = (15 - (15.0 / range * distance).floor() as i32).max(1);
        let frequency = frequency(&v.event);
        self.set_block_entity_value(world, pos, "last_vibration_frequency", Tag::Int(frequency));
        self.activate_sensor(world, pos, state, power, frequency, source);
    }

    /// `SculkSensorBlock.activate`: plugins may change the power (`BlockRedstoneEvent`); the
    /// amethyst around resonates, and shriekers near hear the clicking.
    fn activate_sensor(&mut self, world: usize, pos: BlockPos, state: u32, power: i32, frequency: i32, source: Option<i32>) {
        let old = blocks::property(state, "power").and_then(|p| p.parse().ok()).unwrap_or(0);
        let power = crate::plugins::events::on_block_redstone(self, world, pos, old, power);
        if power <= 0 {
            return;
        }
        block::activate(&mut Level::new(&mut self.worlds[world]), state, pos, power);
        for (dx, dy, dz) in [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)] {
            let p = pos.offset(dx, dy, dz);
            let Some(s) = self.worlds[world].block_at(p.x, p.y, p.z) else { continue };
            if block_in("minecraft:vibration_resonators", s) {
                self.post_game_event(world, &format!("minecraft:resonate_{frequency}"), Vec3::new(p.x as f64 + 0.5, p.y as f64 + 0.5, p.z as f64 + 0.5), source, Some(s), None);
                let pitch = RESONATE_PITCH.get(frequency as usize).copied().unwrap_or(1.0);
                self.play_sound(world, Vec3::new(p.x as f64 + 0.5, p.y as f64 + 0.5, p.z as f64 + 0.5), "minecraft:block.amethyst_block.resonate", crate::world::block_updates::sound_source::BLOCKS, 1.0, pitch, None);
            }
        }
        self.post_game_event(world, "minecraft:sculk_sensor_tendrils_clicking", Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5), source, None, None);
    }

    // --- shriekers and the warden -------------------------------------------------------------

    /// `SculkShriekerBlockEntity.tryShriek`: a player set it off (by a sensor or by stepping
    /// on it); one that can call the warden first warns the players near.
    pub(super) fn try_shriek(&mut self, world: usize, pos: BlockPos, player: Option<i32>) {
        let Some(player) = player else { return };
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        if blocks::block_of_state(state).name != SHRIEKER || blocks::property(state, "shrieking") == Some("true") {
            return;
        }
        let mut warning = 0;
        if self.can_respond(world, state) {
            match self.try_warn(world, pos, player) {
                Some(level) => warning = level,
                None => return,
            }
        }
        self.set_block_entity_value(world, pos, "warning_level", Tag::Int(warning));
        self.shriek(world, pos, state, Some(player));
    }

    fn can_respond(&self, world: usize, state: u32) -> bool {
        blocks::property(state, "can_summon") == Some("true") && self.difficulty != crate::config::Difficulty::Peaceful && self.worlds[world].rules.bool("spawn_wardens")
    }

    /// `WardenSpawnTracker.tryWarn`: unless a warden is near or someone was warned just now,
    /// everyone within 16 blocks gets the highest warning among them, one higher.
    fn try_warn(&mut self, world: usize, pos: BlockPos, player: i32) -> Option<i32> {
        let center = Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5);
        let area = crate::world::entity::Aabb::new(Vec3::new(center.x - 24.0, center.y - 24.0, center.z - 24.0), Vec3::new(center.x + 24.0, center.y + 24.0, center.z + 24.0));
        let w = &self.worlds[world];
        if w.entities_in(&area).into_iter().any(|id| w.entities.get(&id).is_some_and(|e| e.info.name == "minecraft:warden")) {
            return None;
        }
        let mut near: Vec<i32> = self.players.values().filter(|p| p.world == world && p.game_mode != GameMode::Spectator && !p.dead && Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(center) < 16.0 * 16.0).map(|p| p.entity_id).collect();
        if !near.contains(&player) {
            near.push(player);
        }
        if near.iter().any(|p| self.players.get(p).is_some_and(|p| p.warden.on_cooldown())) {
            return None;
        }
        let mut tracker = near.iter().filter_map(|p| self.players.get(p)).map(|p| p.warden).max_by_key(|t| t.warning_level)?;
        tracker.increase();
        for p in near {
            if let Some(p) = self.players.get_mut(&p) {
                p.warden = tracker;
            }
        }
        Some(tracker.warning_level)
    }

    /// `shriek`: shrieking for 90 ticks, with the shriek particle and sound.
    fn shriek(&mut self, world: usize, pos: BlockPos, state: u32, source: Option<i32>) {
        let shrieking = blocks::with_property(state, "shrieking", "true").unwrap_or(state);
        let mut level = Level::new(&mut self.worlds[world]);
        level.set_block(pos, shrieking, crate::world::block_updates::flags::CLIENTS);
        level.schedule_tick(pos, crate::world::block_updates::state::block_id(state), 90);
        level.level_event(3007, pos, 0);
        self.publish_block_updates(world);
        self.post_game_event(world, "minecraft:shriek", Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5), source, None, None);
    }

    /// The shriek is over (`tryRespond`): at the fourth warning the warden comes up, else it
    /// is heard from afar; darkness falls on players within 40 blocks either way.
    pub(super) fn shrieker_done(&mut self, world: usize, pos: BlockPos) {
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let warning = self.block_entity_data(world, pos).and_then(|c| c.i32("warning_level")).unwrap_or(0);
        if !self.can_respond(world, state) || warning <= 0 {
            return;
        }
        if warning < 4 || !self.summon_warden(world, pos) {
            let sound = match warning {
                1 => "minecraft:entity.warden.nearby_close",
                2 => "minecraft:entity.warden.nearby_closer",
                3 => "minecraft:entity.warden.nearby_closest",
                _ => "minecraft:entity.warden.listening_angry",
            };
            let mut rng = rand::thread_rng();
            let at = Vec3::new((pos.x + rng.gen_range(-10..=10)) as f64, (pos.y + rng.gen_range(-10..=10)) as f64, (pos.z + rng.gen_range(-10..=10)) as f64);
            self.play_sound(world, at, sound, super::survival::category::HOSTILE, 5.0, 1.0, None);
        }
        self.darkness_around(world, Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5), 40.0);
    }

    /// `Warden.applyDarknessAround`: survival and adventure players in range whose darkness
    /// is about to end (or who have none) get 13 seconds of it.
    pub(super) fn darkness_around(&mut self, world: usize, at: Vec3, radius: f64) {
        let Some(darkness) = paper_data::effects::by_name("minecraft:darkness").map(|e| e.id) else { return };
        let players: Vec<i32> = self
            .players
            .values()
            .filter(|p| p.world == world && !p.dead && matches!(p.game_mode, GameMode::Survival | GameMode::Adventure) && Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(at) < radius * radius)
            .filter(|p| p.effects.iter().find(|e| e.id == darkness).is_none_or(|e| e.duration >= 0 && e.duration < 200))
            .map(|p| p.entity_id)
            .collect();
        for p in players {
            if let Some(effect) = ActiveEffect::new("minecraft:darkness", 260, 0) {
                let effect = ActiveEffect { ambient: false, particles: false, ..effect };
                self.add_effect(world, p, effect, "WARDEN");
            }
        }
    }

    /// `trySummonWarden` (`SpawnUtil.trySpawnMob`, `ON_TOP_OF_COLLIDER`): twenty tries at a
    /// spot within 5 blocks across and 6 up or down, on top of a block, with room for it.
    fn summon_warden(&mut self, world: usize, pos: BlockPos) -> bool {
        use paper_data::physics;
        let mut rng = rand::thread_rng();
        for _ in 0..20 {
            let (x, z) = (pos.x + rng.gen_range(-5..=5), pos.z + rng.gen_range(-5..=5));
            let mut y = pos.y + 6;
            let w = &self.worlds[world];
            if !w.border.contains(x as f64 + 0.5, z as f64 + 0.5) {
                continue;
            }
            let mut above = w.block_at(x, y, z).unwrap_or(blocks::AIR);
            let mut found = None;
            for _ in 0..=12 {
                y -= 1;
                let here = w.block_at(x, y, z).unwrap_or(blocks::AIR);
                if !blocks::has_collision(above) && physics::collision_face_full(here, 1) {
                    found = Some(y + 1);
                    break;
                }
                above = here;
            }
            let Some(y) = found else { continue };
            let at = Vec3::new(x as f64 + 0.5, y as f64, z as f64 + 0.5);
            let room = crate::world::entity::Aabb::of_entity(at, 0.9, 2.9);
            if crate::world::entity::collides(w, &room) {
                continue;
            }
            if self.spawn_mob(world, "minecraft:warden", at, SpawnReason::Natural).is_some() {
                self.play_sound(world, at, "minecraft:entity.warden.agitated", super::survival::category::HOSTILE, 5.0, 1.0, None);
                return true;
            }
        }
        false
    }

    // --- catalysts -----------------------------------------------------------------------------

    /// `SculkCatalystBlockEntity.CatalystListener`: it blooms, and with `charge` (the first
    /// catalyst to hear a death gets the experience) the sculk spreads from where it died.
    fn catalyst_hears_death(&mut self, world: usize, pos: BlockPos, at: Vec3, charge: Option<i32>) {
        if let Some(xp) = charge.filter(|xp| *xp > 0) {
            let mut cursors = self.catalyst_cursors(world, pos);
            block::add_cursors(&mut cursors, BlockPos::new(at.x.floor() as i32, (at.y + 0.5).floor() as i32, at.z.floor() as i32), xp);
            self.set_catalyst_cursors(world, pos, &cursors);
            self.worlds[world].sculk_active.insert((pos.x, pos.y, pos.z));
        }
        let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return };
        let bloom = blocks::with_property(state, "bloom", "true").unwrap_or(state);
        let mut level = Level::new(&mut self.worlds[world]);
        level.set_block(pos, bloom, crate::world::block_updates::flags::ALL);
        level.schedule_tick(pos, crate::world::block_updates::state::block_id(state), 8);
        let pitch = 0.6 + level.random_float() * 0.4;
        level.play_sound(None, pos, "minecraft:block.sculk_catalyst.bloom", crate::world::block_updates::sound_source::BLOCKS, 2.0, pitch);
        self.publish_block_updates(world);
        self.soul_particles(world, Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 1.15, pos.z as f64 + 0.5));
    }

    fn soul_particles(&self, world: usize, at: Vec3) {
        let Some(id) = paper_data::registries::id("minecraft:particle_type", "minecraft:sculk_soul") else { return };
        let mut particle = Vec::new();
        VarInt(id as i32).encode(&mut particle);
        let packet = play::LevelParticles { particle, override_limiter: false, always_show: false, x: at.x, y: at.y, z: at.z, offset: (0.2, 0.0, 0.2), max_speed: (0.0, 0.0, 0.0), count: 2, randomization: 0 };
        for p in self.players.values().filter(|p| p.world == world) {
            if Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(at) < 32.0 * 32.0 {
                p.conn.send(&packet);
            }
        }
    }

    fn catalyst_cursors(&self, world: usize, pos: BlockPos) -> Vec<Cursor> {
        self.block_entity_data(world, pos).and_then(|c| c.list("cursors")).unwrap_or(&[]).iter().filter_map(|t| t.as_compound()).filter_map(Cursor::from_nbt).collect()
    }

    fn set_catalyst_cursors(&mut self, world: usize, pos: BlockPos, cursors: &[Cursor]) {
        self.set_block_entity_value(world, pos, "cursors", Tag::List(cursors.iter().map(|c| Tag::Compound(c.to_nbt())).collect()));
    }

    /// The catalyst's spreader moves on. Returns whether charge is left.
    fn tick_catalyst(&mut self, world: usize, pos: BlockPos) -> bool {
        let mut cursors = self.catalyst_cursors(world, pos);
        if cursors.is_empty() {
            return false;
        }
        block::update_cursors(&mut Level::new(&mut self.worlds[world]), pos, &mut cursors);
        // The catalyst itself may be gone now.
        if self.block_entity_data(world, pos).is_some() {
            self.set_catalyst_cursors(world, pos, &cursors);
        }
        !cursors.is_empty()
    }

    /// A chunk was loaded: its sculk with vibrations or charge on the way goes on.
    pub(super) fn sculk_chunk_loaded(&mut self, world: usize, pos: ChunkPos) {
        let Some(chunk) = self.worlds[world].chunks.get(&pos) else { return };
        let mut found = Vec::new();
        for ((y, z, x), nbt) in &chunk.block_entities {
            let busy = match nbt.string("id") {
                Some(CATALYST) => nbt.list("cursors").is_some_and(|l| !l.is_empty()),
                Some(SENSOR | CALIBRATED | SHRIEKER) => ListenerData::load(nbt.compound("listener")).busy(),
                _ => false,
            };
            if busy {
                found.push(BlockPos::new(pos.x * 16 + *x as i32, *y, pos.z * 16 + *z as i32));
            }
        }
        self.worlds[world].sculk_active.extend(found.into_iter().map(|p| (p.x, p.y, p.z)));
    }
}

/// `SculkSensorBlock.RESONATE_PITCH_BEND`: the amethyst's note for each frequency.
const RESONATE_PITCH: [f32; 16] = [0.5, 0.5, 0.529_731_5, 0.561_231_0, 0.594_603_6, 0.629_960_5, 0.667_419_9, 0.707_106_8, 0.749_153_5, 0.793_700_5, 0.840_896_4, 0.890_898_7, 0.943_874_3, 1.0, 1.059_463_1, 1.122_462_0];

/// The blocks a line passes through, in order (`BlockGetter.traverseBlocks`).
fn blocks_on_line(from: Vec3, to: Vec3) -> Vec<BlockPos> {
    let mut out = Vec::new();
    if from.x == to.x && from.y == to.y && from.z == to.z {
        return out;
    }
    // Vanilla nudges both ends towards each other by a hair.
    let lerp = |a: f64, b: f64| b + (a - b) * -1.0e-7;
    let (ex, ey, ez) = (lerp(to.x, from.x), lerp(to.y, from.y), lerp(to.z, from.z));
    let (sx, sy, sz) = (lerp(from.x, to.x), lerp(from.y, to.y), lerp(from.z, to.z));
    let (mut x, mut y, mut z) = (sx.floor() as i32, sy.floor() as i32, sz.floor() as i32);
    out.push(BlockPos::new(x, y, z));
    let (dx, dy, dz) = (ex - sx, ey - sy, ez - sz);
    let step = |d: f64| if d > 0.0 { 1 } else if d < 0.0 { -1 } else { 0 };
    let (stx, sty, stz) = (step(dx), step(dy), step(dz));
    let delta = |s: i32, d: f64| if s == 0 { f64::MAX } else { s as f64 / d };
    let (tdx, tdy, tdz) = (delta(stx, dx), delta(sty, dy), delta(stz, dz));
    let frac = |v: f64| v - v.floor();
    let first = |s: i32, t: f64, v: f64| t * if s > 0 { 1.0 - frac(v) } else { frac(v) };
    let (mut tx, mut ty, mut tz) = (first(stx, tdx, sx), first(sty, tdy, sy), first(stz, tdz, sz));
    while tx <= 1.0 || ty <= 1.0 || tz <= 1.0 {
        if tx < ty {
            if tx < tz {
                x += stx;
                tx += tdx;
            } else {
                z += stz;
                tz += tdz;
            }
        } else if ty < tz {
            y += sty;
            ty += tdy;
        } else {
            z += stz;
            tz += tdz;
        }
        out.push(BlockPos::new(x, y, z));
        if out.len() > 128 {
            break;
        }
    }
    out
}
