//! Events fired at plugins, and notifications from the plugin host.
//!
//! Each `on_*` function is called from the game logic at the point where Paper fires the
//! corresponding Bukkit event and applies the plugins' decision. When no plugin host is
//! connected they return the default outcome immediately.

use super::{component_from, component_value, PluginInfo};
use crate::server::commands::CommandSource;
use crate::server::Server;
use paper_bridge::{bmap, Value};
use paper_protocol::codec::BlockPos;
use paper_protocol::text::Component;
use std::collections::HashSet;

/// Sends the host the state of every player that changed since it last saw it, so plugin
/// code always reads current positions, game modes and so on without asking the server.
pub fn flush_states(server: &mut Server) {
    let Some(conn) = server.plugins.clone() else { return };
    let mut changed = Vec::new();
    for id in server.player_order.clone() {
        let Some(player) = server.players.get(&id) else { continue };
        let state = super::api::player_state(server, player);
        if server.plugin_state_cache.get(&id) != Some(&state) {
            server.plugin_state_cache.insert(id, state.clone());
            changed.push(state);
        }
    }
    if !changed.is_empty() {
        conn.notify("players.state", bmap! { "players" => Value::List(changed) });
    }
}

/// Calls the host synchronously if it is connected and the event has listeners.
fn fire(server: &mut Server, event: &str, method: &str, args: Value) -> Option<Value> {
    let conn = server.plugins.clone()?;
    if !event.is_empty() && !server.plugin_events.contains(event) {
        return None;
    }
    flush_states(server);
    match conn.call(server, method, args) {
        Ok(v) => Some(v),
        Err(e) => {
            log::error!("Firing {method} at plugins failed: {e}");
            None
        }
    }
}

/// Runs the plugin scheduler and the tick events once per server tick.
pub fn tick(server: &mut Server) {
    // Scores of criteria that follow player values (health, food, ...), twice a second.
    if server.tick.is_multiple_of(10) {
        server.tick_scoreboards();
    }
    if server.tick.is_multiple_of(200) {
        super::inventory::collect_released(server);
    }
    let Some(conn) = server.plugins.clone() else { return };
    flush_states(server);
    let tick = server.tick as i64;
    if let Err(e) = conn.call(server, "host.tick", bmap! { "tick" => tick }) {
        if !conn.is_closed() {
            log::error!("Plugin tick failed: {e}");
        }
    }
}

/// A player joined. Returns the join message to broadcast (`None` to send none).
///
/// Also sends the main scoreboard (with or without plugins).
pub fn on_join(server: &mut Server, id: i32, default: Component) -> Option<Component> {
    if server.players.contains_key(&id) {
        server.send_scoreboard(id);
    }
    let Some(player) = server.players.get(&id) else { return Some(default) };
    let state = super::api::player_state(server, player);
    let inventory = Value::List(player.inventory.iter().map(super::api::item_value).collect());
    server.plugin_state_cache.insert(id, state.clone());
    let args = bmap! { "player" => state, "inventory" => inventory, "message" => component_value(&default) };
    match fire(server, "", "event.join", args) {
        Some(result) => component_from(result.get("message")),
        None => Some(default),
    }
}

/// A player left. Returns the quit message to broadcast.
pub fn on_quit(server: &mut Server, uuid: uuid::Uuid, reason: &str, default: Component) -> Option<Component> {
    if let Some(id) = server.by_uuid.get(&uuid).copied() {
        // InventoryCloseEvent (DISCONNECT) for an open window, before PlayerQuitEvent as in Paper.
        if server.players.get(&id).is_some_and(|p| p.open_menu.is_some()) {
            on_inventory_close(server, id, "DISCONNECT");
        }
        server.scoreboards.player_left(id);
        super::bossbar::player_left(server, id);
    }
    server.plugin_state_cache.retain(|_, v| v.str("uuid") != Some(uuid.to_string().as_str()));
    let args = bmap! { "uuid" => uuid.to_string(), "reason" => reason, "message" => component_value(&default) };
    match fire(server, "", "event.quit", args) {
        Some(result) => component_from(result.get("message")),
        None => Some(default),
    }
}

/// A player sent a command (without the slash). Returns the command to run, or `None` if a
/// plugin cancelled it.
pub fn on_command_preprocess(server: &mut Server, id: i32, command: &str) -> Option<String> {
    let Some(player) = server.players.get(&id) else { return Some(command.to_owned()) };
    let args = bmap! { "uuid" => player.uuid().to_string(), "message" => format!("/{command}") };
    match fire(server, "PlayerCommandPreprocessEvent", "event.command_preprocess", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.str("message").map_or(command, |m| m.strip_prefix('/').unwrap_or(m)).to_owned()),
        None => Some(command.to_owned()),
    }
}

/// The console ran a command. Returns the command to run, or `None` if cancelled.
pub fn on_server_command(server: &mut Server, command: &str) -> Option<String> {
    match fire(server, "ServerCommandEvent", "event.server_command", bmap! { "command" => command }) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.str("command").unwrap_or(command).to_owned()),
        None => Some(command.to_owned()),
    }
}

fn source_value(server: &Server, source: &CommandSource) -> Value {
    match source {
        CommandSource::Console => Value::Str("console".into()),
        CommandSource::Player(id) => server.players.get(id).map_or(Value::Null, |p| Value::Str(p.uuid().to_string())),
    }
}

/// Whether `label` (as typed, possibly namespaced) belongs to a plugin command.
pub fn is_plugin_command(server: &Server, label: &str) -> bool {
    server.plugins.is_some() && server.plugin_commands.contains(&label.to_ascii_lowercase())
}

/// Runs a plugin command. Returns `false` if no plugin command handled the input.
pub fn run_plugin_command(server: &mut Server, source: &CommandSource, command: &str) -> bool {
    let sender = source_value(server, source);
    match fire(server, "", "command.execute", bmap! { "sender" => sender, "command" => command }) {
        Some(result) => result.bool("handled").unwrap_or(false),
        None => false,
    }
}

/// Tab completion for a plugin command: `(start, length, matches)`.
pub fn plugin_suggestions(server: &mut Server, source: &CommandSource, text: &str) -> Option<(i32, i32, Vec<(String, Option<Component>)>)> {
    let sender = source_value(server, source);
    let result = fire(server, "", "command.suggest", bmap! { "sender" => sender, "text" => text })?;
    let matches = result
        .list("matches")
        .unwrap_or(&[])
        .iter()
        .filter_map(|m| Some((m.str("text")?.to_owned(), component_from(m.get("tooltip")))))
        .collect();
    Some((result.i32("start")?, result.i32("length")?, matches))
}

/// The plugin command labels a player may see in the command tree.
pub fn visible_plugin_commands(server: &mut Server, id: i32) -> Vec<String> {
    if server.plugins.is_none() || server.plugin_commands.is_empty() {
        return Vec::new();
    }
    let Some(uuid) = server.players.get(&id).map(|p| p.uuid().to_string()) else { return Vec::new() };
    match fire(server, "", "command.visible", bmap! { "uuid" => uuid }) {
        Some(result) => result.list("labels").unwrap_or(&[]).iter().filter_map(|l| l.as_str().map(str::to_owned)).collect(),
        None => Vec::new(),
    }
}

/// The client answered a resource pack offer (`PlayerResourcePackStatusEvent`).
pub fn on_resource_pack_status(server: &mut Server, id: i32, pack: uuid::Uuid, status: i32) {
    let Some(player) = server.players.get(&id) else { return };
    let args = bmap! { "uuid" => player.uuid().to_string(), "id" => pack.to_string(), "status" => status };
    fire(server, "PlayerResourcePackStatusEvent", "event.resource_pack_status", args);
}

/// A player is about to break a block. Returns whether the break may proceed.
pub fn on_block_break(server: &mut Server, id: i32, world: usize, pos: BlockPos, state: u32) -> bool {
    let Some(player) = server.players.get(&id) else { return true };
    let args = bmap! {
        "uuid" => player.uuid().to_string(),
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "state" => paper_data::blocks::state_to_string(state),
    };
    match fire(server, "BlockBreakEvent", "event.block_break", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// A player is about to place a block. Returns whether the placement may proceed.
pub fn on_block_place(server: &mut Server, id: i32, world: usize, pos: BlockPos, state: u32, replaced: u32, against: BlockPos, offhand: bool) -> bool {
    let Some(player) = server.players.get(&id) else { return true };
    let args = bmap! {
        "uuid" => player.uuid().to_string(),
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "state" => paper_data::blocks::state_to_string(state),
        "replaced" => paper_data::blocks::state_to_string(replaced),
        "against" => Value::List(vec![against.x.into(), against.y.into(), against.z.into()]),
        "offhand" => offhand,
    };
    match fire(server, "BlockPlaceEvent", "event.block_place", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// Outcome of a movement event.
pub enum MoveOutcome {
    Allow,
    Cancel,
    Redirect(crate::server::player::Location),
}

/// A player moved (only fired if a plugin listens for `PlayerMoveEvent`).
pub fn on_move(server: &mut Server, id: i32, from: crate::server::player::Location, to: crate::server::player::Location) -> MoveOutcome {
    if !server.plugin_events.contains("PlayerMoveEvent") {
        return MoveOutcome::Allow;
    }
    let Some(player) = server.players.get(&id) else { return MoveOutcome::Allow };
    let loc = |l: crate::server::player::Location| bmap! { "x" => l.x, "y" => l.y, "z" => l.z, "yaw" => l.yaw, "pitch" => l.pitch };
    let args = bmap! { "uuid" => player.uuid().to_string(), "from" => loc(from), "to" => loc(to) };
    match fire(server, "PlayerMoveEvent", "event.move", args) {
        Some(result) if result.bool("cancelled") == Some(true) => MoveOutcome::Cancel,
        Some(result) => match result.get("to") {
            Value::Map(_) => {
                let t = result.get("to");
                let target = crate::server::player::Location {
                    x: t.f64("x").unwrap_or(to.x),
                    y: t.f64("y").unwrap_or(to.y),
                    z: t.f64("z").unwrap_or(to.z),
                    yaw: t.f64("yaw").map_or(to.yaw, |v| v as f32),
                    pitch: t.f64("pitch").map_or(to.pitch, |v| v as f32),
                };
                if target == to {
                    MoveOutcome::Allow
                } else {
                    MoveOutcome::Redirect(target)
                }
            }
            _ => MoveOutcome::Allow,
        },
        None => MoveOutcome::Allow,
    }
}

/// A player's game mode is about to change. Returns whether it may.
pub fn on_game_mode_change(server: &mut Server, id: i32, mode: crate::config::GameMode) -> bool {
    let Some(player) = server.players.get(&id) else { return true };
    let args = bmap! { "uuid" => player.uuid().to_string(), "mode" => mode.name() };
    match fire(server, "PlayerGameModeChangeEvent", "event.game_mode_change", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// A player chatted. Plugins see the message asynchronously (as with Paper's
/// `AsyncChatEvent`); the rendered lines come back per recipient and are delivered here.
pub fn on_chat(server: &mut Server, id: i32, message: &str, default: Component) -> bool {
    let Some(conn) = server.plugins.clone() else { return false };
    if !server.plugin_events.contains("AsyncChatEvent") && !server.plugin_events.contains("AsyncPlayerChatEvent") {
        return false;
    }
    let Some(uuid) = server.players.get(&id).map(|p| p.uuid().to_string()) else { return false };
    flush_states(server);
    let args = bmap! { "uuid" => uuid, "message" => message, "default" => component_value(&default) };
    let fallback = default.clone();
    conn.call_then("event.chat", args, move |server, result| match result {
        Ok(result) => {
            if result.bool("cancelled") == Some(true) {
                return;
            }
            for entry in result.list("deliveries").unwrap_or(&[]) {
                let Some(message) = component_from(entry.get("message")) else { continue };
                match entry.str("viewer") {
                    Some("console") => log::info!("{}", message.to_ansi()),
                    Some(uuid) => {
                        // Player chat only reaches clients that have chat fully enabled.
                        let viewer = uuid.parse().ok().and_then(|u| server.by_uuid.get(&u).copied());
                        if let Some(id) = viewer.filter(|id| server.players.get(id).is_some_and(|p| p.settings.chat_mode == 0)) {
                            server.send_message(id, &message);
                        }
                    }
                    None => {}
                }
            }
        }
        Err(e) => {
            log::error!("Chat event failed: {e}");
            log::info!("{}", fallback.to_ansi());
            let packet = paper_protocol::packets::play::SystemChat { content: fallback, overlay: false };
            for player in server.players.values().filter(|p| p.settings.chat_mode == 0) {
                player.conn.send(&packet);
            }
        }
    });
    true
}

/// A player is about to pick up an item entity (`PlayerAttemptPickupItemEvent`, then
/// `EntityPickupItemEvent`). Returns whether the pickup may happen.
pub fn on_pickup_item(server: &mut Server, player: i32, _world: usize, item: i32, stack: &paper_protocol::types::ItemStack) -> bool {
    if !["EntityPickupItemEvent", "PlayerAttemptPickupItemEvent", "PlayerPickupItemEvent"].iter().any(|e| server.plugin_events.contains(*e)) {
        return true;
    }
    let Some(uuid) = server.players.get(&player).map(|p| p.uuid().to_string()) else { return true };
    let remaining = stack.count - server.inventory_room(player, stack).min(stack.count);
    let args = bmap! { "uuid" => uuid, "item_entity" => item, "item" => super::api::item_value(&paper_protocol::types::Slot(Some(stack.clone()))), "remaining" => remaining };
    match fire(server, "", "event.pickup_item", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `PlayerDropItemEvent`: the item entity exists already; returns false if a plugin cancelled
/// the drop (the caller then takes the item back).
pub fn on_drop_item(server: &mut Server, player: i32, item: i32, stack: &paper_protocol::types::Slot) -> bool {
    if server.plugins.is_none() || !server.plugin_events.contains("PlayerDropItemEvent") {
        return true;
    }
    let Some(uuid) = server.players.get(&player).map(|p| p.uuid().to_string()) else { return true };
    let args = bmap! { "uuid" => uuid, "item_entity" => item, "item" => super::api::item_value(stack) };
    match fire(server, "PlayerDropItemEvent", "event.drop_item", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// InventoryOpenEvent: returns false if a plugin cancelled it.
pub fn on_inventory_open(server: &mut Server, id: i32, kind: &crate::server::containers::MenuKind) -> bool {
    if server.plugins.is_none() || !server.plugin_events.contains("InventoryOpenEvent") {
        return true;
    }
    let Some(uuid) = server.players.get(&id).map(|p| p.uuid().to_string()) else { return true };
    let args = bmap! { "uuid" => uuid, "inventory" => super::inventory::menu_value(server, id, Some(kind)) };
    match fire(server, "InventoryOpenEvent", "event.inventory_open", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// InventoryCloseEvent for the window a player has open (or their own inventory), with a
/// Bukkit close `reason` (`PLAYER`, `OPEN_NEW`, `DISCONNECT`, ...).
pub fn on_inventory_close(server: &mut Server, id: i32, reason: &str) {
    if server.plugins.is_none() || !server.plugin_events.contains("InventoryCloseEvent") {
        return;
    }
    // A listener that opens a window from the close event must not close it again (Bukkit
    // would recurse here).
    if !server.plugin_data.closing.insert(id) {
        return;
    }
    if let Some(uuid) = server.players.get(&id).map(|p| p.uuid().to_string()) {
        let args = bmap! { "uuid" => uuid, "inventory" => super::inventory::window_value(server, id), "reason" => reason };
        fire(server, "InventoryCloseEvent", "event.inventory_close", args);
    }
    server.plugin_data.closing.remove(&id);
}

/// InventoryClickEvent, InventoryDragEvent for drags, or CraftItemEvent when the click takes
/// a crafting result (`craft_result`): returns false if cancelled. Listeners may change the
/// window's slots; the click then works on what they left.
pub fn on_inventory_click(
    server: &mut Server,
    id: i32,
    click: &paper_protocol::packets::play::ContainerClick,
    kind: Option<&crate::server::containers::MenuKind>,
    craft_result: Option<paper_protocol::types::Slot>,
) -> bool {
    let drag = click.mode == 5;
    // A drag is one event when it ends (stage 2), as in Bukkit.
    if drag && click.button & 3 != 2 {
        return true;
    }
    let event = if craft_result.is_some() { "CraftItemEvent" } else if drag { "InventoryDragEvent" } else { "InventoryClickEvent" };
    // CraftItemEvent is an InventoryClickEvent: listeners of either see it.
    let listened = server.plugin_events.contains(event) || craft_result.is_some() && server.plugin_events.contains("InventoryClickEvent");
    if server.plugins.is_none() || !listened {
        return true;
    }
    let Some(player) = server.players.get(&id) else { return true };
    let (uuid, cursor, held_slot) = (player.uuid().to_string(), super::api::item_value(&player.carried), player.held_slot as i64);
    let drag_slots = Value::List(player.drag.slots.iter().map(|s| Value::from(*s as i64)).collect());
    let mut args = bmap! {
        "uuid" => uuid,
        "inventory" => super::inventory::menu_value(server, id, kind),
        "slot" => click.slot as i64,
        "button" => click.button as i64,
        "mode" => click.mode as i64,
        "cursor" => cursor,
        "held_slot" => held_slot,
    };
    if let Some(result) = &craft_result {
        args.set("craft", super::api::item_value(result));
    }
    if drag {
        // The slots dragged over and how the items are spread (0 evenly, 1 one each, 2 full
        // stacks in creative).
        args.set("drag_slots", drag_slots);
        args.set("drag_kind", server.players.get(&id).map_or(0, |p| p.drag.kind as i64));
    }
    match fire(server, "", "event.inventory_click", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// EntityDamageEvent (and its ByEntity/ByBlock forms): plugins may cancel the damage or
/// change the amount.
pub fn on_entity_damage(server: &mut Server, id: i32, damage: &crate::server::survival::Damage, amount: &mut f32) -> bool {
    let listened = ["EntityDamageEvent", "EntityDamageByEntityEvent", "EntityDamageByBlockEvent"].iter().any(|e| server.plugin_events.contains(*e));
    if server.plugins.is_none() || !listened {
        return true;
    }
    let target = entity_ref(server, id);
    let mut args = bmap! { "target" => target, "cause" => damage.kind, "amount" => *amount as f64 };
    if let Some(attacker) = damage.attacker {
        match server.players.get(&attacker) {
            Some(p) => args.set("attacker_player", p.uuid().to_string()),
            None => args.set("attacker_entity", attacker as i64),
        }
        args.set("attacker", entity_ref(server, attacker));
    }
    // The projectile, when it is not the attacker itself.
    if let Some(direct) = damage.direct.filter(|d| Some(*d) != damage.attacker) {
        args.set("direct", entity_ref(server, direct));
    }
    if let Some(pos) = damage.position {
        args.set("source_position", Value::List(vec![pos.x.into(), pos.y.into(), pos.z.into()]));
    }
    match fire(server, "", "event.entity_damage", args) {
        Some(result) => {
            if result.bool("cancelled") == Some(true) {
                return false;
            }
            if let Some(a) = result.f64("amount") {
                *amount = a as f32;
            }
            true
        }
        None => true,
    }
}

/// A player or another entity as events name it: `{player: uuid}`, or `{entity: id, value:
/// <entity state>}` so the host knows its type without asking.
pub fn entity_ref(server: &Server, id: i32) -> Value {
    if let Some(p) = server.players.get(&id) {
        return bmap! { "player" => p.uuid().to_string() };
    }
    match super::entity::world_of(server, id) {
        Some(world) => bmap! { "entity" => id as i64, "value" => super::entity::entity_value(server, world, &server.worlds[world].entities[&id]) },
        None => bmap! { "entity" => id as i64 },
    }
}

/// What happens when a player dies, after `PlayerDeathEvent`.
pub struct PlayerDeath {
    /// The death message to show (`None`: none).
    pub message: Option<Component>,
    /// Keep the inventory instead of clearing it.
    pub keep_inventory: bool,
    /// Keep the experience level instead of resetting it.
    pub keep_level: bool,
    /// The items dropped (dropped even when the inventory is kept, as in Paper).
    pub drops: Vec<paper_protocol::types::Slot>,
    /// Experience dropped.
    pub xp: i32,
}

/// PlayerDeathEvent. Returns `None` when a plugin cancelled the death (which Paper allows;
/// the player keeps half a heart). Without plugins, the outcome follows the game rules.
pub fn on_player_death(server: &mut Server, id: i32, damage: &crate::server::survival::Damage, message: Component) -> Option<PlayerDeath> {
    let p = server.players.get(&id)?;
    let keep = server.worlds[p.world].rules.bool("keep_inventory");
    let mut death = PlayerDeath {
        message: Some(message.clone()),
        keep_inventory: keep,
        keep_level: keep,
        drops: if keep { Vec::new() } else { p.inventory.iter().filter(|s| !s.is_empty()).cloned().collect() },
        xp: if keep { 0 } else { (p.xp_level * 7).min(100) },
    };
    if server.plugins.is_none() || !server.plugin_events.contains("PlayerDeathEvent") {
        return Some(death);
    }
    let uuid = p.uuid().to_string();
    let killer = damage.attacker.filter(|a| *a != id).map(|a| entity_ref(server, a));
    let args = bmap! {
        "uuid" => uuid,
        "message" => component_value(&message),
        "cause" => damage.kind,
        "killer" => killer,
        "drops" => Value::List(death.drops.iter().map(super::api::item_value).collect()),
        "xp" => death.xp,
        "keep_inventory" => death.keep_inventory,
        "keep_level" => death.keep_level,
    };
    match fire(server, "PlayerDeathEvent", "event.player_death", args) {
        Some(result) if result.bool("cancelled") == Some(true) => {
            if let Some(p) = server.players.get_mut(&id) {
                p.health = 1.0;
            }
            None
        }
        Some(result) => {
            death.message = match result.get("message") {
                Value::Null => None,
                m => component_from(m),
            };
            death.keep_inventory = result.bool("keep_inventory").unwrap_or(death.keep_inventory);
            death.keep_level = result.bool("keep_level").unwrap_or(death.keep_level);
            death.xp = result.i32("xp").unwrap_or(death.xp);
            if let Some(drops) = result.list("drops") {
                death.drops = drops.iter().filter_map(|d| super::api::slot_from(d).ok()).filter(|s| !s.is_empty()).collect();
            }
            Some(death)
        }
        None => Some(death),
    }
}

/// EntityDeathEvent for mobs, with the loot and experience the mob drops. Returns the drops
/// and experience plugins left, or `None` if a plugin cancelled the death.
pub fn on_entity_death(server: &mut Server, world: usize, id: i32, damage: &crate::server::survival::Damage, drops: Vec<paper_protocol::types::Slot>, xp: i32) -> Option<(Vec<paper_protocol::types::Slot>, i32)> {
    if server.plugins.is_none() || !server.plugin_events.contains("EntityDeathEvent") {
        return Some((drops, xp));
    }
    let Some(e) = server.worlds.get(world).and_then(|w| w.entities.get(&id)) else { return Some((drops, xp)) };
    let killer = damage.attacker.filter(|a| *a != id).map(|a| entity_ref(server, a));
    let args = bmap! {
        "entity" => id as i64,
        "type" => e.info.name.clone(),
        "value" => super::entity::entity_value(server, world, e),
        "cause" => damage.kind,
        "killer" => killer,
        "drops" => Value::List(drops.iter().map(super::api::item_value).collect()),
        "xp" => xp,
    };
    match fire(server, "EntityDeathEvent", "event.entity_death", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => {
            let drops = match result.list("drops") {
                Some(list) => list.iter().filter_map(|d| super::api::slot_from(d).ok()).filter(|s| !s.is_empty()).collect(),
                None => drops,
            };
            Some((drops, result.i32("xp").unwrap_or(xp)))
        }
        None => Some((drops, xp)),
    }
}

/// PlayerRespawnEvent: plugins may change where the player respawns.
pub fn on_respawn(server: &mut Server, id: i32, world: usize, target: crate::server::player::Location) -> crate::server::player::Location {
    if server.plugins.is_none() || !server.plugin_events.contains("PlayerRespawnEvent") {
        return target;
    }
    let Some(uuid) = server.players.get(&id).map(|p| p.uuid().to_string()) else { return target };
    let world_name = server.worlds[world].name.clone();
    let args = bmap! { "uuid" => uuid, "world" => world_name, "x" => target.x, "y" => target.y, "z" => target.z, "yaw" => target.yaw as f64, "pitch" => target.pitch as f64 };
    match fire(server, "PlayerRespawnEvent", "event.respawn", args) {
        Some(result) => crate::server::player::Location {
            x: result.f64("x").unwrap_or(target.x),
            y: result.f64("y").unwrap_or(target.y),
            z: result.f64("z").unwrap_or(target.z),
            yaw: result.f64("yaw").map_or(target.yaw, |v| v as f32),
            pitch: result.f64("pitch").map_or(target.pitch, |v| v as f32),
        },
        None => target,
    }
}

/// CreatureSpawnEvent, after the fact: a cancelled spawn is removed again.
pub fn on_creature_spawn(server: &mut Server, world: usize, id: i32, reason: crate::server::mobs::SpawnReason) {
    if server.plugins.is_none() || !server.plugin_events.contains("CreatureSpawnEvent") {
        return;
    }
    let Some(e) = server.worlds.get(world).and_then(|w| w.entities.get(&id)) else { return };
    let reason = match reason {
        crate::server::mobs::SpawnReason::Natural => "NATURAL",
        crate::server::mobs::SpawnReason::ChunkGeneration => "CHUNK_GEN",
        crate::server::mobs::SpawnReason::Command => "COMMAND",
        crate::server::mobs::SpawnReason::Plugin => "CUSTOM",
        crate::server::mobs::SpawnReason::Egg => "SPAWNER_EGG",
        crate::server::mobs::SpawnReason::Spawner => "SPAWNER",
        crate::server::mobs::SpawnReason::Breeding => "BREEDING",
    };
    let args = bmap! {
        "entity" => id as i64,
        "type" => e.info.name.clone(),
        "world" => server.worlds[world].name.clone(),
        "x" => e.pos.x, "y" => e.pos.y, "z" => e.pos.z,
        "reason" => reason,
        "value" => super::entity::entity_value(server, world, e),
    };
    if let Some(result) = fire(server, "CreatureSpawnEvent", "event.creature_spawn", args) {
        if result.bool("cancelled") == Some(true) {
            if let Some(e) = server.worlds[world].entities.get_mut(&id) {
                e.removed = true;
            }
        }
    }
}

/// PlayerChangedWorldEvent, after a player moved to another world.
pub fn on_changed_world(server: &mut Server, id: i32, from: usize) {
    if server.plugins.is_none() || !server.plugin_events.contains("PlayerChangedWorldEvent") {
        return;
    }
    let Some(uuid) = server.players.get(&id).map(|p| p.uuid().to_string()) else { return };
    let from = server.worlds.get(from).map(|w| w.name.clone()).unwrap_or_default();
    fire(server, "PlayerChangedWorldEvent", "event.changed_world", bmap! { "uuid" => uuid, "from" => from });
}

/// BlockIgniteEvent for a player lighting a fire: returns false if cancelled.
pub fn on_block_ignite(server: &mut Server, id: i32, world: usize, pos: BlockPos) -> bool {
    if server.plugins.is_none() || !server.plugin_events.contains("BlockIgniteEvent") {
        return true;
    }
    let Some(uuid) = server.players.get(&id).map(|p| p.uuid().to_string()) else { return true };
    let world_name = server.worlds[world].name.clone();
    let args = bmap! { "uuid" => uuid, "world" => world_name, "x" => pos.x, "y" => pos.y, "z" => pos.z, "cause" => "FLINT_AND_STEEL" };
    match fire(server, "BlockIgniteEvent", "event.block_ignite", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// Applies a plugin list / command list / event subscription update from the host.
pub fn apply_plugin_list(server: &mut Server, value: &Value) {
    if let Some(plugins) = value.list("plugins") {
        server.plugin_list = plugins
            .iter()
            .map(|p| PluginInfo {
                name: p.str("name").unwrap_or("?").to_owned(),
                version: p.str("version").unwrap_or("").to_owned(),
                enabled: p.bool("enabled").unwrap_or(false),
                kind: p.str("kind").unwrap_or("bukkit").to_owned(),
            })
            .collect();
    }
    if let Some(labels) = value.list("commands") {
        server.plugin_commands = labels.iter().filter_map(|l| l.as_str().map(str::to_ascii_lowercase)).collect::<HashSet<_>>();
        server.resend_command_trees();
    }
    if let Some(events) = value.list("events") {
        server.plugin_events = events.iter().filter_map(|e| e.as_str().map(str::to_owned)).collect();
    }
}

/// Notifications from the host.
pub fn handle_notify(server: &mut Server, method: &str, args: &Value) {
    match method {
        "host.update" => apply_plugin_list(server, args),
        other => log::debug!("Ignoring plugin host notification {other}"),
    }
}

/// The status ping, as `PaperServerListPingEvent`. Runs on a network thread.
pub fn on_server_list_ping(conn: &super::HostConnection, address: &str, status: &crate::net::StatusSnapshot) -> Option<Value> {
    let args = bmap! {
        "address" => address,
        "motd" => component_value(&status.motd),
        "online" => status.online,
        "max" => status.max,
        "protocol" => paper_data::PROTOCOL_VERSION,
        "version" => format!("Paper-RS {}", paper_data::VERSION_NAME),
    };
    conn.call_blocking("event.server_list_ping", args, std::time::Duration::from_secs(5)).ok()
}

/// The status document after plugins changed the server list entry.
pub fn server_list_ping_json(status: &crate::net::StatusSnapshot, result: &Value) -> String {
    let motd = component_from(result.get("motd")).unwrap_or_else(|| status.motd.clone());
    let online = result.i64("online").unwrap_or(status.online as i64);
    let max = result.i64("max").unwrap_or(status.max as i64);
    let mut players = serde_json::json!({ "max": max, "online": online });
    if result.bool("hide_players") != Some(true) {
        let sample: Vec<serde_json::Value> = match result.list("sample") {
            Some(list) => list.iter().filter_map(|p| Some(serde_json::json!({ "name": p.str("name")?, "id": p.str("uuid")? }))).collect(),
            None => status.sample.iter().map(|(name, id)| serde_json::json!({ "name": name, "id": id.to_string() })).collect(),
        };
        if !sample.is_empty() {
            players["sample"] = serde_json::Value::Array(sample);
        }
    }
    let version_name = result.str("version").map_or_else(|| format!("Paper-RS {}", paper_data::VERSION_NAME), str::to_owned);
    let protocol = result.i64("protocol").unwrap_or(paper_data::PROTOCOL_VERSION as i64);
    let mut doc = serde_json::json!({
        "version": { "name": version_name, "protocol": protocol },
        "description": motd.0,
        "enforcesSecureChat": false,
    });
    if result.bool("hide_players") != Some(true) {
        doc["players"] = players;
    }
    match result.get("favicon") {
        Value::Str(icon) => doc["favicon"] = serde_json::Value::String(icon.clone()),
        _ => {
            if let Some(icon) = &status.favicon {
                doc["favicon"] = serde_json::Value::String(icon.clone());
            }
        }
    }
    doc.to_string()
}

/// The login check, as `AsyncPlayerPreLoginEvent`. Runs on a network thread. Returns the
/// kick message if a plugin refused the player.
pub fn on_pre_login(conn: &super::HostConnection, name: &str, uuid: uuid::Uuid, address: &str) -> Option<Component> {
    let args = bmap! { "name" => name, "uuid" => uuid.to_string(), "address" => address };
    match conn.call_blocking("event.pre_login", args, std::time::Duration::from_secs(30)) {
        Ok(result) if result.str("result").is_some_and(|r| r != "ALLOWED") => {
            Some(component_from(result.get("message")).unwrap_or_else(|| Component::translatable("multiplayer.disconnect.generic", vec![])))
        }
        Ok(_) => None,
        Err(e) => {
            log::error!("Pre-login event failed for {name}: {e}");
            None
        }
    }
}

// --- merchants -------------------------------------------------------------------------------

/// A player takes a trade (`PlayerTradeEvent` for villagers and wandering traders,
/// `PlayerPurchaseEvent` for merchants plugins made). Returns the trade to make (plugins may
/// change it; changes stay on the merchant's offer, as with Paper's shared recipe), whether
/// it rewards experience and whether it counts as a use; `None` if cancelled.
pub fn on_trade(server: &mut Server, player: i32, merchant: crate::server::containers::MerchantRef, index: usize, offer: crate::world::merchant::Offer) -> Option<(crate::world::merchant::Offer, bool, bool)> {
    use crate::server::containers::MerchantRef;
    let custom = matches!(merchant, MerchantRef::Custom(_));
    // Paper starts villager trades with the reward on and purchases from plugin merchants
    // with it off.
    let default = (offer.clone(), !custom, true);
    // `PlayerTradeEvent` shares `PlayerPurchaseEvent`'s handler list.
    if server.plugins.is_none() || !server.plugin_events.contains("PlayerPurchaseEvent") {
        return Some(default);
    }
    let Some(p) = server.players.get(&player) else { return Some(default) };
    let args = bmap! {
        "uuid" => p.uuid().to_string(),
        "merchant" => super::merchant::merchant_ref_value(server, merchant),
        "recipe" => super::merchant::offer_value(&offer),
        "reward_exp" => !custom,
        "increase_uses" => true,
    };
    let Some(result) = fire(server, "PlayerPurchaseEvent", "event.trade", args) else { return Some(default) };
    if result.bool("cancelled") == Some(true) {
        return None;
    }
    let changed = match super::merchant::offer_from(result.get("recipe")) {
        Ok(changed) => changed,
        Err(_) => return Some(default),
    };
    if changed != offer {
        let mut offers = server.offers_of(merchant);
        if let Some(slot) = offers.get_mut(index) {
            *slot = changed.clone();
            server.set_offers(merchant, offers);
        }
    }
    Some((changed, result.bool("reward_exp").unwrap_or(!custom), result.bool("increase_uses").unwrap_or(true)))
}

/// A player picks a trade in a trading window (`TradeSelectEvent`). Returns whether it may.
pub fn on_trade_select(server: &mut Server, player: i32, index: i32) -> bool {
    let Some(p) = server.players.get(&player) else { return true };
    let args = bmap! { "uuid" => p.uuid().to_string(), "index" => index };
    match fire(server, "TradeSelectEvent", "event.trade_select", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// A villager or wandering trader gets new trades (`VillagerAcquireTradeEvent`, one per
/// trade). Returns the trades it keeps, as plugins left them.
pub fn on_acquire_trades(server: &mut Server, world: usize, id: i32, offers: Vec<crate::world::merchant::Offer>) -> Vec<crate::world::merchant::Offer> {
    if server.plugins.is_none() || !server.plugin_events.contains("VillagerAcquireTradeEvent") {
        return offers;
    }
    let mut out = Vec::new();
    for offer in offers {
        let Some(e) = server.worlds.get(world).and_then(|w| w.entities.get(&id)) else { return out };
        let args = bmap! { "entity" => id as i64, "value" => super::entity::entity_value(server, world, e), "recipe" => super::merchant::offer_value(&offer) };
        match fire(server, "VillagerAcquireTradeEvent", "event.villager_acquire_trade", args) {
            Some(result) if result.bool("cancelled") == Some(true) => {}
            Some(result) => out.push(super::merchant::offer_from(result.get("recipe")).unwrap_or(offer)),
            None => out.push(offer),
        }
    }
    out
}

/// A villager takes or loses a job (`VillagerCareerChangeEvent`, reason `EMPLOYED` or
/// `LOSING_JOB`). Returns the profession it gets, or `None` if a plugin cancelled.
pub fn on_career_change(server: &mut Server, world: usize, id: i32, profession: &str, reason: &str) -> Option<String> {
    let Some(e) = server.worlds.get(world).and_then(|w| w.entities.get(&id)) else { return Some(profession.to_owned()) };
    let args = bmap! { "entity" => id as i64, "value" => super::entity::entity_value(server, world, e), "profession" => profession, "reason" => reason };
    match fire(server, "VillagerCareerChangeEvent", "event.villager_career_change", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.str("profession").unwrap_or(profession).to_owned()),
        None => Some(profession.to_owned()),
    }
}

/// A villager restocks (`VillagerReplenishTradeEvent`, one per trade). Returns each trade as
/// plugins left it and whether it is replenished.
pub fn on_replenish(server: &mut Server, world: usize, id: i32, offers: Vec<crate::world::merchant::Offer>) -> Vec<(crate::world::merchant::Offer, bool)> {
    if server.plugins.is_none() || !server.plugin_events.contains("VillagerReplenishTradeEvent") {
        return offers.into_iter().map(|o| (o, true)).collect();
    }
    let mut out = Vec::new();
    for offer in offers {
        let Some(e) = server.worlds.get(world).and_then(|w| w.entities.get(&id)) else {
            out.push((offer, true));
            continue;
        };
        let args = bmap! { "entity" => id as i64, "value" => super::entity::entity_value(server, world, e), "recipe" => super::merchant::offer_value(&offer) };
        match fire(server, "VillagerReplenishTradeEvent", "event.villager_replenish_trade", args) {
            Some(result) => {
                let recipe = super::merchant::offer_from(result.get("recipe")).unwrap_or(offer);
                out.push((recipe, result.bool("cancelled") != Some(true)));
            }
            None => out.push((offer, true)),
        }
    }
    out
}

// --- riding and taming -----------------------------------------------------------------------

/// How plugins name a player or an entity in an event.
fn actor_value(server: &Server, world: usize, id: i32) -> Value {
    if let Some(p) = server.players.get(&id) {
        return bmap! { "player" => p.uuid().to_string() };
    }
    match server.worlds.get(world).and_then(|w| w.entities.get(&id)) {
        Some(e) => bmap! { "entity" => id as i64, "value" => super::entity::entity_value(server, world, e) },
        None => Value::Null,
    }
}

/// A player or entity gets on another (`EntityMountEvent`). Returns whether it may.
pub fn on_mount(server: &mut Server, world: usize, rider: i32, vehicle: i32) -> bool {
    if server.plugins.is_none() || !server.plugin_events.contains("EntityMountEvent") {
        return true;
    }
    let args = bmap! { "entity" => actor_value(server, world, rider), "mount" => actor_value(server, world, vehicle) };
    match fire(server, "EntityMountEvent", "event.mount", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// A rider gets off (`EntityDismountEvent`). Returns whether it may.
pub fn on_dismount(server: &mut Server, world: usize, rider: i32, vehicle: i32) -> bool {
    if server.plugins.is_none() || !server.plugin_events.contains("EntityDismountEvent") {
        return true;
    }
    let args = bmap! { "entity" => actor_value(server, world, rider), "dismounted" => actor_value(server, world, vehicle) };
    match fire(server, "EntityDismountEvent", "event.dismount", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// A player is about to tame an animal (`EntityTameEvent`). Returns whether it may.
pub fn on_tame(server: &mut Server, world: usize, animal: i32, player: i32) -> bool {
    if server.plugins.is_none() || !server.plugin_events.contains("EntityTameEvent") {
        return true;
    }
    let args = bmap! { "entity" => actor_value(server, world, animal), "owner" => actor_value(server, world, player) };
    match fire(server, "EntityTameEvent", "event.tame", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}
