//! Players entering and leaving the server.

use super::player::{ChunkView, KeepAlive, Location, Player, INVENTORY_SLOTS};
use super::{JoinRequest, Server};
use crate::config::GameMode;
use paper_protocol::codec::{ByteArray, VarInt};
use paper_protocol::packets::play;
use paper_protocol::text::Component;
use paper_protocol::types::Slot;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Instant;

impl Server {
    pub(super) fn handle_join(&mut self, request: JoinRequest) {
        let JoinRequest { entity_id, profile, settings, brand, virtual_host, conn } = request;

        // A second login with the same identity replaces the first, as in vanilla.
        if let Some(existing) = self.by_uuid.get(&profile.uuid).copied() {
            self.kick(existing, &Component::translatable("multiplayer.disconnect.duplicate_login", vec![]));
        }

        // Returning players continue where they left off.
        let saved = self.load_player_data(profile.uuid);
        let world_index = saved.as_ref().map_or(0, |s| s.world.min(self.worlds.len() - 1));
        let world = &self.worlds[world_index];
        let spawn = world.spawn;
        let loc = match &saved {
            Some(s) => s.loc,
            None => Location { x: spawn.x as f64 + 0.5, y: spawn.y as f64, z: spawn.z as f64 + 0.5, yaw: world.spawn_yaw, pitch: 0.0 },
        };
        let server_view = self.config.properties.view_distance;
        let op_level = self.shared.access.read().op_level(profile.uuid);
        let now = Instant::now();

        let mut player = Player {
            entity_id,
            profile,
            conn,
            settings,
            brand,
            world: world_index,
            loc,
            on_ground: false,
            tracked_loc: loc,
            tracked_on_ground: false,
            force_position_sync: true,
            ticks_since_position_sync: 0,
            visible: HashSet::new(),
            seen_by: HashSet::new(),
            game_mode: self.default_game_mode,
            previous_game_mode: None,
            flying: false,
            allow_flight: false,
            sneaking: false,
            sprinting: false,
            metadata_dirty: false,
            equipment_dirty: false,
            inventory: vec![Slot::EMPTY; INVENTORY_SLOTS],
            held_slot: 0,
            carried: Slot::EMPTY,
            inventory_state: 0,
            view: ChunkView::new(loc.chunk(), server_view),
            pending_teleport: None,
            next_teleport_id: 1,
            keep_alive: KeepAlive { last_sent: now, pending: None, latency: 0 },
            loaded: false,
            op_level,
            display_name: None,
            list_name: None,
            health: 20.0,
            food: 20,
            saturation: 5.0,
            joined_at: now,
            last_action: now,
            virtual_host,
            last_move_event: loc,
            leaving: false,
            open_menu: None,
            drag: Default::default(),
            ender_items: vec![Slot::EMPTY; 27],
            next_window_id: 1,
            exhaustion: 0.0,
            food_timer: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            air: 300,
            invulnerable_time: 0,
            last_hurt: 0.0,
            dead: false,
            using_item: None,
            last_swing: 0,
            xp_level: 0,
            xp_progress: 0.0,
            xp_total: 0,
            respawn: None,
            last_attacker: None,
        };
        player.view.radius = player.view_distance(server_view);
        if let Some(saved) = saved {
            if let Some(mode) = saved.game_mode {
                player.game_mode = mode;
            }
            player.previous_game_mode = saved.previous_game_mode;
            player.health = saved.health;
            player.food = saved.food;
            player.saturation = saved.saturation;
            player.held_slot = saved.held_slot;
            player.inventory = saved.inventory;
            player.apply_game_mode_abilities();
            player.flying = saved.flying && player.allow_flight;
        } else {
            player.apply_game_mode_abilities();
        }

        let world = &self.worlds[world_index];
        let props = &self.config.properties;
        player.conn.send(&play::Login {
            entity_id,
            hardcore: props.hardcore,
            dimension_names: self.worlds.iter().map(|w| w.dimension.clone()).collect(),
            max_players: VarInt(props.max_players),
            view_distance: VarInt(player.view.radius),
            simulation_distance: VarInt(props.simulation_distance),
            reduced_debug_info: false,
            enable_respawn_screen: true,
            limited_crafting: false,
            spawn: self.spawn_info(world_index, player.game_mode, None),
            online_mode: props.online_mode,
            enforces_secure_chat: false,
        });
        player.conn.send(&play::ChangeDifficulty { difficulty: self.difficulty.id(), locked: false });
        player.conn.send(&play::PlayerAbilities { flags: player.ability_flags(), flying_speed: 0.05, fov_modifier: 0.1 });
        player.conn.send(&play::SetHeldSlot { slot: VarInt(player.held_slot as i32) });
        // Entity status 24-28 tells the client its operator permission level (0-4).
        player.conn.send(&play::EntityEvent { entity_id, status: 24 + player.op_level.min(4) as i8 });

        // Place the player; the client confirms the teleport before its movement counts.
        let teleport_id = player.next_teleport_id;
        player.next_teleport_id += 1;
        player.pending_teleport = Some((teleport_id, loc));
        player.conn.send(&play::PlayerPosition { teleport_id: VarInt(teleport_id), x: loc.x, y: loc.y, z: loc.z, velocity_x: 0.0, velocity_y: 0.0, velocity_z: 0.0, yaw: loc.yaw, pitch: loc.pitch, flags: 0 });

        let status = self.shared.status.read().clone();
        player.conn.send(&play::ServerData { motd: status.motd.clone(), icon: None::<ByteArray> });

        // The player list: everyone already here (and the newcomer) for the newcomer, the
        // newcomer for everyone else.
        let actions = play::player_info::ADD_PLAYER
            | play::player_info::UPDATE_GAME_MODE
            | play::player_info::UPDATE_LISTED
            | play::player_info::UPDATE_LATENCY
            | play::player_info::UPDATE_DISPLAY_NAME;
        let own_entry = info_entry(&player);
        let mut entries: Vec<play::PlayerInfoEntry> = self.online_players().map(info_entry).collect();
        entries.push(own_entry.clone());
        player.conn.send(&play::PlayerInfoUpdate { actions, entries });
        self.broadcast(&play::PlayerInfoUpdate { actions, entries: vec![own_entry] });

        player.conn.send_payload(self.time_payload(world_index));
        player.conn.send(&play::SetDefaultSpawnPosition { dimension: world.dimension.clone(), pos: world.spawn, yaw: world.spawn_yaw, pitch: 0.0 });
        player.conn.send(&play::GameEvent { event: play::game_event::START_WAITING_FOR_CHUNKS, value: 0.0 });
        player.conn.send(&play::SetChunkCacheCenter { x: VarInt(player.view.center.x), z: VarInt(player.view.center.z) });
        player.conn.send(&play::ContainerSetContent { window_id: 0, state_id: player.inventory_state, slots: player.inventory.clone(), carried: Slot::EMPTY });
        player.conn.send(&play::SetHealth { health: player.health, food: VarInt(player.food), saturation: player.saturation });
        player.conn.send(&play::SetExperience { bar: 0.0, level: VarInt(0), total: VarInt(0) });

        let name = player.name().to_owned();
        let uuid = player.uuid();
        let address = player.conn.addr;
        self.players.insert(entity_id, player);
        self.player_order.push(entity_id);
        self.by_uuid.insert(uuid, entity_id);
        self.shared.player_count.store(self.players.len(), Ordering::Relaxed);
        self.mark_status_dirty();
        self.refresh_chunk_view(entity_id);

        let shown_address = if self.config.properties.log_ips { address.to_string() } else { "<ip address withheld>".to_owned() };
        log::info!("{name}[/{shown_address}] logged in with entity id {entity_id} at ([{}]{:.1}, {:.1}, {:.1})", self.worlds[world_index].name, loc.x, loc.y, loc.z);

        // Plugins see the player from here on (PlayerJoinEvent), so the command tree, which
        // depends on their permissions, is sent afterwards.
        let message = Component::translatable("multiplayer.player.joined", vec![Component::text(name)]).color("yellow");
        let message = crate::plugins::events::on_join(self, entity_id, message);
        if let Some(message) = message {
            self.broadcast_message(&message);
        }
        if let Some(level) = self.players.get(&entity_id).map(|p| p.op_level) {
            let tree = self.command_tree_for(entity_id, level);
            self.send(entity_id, &tree);
        }
    }

    /// The world description sent in Login and Respawn packets.
    pub(super) fn spawn_info(&self, world: usize, game_mode: GameMode, previous: Option<GameMode>) -> play::SpawnInfo {
        let w = &self.worlds[world];
        play::SpawnInfo {
            dimension_type: w.dimension_type,
            dimension_name: w.dimension.clone(),
            hashed_seed: w.hashed_seed(),
            game_mode: game_mode.id(),
            previous_game_mode: previous.map(GameMode::id),
            is_debug: false,
            is_flat: w.generator.is_flat(),
            death_location: None,
            portal_cooldown: 0,
            sea_level: w.generator.sea_level(),
        }
    }

    /// The connection of a player ended (closed by the client, timed out, or bad data).
    pub(super) fn handle_disconnect(&mut self, entity_id: i32, reason: &str) {
        self.remove_player(entity_id, reason);
    }

    /// Disconnects a player with a message shown on their screen.
    pub fn kick(&mut self, entity_id: i32, reason: &Component) {
        let Some(player) = self.players.get(&entity_id) else { return };
        player.conn.disconnect(reason);
        let plain = reason.to_plain();
        self.remove_player(entity_id, &plain);
    }

    /// Removes a player from the game and tells everyone else. Idempotent.
    pub(super) fn remove_player(&mut self, entity_id: i32, reason: &str) {
        let Some(player) = self.players.get_mut(&entity_id) else { return };
        if player.leaving {
            // A quit listener kicked the player that is already leaving.
            return;
        }
        player.leaving = true;
        let (uuid, name) = (player.uuid(), player.name().to_owned());
        // PlayerQuitEvent fires while the player is still in the game, as in Paper.
        let message = Component::translatable("multiplayer.player.left", vec![Component::text(name)]).color("yellow");
        let message = crate::plugins::events::on_quit(self, uuid, reason, message);
        if let Some(player) = self.players.get(&entity_id) {
            self.save_player_data(player);
        }
        let Some(player) = self.players.remove(&entity_id) else { return };
        self.player_order.retain(|id| *id != entity_id);
        if self.by_uuid.get(&player.uuid()) == Some(&entity_id) {
            self.by_uuid.remove(&player.uuid());
        }
        self.shared.player_count.store(self.players.len(), Ordering::Relaxed);
        self.mark_status_dirty();

        // Release the chunks this player kept loaded.
        if let Some(world) = self.worlds.get_mut(player.world) {
            for pos in &player.view.tracked {
                world.remove_ticket(*pos);
            }
        }

        // Others forget the entity and the player list entry.
        let remove_entity = play::RemoveEntities { entity_ids: vec![VarInt(entity_id)] };
        for viewer in &player.seen_by {
            if let Some(other) = self.players.get_mut(viewer) {
                other.visible.remove(&entity_id);
                other.conn.send(&remove_entity);
            }
        }
        for target in &player.visible {
            if let Some(other) = self.players.get_mut(target) {
                other.seen_by.remove(&entity_id);
            }
        }
        self.broadcast(&play::PlayerInfoRemove { uuids: vec![player.uuid()] });

        log::info!("{} lost connection: {reason}", player.name());
        if let Some(message) = message {
            self.broadcast_message(&message);
        }
    }
}

/// The player list entry describing `player`.
pub fn info_entry(player: &Player) -> play::PlayerInfoEntry {
    play::PlayerInfoEntry {
        uuid: player.uuid(),
        name: player.name().to_owned(),
        properties: player.profile.properties.clone(),
        game_mode: player.game_mode.id(),
        listed: true,
        latency: player.keep_alive.latency,
        display_name: player.list_name.clone(),
        list_priority: 0,
        show_hat: player.settings.skin_parts & 0x40 != 0,
    }
}
