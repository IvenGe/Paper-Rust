//! Handlers for the packets players send during play.

use super::commands::CommandSource;
use super::player::{Location, HOTBAR_START, INVENTORY_SLOTS, OFFHAND_SLOT};
use super::Server;
use crate::config::GameMode;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, Reader, VarInt};
use paper_protocol::packets::play::{self, Serverbound};
use paper_protocol::text::Component;
use paper_protocol::types::Slot;
use std::time::Instant;

/// Farthest block a player may interact with, measured from the eyes to the block centre.
/// Creative reach is 5 blocks; the slack absorbs latency and the block's own extent.
const MAX_REACH: f64 = 7.0;

/// Offsets of the six block faces in protocol order: bottom, top, north, south, west, east.
const FACE_OFFSETS: [(i32, i32, i32); 6] = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
const FACE_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// The horizontal direction a yaw faces: south, west, north, east.
fn horizontal_facing(yaw: f32) -> &'static str {
    ["south", "west", "north", "east"][((yaw / 90.0 + 0.5).floor() as i32 & 3) as usize]
}

fn opposite(direction: &str) -> &'static str {
    match direction {
        "north" => "south",
        "south" => "north",
        "west" => "east",
        "east" => "west",
        "up" => "down",
        _ => "up",
    }
}

fn direction_offset(direction: &str) -> (i32, i32, i32) {
    match direction {
        "north" => (0, 0, -1),
        "south" => (0, 0, 1),
        "west" => (-1, 0, 0),
        "east" => (1, 0, 0),
        "up" => (0, 1, 0),
        _ => (0, -1, 0),
    }
}

/// The wall-mounted variant of a block placed against the side of another block, if the
/// block has one (`torch` -> `wall_torch`, `oak_sign` -> `oak_wall_sign`, ...).
fn wall_variant(block: &str) -> Option<&'static paper_data::BlockInfo> {
    let name = block.strip_prefix("minecraft:").unwrap_or(block);
    for suffix in ["hanging_sign", "torch", "sign", "banner", "skull", "head", "fan"] {
        if let Some(prefix) = name.strip_suffix(suffix) {
            if let Some(info) = blocks::by_name(&format!("minecraft:{prefix}wall_{suffix}")) {
                return Some(info);
            }
        }
    }
    None
}

/// Chooses the block state for a newly placed block from how it was placed. This covers
/// the orientation rules builders rely on most (axes, stairs, slabs, wall attachments,
/// signs); it is not the full vanilla placement logic.
fn placement_state(block: &'static paper_data::BlockInfo, face: usize, cursor_y: f32, yaw: f32, pitch: f32) -> u32 {
    let name = block.name;
    let mut state = block.default_state;
    let has = |prop: &str, value: &str| block.properties.iter().any(|(p, values)| *p == prop && values.contains(&value));
    let set = |state: &mut u32, prop: &str, value: &str| {
        if let Some(s) = blocks::with_property(*state, prop, value) {
            *state = s;
        }
    };
    let looking = horizontal_facing(yaw);
    let face_name = FACE_NAMES[face];
    let upper_half = face == 0 || (face != 1 && cursor_y > 0.5);

    if block.properties.iter().any(|(p, _)| *p == "axis") {
        set(&mut state, "axis", ["y", "y", "z", "z", "x", "x"][face]);
    }
    if let Some((_, values)) = block.properties.iter().find(|(p, _)| *p == "facing") {
        let vertical_ok = values.contains(&"up");
        let facing = if name.contains("wall_") || name.ends_with("ladder") || name.ends_with("_button") || name.ends_with("lever") {
            // Attached to the clicked block: points away from it.
            if face >= 2 || vertical_ok {
                face_name
            } else {
                opposite(looking)
            }
        } else if name.ends_with("_stairs") || name.ends_with("_door") || name.ends_with("_fence_gate") || name.ends_with("_bed") {
            looking
        } else if vertical_ok {
            // Six-way blocks (pistons, dispensers, barrels, rods...) face the placer.
            if pitch > 45.0 {
                "up"
            } else if pitch < -45.0 {
                "down"
            } else {
                opposite(looking)
            }
        } else {
            opposite(looking)
        };
        set(&mut state, "facing", facing);
    }
    if has("half", "top") {
        set(&mut state, "half", if upper_half { "top" } else { "bottom" });
    }
    if has("type", "top") && has("type", "bottom") {
        set(&mut state, "type", if upper_half { "top" } else { "bottom" });
    }
    if has("face", "floor") {
        set(&mut state, "face", ["ceiling", "floor", "wall", "wall", "wall", "wall"][face]);
    }
    if block.properties.iter().any(|(p, _)| *p == "rotation") {
        let rotation = (((180.0 + yaw) * 16.0 / 360.0 + 0.5).floor() as i32) & 15;
        set(&mut state, "rotation", &rotation.to_string());
    }
    state
}

impl Server {
    pub(super) fn handle_packet(&mut self, id: i32, packet: Serverbound) {
        let Some(player) = self.players.get_mut(&id) else { return };
        match packet {
            Serverbound::AcceptTeleportation(p) => {
                if let Some((teleport_id, target)) = player.pending_teleport {
                    if teleport_id == p.teleport_id.0 {
                        player.loc = target;
                        player.pending_teleport = None;
                    }
                }
            }
            Serverbound::MovePlayerPos(p) => self.handle_move(id, Some((p.x, p.y, p.z)), None, p.flags),
            Serverbound::MovePlayerPosRot(p) => self.handle_move(id, Some((p.x, p.y, p.z)), Some((p.yaw, p.pitch)), p.flags),
            Serverbound::MovePlayerRot(p) => self.handle_move(id, None, Some((p.yaw, p.pitch)), p.flags),
            Serverbound::MovePlayerStatusOnly(p) => self.handle_move(id, None, None, p.flags),
            Serverbound::KeepAlive(p) => self.handle_keep_alive(id, p.id),
            Serverbound::ChunkBatchReceived(p) => self.handle_chunk_batch_received(id, p.chunks_per_tick),
            Serverbound::PingRequest(p) => player.conn.send(&play::PongResponse { payload: p.payload }),
            Serverbound::PlayerLoaded => player.loaded = true,
            Serverbound::ClientInformation(p) => {
                let skin_changed = player.settings.skin_parts != p.settings.skin_parts;
                player.settings = p.settings;
                if skin_changed {
                    player.metadata_dirty = true;
                }
                self.update_view_distance(id);
            }
            Serverbound::Chat(p) => self.handle_player_chat(id, &p.message),
            Serverbound::ChatCommand(p) => self.handle_player_command(id, &p.command),
            Serverbound::ChatCommandSigned(p) => self.handle_player_command(id, &p.command),
            Serverbound::CommandSuggestion(p) => self.handle_suggestion_request(id, p.transaction_id.0, &p.text),
            Serverbound::PlayerInput(p) => {
                let sneaking = p.flags & play::input_flags::SNEAK != 0;
                if sneaking != player.sneaking {
                    player.sneaking = sneaking;
                    player.metadata_dirty = true;
                }
            }
            Serverbound::PlayerCommand(p) if p.action.0 == play::player_command::LEAVE_BED => self.wake_up(id),
            Serverbound::PlayerCommand(p) => {
                let sprinting = match p.action.0 {
                    play::player_command::START_SPRINTING => Some(true),
                    play::player_command::STOP_SPRINTING => Some(false),
                    _ => None,
                };
                if let Some(sprinting) = sprinting {
                    if sprinting != player.sprinting {
                        player.sprinting = sprinting;
                        player.metadata_dirty = true;
                    }
                }
            }
            Serverbound::PlayerAbilities(p) => {
                player.flying = player.allow_flight && p.flags & play::abilities::FLYING != 0;
            }
            Serverbound::SetCarriedItem(p) => {
                if (0..9).contains(&p.slot) {
                    if player.held_slot != p.slot as u8 {
                        // Switching items resets the attack cooldown and stops eating.
                        player.last_swing = self.tick;
                        if player.using_item.take().is_some() {
                            player.metadata_dirty = true;
                        }
                    }
                    player.held_slot = p.slot as u8;
                    player.equipment_dirty = true;
                } else {
                    self.kick(id, &Component::text("Invalid hotbar selection (Hacking?)"));
                }
            }
            Serverbound::SetCreativeModeSlot(p) => self.handle_creative_slot(id, p.slot, p.item),
            Serverbound::PlayerAction(p) => self.handle_player_action(id, p),
            Serverbound::UseItemOn(p) => self.handle_use_item_on(id, p),
            Serverbound::UseItem(p) => self.handle_use_item(id, p.hand.0, p.sequence.0, p.yaw, p.pitch),
            Serverbound::Attack(p) => self.handle_attack(id, p.entity_id.0),
            Serverbound::ClientCommand(p) => {
                // 0: respawn after death, 1: statistics request.
                if p.action.0 == 0 {
                    self.respawn_player(id);
                }
            }
            Serverbound::Punch => {
                self.broadcast_to_viewers(id, &play::SwingAnimation { entity_id: VarInt(id), hand: VarInt(0), animation_type: VarInt(1), duration: VarInt(6) });
            }
            Serverbound::ContainerClose(p) => {
                let open = player.open_menu.as_ref().map_or(0, |m| m.window_id);
                if p.window_id.0 == open {
                    self.close_menu(id, false);
                }
            }
            Serverbound::ContainerClick(p) => self.handle_container_click(id, p),
            Serverbound::ContainerButtonClick(p) => self.handle_enchant_button(id, p.window_id.0, p.button_id.0),
            Serverbound::RenameItem(p) => self.handle_rename(id, p.name),
            Serverbound::PickItemFromBlock(p) => self.handle_pick_block(id, p.pos),
            Serverbound::ChangeGameMode(p) => {
                if player.op_level >= 2 {
                    if let Some(mode) = GameMode::from_id(p.game_mode.0) {
                        self.set_game_mode(id, mode);
                    }
                }
            }
            Serverbound::CustomPayload(p) => {
                if p.channel == "minecraft:brand" {
                    player.brand = Reader::new(&p.data.0).string(256).ok();
                }
            }
            Serverbound::Interact(p) => self.handle_interact(id, p.entity_id.0, p.hand.0),
            Serverbound::ChatAck(_)
            | Serverbound::ChatSessionUpdate(_)
            | Serverbound::ClientTickEnd
            | Serverbound::ConfigurationAcknowledged
            | Serverbound::CookieResponse(_)
            | Serverbound::Pong(_)
            | Serverbound::ResourcePack(_)
            | Serverbound::Other { .. } => {}
        }
    }

    fn handle_move(&mut self, id: i32, pos: Option<(f64, f64, f64)>, rot: Option<(f32, f32)>, move_flags: u8) {
        let Some(player) = self.players.get_mut(&id) else { return };
        let finite = pos.is_none_or(|(x, y, z)| x.is_finite() && y.is_finite() && z.is_finite()) && rot.is_none_or(|(yaw, pitch)| yaw.is_finite() && pitch.is_finite());
        if !finite {
            self.kick(id, &Component::translatable("multiplayer.disconnect.invalid_player_movement", vec![]));
            return;
        }
        // Until a teleport is confirmed the client is still reporting its old position; in
        // bed the player stays put.
        if player.pending_teleport.is_some() || player.sleeping.is_some() {
            return;
        }
        let old_chunk = player.loc.chunk();
        let (old_loc, was_on_ground) = (player.loc, player.on_ground);
        if let Some((x, y, z)) = pos {
            player.loc.x = x.clamp(-3.0e7, 3.0e7);
            player.loc.y = y.clamp(-2.0e7, 2.0e7);
            player.loc.z = z.clamp(-3.0e7, 3.0e7);
        }
        if let Some((yaw, pitch)) = rot {
            player.loc.yaw = yaw;
            player.loc.pitch = pitch.clamp(-90.0, 90.0);
        }
        player.on_ground = move_flags & play::move_flags::ON_GROUND != 0;
        player.last_action = Instant::now();

        // PlayerMoveEvent, rate-limited like Paper: a pixel of movement or ten degrees of turn.
        let (from, to) = (player.last_move_event, player.loc);
        let delta = from.distance_squared(&to);
        let turn = (from.yaw - to.yaw).abs() + (from.pitch - to.pitch).abs();
        if (delta > 1.0 / 256.0 || turn > 10.0) && self.plugin_events.contains("PlayerMoveEvent") {
            player.last_move_event = to;
            match crate::plugins::events::on_move(self, id, from, to) {
                crate::plugins::events::MoveOutcome::Allow => {}
                crate::plugins::events::MoveOutcome::Cancel => {
                    self.teleport(id, from);
                    return;
                }
                crate::plugins::events::MoveOutcome::Redirect(target) => {
                    self.teleport(id, target);
                    return;
                }
            }
        } else if delta > 1.0 / 256.0 || turn > 10.0 {
            player.last_move_event = to;
        }
        self.after_player_moved(id, old_loc, was_on_ground);
        if self.players.get(&id).is_some_and(|p| p.loc.chunk() != old_chunk) {
            self.refresh_chunk_view(id);
        }
    }

    /// Whether `pos` is close enough for the player to act on.
    fn within_reach(&self, id: i32, pos: BlockPos) -> bool {
        let Some(player) = self.players.get(&id) else { return false };
        let eye = Location { y: player.loc.y + player.eye_height(), ..player.loc };
        let center = Location { x: pos.x as f64 + 0.5, y: pos.y as f64 + 0.5, z: pos.z as f64 + 0.5, yaw: 0.0, pitch: 0.0 };
        eye.distance_squared(&center) <= MAX_REACH * MAX_REACH
    }

    /// Sends the real state of a block to a player, undoing a client-side prediction.
    pub(super) fn resend_block(&self, id: i32, pos: BlockPos) {
        let Some(player) = self.players.get(&id) else { return };
        if let Some(state) = self.worlds[player.world].block_at(pos.x, pos.y, pos.z) {
            player.conn.send(&play::BlockUpdate { pos, state: VarInt(state as i32) });
        }
    }

    fn handle_player_action(&mut self, id: i32, action: play::PlayerAction) {
        use play::player_action::*;
        let Some(player) = self.players.get_mut(&id) else { return };
        let mode = player.game_mode;
        if player.dead {
            self.send(id, &play::BlockChangedAck { sequence: action.sequence });
            return;
        }
        match action.status.0 {
            RELEASE_USE_ITEM => self.release_using_item(id),
            START_DESTROY_BLOCK | STOP_DESTROY_BLOCK => {
                let world = player.world;
                let state = self.worlds[world].block_at(action.pos.x, action.pos.y, action.pos.z).unwrap_or(blocks::AIR);
                let finishing = action.status.0 == STOP_DESTROY_BLOCK;
                let allowed = matches!(mode, GameMode::Creative | GameMode::Survival);
                if !allowed || !self.within_reach(id, action.pos) {
                    self.resend_block(id, action.pos);
                } else if mode == GameMode::Creative {
                    if !finishing && !self.break_block(id, action.pos) {
                        self.resend_block(id, action.pos);
                    }
                } else if !finishing {
                    // Survival: instant blocks break now, others start the clock.
                    self.start_digging(id, action.pos, state);
                } else if self.finish_digging(id, action.pos, state) == Some(false) {
                    self.resend_block(id, action.pos);
                }
                self.send(id, &play::BlockChangedAck { sequence: action.sequence });
            }
            ABORT_DESTROY_BLOCK => {
                self.abort_digging(id);
                self.send(id, &play::BlockChangedAck { sequence: action.sequence });
            }
            DROP_ITEM | DROP_ALL_ITEMS => {
                let slot = HOTBAR_START + player.held_slot as usize;
                let Some(stack) = player.inventory[slot].0.clone() else { return };
                let amount = if action.status.0 == DROP_ALL_ITEMS { stack.count } else { 1 };
                let rest = if stack.count > amount { Slot(Some(stack.with_count(stack.count - amount))) } else { Slot::EMPTY };
                self.set_inventory_slot(id, slot, rest);
                self.player_drop(id, Slot(Some(stack.with_count(amount))), true);
            }
            SWAP_ITEM_WITH_OFFHAND => {
                let held = HOTBAR_START + player.held_slot as usize;
                player.inventory.swap(held, OFFHAND_SLOT);
                player.equipment_dirty = true;
            }
            _ => {}
        }
    }

    /// Breaks the block at `pos` on behalf of a player. Returns whether a block was broken.
    pub fn break_block(&mut self, id: i32, pos: BlockPos) -> bool {
        let Some(player) = self.players.get(&id) else { return false };
        let world = player.world;
        if !self.within_reach(id, pos) {
            return false;
        }
        let Some(old) = self.worlds[world].block_at(pos.x, pos.y, pos.z) else { return false };
        if blocks::is_air(old) || blocks::block_of_state(old).hardness < 0.0 && self.players[&id].game_mode != GameMode::Creative {
            return false;
        }
        if !crate::plugins::events::on_block_break(self, id, world, pos, old) {
            return false;
        }
        // Block updates: neighbours react, a waterlogged block leaves its water.
        if !self.player_destroy_block(world, pos, id) {
            return false;
        }
        self.after_player_broke_block(id, world, pos, old);
        // Break particles and sound for everyone else; the breaker's client plays its own.
        let effect = play::LevelEvent { event: 2001, pos, data: old as i32, global: false };
        let chunk = crate::world::chunk::ChunkPos::of_block(pos.x, pos.z);
        let payload = bytes::Bytes::from(paper_protocol::packets::encode_packet(&effect));
        for other in self.players.values().filter(|p| p.entity_id != id && p.world == world && p.view.sent.contains(&chunk)) {
            other.conn.send_payload(payload.clone());
        }
        true
    }

    fn handle_use_item_on(&mut self, id: i32, use_on: play::UseItemOn) {
        if self.players.get(&id).is_some_and(|p| p.dead) {
            self.resend_block(id, use_on.pos);
            self.send(id, &play::BlockChangedAck { sequence: use_on.sequence });
            return;
        }
        // Using a block (opening a chest, ...) comes first, unless sneaking with an item.
        if let Some(player) = self.players.get(&id) {
            let world = player.world;
            let hands_empty = player.inventory[HOTBAR_START + player.held_slot as usize].is_empty() && player.inventory[OFFHAND_SLOT].is_empty();
            let usable = player.game_mode != GameMode::Spectator && (!player.sneaking || hands_empty) && self.within_reach(id, use_on.pos);
            if usable && (self.try_open_container(id, world, use_on.pos) || self.use_bed(id, use_on.pos)) {
                self.send(id, &play::BlockChangedAck { sequence: use_on.sequence });
                return;
            }
        }
        // Flint and steel, fire charges and eyes of ender act on the block they are used on.
        if self.within_reach(id, use_on.pos) && (self.use_igniter(id, &use_on) || self.use_ender_eye(id, &use_on)) {
            self.send(id, &play::BlockChangedAck { sequence: use_on.sequence });
            return;
        }
        // Levers, buttons, doors, repeaters... react to the click instead of a block being placed.
        let used = self.within_reach(id, use_on.pos) && self.use_block(id, &use_on);
        let placed = used || self.try_place_block(id, &use_on);
        if !placed {
            // Undo whatever the client predicted at the clicked block and beside it.
            self.resend_block(id, use_on.pos);
            if let Some((dx, dy, dz)) = FACE_OFFSETS.get(use_on.face.0 as usize) {
                self.resend_block(id, use_on.pos.offset(*dx, *dy, *dz));
            }
        }
        self.send(id, &play::BlockChangedAck { sequence: use_on.sequence });
    }

    fn try_place_block(&mut self, id: i32, use_on: &play::UseItemOn) -> bool {
        let Some(player) = self.players.get(&id) else { return false };
        let world = player.world;
        let face = use_on.face.0 as usize;
        if face >= 6 || !matches!(player.game_mode, GameMode::Creative | GameMode::Survival) || !self.within_reach(id, use_on.pos) {
            return false;
        }
        let slot = if use_on.hand.0 == 1 { OFFHAND_SLOT } else { HOTBAR_START + player.held_slot as usize };
        let Some(stack) = player.inventory[slot].0.clone() else { return false };
        let Some(mut block) = items::get(stack.item as u32).and_then(|item| item.block).and_then(blocks::by_name) else { return false };
        let (yaw, pitch, mode, sneaking) = (player.loc.yaw, player.loc.pitch, player.game_mode, player.sneaking);

        let w = &self.worlds[world];
        let Some(clicked) = w.block_at(use_on.pos.x, use_on.pos.y, use_on.pos.z) else { return false };
        let (dx, dy, dz) = FACE_OFFSETS[face];
        let target = if blocks::is_replaceable(clicked) { use_on.pos } else { use_on.pos.offset(dx, dy, dz) };
        if !w.in_bounds(target.y) || !w.block_at(target.x, target.y, target.z).is_some_and(blocks::is_replaceable) {
            return false;
        }
        // Against a wall, items with a wall-mounted form place that form.
        if face >= 2 {
            if let Some(wall) = wall_variant(block.name) {
                block = wall;
            }
        }
        let state = placement_state(block, face, use_on.cursor_y, yaw, pitch);
        // Vanilla's placement rules (wire shapes, waterlogging, powered doors...) and survival.
        let Some(mut state) = self.placement_state(world, target, state, id, use_on) else { return false };
        // A chest beside a single chest facing the same way joins it.
        let mut neighbour: Option<(BlockPos, u32)> = None;
        if !sneaking {
            if let Some((joined, other)) = self.chest_placement(world, target, state) {
                state = joined;
                neighbour = Some(other);
            }
        }

        // A block with collision cannot be placed inside a player.
        if blocks::has_collision(state) {
            let (bx, by, bz) = (target.x as f64, target.y as f64, target.z as f64);
            let blocked = self.players.values().filter(|p| p.world == world && p.game_mode != GameMode::Spectator).any(|p| {
                let (px, py, pz) = (p.loc.x, p.loc.y, p.loc.z);
                px + 0.3 > bx && px - 0.3 < bx + 1.0 && py + 1.8 > by && py < by + 1.0 && pz + 0.3 > bz && pz - 0.3 < bz + 1.0
            });
            if blocked {
                return false;
            }
        }

        // Two-block-tall and two-block-long blocks place their second half as well.
        let mut second: Option<(BlockPos, u32)> = None;
        if let Some(upper) = blocks::with_property(state, "half", "upper") {
            second = Some((target.offset(0, 1, 0), upper));
        } else if let Some(head) = blocks::with_property(state, "part", "head") {
            if let Some(facing) = blocks::property(state, "facing") {
                let (fx, fy, fz) = direction_offset(facing);
                second = Some((target.offset(fx, fy, fz), head));
            }
        }
        if let Some((pos, _)) = second {
            let w = &self.worlds[world];
            if !w.in_bounds(pos.y) || !w.block_at(pos.x, pos.y, pos.z).is_some_and(blocks::is_replaceable) {
                return false;
            }
        }

        let replaced = self.worlds[world].block_at(target.x, target.y, target.z).unwrap_or(blocks::AIR);
        if !crate::plugins::events::on_block_place(self, id, world, target, state, replaced, use_on.pos, use_on.hand.0 == 1) {
            return false;
        }
        if !self.set_block_with_flags(world, target, state, crate::world::block_updates::flags::ALL_IMMEDIATE) {
            return false;
        }
        if let Some((pos, second_state)) = second.or(neighbour) {
            self.set_block_and_update(world, pos, second_state);
        }
        self.block_placed(world, target, state);
        if mode == GameMode::Survival {
            if let Some(player) = self.players.get_mut(&id) {
                let remaining = stack.count - 1;
                player.inventory[slot] = if remaining > 0 { Slot(Some(stack.with_count(remaining))) } else { Slot::EMPTY };
                player.equipment_dirty = true;
            }
        }
        self.broadcast_to_viewers(id, &play::SwingAnimation { entity_id: VarInt(id), hand: use_on.hand, animation_type: VarInt(1), duration: VarInt(6) });
        true
    }

    fn handle_creative_slot(&mut self, id: i32, slot: i16, item: Slot) {
        let Some(player) = self.players.get_mut(&id) else { return };
        if player.game_mode != GameMode::Creative {
            self.resync_inventory(id);
            return;
        }
        // Slot -1 means "dropped outside the window".
        if slot < 0 {
            if !item.is_empty() {
                self.player_drop(id, item, true);
            }
            return;
        }
        let slot = slot as usize;
        if !(1..INVENTORY_SLOTS).contains(&slot) {
            return;
        }
        let valid = match &item.0 {
            None => true,
            Some(stack) => items::get(stack.item as u32).is_some_and(|info| stack.count >= 1 && stack.count <= (info.max_stack_size as i32).max(1).max(64)),
        };
        if !valid {
            self.resync_inventory(id);
            return;
        }
        player.inventory[slot] = item;
        if slot == HOTBAR_START + player.held_slot as usize || slot == OFFHAND_SLOT || (5..=8).contains(&slot) {
            player.equipment_dirty = true;
        }
    }

    /// Sends the whole inventory, overriding whatever the client believes it holds.
    pub fn resync_inventory(&mut self, id: i32) {
        let Some(player) = self.players.get_mut(&id) else { return };
        player.inventory_state = player.inventory_state.wrapping_add(1) & 0x7FFF;
        player.conn.send(&play::ContainerSetContent { window_id: 0, state_id: player.inventory_state, slots: player.inventory.clone(), carried: player.carried.clone() });
    }

    /// Sets one inventory slot and tells the client.
    pub fn set_inventory_slot(&mut self, id: i32, slot: usize, item: Slot) {
        let Some(player) = self.players.get_mut(&id) else { return };
        if slot >= INVENTORY_SLOTS {
            return;
        }
        player.inventory[slot] = item.clone();
        player.inventory_state = player.inventory_state.wrapping_add(1) & 0x7FFF;
        player.conn.send(&play::ContainerSetSlot { window_id: 0, state_id: player.inventory_state, slot: slot as i16, item });
        if slot == HOTBAR_START + player.held_slot as usize || slot == OFFHAND_SLOT || (5..=8).contains(&slot) {
            player.equipment_dirty = true;
        }
    }

    /// Adds items to a player's inventory, stacking where possible. Returns how many did
    /// not fit.
    pub fn give_item(&mut self, id: i32, item: i32, mut count: i32) -> i32 {
        let max = items::get(item as u32).map_or(64, |i| i.max_stack_size as i32).max(1);
        // Hotbar first, then the main inventory, like picking items up.
        let order: Vec<usize> = (HOTBAR_START..HOTBAR_START + 9).chain(9..HOTBAR_START).collect();
        for pass in 0..2 {
            for &slot in &order {
                if count <= 0 {
                    return 0;
                }
                let Some(player) = self.players.get(&id) else { return count };
                let current = player.inventory[slot].clone();
                let updated = match (&current.0, pass) {
                    // First pass: top up existing plain stacks of the same item.
                    (Some(stack), 0) if stack.item == item && stack.added.is_empty() && stack.removed.is_empty() && stack.count < max => {
                        let add = (max - stack.count).min(count);
                        count -= add;
                        Some(Slot(Some(stack.with_count(stack.count + add))))
                    }
                    // Second pass: fill empty slots.
                    (None, 1) => {
                        let add = max.min(count);
                        count -= add;
                        Some(Slot::of(item, add))
                    }
                    _ => None,
                };
                if let Some(updated) = updated {
                    self.set_inventory_slot(id, slot, updated);
                }
            }
        }
        count
    }

    fn handle_pick_block(&mut self, id: i32, pos: BlockPos) {
        let Some(player) = self.players.get(&id) else { return };
        if !self.within_reach(id, pos) {
            return;
        }
        let Some(state) = self.worlds[player.world].block_at(pos.x, pos.y, pos.z) else { return };
        let Some(item) = blocks::block_of_state(state).item.and_then(items::id) else { return };
        let item = item as i32;
        // Already on the hotbar: select it.
        if let Some(index) = (0..9).find(|i| player.inventory[HOTBAR_START + i].0.as_ref().is_some_and(|s| s.item == item)) {
            let player = self.players.get_mut(&id).unwrap();
            player.held_slot = index as u8;
            player.equipment_dirty = true;
            player.conn.send(&play::SetHeldSlot { slot: VarInt(index as i32) });
            return;
        }
        if player.game_mode != GameMode::Creative {
            return;
        }
        // Creative: put the block's item into the selected slot (or the first empty one).
        let target = (0..9).find(|i| player.inventory[HOTBAR_START + i].is_empty()).unwrap_or(player.held_slot as usize);
        self.set_inventory_slot(id, HOTBAR_START + target, Slot::of(item, 1));
        let player = self.players.get_mut(&id).unwrap();
        player.held_slot = target as u8;
        player.conn.send(&play::SetHeldSlot { slot: VarInt(target as i32) });
    }

    /// Changes a player's game mode and informs everyone who needs to know.
    pub fn set_game_mode(&mut self, id: i32, mode: GameMode) {
        if self.players.get(&id).is_none_or(|p| p.game_mode == mode) {
            return;
        }
        if !crate::plugins::events::on_game_mode_change(self, id, mode) {
            return;
        }
        let Some(player) = self.players.get_mut(&id) else { return };
        if player.game_mode == mode {
            // A listener already changed it.
            return;
        }
        player.previous_game_mode = Some(player.game_mode);
        player.game_mode = mode;
        player.apply_game_mode_abilities();
        player.conn.send(&play::GameEvent { event: play::game_event::CHANGE_GAME_MODE, value: mode.id() as f32 });
        player.conn.send(&play::PlayerAbilities { flags: player.ability_flags(), flying_speed: 0.05, fov_modifier: 0.1 });
        let entry = super::join::info_entry(player);
        self.broadcast(&play::PlayerInfoUpdate { actions: play::player_info::UPDATE_GAME_MODE, entries: vec![entry] });
    }

    /// A command typed by a player (without the slash).
    pub fn handle_player_command(&mut self, id: i32, command: &str) {
        let Some(player) = self.players.get(&id) else { return };
        log::info!("{} issued server command: /{command}", player.name());
        let Some(command) = crate::plugins::events::on_command_preprocess(self, id, command) else { return };
        self.dispatch_command(&CommandSource::Player(id), &command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_from_yaw() {
        assert_eq!(horizontal_facing(0.0), "south");
        assert_eq!(horizontal_facing(90.0), "west");
        assert_eq!(horizontal_facing(180.0), "north");
        assert_eq!(horizontal_facing(-90.0), "east");
        assert_eq!(horizontal_facing(359.0), "south");
        assert_eq!(horizontal_facing(-180.0), "north");
    }

    #[test]
    fn placement_orientation() {
        let log = blocks::by_name("minecraft:oak_log").unwrap();
        let s = placement_state(log, 4, 0.5, 0.0, 0.0);
        assert_eq!(blocks::property(s, "axis"), Some("x"));
        let s = placement_state(log, 1, 0.5, 0.0, 0.0);
        assert_eq!(blocks::property(s, "axis"), Some("y"));

        let stairs = blocks::by_name("minecraft:oak_stairs").unwrap();
        let s = placement_state(stairs, 1, 0.0, 180.0, 0.0);
        assert_eq!(blocks::property(s, "facing"), Some("north"));
        assert_eq!(blocks::property(s, "half"), Some("bottom"));
        let s = placement_state(stairs, 2, 0.9, 180.0, 0.0);
        assert_eq!(blocks::property(s, "half"), Some("top"));

        let slab = blocks::by_name("minecraft:stone_slab").unwrap();
        assert_eq!(blocks::property(placement_state(slab, 0, 0.0, 0.0, 0.0), "type"), Some("top"));
        assert_eq!(blocks::property(placement_state(slab, 1, 0.0, 0.0, 0.0), "type"), Some("bottom"));

        let furnace = blocks::by_name("minecraft:furnace").unwrap();
        assert_eq!(blocks::property(placement_state(furnace, 1, 0.0, 0.0, 0.0), "facing"), Some("north"), "faces the player");

        let wall_torch = wall_variant("minecraft:torch").unwrap();
        assert_eq!(wall_torch.name, "minecraft:wall_torch");
        assert_eq!(blocks::property(placement_state(wall_torch, 5, 0.5, 0.0, 0.0), "facing"), Some("east"));
        assert_eq!(wall_variant("minecraft:oak_sign").unwrap().name, "minecraft:oak_wall_sign");
        assert_eq!(wall_variant("minecraft:oak_hanging_sign").unwrap().name, "minecraft:oak_wall_hanging_sign");
        assert!(wall_variant("minecraft:stone").is_none());

        let sign = blocks::by_name("minecraft:oak_sign").unwrap();
        // Facing north (yaw 180) the sign text faces south towards the player: rotation 0.
        assert_eq!(blocks::property(placement_state(sign, 1, 0.0, 180.0, 0.0), "rotation"), Some("0"));
        assert_eq!(blocks::property(placement_state(sign, 1, 0.0, 0.0, 0.0), "rotation"), Some("8"));
    }
}
