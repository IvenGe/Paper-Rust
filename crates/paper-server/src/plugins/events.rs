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

/// Whether a plugin listens for an event (so the server need not prepare it otherwise).
pub fn listens(server: &Server, event: &str) -> bool {
    server.plugins.is_some() && server.plugin_events.contains(event)
}

/// What plugins decided about a click (`Event.Result`): whether the clicked block and the
/// item in hand are used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Default,
    Allow,
    Deny,
}

impl Use {
    fn from(value: Option<&str>) -> Use {
        match value {
            Some("ALLOW") => Use::Allow,
            Some("DENY") => Use::Deny,
            _ => Use::Default,
        }
    }
}

/// The outcome of a `PlayerInteractEvent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interaction {
    pub block: Use,
    pub item: Use,
}

impl Interaction {
    pub const UNTOUCHED: Interaction = Interaction { block: Use::Default, item: Use::Default };
}

/// The kind of click (`Action`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    LeftBlock,
    RightBlock,
    LeftAir,
    RightAir,
    /// Stepping on a pressure plate, trampling farmland.
    Physical,
}

/// A player clicked a block or the air, or stepped on something (`PlayerInteractEvent`):
/// `block` with the face clicked (0 down to 5 east) and where on it, `hand` 0 main, 1 off.
pub fn on_interact(server: &mut Server, id: i32, click: Click, block: Option<(BlockPos, i32, Option<(f32, f32, f32)>)>, hand: i32) -> Interaction {
    if !listens(server, "PlayerInteractEvent") {
        return Interaction::UNTOUCHED;
    }
    let Some(player) = server.players.get(&id) else { return Interaction::UNTOUCHED };
    let mut args = bmap! {
        "uuid" => player.uuid().to_string(),
        "world" => server.worlds[player.world].name.clone(),
        "action" => match click {
            Click::LeftBlock => "LEFT_CLICK_BLOCK",
            Click::RightBlock => "RIGHT_CLICK_BLOCK",
            Click::LeftAir => "LEFT_CLICK_AIR",
            Click::RightAir => "RIGHT_CLICK_AIR",
            Click::Physical => "PHYSICAL",
        },
        "hand" => hand,
    };
    if let Some((pos, face, cursor)) = block {
        args.set("x", pos.x);
        args.set("y", pos.y);
        args.set("z", pos.z);
        args.set("face", face);
        if let Some((cx, cy, cz)) = cursor {
            args.set("cursor", Value::List(vec![(cx as f64).into(), (cy as f64).into(), (cz as f64).into()]));
        }
    }
    match fire(server, "PlayerInteractEvent", "event.interact", args) {
        Some(result) => Interaction { block: Use::from(result.str("use_block")), item: Use::from(result.str("use_item")) },
        None => Interaction::UNTOUCHED,
    }
}

/// A player right-clicked an entity at `at` (relative to the entity's position):
/// `PlayerInteractAtEntityEvent`, which `PlayerInteractEntityEvent` listeners hear too, as in
/// Paper, which fires it for every right-click on an entity. Returns whether the interaction
/// goes ahead.
pub fn on_interact_entity(server: &mut Server, id: i32, target: i32, hand: i32, at: (f64, f64, f64)) -> bool {
    if !listens(server, "PlayerInteractEntityEvent") {
        return true;
    }
    let Some(player) = server.players.get(&id) else { return true };
    let args = bmap! {
        "uuid" => player.uuid().to_string(),
        "entity" => super::entity::shallow_ref(server, target),
        "hand" => hand,
        "at" => Value::List(vec![at.0.into(), at.1.into(), at.2.into()]),
    };
    match fire(server, "PlayerInteractEntityEvent", "event.interact_entity", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// An explosion is about to break blocks (`EntityExplodeEvent`, or `BlockExplodeEvent` when
/// no entity caused it): the blocks plugins leave it, or `None` if they cancelled it.
pub fn on_explode(server: &mut Server, world: usize, source: Option<i32>, center: crate::world::entity::Vec3, blocks: Vec<(i32, i32, i32)>, decay: bool) -> Option<Vec<(i32, i32, i32)>> {
    let source = source.filter(|s| server.players.contains_key(s) || server.worlds[world].entities.contains_key(s));
    let event = if source.is_some() { "EntityExplodeEvent" } else { "BlockExplodeEvent" };
    if !listens(server, event) {
        return Some(blocks);
    }
    let mut args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "x" => center.x, "y" => center.y, "z" => center.z,
        "blocks" => Value::List(blocks.iter().map(|(x, y, z)| Value::List(vec![(*x).into(), (*y).into(), (*z).into()])).collect()),
        "result" => if decay { "DESTROY_WITH_DECAY" } else { "DESTROY" },
    };
    if let Some(s) = source {
        args.set("entity", super::entity::shallow_ref(server, s));
    }
    let method = if source.is_some() { "event.entity_explode" } else { "event.block_explode" };
    match fire(server, event, method, args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => match result.list("blocks") {
            Some(left) => Some(left.iter().filter_map(|b| Some((b.at(0).as_i64()? as i32, b.at(1).as_i64()? as i32, b.at(2).as_i64()? as i32))).collect()),
            None => Some(blocks),
        },
        None => Some(blocks),
    }
}

/// A player-only event that only says whether plugins cancelled it: `PlayerToggleSneakEvent`
/// (`args`: `sneaking`), `PlayerToggleSprintEvent` (`sprinting`), `PlayerToggleFlightEvent`
/// (`flying`), `PlayerItemHeldEvent` (`previous`, `new`). Returns whether it goes ahead.
pub fn on_player_toggle(server: &mut Server, id: i32, event: &str, method: &str, mut args: Value) -> bool {
    if !listens(server, event) {
        return true;
    }
    let Some(player) = server.players.get(&id) else { return true };
    args.set("uuid", player.uuid().to_string());
    match fire(server, event, method, args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `PlayerSwapHandItemsEvent`: the items each hand gets as plugins leave them, or `None`
/// if they cancelled the swap.
pub fn on_swap_hands(server: &mut Server, id: i32, main: &paper_protocol::types::Slot, off: &paper_protocol::types::Slot) -> Option<(paper_protocol::types::Slot, paper_protocol::types::Slot)> {
    if !listens(server, "PlayerSwapHandItemsEvent") {
        return Some((main.clone(), off.clone()));
    }
    let player = server.players.get(&id)?;
    let args = bmap! { "uuid" => player.uuid().to_string(), "main" => super::api::item_value(main), "off" => super::api::item_value(off) };
    match fire(server, "PlayerSwapHandItemsEvent", "event.swap_hands", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some((super::api::slot_from(result.get("main")).unwrap_or_else(|_| main.clone()), super::api::slot_from(result.get("off")).unwrap_or_else(|_| off.clone()))),
        None => Some((main.clone(), off.clone())),
    }
}

/// `PlayerItemConsumeEvent`: what a player finishes eating or drinking. Returns `None` if
/// plugins cancelled it, else the item that is consumed (they may replace it) and whether
/// they replaced the hand's item afterwards (`setReplacement`).
pub fn on_item_consume(server: &mut Server, id: i32, item: &paper_protocol::types::Slot, hand: i32) -> Option<(paper_protocol::types::Slot, Option<paper_protocol::types::Slot>)> {
    if !listens(server, "PlayerItemConsumeEvent") {
        return Some((item.clone(), None));
    }
    let player = server.players.get(&id)?;
    let args = bmap! { "uuid" => player.uuid().to_string(), "item" => super::api::item_value(item), "hand" => hand };
    match fire(server, "PlayerItemConsumeEvent", "event.item_consume", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => {
            let consumed = super::api::slot_from(result.get("item")).unwrap_or_else(|_| item.clone());
            let replacement = (!result.get("replacement").is_null()).then(|| super::api::slot_from(result.get("replacement")).ok()).flatten();
            Some((consumed, replacement))
        }
        None => Some((item.clone(), None)),
    }
}

/// `FoodLevelChangeEvent`: the food level a player gets as plugins leave it (`item` is what
/// they ate, if anything), or `None` if they cancelled the change.
pub fn on_food_level_change(server: &mut Server, id: i32, level: i32, item: Option<&paper_protocol::types::Slot>) -> Option<i32> {
    if !listens(server, "FoodLevelChangeEvent") {
        return Some(level);
    }
    let player = server.players.get(&id)?;
    let mut args = bmap! { "uuid" => player.uuid().to_string(), "level" => level };
    if let Some(item) = item {
        args.set("item", super::api::item_value(item));
    }
    match fire(server, "FoodLevelChangeEvent", "event.food_level_change", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.i32("level").unwrap_or(level).clamp(0, 20)),
        None => Some(level),
    }
}

/// `PlayerBucketEmptyEvent` / `PlayerBucketFillEvent`: a player empties a bucket at
/// `block` (clicking `clicked` on `face`) or fills one from it. Returns the item the bucket
/// becomes as plugins leave it (empty for the usual one), or `None` if they cancelled it.
pub fn on_bucket(server: &mut Server, id: i32, fill: bool, block: BlockPos, clicked: BlockPos, face: i32, bucket: &str, result_item: &paper_protocol::types::Slot, hand: i32) -> Option<Option<paper_protocol::types::Slot>> {
    let event = if fill { "PlayerBucketFillEvent" } else { "PlayerBucketEmptyEvent" };
    if !listens(server, event) {
        return Some(None);
    }
    let player = server.players.get(&id)?;
    let args = bmap! {
        "uuid" => player.uuid().to_string(),
        "world" => server.worlds[player.world].name.clone(),
        "x" => block.x, "y" => block.y, "z" => block.z,
        "clicked" => Value::List(vec![clicked.x.into(), clicked.y.into(), clicked.z.into()]),
        "face" => face,
        "bucket" => bucket,
        "item" => super::api::item_value(result_item),
        "hand" => hand,
    };
    match fire(server, event, if fill { "event.bucket_fill" } else { "event.bucket_empty" }, args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) if !result.get("item").is_null() => Some(super::api::slot_from(result.get("item")).ok()),
        _ => Some(None),
    }
}

/// `WeatherChangeEvent` / `ThunderChangeEvent`: whether a world's rain or thunder may
/// start or stop (`cause`: `NATURAL`, `COMMAND`, `PLUGIN`, `SLEEP`).
pub fn on_weather_change(server: &mut Server, world: usize, thunder: bool, to: bool, cause: &str) -> bool {
    let event = if thunder { "ThunderChangeEvent" } else { "WeatherChangeEvent" };
    if !listens(server, event) {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "thunder" => thunder, "to" => to, "cause" => cause };
    match fire(server, event, "event.weather_change", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `EntityTargetLivingEntityEvent`: a mob is about to target a player or another mob
/// (`reason` as Bukkit's `TargetReason`). Returns whether it may.
pub fn on_entity_target(server: &mut Server, world: usize, mob: i32, target: i32, reason: &str) -> bool {
    if !listens(server, "EntityTargetLivingEntityEvent") && !listens(server, "EntityTargetEvent") {
        return true;
    }
    if !server.worlds[world].entities.contains_key(&mob) {
        return true;
    }
    let args = bmap! { "entity" => super::entity::shallow_ref(server, mob), "target" => super::entity::shallow_ref(server, target), "reason" => reason };
    match fire(server, "", "event.entity_target", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

fn location_value(server: &Server, world: usize, loc: &crate::server::player::Location) -> Value {
    bmap! { "world" => server.worlds[world].name.clone(), "x" => loc.x, "y" => loc.y, "z" => loc.z, "yaw" => loc.yaw as f64, "pitch" => loc.pitch as f64 }
}

/// A location a plugin gave back: its world (by name) and position.
fn location_from(server: &Server, value: &Value) -> Option<(usize, crate::server::player::Location)> {
    let world = server.worlds.iter().position(|w| Some(w.name.as_str()) == value.str("world"))?;
    let loc = crate::server::player::Location { x: value.f64("x")?, y: value.f64("y")?, z: value.f64("z")?, yaw: value.f64("yaw").unwrap_or(0.0) as f32, pitch: value.f64("pitch").unwrap_or(0.0) as f32 };
    Some((world, loc))
}

/// The server moves a player (`PlayerTeleportEvent`; `cause` as Bukkit's `TeleportCause`):
/// where to, as plugins leave it (perhaps another world), or `None` if they cancelled it.
pub fn on_teleport(server: &mut Server, id: i32, to_world: usize, to: crate::server::player::Location, cause: &str) -> Option<(usize, crate::server::player::Location)> {
    if !listens(server, "PlayerTeleportEvent") {
        return Some((to_world, to));
    }
    let player = server.players.get(&id)?;
    let args = bmap! {
        "uuid" => player.uuid().to_string(),
        "from" => location_value(server, player.world, &player.loc),
        "to" => location_value(server, to_world, &to),
        "cause" => cause,
    };
    match fire(server, "PlayerTeleportEvent", "event.teleport", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(location_from(server, result.get("to")).unwrap_or((to_world, to))),
        None => Some((to_world, to)),
    }
}

/// A player is about to go through a portal (`PlayerPortalEvent`, `cause` `NETHER_PORTAL`
/// or `END_PORTAL`): where to, as plugins leave it (perhaps another world), or `None` if
/// they cancelled the trip.
pub fn on_portal(server: &mut Server, id: i32, to_world: usize, to: crate::server::player::Location, cause: &str) -> Option<(usize, crate::server::player::Location)> {
    if !listens(server, "PlayerPortalEvent") {
        return Some((to_world, to));
    }
    let player = server.players.get(&id)?;
    let nether = cause == "NETHER_PORTAL";
    let args = bmap! {
        "uuid" => player.uuid().to_string(),
        "from" => location_value(server, player.world, &player.loc),
        "to" => location_value(server, to_world, &to),
        "cause" => cause,
        // Vanilla's search and creation radii (`PortalForcer`).
        "search_radius" => if nether { if server.worlds[to_world].dimension == "minecraft:the_nether" { 16 } else { 128 } } else { 0 },
        "can_create" => nether,
        "creation_radius" => if nether { 16 } else { 0 },
    };
    match fire(server, "PlayerPortalEvent", "event.portal", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(location_from(server, result.get("to")).unwrap_or((to_world, to))),
        None => Some((to_world, to)),
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

/// A player wrote on a sign (`SignChangeEvent`): the lines as plugins leave them, or `None` if
/// a plugin cancelled the change.
pub fn on_sign_change(server: &mut Server, id: i32, world: usize, pos: BlockPos, front: bool, lines: &[paper_protocol::text::Component]) -> Option<Vec<paper_protocol::text::Component>> {
    let Some(player) = server.players.get(&id) else { return Some(lines.to_vec()) };
    let args = bmap! {
        "uuid" => player.uuid().to_string(),
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "front" => front,
        "lines" => Value::List(lines.iter().map(super::component_value).collect()),
    };
    match fire(server, "SignChangeEvent", "event.sign_change", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(match result.list("lines") {
            Some(given) => given.iter().map(|l| super::component_from(l).unwrap_or_default()).collect(),
            None => lines.to_vec(),
        }),
        None => Some(lines.to_vec()),
    }
}

/// A player is about to place a block (`BlockPlaceEvent`), or one that comes in two
/// (`second`: where the other half goes, its state and what it replaces;
/// `BlockMultiPlaceEvent`). Returns whether the placement may proceed.
#[allow(clippy::too_many_arguments)]
pub fn on_block_place(server: &mut Server, id: i32, world: usize, pos: BlockPos, state: u32, replaced: u32, against: BlockPos, offhand: bool, second: Option<(BlockPos, u32, u32)>) -> bool {
    let Some(player) = server.players.get(&id) else { return true };
    let mut args = bmap! {
        "uuid" => player.uuid().to_string(),
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "state" => paper_data::blocks::state_to_string(state),
        "replaced" => paper_data::blocks::state_to_string(replaced),
        "against" => Value::List(vec![against.x.into(), against.y.into(), against.z.into()]),
        "offhand" => offhand,
    };
    if let Some((at, state, replaced)) = second {
        args.set("second", bmap! { "x" => at.x, "y" => at.y, "z" => at.z, "state" => paper_data::blocks::state_to_string(state), "replaced" => paper_data::blocks::state_to_string(replaced) });
    }
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
/// a crafting result (`craft_result`, with the key of its recipe): returns false if cancelled. Listeners may change the
/// window's slots; the click then works on what they left.
pub fn on_inventory_click(
    server: &mut Server,
    id: i32,
    click: &paper_protocol::packets::play::ContainerClick,
    kind: Option<&crate::server::containers::MenuKind>,
    craft_result: Option<(paper_protocol::types::Slot, Option<String>)>,
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
    if let Some((result, recipe)) = &craft_result {
        args.set("craft", super::api::item_value(result));
        if let Some(recipe) = recipe {
            args.set("recipe", recipe.clone());
        }
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
    let args = damage_args(server, id, damage, *amount);
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

/// What hurts `target` by `amount`, as the damage events describe it.
fn damage_args(server: &Server, target: i32, damage: &crate::server::survival::Damage, amount: f32) -> Value {
    let mut args = bmap! { "target" => entity_ref(server, target), "cause" => damage.kind, "amount" => amount as f64 };
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
    args
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
    let reason = reason.bukkit_name(&e.info.name);
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

/// `PlayerLoginEvent` (Paper still fires it for listeners): plugins may change the verdict
/// of the server's own checks (bans, the whitelist, a full server). The host keeps the
/// player it gives the event and joins it as that player.
pub fn on_login(server: &mut Server, profile: &paper_protocol::types::GameProfile, address: &str, hostname: &str, verdict: Result<(), (crate::access::Refusal, Component)>) -> Result<(), Component> {
    if !listens(server, "PlayerLoginEvent") {
        return verdict.map_err(|(_, message)| message);
    }
    let (result, message) = match &verdict {
        Ok(()) => ("ALLOWED", Component::default()),
        Err((refusal, message)) => (refusal.bukkit_name(), message.clone()),
    };
    // Where a new player would appear; the server sends the real state as the player joins.
    let world = &server.worlds[0];
    let spawn = world.spawn;
    let player = bmap! {
        "uuid" => profile.uuid.to_string(),
        "name" => profile.name.clone(),
        "entity_id" => 0,
        "world" => world.name.clone(),
        "x" => spawn.x as f64 + 0.5, "y" => spawn.y as f64, "z" => spawn.z as f64 + 0.5, "yaw" => 0.0, "pitch" => 0.0,
        "game_mode" => server.default_game_mode.name(),
        "op_level" => server.shared.access.read().op_level(profile.uuid) as i64,
        "health" => 20.0, "food" => 20, "saturation" => 5.0,
        "address" => address, "virtual_host" => hostname,
        "properties" => Value::List(profile.properties.iter().map(|prop| bmap! { "name" => prop.name.clone(), "value" => prop.value.clone(), "signature" => prop.signature.clone() }).collect()),
    };
    let args = bmap! { "player" => player, "address" => address, "hostname" => hostname, "result" => result, "message" => component_value(&message) };
    match fire(server, "PlayerLoginEvent", "event.login", args) {
        Some(answer) => match answer.str("result") {
            Some("ALLOWED") => Ok(()),
            Some(_) => Err(component_from(answer.get("message")).unwrap_or(message)),
            None => verdict.map_err(|(_, message)| message),
        },
        None => verdict.map_err(|(_, message)| message),
    }
}

/// `ChunkLoadEvent`: a chunk became ready in a world (`new`: just generated).
pub fn on_chunk_load(server: &mut Server, world: usize, pos: crate::world::chunk::ChunkPos, new: bool) {
    if !listens(server, "ChunkLoadEvent") {
        return;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "x" => pos.x, "z" => pos.z, "new" => new };
    fire(server, "ChunkLoadEvent", "event.chunk_load", args);
}

/// `ChunkUnloadEvent`: a chunk is about to leave a world (its changes are saved either way).
pub fn on_chunk_unload(server: &mut Server, world: usize, pos: crate::world::chunk::ChunkPos) {
    let args = bmap! { "world" => server.worlds[world].name.clone(), "x" => pos.x, "z" => pos.z };
    fire(server, "ChunkUnloadEvent", "event.chunk_unload", args);
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

/// What sets a fire (`BlockIgniteEvent.getIgnitingEntity` / `getIgnitingBlock`).
#[derive(Clone, Copy, Debug)]
pub enum Igniter {
    /// An entity: a player with flint and steel or a fire charge, a lightning bolt, the
    /// shooter of a fireball, what exploded.
    Entity(i32),
    /// A block: lava, a dispenser.
    Block(BlockPos),
    Nothing,
}

/// `BlockIgniteEvent`: a fire is to start at `pos`, with `cause` (`IgniteCause`: a player's
/// `FLINT_AND_STEEL` or `FIREBALL`, `LIGHTNING`, `EXPLOSION`, `LAVA`, ...). Returns false if
/// cancelled.
pub fn on_block_ignite(server: &mut Server, world: usize, pos: BlockPos, cause: &str, by: Igniter) -> bool {
    if !listens(server, "BlockIgniteEvent") {
        return true;
    }
    let mut args = bmap! { "world" => server.worlds[world].name.clone(), "x" => pos.x, "y" => pos.y, "z" => pos.z, "cause" => cause };
    match by {
        Igniter::Entity(id) if server.players.contains_key(&id) || server.worlds[world].entities.contains_key(&id) => args.set("entity", super::entity::shallow_ref(server, id)),
        Igniter::Block(b) => args.set("from", pos_value(b)),
        _ => {}
    }
    match fire(server, "BlockIgniteEvent", "event.block_ignite", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

fn pos_value(pos: BlockPos) -> Value {
    Value::List(vec![pos.x.into(), pos.y.into(), pos.z.into()])
}

/// `BlockPistonExtendEvent` / `BlockPistonRetractEvent`: a piston is to move (and break)
/// `blocks` towards `dir`. Returns whether plugins let it.
pub fn on_piston(server: &mut Server, world: usize, pos: BlockPos, extending: bool, dir: crate::world::block_updates::Dir, blocks: &[BlockPos]) -> bool {
    let event = if extending { "BlockPistonExtendEvent" } else { "BlockPistonRetractEvent" };
    let args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "extending" => extending,
        "direction" => ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"][dir.index()],
        "blocks" => Value::List(blocks.iter().map(|b| pos_value(*b)).collect()),
    };
    match fire(server, event, "event.piston", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// Block changes of the block engine (`Ask::Change`), fired in one call: `BlockGrowEvent`,
/// `BlockFadeEvent`, `BlockFormEvent`, `BlockSpreadEvent`, `LeavesDecayEvent`,
/// `MoistureChangeEvent`. For each, the state to change to (plugins may change it), or `None`
/// if cancelled.
pub fn on_block_changes(server: &mut Server, world: usize, changes: &[crate::world::block_updates::Ask]) -> Vec<Option<u32>> {
    use crate::world::block_updates::{Ask, Change};
    let mut list = Vec::with_capacity(changes.len());
    for ask in changes {
        let Ask::Change { change, pos, new, .. } = ask else { continue };
        let mut entry = bmap! {
            "event" => match change {
                Change::Grow => "grow",
                Change::Fade => "fade",
                Change::Form => "form",
                Change::Spread { .. } => "spread",
                Change::Decay => "decay",
                Change::Moisture => "moisture",
            },
            "x" => pos.x, "y" => pos.y, "z" => pos.z,
            "state" => paper_data::blocks::state_to_string(*new),
        };
        if let Change::Spread { from } = change {
            entry.set("from", pos_value(*from));
        }
        list.push(entry);
    }
    let defaults: Vec<Option<u32>> = changes.iter().map(|a| match a {
        Ask::Change { new, .. } => Some(*new),
        _ => None,
    }).collect();
    let args = bmap! { "world" => server.worlds[world].name.clone(), "changes" => Value::List(list) };
    match fire(server, "", "event.block_changes", args).as_ref().and_then(|r| r.list("results")) {
        Some(results) if results.len() == defaults.len() => results
            .iter()
            .zip(&defaults)
            .map(|(r, default)| if r.bool("cancelled") == Some(true) { None } else { r.str("state").and_then(paper_data::blocks::parse_state).or(*default) })
            .collect(),
        _ => defaults,
    }
}

/// `BlockDamageEvent`: a survival player starts breaking a block, which breaks at once if
/// `insta`. Whether it breaks at once (plugins may say so), or `None` if cancelled.
pub fn on_block_damage(server: &mut Server, id: i32, pos: BlockPos, face: u8, insta: bool) -> Option<bool> {
    if !listens(server, "BlockDamageEvent") {
        return Some(insta);
    }
    let player = server.players.get(&id)?;
    let args = bmap! {
        "uuid" => player.uuid().to_string(),
        "world" => server.worlds[player.world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "face" => ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"].get(face as usize).copied().unwrap_or("SELF"),
        "insta" => insta,
    };
    match fire(server, "BlockDamageEvent", "event.block_damage", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.bool("insta").unwrap_or(insta)),
        None => Some(insta),
    }
}

/// `PlayerKickEvent`: a player is to be kicked with `cause` (`PlayerKickEvent.Cause`). The
/// reason and leave message as plugins leave them, or `None` if cancelled.
pub fn on_kick(server: &mut Server, id: i32, reason: &Component, leave: &Component, cause: &str) -> Option<(Component, Component)> {
    if !listens(server, "PlayerKickEvent") {
        return Some((reason.clone(), leave.clone()));
    }
    let player = server.players.get(&id)?;
    let args = bmap! { "uuid" => player.uuid().to_string(), "reason" => component_value(reason), "leave" => component_value(leave), "cause" => cause };
    match fire(server, "PlayerKickEvent", "event.kick", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some((component_from(result.get("reason")).unwrap_or_else(|| reason.clone()), component_from(result.get("leave")).unwrap_or_else(|| leave.clone()))),
        None => Some((reason.clone(), leave.clone())),
    }
}

/// `EntityRegainHealthEvent`: the health a player or mob regains (`reason`: `RegainReason`),
/// or `None` if cancelled.
pub fn on_regain_health(server: &mut Server, world: usize, id: i32, amount: f32, reason: &str) -> Option<f32> {
    if !listens(server, "EntityRegainHealthEvent") || !(server.players.contains_key(&id) || server.worlds[world].entities.contains_key(&id)) {
        return Some(amount);
    }
    let args = bmap! { "entity" => super::entity::shallow_ref(server, id), "amount" => amount as f64, "reason" => reason };
    match fire(server, "EntityRegainHealthEvent", "event.regain_health", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.f64("amount").map_or(amount, |a| a as f32)),
        None => Some(amount),
    }
}

/// What sets an entity alight, for `EntityCombustEvent`.
#[derive(Clone, Copy, Debug)]
pub enum Combuster {
    Nothing,
    /// `EntityCombustByEntityEvent`: an attacker with fire aspect, a burning zombie, a bolt,
    /// a fireball, ...
    Entity(i32),
    /// `EntityCombustByBlockEvent`: lava, fire.
    Block(BlockPos),
}

/// `EntityCombustEvent` (or its `ByEntity` / `ByBlock` form): how many ticks the entity
/// burns, as plugins leave it, or `None` if cancelled.
pub fn on_combust(server: &mut Server, world: usize, id: i32, ticks: i32, by: Combuster) -> Option<i32> {
    if !listens(server, "EntityCombustEvent") || !(server.players.contains_key(&id) || server.worlds[world].entities.contains_key(&id)) {
        return Some(ticks);
    }
    let mut args = bmap! { "entity" => super::entity::shallow_ref(server, id), "world" => server.worlds[world].name.clone(), "seconds" => ticks as f64 / 20.0 };
    match by {
        Combuster::Entity(e) if server.players.contains_key(&e) || server.worlds[world].entities.contains_key(&e) => args.set("by_entity", super::entity::shallow_ref(server, e)),
        Combuster::Block(b) => args.set("by_block", pos_value(b)),
        _ => {}
    }
    match fire(server, "EntityCombustEvent", "event.combust", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.f64("seconds").map_or(ticks, |s| (s * 20.0) as i32)),
        None => Some(ticks),
    }
}

/// `ProjectileLaunchEvent`: a projectile is to fly (it is in the world already, unseen yet).
/// Returns false if cancelled.
pub fn on_projectile_launch(server: &mut Server, world: usize, id: i32) -> bool {
    if !listens(server, "ProjectileLaunchEvent") || !server.worlds[world].entities.contains_key(&id) {
        return true;
    }
    let args = bmap! { "entity" => super::entity::shallow_ref(server, id) };
    match fire(server, "ProjectileLaunchEvent", "event.projectile_launch", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `ProjectileHitEvent`: a projectile hits an entity or a block. Returns false if cancelled
/// (it flies on, as in Paper).
pub fn on_projectile_hit(server: &mut Server, world: usize, id: i32, entity: Option<i32>, block: Option<BlockPos>) -> bool {
    if !listens(server, "ProjectileHitEvent") || !server.worlds[world].entities.contains_key(&id) {
        return true;
    }
    let mut args = bmap! { "entity" => super::entity::shallow_ref(server, id), "world" => server.worlds[world].name.clone() };
    if let Some(e) = entity.filter(|e| server.players.contains_key(e) || server.worlds[world].entities.contains_key(e)) {
        args.set("hit_entity", super::entity::shallow_ref(server, e));
    }
    if let Some(b) = block {
        args.set("hit_block", pos_value(b));
    }
    match fire(server, "ProjectileHitEvent", "event.projectile_hit", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `BlockDispenseEvent`: a dispenser or dropper at `pos` is to shoot out (one of) `item`
/// towards `velocity`. The item to dispense as plugins leave it, or `None` if cancelled.
pub fn on_dispense(server: &mut Server, world: usize, pos: BlockPos, item: &paper_protocol::types::ItemStack, velocity: (i32, i32, i32)) -> Option<paper_protocol::types::ItemStack> {
    if !listens(server, "BlockDispenseEvent") {
        return Some(item.clone());
    }
    let args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "item" => super::api::item_value(&paper_protocol::types::Slot(Some(item.with_count(1)))),
        "velocity" => Value::List(vec![velocity.0.into(), velocity.1.into(), velocity.2.into()]),
    };
    match fire(server, "BlockDispenseEvent", "event.dispense", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => match super::api::slot_from(result.get("item")) {
            Ok(paper_protocol::types::Slot(Some(new))) => Some(new),
            _ => Some(item.clone()),
        },
        None => Some(item.clone()),
    }
}

fn blocks_value(blocks: &[(BlockPos, u32)]) -> Value {
    Value::List(blocks.iter().map(|(p, s)| bmap! { "x" => p.x, "y" => p.y, "z" => p.z, "state" => paper_data::blocks::state_to_string(*s) }).collect())
}

fn blocks_from(value: &Value) -> Option<Vec<(BlockPos, u32)>> {
    value.as_list()?.iter().map(|b| Some((BlockPos::new(b.i64("x")? as i32, b.i64("y")? as i32, b.i64("z")? as i32), paper_data::blocks::parse_state(b.str("state")?)?))).collect()
}

/// `StructureGrowEvent`: a tree or huge mushroom is to grow (`capture` holds its changes, its
/// feature and the sapling), on its own or from bone meal (`bonemeal`, by `player`). The
/// changes as plugins leave them, or `None` if cancelled.
pub fn on_structure_grow(server: &mut Server, world: usize, capture: &crate::world::block_updates::Capture, player: Option<i32>, bonemeal: bool) -> Option<Vec<(BlockPos, u32)>> {
    let blocks: Vec<(BlockPos, u32)> = capture.blocks.iter().map(|(p, s, _)| (*p, *s)).collect();
    let Some((feature, sapling)) = capture.structure.clone() else { return Some(blocks) };
    if !listens(server, "StructureGrowEvent") {
        return Some(blocks);
    }
    let mut args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "x" => sapling.x, "y" => sapling.y, "z" => sapling.z,
        "feature" => feature,
        "bonemeal" => bonemeal,
        "blocks" => blocks_value(&blocks),
    };
    if let Some(p) = player.and_then(|id| server.players.get(&id)) {
        args.set("uuid", p.uuid().to_string());
    }
    match fire(server, "StructureGrowEvent", "event.structure_grow", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(blocks_from(result.get("blocks")).unwrap_or(blocks)),
        None => Some(blocks),
    }
}

/// `BlockFertilizeEvent`: bone meal on `pos` (by `player`, or a dispenser) is to make the
/// changes `blocks`; `cancelled` presets it (when the tree it grows was stopped). The changes
/// as plugins leave them, or `None` if cancelled.
pub fn on_fertilize(server: &mut Server, world: usize, pos: BlockPos, player: Option<i32>, blocks: Vec<(BlockPos, u32)>, cancelled: bool) -> Option<Vec<(BlockPos, u32)>> {
    if !listens(server, "BlockFertilizeEvent") {
        return (!cancelled).then_some(blocks);
    }
    let mut args = bmap! { "world" => server.worlds[world].name.clone(), "x" => pos.x, "y" => pos.y, "z" => pos.z, "blocks" => blocks_value(&blocks), "cancelled" => cancelled };
    if let Some(p) = player.and_then(|id| server.players.get(&id)) {
        args.set("uuid", p.uuid().to_string());
    }
    match fire(server, "BlockFertilizeEvent", "event.fertilize", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(blocks_from(result.get("blocks")).unwrap_or(blocks)),
        None => (!cancelled).then_some(blocks),
    }
}

/// `EntityChangeBlockEvent`: an entity is to change the block at `pos` to `to` (an enderman
/// taking or placing a block, a block starting to fall or landing, a sheep eating grass, a
/// player trampling farmland). `cancelled` presets it (a sheep without mob griefing). Returns
/// whether the change goes ahead.
pub fn on_entity_change_block(server: &mut Server, world: usize, entity: i32, pos: BlockPos, to: u32, cancelled: bool) -> bool {
    if !listens(server, "EntityChangeBlockEvent") || !(server.players.contains_key(&entity) || server.worlds[world].entities.contains_key(&entity)) {
        return !cancelled;
    }
    let args = bmap! {
        "entity" => super::entity::shallow_ref(server, entity),
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "to" => paper_data::blocks::state_to_string(to),
        "cancelled" => cancelled,
    };
    match fire(server, "EntityChangeBlockEvent", "event.entity_change_block", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => !cancelled,
    }
}

/// Fluids flowing (`Ask::Flow`), fired in one call: a `BlockFromToEvent` for each, from the
/// fluid's block towards a face. Returns whether each may go ahead.
pub fn on_flows(server: &mut Server, world: usize, flows: &[crate::world::block_updates::Ask]) -> Vec<bool> {
    use crate::world::block_updates::Ask;
    let list: Vec<Value> = flows
        .iter()
        .filter_map(|ask| match ask {
            Ask::Flow { from, dir, .. } => Some(bmap! { "x" => from.x, "y" => from.y, "z" => from.z, "face" => ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"][dir.index()] }),
            _ => None,
        })
        .collect();
    let count = list.len();
    let args = bmap! { "world" => server.worlds[world].name.clone(), "flows" => Value::List(list) };
    match fire(server, "BlockFromToEvent", "event.block_flows", args).as_ref().and_then(|r| r.list("results")) {
        Some(results) if results.len() == count => results.iter().map(|r| r.bool("cancelled") != Some(true)).collect(),
        _ => vec![true; count],
    }
}

/// `BlockBurnEvent`: fire at `by` is to burn the block at `pos` away. Returns whether plugins
/// let it.
pub fn on_block_burn(server: &mut Server, world: usize, pos: BlockPos, by: BlockPos) -> bool {
    let args = bmap! { "world" => server.worlds[world].name.clone(), "x" => pos.x, "y" => pos.y, "z" => pos.z, "by" => pos_value(by) };
    match fire(server, "BlockBurnEvent", "event.block_burn", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// Fire at `from` is to spread to `pos` as `state`: `BlockIgniteEvent` (cause `SPREAD`), then
/// `BlockSpreadEvent`. Returns the state it spreads as (plugins may change it), or `None` if
/// either was cancelled.
pub fn on_fire_spread(server: &mut Server, world: usize, pos: BlockPos, from: BlockPos, state: u32) -> Option<u32> {
    if !listens(server, "BlockIgniteEvent") && !listens(server, "BlockSpreadEvent") {
        return Some(state);
    }
    let args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "from" => pos_value(from),
        "state" => paper_data::blocks::state_to_string(state),
    };
    match fire(server, "", "event.fire_spread", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.str("state").and_then(paper_data::blocks::parse_state).unwrap_or(state)),
        None => Some(state),
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
    if !listens(server, "EntityMountEvent") && !listens(server, "VehicleEnterEvent") {
        return true;
    }
    // (`VehicleEnterEvent` first, for a vehicle: the host knows which are.)
    let args = bmap! { "entity" => actor_value(server, world, rider), "mount" => actor_value(server, world, vehicle) };
    match fire(server, "", "event.mount", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// A rider gets off (`EntityDismountEvent`). Returns whether it may.
pub fn on_dismount(server: &mut Server, world: usize, rider: i32, vehicle: i32) -> bool {
    if !listens(server, "EntityDismountEvent") && !listens(server, "VehicleExitEvent") {
        return true;
    }
    // (`VehicleExitEvent` first, for a vehicle.)
    let args = bmap! { "entity" => actor_value(server, world, rider), "dismounted" => actor_value(server, world, vehicle) };
    match fire(server, "", "event.dismount", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// A player is about to tame an animal (`EntityTameEvent`). Returns whether it may.
/// Custom statistics counted too often for an event (CraftBukkit's `handleStatisticsIncrease`).
const FREQUENT_STATISTICS: &[&str] = &[
    "minecraft:fall_one_cm",
    "minecraft:boat_one_cm",
    "minecraft:climb_one_cm",
    "minecraft:walk_on_water_one_cm",
    "minecraft:walk_under_water_one_cm",
    "minecraft:fly_one_cm",
    "minecraft:horse_one_cm",
    "minecraft:minecart_one_cm",
    "minecraft:pig_one_cm",
    "minecraft:play_time",
    "minecraft:swim_one_cm",
    "minecraft:walk_one_cm",
    "minecraft:sprint_one_cm",
    "minecraft:crouch_one_cm",
    "minecraft:time_since_death",
    "minecraft:sneak_time",
    "minecraft:total_world_time",
    "minecraft:time_since_rest",
    "minecraft:aviate_one_cm",
    "minecraft:strider_one_cm",
    "minecraft:happy_ghast_one_cm",
    "minecraft:nautilus_one_cm",
];

/// `PlayerStatisticIncrementEvent`: whether a statistic may go up from `old` to `new`.
pub fn on_statistic_increment(server: &mut Server, id: i32, kind: &str, key: &str, old: i32, new: i32) -> bool {
    if server.plugins.is_none() || !server.plugin_events.contains("PlayerStatisticIncrementEvent") {
        return true;
    }
    if kind == crate::server::stats::CUSTOM && FREQUENT_STATISTICS.contains(&key) {
        return true;
    }
    let Some(p) = server.players.get(&id) else { return true };
    let args = bmap! { "uuid" => p.uuid().to_string(), "type" => kind, "key" => key, "old" => old, "new" => new };
    match fire(server, "PlayerStatisticIncrementEvent", "event.statistic_increment", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

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

/// `PortalCreateEvent`: a Nether portal (or the End platform) is to be made in `world` from
/// `blocks` (where and the state each place gets; a lit frame lists its frame as it is), by
/// `entity` (the player who lit it, or who travels) for `reason` (`FIRE`, `NETHER_PAIR`,
/// `END_PLATFORM`). Returns whether plugins let it.
pub fn on_portal_create(server: &mut Server, world: usize, blocks: &[(BlockPos, u32)], entity: Option<i32>, reason: &str) -> bool {
    if !listens(server, "PortalCreateEvent") {
        return true;
    }
    let mut args = bmap! { "world" => server.worlds[world].name.clone(), "blocks" => blocks_value(blocks), "reason" => reason };
    if let Some(e) = entity.filter(|e| server.players.contains_key(e) || server.worlds.iter().any(|w| w.entities.contains_key(e))) {
        args.set("entity", super::entity::shallow_ref(server, e));
    }
    match fire(server, "PortalCreateEvent", "event.portal_create", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// What a water bottle's splash does: hurts the water-sensitive `damage`, puts out
/// `extinguish` and rehydrates `rehydrate` (entity ids).
#[derive(Clone, Debug, Default)]
pub struct WaterSplash {
    pub damage: Vec<i32>,
    pub extinguish: Vec<i32>,
    pub rehydrate: Vec<i32>,
}

/// The thrown potion `potion` and what it hit (an entity, or a block and its side: 0 down,
/// 1 up, 2 north, 3 south, 4 west, 5 east), for the splash events.
fn splash_args(server: &Server, world: usize, potion: i32, hit_entity: Option<i32>, hit_block: Option<(BlockPos, usize)>) -> Value {
    let mut args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, potion) };
    if let Some(e) = hit_entity.filter(|e| server.players.contains_key(e) || server.worlds[world].entities.contains_key(e)) {
        args.set("hit_entity", actor_value(server, world, e));
    }
    if let Some((b, face)) = hit_block {
        args.set("hit_block", pos_value(b));
        args.set("hit_face", ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"][face.min(5)]);
    }
    args
}

fn ids_from(value: &Value) -> Vec<i32> {
    value.as_list().map(|l| l.iter().filter_map(|v| v.as_i64().map(|i| i as i32)).collect()).unwrap_or_default()
}

/// `WaterBottleSplashEvent`, which `PotionSplashEvent` listeners hear too: the thrown potion
/// `potion` broke on `hit_entity` or `hit_block`, and as a water bottle does `splash`
/// (nothing for other potions; Paper fires it for every one). What it does as plugins leave
/// it, or `None` if they cancelled it (no fires put out either).
pub fn on_water_splash(server: &mut Server, world: usize, potion: i32, hit_entity: Option<i32>, hit_block: Option<(BlockPos, usize)>, splash: WaterSplash) -> Option<WaterSplash> {
    if !listens(server, "PotionSplashEvent") || !server.worlds[world].entities.contains_key(&potion) {
        return Some(splash);
    }
    let mut args = splash_args(server, world, potion, hit_entity, hit_block);
    let refs = |ids: &[i32]| Value::List(ids.iter().map(|i| actor_value(server, world, *i)).filter(|v| !v.is_null()).collect());
    let (damage, extinguish, rehydrate) = (refs(&splash.damage), refs(&splash.extinguish), refs(&splash.rehydrate));
    args.set("damage", damage);
    args.set("extinguish", extinguish);
    args.set("rehydrate", rehydrate);
    match fire(server, "PotionSplashEvent", "event.water_splash", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => {
            let pick = |key: &str, ours: Vec<i32>| if result.get(key).is_null() { ours } else { ids_from(result.get(key)) };
            Some(WaterSplash { damage: pick("damage", splash.damage), extinguish: pick("extinguish", splash.extinguish), rehydrate: pick("rehydrate", splash.rehydrate) })
        }
        None => Some(splash),
    }
}

/// `PotionSplashEvent`: the splash potion `potion` broke on `hit_entity` or `hit_block`, its
/// effects reaching `affected` (entity, intensity from 0 to 1). Who it reaches and how
/// strongly as plugins leave it, or `None` if they cancelled it.
pub fn on_potion_splash(server: &mut Server, world: usize, potion: i32, hit_entity: Option<i32>, hit_block: Option<(BlockPos, usize)>, affected: Vec<(i32, f64)>) -> Option<Vec<(i32, f64)>> {
    if !listens(server, "PotionSplashEvent") || !server.worlds[world].entities.contains_key(&potion) {
        return Some(affected);
    }
    let mut args = splash_args(server, world, potion, hit_entity, hit_block);
    let list: Vec<Value> = affected.iter().filter_map(|(id, intensity)| {
        let entity = actor_value(server, world, *id);
        (!entity.is_null()).then(|| bmap! { "entity" => entity, "intensity" => *intensity })
    }).collect();
    args.set("affected", Value::List(list));
    match fire(server, "PotionSplashEvent", "event.potion_splash", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.get("affected").as_list().map(|l| l.iter().filter_map(|a| Some((a.i32("id")?, a.f64("intensity")?))).collect()).unwrap_or(affected)),
        None => Some(affected),
    }
}

/// `LingeringPotionSplashEvent`: the lingering potion `potion` broke on `hit_entity` or
/// `hit_block`, leaving the cloud `cloud`. Returns whether plugins let the cloud stay.
pub fn on_lingering_splash(server: &mut Server, world: usize, potion: i32, hit_entity: Option<i32>, hit_block: Option<(BlockPos, usize)>, cloud: i32) -> bool {
    if !listens(server, "LingeringPotionSplashEvent") || !server.worlds[world].entities.contains_key(&potion) {
        return true;
    }
    let mut args = splash_args(server, world, potion, hit_entity, hit_block);
    args.set("cloud", actor_value(server, world, cloud));
    match fire(server, "LingeringPotionSplashEvent", "event.lingering_splash", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `AreaEffectCloudApplyEvent`: the cloud `cloud` is to give its effects to `affected`. Who
/// gets them as plugins leave it (none if cancelled).
pub fn on_cloud_apply(server: &mut Server, world: usize, cloud: i32, affected: Vec<i32>) -> Vec<i32> {
    if !listens(server, "AreaEffectCloudApplyEvent") || !server.worlds[world].entities.contains_key(&cloud) {
        return affected;
    }
    let list: Vec<Value> = affected.iter().map(|id| actor_value(server, world, *id)).filter(|v| !v.is_null()).collect();
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, cloud), "affected" => Value::List(list) };
    match fire(server, "AreaEffectCloudApplyEvent", "event.cloud_apply", args) {
        Some(result) if result.bool("cancelled") == Some(true) => Vec::new(),
        Some(result) if !result.get("affected").is_null() => ids_from(result.get("affected")),
        _ => affected,
    }
}

/// `PlayerItemDamageEvent`: the player's `item` is to wear by `wear` points (`original` before
/// Unbreaking spared some). The wear plugins leave, or `None` if they cancelled it.
pub fn on_item_damage(server: &mut Server, id: i32, item: &paper_protocol::types::Slot, wear: i32, original: i32) -> Option<i32> {
    if !listens(server, "PlayerItemDamageEvent") {
        return Some(wear);
    }
    let Some(p) = server.players.get(&id) else { return Some(wear) };
    let args = bmap! { "uuid" => p.uuid().to_string(), "item" => super::api::item_value(item), "damage" => wear, "original" => original };
    match fire(server, "PlayerItemDamageEvent", "event.item_damage", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.i32("damage").unwrap_or(wear).max(0)),
        None => Some(wear),
    }
}

/// `PlayerItemBreakEvent`: the player's `item` wore out.
pub fn on_item_break(server: &mut Server, id: i32, item: &paper_protocol::types::Slot) {
    if !listens(server, "PlayerItemBreakEvent") {
        return;
    }
    let Some(p) = server.players.get(&id) else { return };
    let args = bmap! { "uuid" => p.uuid().to_string(), "item" => super::api::item_value(item) };
    fire(server, "PlayerItemBreakEvent", "event.item_break", args);
}

/// Who shears an entity: a player with the shears in a hand (0 main, 1 off), or a dispenser.
#[derive(Clone, Copy, Debug)]
pub enum Shearer {
    Player(i32, i32),
    Dispenser(BlockPos),
}

/// `PlayerShearEntityEvent` / `BlockShearEntityEvent`: `entity` is to be sheared with `tool`,
/// dropping `drops`. The drops plugins leave, or `None` if they cancelled it.
pub fn on_shear(server: &mut Server, world: usize, entity: i32, by: Shearer, tool: &paper_protocol::types::Slot, drops: Vec<paper_protocol::types::Slot>) -> Option<Vec<paper_protocol::types::Slot>> {
    let event = match by {
        Shearer::Player(..) => "PlayerShearEntityEvent",
        Shearer::Dispenser(_) => "BlockShearEntityEvent",
    };
    if !listens(server, event) || !server.worlds[world].entities.contains_key(&entity) {
        return Some(drops);
    }
    let mut args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "entity" => actor_value(server, world, entity),
        "item" => super::api::item_value(tool),
        "drops" => Value::List(drops.iter().map(super::api::item_value).collect()),
    };
    match by {
        Shearer::Player(id, hand) => {
            let Some(p) = server.players.get(&id) else { return Some(drops) };
            args.set("uuid", p.uuid().to_string());
            args.set("hand", hand);
        }
        Shearer::Dispenser(pos) => args.set("block", pos_value(pos)),
    }
    match fire(server, event, "event.shear", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => match result.get("drops").as_list() {
            Some(list) => Some(list.iter().filter_map(|v| super::api::slot_from(v).ok()).collect()),
            None => Some(drops),
        },
        None => Some(drops),
    }
}

/// What plugins decided about a player getting into bed (`PlayerBedEnterEvent.useBed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BedUse {
    /// Sleep, whatever stood in the way.
    Allow,
    /// Do not.
    Deny,
    /// As the server decided.
    Default,
}

/// `PlayerBedEnterEvent`: the player is to get into the bed at `head`, `result` being the
/// server's verdict as a `BedEnterResult` (`OK`, `NOT_POSSIBLE_NOW`, `TOO_FAR_AWAY`, ...), with
/// `message` (a translation key) shown if it fails; `dark` and `can_set_spawn` describe the
/// bed's rules there.
#[allow(clippy::too_many_arguments)]
pub fn on_bed_enter(server: &mut Server, id: i32, world: usize, head: BlockPos, result: &str, dark: bool, can_set_spawn: bool, message: Option<&str>) -> BedUse {
    if !listens(server, "PlayerBedEnterEvent") {
        return BedUse::Default;
    }
    let Some(p) = server.players.get(&id) else { return BedUse::Default };
    let can_sleep = if !can_set_spawn { "NEVER" } else if dark { "ALLOWED" } else { "TOO_MUCH_LIGHT" };
    let mut args = bmap! {
        "uuid" => p.uuid().to_string(),
        "world" => server.worlds[world].name.clone(),
        "x" => head.x, "y" => head.y, "z" => head.z,
        "result" => result,
        "can_sleep" => can_sleep,
        "can_set_spawn" => if can_set_spawn { "ALLOWED" } else { "NEVER" },
    };
    if let Some(message) = message {
        args.set("message", component_value(&Component::translatable(message, vec![])));
    }
    match fire(server, "PlayerBedEnterEvent", "event.bed_enter", args).as_ref().and_then(|r| r.str("use")) {
        Some("ALLOW") => BedUse::Allow,
        Some("DENY") => BedUse::Deny,
        _ => BedUse::Default,
    }
}

/// `PlayerBedLeaveEvent`: the player is to get out of the bed at `head`. Returns whether
/// plugins let it.
pub fn on_bed_leave(server: &mut Server, id: i32, head: BlockPos) -> bool {
    if !listens(server, "PlayerBedLeaveEvent") {
        return true;
    }
    let Some(p) = server.players.get(&id) else { return true };
    let args = bmap! { "uuid" => p.uuid().to_string(), "world" => server.worlds[p.world].name.clone(), "x" => head.x, "y" => head.y, "z" => head.z };
    match fire(server, "PlayerBedLeaveEvent", "event.bed_leave", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `PlayerChangedMainHandEvent`: the player switched hands; `new` is the main hand now (0 left,
/// 1 right).
pub fn on_main_hand_change(server: &mut Server, id: i32, new: i32) {
    if !listens(server, "PlayerChangedMainHandEvent") {
        return;
    }
    let Some(p) = server.players.get(&id) else { return };
    let args = bmap! { "uuid" => p.uuid().to_string(), "hand" => if new == 0 { "LEFT" } else { "RIGHT" } };
    fire(server, "PlayerChangedMainHandEvent", "event.main_hand", args);
}

/// `PlayerLocaleChangeEvent`: the player's client changed its language to `new`.
pub fn on_locale_change(server: &mut Server, id: i32, new: &str) {
    if !listens(server, "PlayerLocaleChangeEvent") {
        return;
    }
    let Some(p) = server.players.get(&id) else { return };
    let args = bmap! { "uuid" => p.uuid().to_string(), "locale" => new };
    fire(server, "PlayerLocaleChangeEvent", "event.locale", args);
}

/// The state of an item entity holding `stack` at `pos` that never enters the world (Paper
/// makes one for `PlayerPickupArrowEvent`).
fn passing_item(server: &Server, world: usize, pos: crate::world::entity::Vec3, stack: &paper_protocol::types::ItemStack) -> Value {
    let Some(info) = paper_data::entity_types::by_name("minecraft:item") else { return Value::Null };
    let kind = crate::world::entity::EntityKind::Item { stack: paper_protocol::types::Slot(Some(stack.clone())), pickup_delay: 0, thrower: None, owner: None };
    let item = crate::world::entity::Entity::new(-1, info, pos, kind);
    bmap! { "entity" => -1i64, "value" => super::entity::entity_value(server, world, &item) }
}

/// `PlayerPickupArrowEvent`: the player is to pick up the arrow `arrow` as `item`. Returns
/// whether plugins let them.
pub fn on_pickup_arrow(server: &mut Server, world: usize, player: i32, arrow: i32, item: &paper_protocol::types::ItemStack) -> bool {
    if !listens(server, "PlayerPickupArrowEvent") {
        return true;
    }
    let (Some(p), Some(pos)) = (server.players.get(&player), server.worlds[world].entities.get(&arrow).map(|e| e.pos)) else { return true };
    let args = bmap! { "uuid" => p.uuid().to_string(), "arrow" => actor_value(server, world, arrow), "item" => passing_item(server, world, pos, item) };
    match fire(server, "PlayerPickupArrowEvent", "event.pickup_arrow", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// A thrown egg `egg` broke: whether it hatches, how many and what (`PlayerEggThrowEvent` when a
/// player threw it, then Paper's `ThrownEggHatchEvent`). How many hatch (0 for none) and what,
/// as plugins leave it.
pub fn on_egg_hatch(server: &mut Server, world: usize, egg: i32, owner: Option<i32>, hatching: bool, count: i32, kind: &str) -> (i32, String) {
    let count = if hatching { count } else { 0 };
    let player = owner.and_then(|o| server.players.get(&o)).map(|p| p.uuid().to_string());
    let throw = player.is_some() && listens(server, "PlayerEggThrowEvent");
    let hatch = listens(server, "ThrownEggHatchEvent");
    if !throw && !hatch || !server.worlds[world].entities.contains_key(&egg) {
        return (count, kind.to_owned());
    }
    let mut args = bmap! { "entity" => actor_value(server, world, egg), "hatching" => hatching, "count" => count, "type" => kind, "throw" => throw, "hatch" => hatch };
    if let Some(uuid) = player {
        args.set("uuid", uuid);
    }
    match fire(server, "", "event.egg", args) {
        Some(result) => {
            let hatching = result.bool("hatching").unwrap_or(hatching);
            let count = if hatching { result.i32("count").unwrap_or(count).clamp(0, 127) } else { 0 };
            (count, result.str("type").unwrap_or(kind).to_owned())
        }
        None => (count, kind.to_owned()),
    }
}

/// `ExpBottleEvent`: the bottle o' enchanting `bottle` broke on `hit_entity` or `hit_block`,
/// to give `amount` experience. The experience and whether its splash shows, as plugins leave
/// them.
pub fn on_exp_bottle(server: &mut Server, world: usize, bottle: i32, hit_entity: Option<i32>, hit_block: Option<(BlockPos, usize)>, amount: i32) -> (i32, bool) {
    if !listens(server, "ExpBottleEvent") || !server.worlds[world].entities.contains_key(&bottle) {
        return (amount, true);
    }
    let mut args = splash_args(server, world, bottle, hit_entity, hit_block);
    args.set("experience", amount);
    match fire(server, "ExpBottleEvent", "event.exp_bottle", args) {
        Some(result) => (result.i32("experience").unwrap_or(amount).max(0), result.bool("show").unwrap_or(true)),
        None => (amount, true),
    }
}

/// `ItemDespawnEvent`: the dropped item `item` is old enough to go. Returns whether plugins let
/// it.
pub fn on_item_despawn(server: &mut Server, world: usize, item: i32) -> bool {
    if !listens(server, "ItemDespawnEvent") || !server.worlds[world].entities.contains_key(&item) {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, item) };
    match fire(server, "ItemDespawnEvent", "event.item_despawn", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `ItemMergeEvent`: the dropped item `item` is to merge into `target`. Returns whether plugins
/// let it.
pub fn on_item_merge(server: &mut Server, world: usize, item: i32, target: i32) -> bool {
    if !listens(server, "ItemMergeEvent") {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, item), "target" => actor_value(server, world, target) };
    match fire(server, "ItemMergeEvent", "event.item_merge", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// What sets TNT off besides its cause: an entity (a player, what exploded), a block (a
/// dispenser, fire, an explosion of no entity), or nothing (redstone).
#[derive(Clone, Copy, Debug)]
pub enum Primer {
    Entity(i32),
    Block(BlockPos),
    Nothing,
}

/// `TNTPrimeEvent`: the TNT at `pos` is to be set off for `cause` (`PLAYER`, `EXPLOSION`,
/// `FIRE`, `REDSTONE`, `DISPENSER`) by `by`. Returns whether plugins let it.
pub fn on_tnt_prime(server: &mut Server, world: usize, pos: BlockPos, cause: &str, by: Primer) -> bool {
    if !listens(server, "TNTPrimeEvent") {
        return true;
    }
    let mut args = bmap! { "world" => server.worlds[world].name.clone(), "x" => pos.x, "y" => pos.y, "z" => pos.z, "cause" => cause };
    match by {
        Primer::Entity(e) if server.players.contains_key(&e) || server.worlds[world].entities.contains_key(&e) => args.set("entity", actor_value(server, world, e)),
        Primer::Block(b) => args.set("block", pos_value(b)),
        _ => {}
    }
    match fire(server, "TNTPrimeEvent", "event.tnt_prime", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `ExplosionPrimeEvent`: `entity` (primed TNT, a creeper, a fireball, an end crystal) is to
/// explode with `radius`, setting fires if `fire`. The radius and fire as plugins leave them,
/// or `None` if they cancelled it.
pub fn on_explosion_prime(server: &mut Server, world: usize, entity: i32, radius: f32, incendiary: bool) -> Option<(f32, bool)> {
    if !listens(server, "ExplosionPrimeEvent") || !server.worlds[world].entities.contains_key(&entity) {
        return Some((radius, incendiary));
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, entity), "radius" => radius as f64, "fire" => incendiary };
    match fire(server, "ExplosionPrimeEvent", "event.explosion_prime", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some((result.f64("radius").map_or(radius, |r| r as f32).max(0.0), result.bool("fire").unwrap_or(incendiary))),
        None => Some((radius, incendiary)),
    }
}

/// `LightningStrikeEvent`: the bolt `bolt` strikes for `cause` (`WEATHER`, `CUSTOM`, ...).
/// Returns whether plugins let it.
pub fn on_lightning(server: &mut Server, world: usize, bolt: i32, cause: &str) -> bool {
    if !listens(server, "LightningStrikeEvent") || !server.worlds[world].entities.contains_key(&bolt) {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, bolt), "cause" => cause };
    match fire(server, "LightningStrikeEvent", "event.lightning", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `CreeperPowerEvent`: the creeper `creeper` is to be charged (or, set by plugins, lose its
/// charge) for `cause` (`LIGHTNING`, with the bolt `bolt`). Returns whether plugins let it.
pub fn on_creeper_power(server: &mut Server, world: usize, creeper: i32, bolt: Option<i32>, cause: &str) -> bool {
    if !listens(server, "CreeperPowerEvent") || !server.worlds[world].entities.contains_key(&creeper) {
        return true;
    }
    let mut args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, creeper), "cause" => cause };
    if let Some(b) = bolt.filter(|b| server.worlds[world].entities.contains_key(b)) {
        args.set("bolt", actor_value(server, world, b));
    }
    match fire(server, "CreeperPowerEvent", "event.creeper_power", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// Lightning (`bolt`) is to turn `entity` into the new mob `into`: `PigZapEvent` for a pig,
/// Paper's `EntityZapEvent` and then `EntityTransformEvent` (reason `LIGHTNING`) for others.
/// Returns whether plugins let it.
pub fn on_zap(server: &mut Server, world: usize, entity: i32, bolt: i32, into: i32) -> bool {
    let pig = server.worlds[world].entities.get(&entity).is_some_and(|e| e.info.name == "minecraft:pig");
    let heard = if pig { listens(server, "PigZapEvent") } else { listens(server, "EntityZapEvent") || listens(server, "EntityTransformEvent") };
    if !heard || ![entity, bolt, into].iter().all(|e| server.worlds[world].entities.contains_key(e)) {
        return true;
    }
    let args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "entity" => actor_value(server, world, entity),
        "bolt" => actor_value(server, world, bolt),
        "into" => actor_value(server, world, into),
        "pig" => pig,
    };
    match fire(server, "", "event.zap", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `VehicleDamageEvent`: the vehicle `vehicle` (a boat) is to take `amount` from `damage`. The
/// amount plugins leave, or `None` if they cancelled it.
pub fn on_vehicle_damage(server: &mut Server, world: usize, vehicle: i32, damage: &crate::server::survival::Damage, amount: f32) -> Option<f32> {
    if !listens(server, "VehicleDamageEvent") || !server.worlds[world].entities.contains_key(&vehicle) {
        return Some(amount);
    }
    let args = damage_args(server, vehicle, damage, amount);
    match fire(server, "VehicleDamageEvent", "event.vehicle_damage", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.f64("amount").map_or(amount, |a| a as f32)),
        None => Some(amount),
    }
}

/// `VehicleDestroyEvent`: the vehicle `vehicle` is to break from `damage`. Returns whether
/// plugins let it.
pub fn on_vehicle_destroy(server: &mut Server, world: usize, vehicle: i32, damage: &crate::server::survival::Damage) -> bool {
    if !listens(server, "VehicleDestroyEvent") || !server.worlds[world].entities.contains_key(&vehicle) {
        return true;
    }
    let args = damage_args(server, vehicle, damage, 0.0);
    match fire(server, "VehicleDestroyEvent", "event.vehicle_destroy", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `VehicleCreateEvent`: the vehicle `vehicle` (a boat) has just come into the world. Returns
/// whether plugins let it stay.
pub fn on_vehicle_create(server: &mut Server, world: usize, vehicle: i32) -> bool {
    if !listens(server, "VehicleCreateEvent") || !server.worlds[world].entities.contains_key(&vehicle) {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, vehicle) };
    match fire(server, "VehicleCreateEvent", "event.vehicle_create", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `EntityPlaceEvent`: the player placed `entity` (a boat) against side `face` (0 down to 5
/// east) of `block`, with the item in `hand`. Returns whether plugins let it stay.
#[allow(clippy::too_many_arguments)]
pub fn on_entity_place(server: &mut Server, world: usize, entity: i32, player: i32, block: BlockPos, face: usize, hand: i32) -> bool {
    if !listens(server, "EntityPlaceEvent") || !server.worlds[world].entities.contains_key(&entity) {
        return true;
    }
    let Some(p) = server.players.get(&player) else { return true };
    let args = bmap! {
        "uuid" => p.uuid().to_string(),
        "world" => server.worlds[world].name.clone(),
        "entity" => actor_value(server, world, entity),
        "x" => block.x, "y" => block.y, "z" => block.z,
        "face" => ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"][face.min(5)],
        "hand" => hand,
    };
    match fire(server, "EntityPlaceEvent", "event.entity_place", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `VehicleMoveEvent`: the vehicle `vehicle` moved (or turned) from `from` (x, y, z, yaw,
/// pitch) to where it is.
pub fn on_vehicle_move(server: &mut Server, world: usize, vehicle: i32, from: (f64, f64, f64, f32, f32)) {
    if !listens(server, "VehicleMoveEvent") || !server.worlds[world].entities.contains_key(&vehicle) {
        return;
    }
    let args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "entity" => actor_value(server, world, vehicle),
        "from" => Value::List(vec![from.0.into(), from.1.into(), from.2.into(), (from.3 as f64).into(), (from.4 as f64).into()]),
    };
    fire(server, "VehicleMoveEvent", "event.vehicle_move", args);
}

fn effect_value(effect: &crate::server::effects::ActiveEffect) -> Value {
    bmap! { "effect" => effect.name(), "duration" => effect.duration, "amplifier" => effect.amplifier, "ambient" => effect.ambient, "particles" => effect.particles, "icon" => effect.icon }
}

/// `EntityPotionEffectEvent`: `entity`'s effect `old` is to become `new` (`action` `ADDED`,
/// `CHANGED`, `REMOVED` or `CLEARED`) for `cause`; `replaces` says whether it would
/// (`isOverride` as the event starts). `None` if plugins cancelled it, else whether the new
/// effect replaces the old one, even if weaker.
#[allow(clippy::too_many_arguments)]
pub fn on_potion_effect(server: &mut Server, world: usize, entity: i32, old: Option<&crate::server::effects::ActiveEffect>, new: Option<&crate::server::effects::ActiveEffect>, cause: &str, action: &str, replaces: bool) -> Option<bool> {
    let override_default = replaces;
    if !listens(server, "EntityPotionEffectEvent") || !(server.players.contains_key(&entity) || server.worlds[world].entities.contains_key(&entity)) {
        return Some(override_default);
    }
    let mut args = bmap! { "entity" => actor_value(server, world, entity), "cause" => cause, "action" => action, "override" => override_default };
    if let Some(old) = old {
        args.set("old", effect_value(old));
    }
    if let Some(new) = new {
        args.set("new", effect_value(new));
    }
    match fire(server, "EntityPotionEffectEvent", "event.potion_effect", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.bool("override").unwrap_or(override_default)),
        None => Some(override_default),
    }
}

/// `EntityShootBowEvent`: `shooter` shot the arrow `arrow` from `bow` in `hand` (0 main, 1
/// off) at `force` (0 to 1), using up `consumable` if `consume`. `None` if plugins cancelled it,
/// else whether the arrow is used up (`setConsumeItem`).
#[allow(clippy::too_many_arguments)]
pub fn on_shoot_bow(server: &mut Server, world: usize, shooter: i32, bow: &paper_protocol::types::Slot, consumable: &paper_protocol::types::Slot, arrow: i32, hand: i32, force: f32, consume: bool) -> Option<bool> {
    if !listens(server, "EntityShootBowEvent") || !server.worlds[world].entities.contains_key(&arrow) {
        return Some(consume);
    }
    let args = bmap! {
        "entity" => actor_value(server, world, shooter),
        "bow" => super::api::item_value(bow),
        "consumable" => super::api::item_value(consumable),
        "projectile" => actor_value(server, world, arrow),
        "hand" => hand,
        "force" => force as f64,
        "consume" => consume,
    };
    match fire(server, "EntityShootBowEvent", "event.shoot_bow", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.bool("consume").unwrap_or(consume)),
        None => Some(consume),
    }
}

/// `ItemSpawnEvent`: the item entity `item` has just come into the world. Returns whether
/// plugins let it stay.
pub fn on_item_spawn(server: &mut Server, world: usize, item: i32) -> bool {
    if !listens(server, "ItemSpawnEvent") || !server.worlds[world].entities.contains_key(&item) {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, item) };
    match fire(server, "ItemSpawnEvent", "event.item_spawn", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `SpawnerSpawnEvent`: the spawner at `spawner` has just spawned `entity`. Returns whether
/// plugins let it stay.
pub fn on_spawner_spawn(server: &mut Server, world: usize, entity: i32, spawner: BlockPos) -> bool {
    if !listens(server, "SpawnerSpawnEvent") || !server.worlds[world].entities.contains_key(&entity) {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, entity), "x" => spawner.x, "y" => spawner.y, "z" => spawner.z };
    match fire(server, "SpawnerSpawnEvent", "event.spawner_spawn", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `UnknownCommandEvent`: `source` ran `line`, which names no command. The error message as
/// plugins leave it, or `None` if they took it away.
pub fn on_unknown_command(server: &mut Server, source: &CommandSource, line: &str, message: Component) -> Option<Component> {
    if !listens(server, "UnknownCommandEvent") {
        return Some(message);
    }
    let args = bmap! { "sender" => source_value(server, source), "command" => line, "message" => component_value(&message) };
    match fire(server, "UnknownCommandEvent", "event.unknown_command", args) {
        Some(result) => component_from(result.get("message")),
        None => Some(message),
    }
}

/// Paper's `PlayerOpenSignEvent` and then, if anyone listens, Bukkit's `PlayerSignOpenEvent`:
/// `id` is about to write on one side of the sign at `pos` (`cause`: `PLACE`, `INTERACT`,
/// `PLUGIN` or `UNKNOWN`). Returns whether plugins let the editor open.
pub fn on_sign_open(server: &mut Server, id: i32, world: usize, pos: BlockPos, front: bool, cause: &str) -> bool {
    if !listens(server, "PlayerOpenSignEvent") && !listens(server, "PlayerSignOpenEvent") {
        return true;
    }
    let Some(player) = server.players.get(&id) else { return true };
    let args = bmap! { "uuid" => player.uuid().to_string(), "world" => server.worlds[world].name.clone(), "x" => pos.x, "y" => pos.y, "z" => pos.z, "front" => front, "cause" => cause };
    match fire(server, "", "event.sign_open", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `PlayerClientLoadedWorldEvent`: the client of `id` has loaded the world after joining or
/// respawning, or the server gave up waiting for it (`timeout`).
pub fn on_client_loaded(server: &mut Server, id: i32, timeout: bool) {
    if !listens(server, "PlayerClientLoadedWorldEvent") {
        return;
    }
    let Some(player) = server.players.get(&id) else { return };
    let args = bmap! { "uuid" => player.uuid().to_string(), "timeout" => timeout };
    fire(server, "PlayerClientLoadedWorldEvent", "event.client_loaded", args);
}

/// Paper's `BlockPreDispenseEvent`: the dispenser or dropper at `pos` is about to use the stack
/// in `slot`. Returns whether plugins let it.
pub fn on_pre_dispense(server: &mut Server, world: usize, pos: BlockPos, stack: &paper_protocol::types::ItemStack, slot: usize) -> bool {
    if !listens(server, "BlockPreDispenseEvent") {
        return true;
    }
    let args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "item" => super::api::item_value(&paper_protocol::types::Slot(Some(stack.clone()))),
        "slot" => slot as i32,
    };
    match fire(server, "BlockPreDispenseEvent", "event.pre_dispense", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// Paper's `BlockFailedDispenseEvent`: the dispenser or dropper at `pos` fired with nothing in
/// it. Returns whether it still clicks.
pub fn on_failed_dispense(server: &mut Server, world: usize, pos: BlockPos) -> bool {
    if !listens(server, "BlockFailedDispenseEvent") {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "x" => pos.x, "y" => pos.y, "z" => pos.z };
    match fire(server, "BlockFailedDispenseEvent", "event.failed_dispense", args) {
        Some(result) => result.bool("effect") != Some(false),
        None => true,
    }
}

/// What plugins made of an item a hopper or dropper moves (`InventoryMoveItemEvent`).
pub enum Move {
    /// Stopped.
    Cancelled,
    /// The item that goes (another one if a listener set it), and whether a listener looked at
    /// it: as in Paper, a hopper whose listeners did not look does not ask again for its other
    /// slots this time.
    Item(paper_protocol::types::ItemStack, bool),
}

/// `InventoryMoveItemEvent`: one item is about to go from the container `source` to
/// `destination` (each its blocks, or a chest boat): `pushed` by a hopper or dropper, or
/// pulled by the hopper below.
pub fn on_inventory_move(server: &mut Server, world: usize, source: (&[BlockPos], Option<i32>), item: &paper_protocol::types::ItemStack, destination: (&[BlockPos], Option<i32>), pushed: bool) -> Move {
    if !listens(server, "InventoryMoveItemEvent") {
        return Move::Item(item.clone(), false);
    }
    let args = bmap! {
        "source" => super::inventory::container_value(server, world, source.0, source.1),
        "item" => super::api::item_value(&paper_protocol::types::Slot(Some(item.clone()))),
        "destination" => super::inventory::container_value(server, world, destination.0, destination.1),
        "pushed" => pushed,
    };
    match fire(server, "InventoryMoveItemEvent", "event.inventory_move", args) {
        Some(result) if result.bool("cancelled") == Some(true) => Move::Cancelled,
        Some(result) => {
            let moved = match super::api::slot_from(result.get("item")) {
                Ok(paper_protocol::types::Slot(Some(other))) => other,
                _ => item.clone(),
            };
            Move::Item(moved, result.bool("looked").unwrap_or(true))
        }
        None => Move::Item(item.clone(), true),
    }
}

/// `InventoryPickupItemEvent`: the hopper at `hopper` is about to take in the item entity
/// `item`. Returns whether plugins let it.
pub fn on_inventory_pickup(server: &mut Server, world: usize, hopper: BlockPos, item: i32) -> bool {
    if !listens(server, "InventoryPickupItemEvent") || !server.worlds[world].entities.contains_key(&item) {
        return true;
    }
    let args = bmap! { "inventory" => super::inventory::container_value(server, world, &[hopper], None), "entity" => actor_value(server, world, item) };
    match fire(server, "InventoryPickupItemEvent", "event.inventory_pickup", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `EntityInteractEvent`: the entity `entity` (not a player) is about to trample the farmland
/// or press the plate or string at `pos`. Returns whether plugins let it.
pub fn on_entity_interact(server: &mut Server, world: usize, entity: i32, pos: BlockPos) -> bool {
    if !listens(server, "EntityInteractEvent") || !server.worlds[world].entities.contains_key(&entity) {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, entity), "x" => pos.x, "y" => pos.y, "z" => pos.z };
    match fire(server, "EntityInteractEvent", "event.entity_interact", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// A newly generated chunk is ready: plugins' block populators of its world run on it
/// (`BlockPopulator#populate(World, Random, Chunk)`), then `ChunkPopulateEvent` fires, as
/// `LevelChunk.loadCallback` does after `ChunkLoadEvent`. (The host reports
/// `ChunkPopulateEvent` as listened while a world has populators too.)
pub fn on_chunk_populate(server: &mut Server, world: usize, pos: crate::world::chunk::ChunkPos) {
    if !listens(server, "ChunkPopulateEvent") {
        return;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "x" => pos.x, "z" => pos.z };
    fire(server, "ChunkPopulateEvent", "event.chunk_populate", args);
}

/// What plugins said about a mob about to be made (`PreCreatureSpawnEvent`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreSpawn {
    Spawn,
    /// Not this one.
    Skip,
    /// None at all this time (`setShouldAbortSpawn`).
    Abort,
}

/// Paper's `PreCreatureSpawnEvent`: a mob of type `entity` is about to be made at `at` for
/// `reason` (`NATURAL`, `SPAWNER`), before anything is made, so that plugins can refuse it
/// cheaply.
pub fn on_pre_creature_spawn(server: &mut Server, world: usize, entity: &str, at: crate::world::entity::Vec3, reason: &str) -> PreSpawn {
    if !listens(server, "PreCreatureSpawnEvent") {
        return PreSpawn::Spawn;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "x" => at.x, "y" => at.y, "z" => at.z, "type" => entity, "reason" => reason };
    match fire(server, "PreCreatureSpawnEvent", "event.pre_creature_spawn", args) {
        Some(result) if result.bool("cancelled") == Some(true) => {
            if result.bool("abort") == Some(true) {
                PreSpawn::Abort
            } else {
                PreSpawn::Skip
            }
        }
        _ => PreSpawn::Spawn,
    }
}

/// Bukkit's plain `EntitySpawnEvent`: the entity `entity` (primed TNT, a falling block, an
/// experience orb, a cloud, an eye of ender, ...) has just come into the world. Returns
/// whether plugins let it stay.
pub fn on_entity_spawn(server: &mut Server, world: usize, entity: i32) -> bool {
    if !listens(server, "EntitySpawnEvent") || !server.worlds[world].entities.contains_key(&entity) {
        return true;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "entity" => actor_value(server, world, entity) };
    match fire(server, "EntitySpawnEvent", "event.entity_spawn", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// Bukkit's `EntityKnockbackEvent` (`EntityKnockbackByEntityEvent` when `attacker` pushed),
/// then Paper's (`EntityKnockbackByEntityEvent`), which starts from what the first one left:
/// `entity`, moving at `velocity`, is about to be pushed by `knock` (the change to its
/// velocity) with `force`, for `cause` (`ENTITY_ATTACK`, `SWEEP_ATTACK`, `DAMAGE`,
/// `EXPLOSION`, ...). `None` if plugins stopped it, else the push as they left it.
#[allow(clippy::too_many_arguments)]
pub fn on_knockback(server: &mut Server, world: usize, entity: i32, attacker: Option<i32>, cause: &str, force: f64, knock: crate::world::entity::Vec3, velocity: crate::world::entity::Vec3) -> Option<crate::world::entity::Vec3> {
    if !listens(server, "EntityKnockbackEvent") && !listens(server, "BukkitEntityKnockbackEvent") {
        return Some(knock);
    }
    if !(server.players.contains_key(&entity) || server.worlds[world].entities.contains_key(&entity)) {
        return Some(knock);
    }
    let vec = |v: crate::world::entity::Vec3| Value::List(vec![v.x.into(), v.y.into(), v.z.into()]);
    let mut args = bmap! { "entity" => actor_value(server, world, entity), "cause" => cause, "force" => force, "knockback" => vec(knock), "velocity" => vec(velocity) };
    if let Some(a) = attacker.filter(|a| server.players.contains_key(a) || server.worlds[world].entities.contains_key(a)) {
        args.set("attacker", actor_value(server, world, a));
    }
    match fire(server, "", "event.knockback", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => match result.list("knockback") {
            Some([x, y, z]) => Some(crate::world::entity::Vec3::new(x.as_f64().unwrap_or(knock.x), y.as_f64().unwrap_or(knock.y), z.as_f64().unwrap_or(knock.z))),
            _ => Some(knock),
        },
        None => Some(knock),
    }
}

/// `EntityBlockFormEvent` (which `BlockFormEvent` listeners hear too): the entity `entity`
/// (a snow golem) is about to form `state` at `pos`. `None` if plugins stopped it, else the
/// state as they left it.
pub fn on_entity_block_form(server: &mut Server, world: usize, entity: i32, pos: BlockPos, state: u32) -> Option<u32> {
    if !listens(server, "BlockFormEvent") || !server.worlds[world].entities.contains_key(&entity) {
        return Some(state);
    }
    let args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "x" => pos.x, "y" => pos.y, "z" => pos.z,
        "entity" => actor_value(server, world, entity),
        "state" => paper_data::blocks::state_to_string(state),
    };
    match fire(server, "BlockFormEvent", "event.entity_block_form", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.str("state").and_then(paper_data::blocks::parse_state).unwrap_or(state)),
        None => Some(state),
    }
}

/// A world border's change as plugins left it (`WorldBorderBoundsChangeEvent`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BorderChange {
    /// Still an `INSTANT_MOVE`, or a move over `ticks`.
    pub instant: bool,
    pub from: f64,
    pub size: f64,
    pub ticks: i64,
}

/// `WorldBorderBoundsChangeEvent`: `world`'s border is about to go from `from` to `to`, at
/// once (`instant`) or over `ticks`. `None` if plugins stopped it.
pub fn on_border_resize(server: &mut Server, world: usize, instant: bool, from: f64, to: f64, ticks: i64) -> Option<BorderChange> {
    let unchanged = BorderChange { instant, from, size: to, ticks };
    if !listens(server, "WorldBorderBoundsChangeEvent") {
        return Some(unchanged);
    }
    let args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "type" => if instant { "INSTANT_MOVE" } else { "STARTED_MOVE" },
        "from" => from,
        "to" => to,
        "ticks" => ticks,
    };
    match fire(server, "WorldBorderBoundsChangeEvent", "event.border_resize", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(BorderChange {
            instant: result.str("type") != Some("STARTED_MOVE"),
            from,
            size: result.f64("to").unwrap_or(to),
            ticks: result.i64("ticks").unwrap_or(ticks),
        }),
        None => Some(unchanged),
    }
}

/// `WorldBorderCenterChangeEvent`: `world`'s border is about to move its center from `old`
/// to `new`. `None` if plugins stopped it, else the center they left.
pub fn on_border_center(server: &mut Server, world: usize, old: (f64, f64), new: (f64, f64)) -> Option<(f64, f64)> {
    if !listens(server, "WorldBorderCenterChangeEvent") {
        return Some(new);
    }
    let args = bmap! {
        "world" => server.worlds[world].name.clone(),
        "old_x" => old.0,
        "old_z" => old.1,
        "x" => new.0,
        "z" => new.1,
    };
    match fire(server, "WorldBorderCenterChangeEvent", "event.border_center", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some((result.f64("x").unwrap_or(new.0), result.f64("z").unwrap_or(new.1))),
        None => Some(new),
    }
}

/// `WorldBorderBoundsChangeFinishEvent`: `world`'s border got from `from` to `to`, a move of
/// `ticks`.
pub fn on_border_arrived(server: &mut Server, world: usize, from: f64, to: f64, ticks: i64) {
    if !listens(server, "WorldBorderBoundsChangeFinishEvent") {
        return;
    }
    let args = bmap! { "world" => server.worlds[world].name.clone(), "from" => from, "to" => to, "ticks" => ticks };
    fire(server, "WorldBorderBoundsChangeFinishEvent", "event.border_arrived", args);
}

/// `HangingPlaceEvent`: the player hung `entity` (an item frame or painting) on side `face`
/// (0 down to 5 east) of `block`, with `item` from `hand`. Returns whether plugins let it stay.
#[allow(clippy::too_many_arguments)]
pub fn on_hanging_place(server: &mut Server, world: usize, entity: i32, player: i32, block: BlockPos, face: usize, hand: i32, item: &paper_protocol::types::ItemStack) -> bool {
    if !listens(server, "HangingPlaceEvent") || !server.worlds[world].entities.contains_key(&entity) {
        return true;
    }
    let Some(p) = server.players.get(&player) else { return true };
    let args = bmap! {
        "uuid" => p.uuid().to_string(),
        "world" => server.worlds[world].name.clone(),
        "entity" => actor_value(server, world, entity),
        "x" => block.x, "y" => block.y, "z" => block.z,
        "face" => ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"][face.min(5)],
        "hand" => hand,
        "item" => super::api::item_value(&paper_protocol::types::Slot(Some(item.clone()))),
    };
    match fire(server, "HangingPlaceEvent", "event.hanging_place", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// `HangingBreakEvent` (or `HangingBreakByEntityEvent` when `damage` names an entity that
/// dealt it), then Paper's `EntityBreakEvent`: `entity` is about to break for `cause`
/// (`ENTITY`, `EXPLOSION`, `OBSTRUCTION`, `PHYSICS` or `DEFAULT`). Returns whether plugins
/// let it.
pub fn on_hanging_break(server: &mut Server, world: usize, entity: i32, cause: &str, damage: Option<&crate::server::survival::Damage>) -> bool {
    if !listens(server, "HangingBreakEvent") && !listens(server, "EntityBreakEvent") || !server.worlds[world].entities.contains_key(&entity) {
        return true;
    }
    let mut args = match damage {
        Some(d) => damage_args(server, entity, d, 0.0),
        None => bmap! { "target" => entity_ref(server, entity) },
    };
    args.set("cause", cause);
    if let Some(d) = damage {
        args.set("damage_type", d.kind);
    }
    if let Some(remover) = damage.and_then(|d| d.direct).filter(|r| server.players.contains_key(r) || server.worlds[world].entities.contains_key(r)) {
        args.set("remover", entity_ref(server, remover));
    }
    match fire(server, "", "event.hanging_break", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// Paper's `PlayerItemFrameChangeEvent`: the player is about to put `item` in the frame
/// `frame`, turn it, or knock it out (`action` `PLACE`, `ROTATE` or `REMOVE`). `None` if
/// plugins stopped it, else the item as they left it.
pub fn on_item_frame_change(server: &mut Server, world: usize, frame: i32, player: i32, item: &paper_protocol::types::Slot, action: &str) -> Option<paper_protocol::types::Slot> {
    if !listens(server, "PlayerItemFrameChangeEvent") || !server.worlds[world].entities.contains_key(&frame) {
        return Some(item.clone());
    }
    let Some(p) = server.players.get(&player) else { return Some(item.clone()) };
    let args = bmap! {
        "uuid" => p.uuid().to_string(),
        "entity" => actor_value(server, world, frame),
        "item" => super::api::item_value(item),
        "action" => action,
    };
    match fire(server, "PlayerItemFrameChangeEvent", "event.item_frame_change", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => match result.get("item") {
            Value::Null => Some(item.clone()),
            value => Some(super::api::slot_from(value).unwrap_or_else(|_| item.clone())),
        },
        None => Some(item.clone()),
    }
}

/// `PlayerFishEvent`: the player's bobber `hook` reached `state` (`FISHING`, `LURED`, `BITE`,
/// `FAILED_ATTEMPT`, `CAUGHT_FISH`, `CAUGHT_ENTITY`, `IN_GROUND` or `REEL_IN`), with what it
/// caught (an item not yet announced, or a hooked entity) and the experience a catch gives.
/// `None` if plugins stopped it, else the experience as they left it.
#[allow(clippy::too_many_arguments)]
pub fn on_fish(server: &mut Server, world: usize, player: i32, caught: Option<i32>, hook: i32, hand: Option<i32>, state: &str, exp: i32) -> Option<i32> {
    if !listens(server, "PlayerFishEvent") || !server.worlds[world].entities.contains_key(&hook) {
        return Some(exp);
    }
    let Some(p) = server.players.get(&player) else { return Some(exp) };
    let mut args = bmap! { "uuid" => p.uuid().to_string(), "hook" => actor_value(server, world, hook), "state" => state, "exp" => exp };
    if let Some(c) = caught.filter(|c| server.players.contains_key(c) || server.worlds[world].entities.contains_key(c)) {
        args.set("caught", entity_ref(server, c));
    }
    if let Some(h) = hand {
        args.set("hand", h);
    }
    match fire(server, "PlayerFishEvent", "event.fish", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.i32("exp").unwrap_or(exp)),
        None => Some(exp),
    }
}

/// Paper's `PlayerElytraBoostEvent`: the gliding player boosts with `rocket`, made from `item`
/// in `hand`. `None` if plugins stopped it, else whether the item is used up.
pub fn on_elytra_boost(server: &mut Server, world: usize, player: i32, rocket: i32, item: &paper_protocol::types::ItemStack, hand: i32) -> Option<bool> {
    if !listens(server, "PlayerElytraBoostEvent") || !server.worlds[world].entities.contains_key(&rocket) {
        return Some(true);
    }
    let Some(p) = server.players.get(&player) else { return Some(true) };
    let args = bmap! {
        "uuid" => p.uuid().to_string(),
        "entity" => actor_value(server, world, rocket),
        "item" => super::api::item_value(&paper_protocol::types::Slot(Some(item.clone()))),
        "hand" => hand,
    };
    match fire(server, "PlayerElytraBoostEvent", "event.elytra_boost", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.bool("consume") != Some(false)),
        None => Some(true),
    }
}

/// `FireworkExplodeEvent`: whether plugins let the rocket explode (it flies on if not).
pub fn on_firework_explode(server: &mut Server, world: usize, rocket: i32) -> bool {
    if !listens(server, "FireworkExplodeEvent") || !server.worlds[world].entities.contains_key(&rocket) {
        return true;
    }
    let args = bmap! { "entity" => actor_value(server, world, rocket) };
    match fire(server, "FireworkExplodeEvent", "event.firework_explode", args) {
        Some(result) => result.bool("cancelled") != Some(true),
        None => true,
    }
}

/// Paper's `PlayerShieldDisableEvent`: `attacker`'s axe is about to disable the player's
/// shield for `ticks`. `None` if plugins stopped it, else the cooldown as they left it.
pub fn on_shield_disable(server: &mut Server, player: i32, attacker: i32, ticks: i32) -> Option<i32> {
    if !listens(server, "PlayerShieldDisableEvent") {
        return Some(ticks);
    }
    let Some(p) = server.players.get(&player) else { return Some(ticks) };
    let args = bmap! { "uuid" => p.uuid().to_string(), "damager" => entity_ref(server, attacker), "cooldown" => ticks };
    match fire(server, "PlayerShieldDisableEvent", "event.shield_disable", args) {
        Some(result) if result.bool("cancelled") == Some(true) => None,
        Some(result) => Some(result.i32("cooldown").unwrap_or(ticks)),
        None => Some(ticks),
    }
}
