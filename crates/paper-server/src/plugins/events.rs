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
pub fn on_join(server: &mut Server, id: i32, default: Component) -> Option<Component> {
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

/// A player is about to pick up an item entity. Returns whether the pickup may happen.
pub fn on_pickup_item(server: &mut Server, player: i32, _world: usize, item: i32, stack: &paper_protocol::types::ItemStack) -> bool {
    if !server.plugin_events.contains("EntityPickupItemEvent") && !server.plugin_events.contains("PlayerAttemptPickupItemEvent") {
        return true;
    }
    let Some(uuid) = server.players.get(&player).map(|p| p.uuid().to_string()) else { return true };
    let args = bmap! { "uuid" => uuid, "item_entity" => item, "item" => super::api::item_value(&paper_protocol::types::Slot(Some(stack.clone()))) };
    match fire(server, "", "event.pickup_item", args) {
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
