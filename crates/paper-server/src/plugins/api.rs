//! The server side of the plugin API: everything plugin code can ask the server to do.
//!
//! Every method takes a map of arguments and returns a value. Players are identified by UUID
//! and worlds by name. Text components travel as their JSON.

use super::{component_from, component_value};
use crate::config::GameMode;
use crate::server::commands::CommandSource;
use crate::server::player::{Location, Player, HOTBAR_START, INVENTORY_SLOTS};
use crate::server::Server;
use anyhow::{anyhow, bail, Result};
use paper_bridge::{bmap, Value};
use paper_data::{blocks, items};
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use paper_protocol::text::Component;
use paper_protocol::types::Slot;
use uuid::Uuid;

/// What the host learns about the server when it connects.
pub fn welcome(server: &Server) -> Value {
    let props = &server.config.properties;
    bmap! {
        "server_name" => server.config.paper.plugins.server_name.clone(),
        "minecraft_version" => paper_data::VERSION_NAME,
        "protocol" => paper_data::PROTOCOL_VERSION,
        "data_version" => paper_data::DATA_VERSION,
        "implementation_version" => env!("CARGO_PKG_VERSION"),
        "ip" => props.server_ip.clone(),
        "port" => props.server_port as i64,
        "motd" => component_value(&Component::from_legacy(&props.motd)),
        "max_players" => props.max_players,
        "online_mode" => props.online_mode,
        "view_distance" => props.view_distance,
        "simulation_distance" => props.simulation_distance,
        "default_game_mode" => server.default_game_mode.name(),
        "difficulty" => server.difficulty.name(),
        "hardcore" => props.hardcore,
        "allow_flight" => props.allow_flight,
        "spawn_protection" => props.spawn_protection,
        "whitelist" => props.white_list,
        "level_type" => props.level_type.clone(),
        "generate_structures" => props.generate_structures,
        "force_gamemode" => props.force_gamemode,
        "hide_online_players" => props.hide_online_players,
        "player_idle_timeout" => props.player_idle_timeout,
        "accepts_transfers" => props.accepts_transfers,
        // Other server.properties entries as written (`resource-pack`, ...).
        "properties" => Value::Map(props.raw.iter().map(|(k, v)| (k.clone(), Value::Str(v.clone()))).collect()),
        "root" => server.config.root.display().to_string(),
        "worlds" => Value::List(server.worlds.iter().map(world_info).collect()),
        // After `/reload` the new host starts with the players already online.
        "reload" => server.plugin_data.reloading,
        "players" => Value::List(server.online_players().map(|p| player_state(server, p)).collect()),
        "enchantments" => enchantment_info(server),
    }
}

/// Vanilla's enchantment definitions the API exposes (`Enchantment#getMaxLevel`,
/// `conflictsWith`, `canEnchantItem`), read from the game data when it is available.
fn enchantment_info(server: &Server) -> Value {
    let Some(vanilla) = server.vanilla.as_ref() else { return Value::Null };
    let resolve = |registry: &str, list: Option<&serde_json::Value>| -> Vec<Value> {
        let names: Vec<String> = match list {
            Some(serde_json::Value::String(s)) => vec![s.clone()],
            Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
            _ => Vec::new(),
        };
        let mut out = Vec::new();
        for name in names {
            match name.strip_prefix('#') {
                Some(tag) => {
                    let entries = paper_data::synced::entries(registry).unwrap_or(&[]);
                    for &id in paper_data::synced::tag(registry, tag).unwrap_or(&[]) {
                        // Items are a built-in registry; enchantments a synchronised one.
                        let entry = if registry == "minecraft:item" { items::get(id as u32).map(|i| i.name.to_owned()) } else { entries.get(id as usize).map(|e| (*e).to_owned()) };
                        if let Some(entry) = entry {
                            out.push(Value::Str(entry));
                        }
                    }
                }
                None => out.push(Value::Str(name)),
            }
        }
        out
    };
    let mut table = bmap! {};
    for name in paper_data::synced::entries("minecraft:enchantment").unwrap_or(&[]) {
        let path = format!("data/minecraft/enchantment/{}.json", name.strip_prefix("minecraft:").unwrap_or(name));
        let Some(json) = vanilla.read_json(&path) else { continue };
        table.set(
            name,
            bmap! {
                "max_level" => json["max_level"].as_i64().unwrap_or(1),
                "exclusive_set" => Value::List(resolve("minecraft:enchantment", json.get("exclusive_set"))),
                "supported_items" => Value::List(resolve("minecraft:item", json.get("supported_items"))),
                "curse" => paper_data::synced::tag("minecraft:enchantment", "minecraft:curse").zip(paper_data::synced::id("minecraft:enchantment", name)).is_some_and(|(ids, id)| ids.contains(&id)),
                "treasure" => paper_data::synced::tag("minecraft:enchantment", "minecraft:treasure").zip(paper_data::synced::id("minecraft:enchantment", name)).is_some_and(|(ids, id)| ids.contains(&id)),
            },
        );
    }
    table
}

fn world_info(w: &crate::world::World) -> Value {
    bmap! {
        "name" => w.name.clone(),
        "dimension" => w.dimension.clone(),
        "environment" => match w.dimension.as_str() {
            "minecraft:the_nether" => "NETHER",
            "minecraft:the_end" => "THE_END",
            _ => "NORMAL",
        },
        "min_y" => w.min_y,
        "height" => w.height,
        "seed" => w.seed,
        "sea_level" => w.generator.sea_level(),
        "spawn" => Value::List(vec![w.spawn.x.into(), w.spawn.y.into(), w.spawn.z.into()]),
        "spawn_yaw" => w.spawn_yaw,
        "time" => w.day_time,
        "full_time" => w.age,
        "daylight_cycle" => w.daylight_cycle,
        "raining" => w.weather.raining,
        "thundering" => w.weather.thundering,
        "rain_time" => w.weather.rain_time,
        "thunder_time" => w.weather.thunder_time,
        "clear_time" => w.weather.clear_time,
        "border" => bmap! {
            "center_x" => w.border.center_x,
            "center_z" => w.border.center_z,
            "size" => w.border.size,
            "target" => w.border.lerp_target,
            "lerp_left" => w.border.lerp_left,
            "damage_buffer" => w.border.damage_safe_zone,
            "damage_amount" => w.border.damage_per_block,
            "warning_distance" => w.border.warning_blocks,
            "warning_time" => w.border.warning_time,
        },
    }
}

/// The state of a player the host caches. The host refreshes its copy whenever this changes.
pub fn player_state(server: &Server, p: &Player) -> Value {
    bmap! {
        "uuid" => p.uuid().to_string(),
        "name" => p.name(),
        "entity_id" => p.entity_id,
        "vehicle" => p.vehicle.map(|v| super::entity::shallow_ref(server, v)),
        "world" => server.worlds.get(p.world).map(|w| w.name.clone()),
        "x" => p.loc.x,
        "y" => p.loc.y,
        "z" => p.loc.z,
        "yaw" => p.loc.yaw,
        "pitch" => p.loc.pitch,
        "on_ground" => p.on_ground,
        "game_mode" => p.game_mode.name(),
        "op_level" => p.op_level as i64,
        "health" => p.health,
        "resource_pack_status" => p.resource_pack_status.map(i64::from),
        "food" => p.food,
        "saturation" => p.saturation,
        "sneaking" => p.sneaking,
        "sprinting" => p.sprinting,
        "flying" => p.flying,
        "allow_flight" => p.allow_flight,
        "held_slot" => p.held_slot as i64,
        "locale" => p.settings.locale.clone(),
        "view_distance" => p.view_distance(server.config.properties.view_distance),
        "ping" => p.keep_alive.latency,
        "brand" => p.brand.clone(),
        "address" => p.conn.addr.to_string(),
        "virtual_host" => p.virtual_host.clone(),
        "display_name" => p.display_name.as_ref().map(component_value),
        "list_name" => p.list_name.as_ref().map(component_value),
        "skin_parts" => p.settings.skin_parts as i64,
        "main_hand" => if p.settings.main_hand == 0 { "LEFT" } else { "RIGHT" },
        "properties" => Value::List(p.profile.properties.iter().map(|prop| bmap! {
            "name" => prop.name.clone(),
            "value" => prop.value.clone(),
            "signature" => prop.signature.clone(),
        }).collect()),
        "open_inventory" => super::inventory::window_value(server, p.entity_id),
        "scoreboard" => server.scoreboards.board_of(p.entity_id) as i64,
        "fire_ticks" => p.fire_ticks,
        "air" => p.air,
        "dead" => p.dead,
        "xp_level" => p.xp_level,
        "xp_progress" => p.xp_progress,
        "xp_total" => p.xp_total,
        "exhaustion" => p.exhaustion,
        // What plugins set through the API (see `plugins::player`).
        "walk_speed" => p.api.walk_speed * 2.0,
        "fly_speed" => p.api.fly_speed * 2.0,
        "sleeping_ignored" => p.api.sleeping_ignored,
        "player_time" => server.player_time(p.world, p),
        "player_time_offset" => p.api.time.map_or(0, |(offset, _)| offset),
        "player_time_relative" => p.api.time.is_none_or(|(_, relative)| relative),
        "player_weather" => p.api.weather.map(|rain| if rain { "DOWNFALL" } else { "CLEAR" }),
        "compass_target" => p.api.compass_target.filter(|(w, _, _)| *w < server.worlds.len()).map(|(w, pos, yaw)| Value::List(vec![server.worlds[w].name.clone().into(), pos.x.into(), pos.y.into(), pos.z.into(), yaw.into()])),
        "invulnerable" => p.api.invulnerable,
        "glowing" => p.api.glowing,
        "silent" => p.api.silent,
        "no_gravity" => p.api.no_gravity,
        "invisible" => p.api.invisible,
        "custom_name" => p.api.custom_name.as_ref().map(component_value),
        "custom_name_visible" => p.api.custom_name_visible,
        "tags" => Value::List(p.api.tags.iter().map(|t| Value::Str(t.clone())).collect()),
        "max_air" => p.api.max_air,
        "no_damage_ticks" => p.invulnerable_time,
        "max_no_damage_ticks" => p.api.max_no_damage_ticks,
        "last_damage" => p.last_hurt,
        "fall_distance" => p.fall_distance,
        "absorption" => p.absorption,
        "health_scale" => p.api.health_scale,
        "freeze_ticks" => p.api.freeze_ticks,
        "arrows_in_body" => p.api.arrows_in_body,
        "stingers_in_body" => p.api.stingers_in_body,
        "score" => p.api.score,
        "sleeping" => p.sleeping.map(|b| Value::List(vec![b.x.into(), b.y.into(), b.z.into()])),
        "sleep_ticks" => p.sleep_ticks,
        "respawn" => p.respawn.filter(|(w, _)| *w < server.worlds.len()).map(|(w, pos)| Value::List(vec![server.worlds[w].name.clone().into(), pos.x.into(), pos.y.into(), pos.z.into()])),
        "ticks_lived" => server.tick.saturating_sub(p.api.joined_tick) as i64,
        "portal_cooldown" => p.portal_cooldown,
        "chat_visibility" => p.settings.chat_mode,
        "chat_colors" => p.settings.chat_colors,
        "text_filtering" => p.settings.text_filtering,
        "allow_server_listings" => p.settings.allow_server_listings,
        "particle_status" => p.settings.particle_status,
        "hidden" => Value::List(p.api.hidden.iter().map(|u| Value::Str(u.to_string())).collect()),
        "seen_credits" => p.seen_credits,
        "killer" => p.last_attacker.filter(|(_, at)| server.tick.saturating_sub(*at) <= 100).and_then(|(a, _)| server.players.get(&a)).map(|k| k.uuid().to_string()),
        "using_item" => p.using_item.map(|(hand, ticks)| Value::List(vec![(hand as i64).into(), (ticks as i64).into()])),
        "ticks_since_swing" => server.tick.saturating_sub(p.last_swing) as i64,
        "previous_game_mode" => p.previous_game_mode.map(|m| m.name()),
    }
}

/// An item stack as the host models it: item id, count, every added component as its network
/// bytes (`raw`: `[type id, bytes]`), the ids of removed default components (`removed`), and
/// the text components the server can translate as JSON as well (`name`, `item_name`,
/// `lore`).
pub fn item_value(slot: &Slot) -> Value {
    let Some(stack) = slot.0.as_ref().filter(|s| s.count > 0) else { return Value::Null };
    let Some(info) = items::get(stack.item as u32) else { return Value::Null };
    let mut out = bmap! { "item" => info.name, "count" => stack.count };
    let mut raw = Vec::new();
    for (ty, data) in &stack.added {
        let name = paper_data::registries::name("minecraft:data_component_type", *ty as u32).unwrap_or("");
        match name {
            "minecraft:custom_name" | "minecraft:item_name" => {
                if let Ok(c) = paper_protocol::codec::Reader::new(data).component() {
                    out.set(if name.ends_with("custom_name") { "name" } else { "item_name" }, component_value(&c));
                }
            }
            "minecraft:lore" => {
                if let Ok(lines) = paper_protocol::codec::Reader::new(data).list(256, |r| r.component()) {
                    out.set("lore", Value::List(lines.iter().map(component_value).collect()));
                }
            }
            "minecraft:written_book_content" => {
                if let Ok(book) = WrittenBook::read(data) {
                    out.set("book", book.value());
                }
            }
            _ => {}
        }
        raw.push(Value::List(vec![(*ty).into(), Value::Bytes(data.clone())]));
    }
    if !raw.is_empty() {
        out.set("raw", Value::List(raw));
    }
    if !stack.removed.is_empty() {
        out.set("removed", Value::List(stack.removed.iter().map(|r| Value::from(*r)).collect()));
    }
    out
}

/// `minecraft:written_book_content` as the host sees it: the pages as JSON text, like lore.
/// The chat-filtered variants of the title and pages are not kept.
#[derive(Debug, PartialEq)]
struct WrittenBook {
    title: String,
    author: String,
    generation: i32,
    pages: Vec<Component>,
    resolved: bool,
}

impl WrittenBook {
    fn read(data: &[u8]) -> Result<Self> {
        let mut r = paper_protocol::codec::Reader::new(data);
        let title = r.string(32767)?;
        r.option(|r| r.string(32767))?;
        let author = r.string(32767)?;
        let generation = r.varint()?;
        let pages = r.list(100, |r| {
            let page = r.component()?;
            r.option(|r| r.component())?;
            Ok(page)
        })?;
        let resolved = r.bool()?;
        Ok(Self { title, author, generation, pages, resolved })
    }

    fn write(&self) -> Vec<u8> {
        use paper_protocol::codec::WriteExt;
        let mut out = Vec::new();
        out.put_string(&self.title);
        out.put_bool(false);
        out.put_string(&self.author);
        out.put_varint(self.generation);
        out.put_list(&self.pages, |w, page| {
            w.put_component(page);
            w.put_bool(false);
        });
        out.put_bool(self.resolved);
        out
    }

    fn value(&self) -> Value {
        bmap! {
            "title" => self.title.clone(),
            "author" => self.author.clone(),
            "generation" => self.generation,
            "pages" => Value::List(self.pages.iter().map(component_value).collect()),
            "resolved" => self.resolved,
        }
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.str("title")?;
        Some(Self {
            title: value.str("title").unwrap_or_default().to_owned(),
            author: value.str("author").unwrap_or_default().to_owned(),
            generation: value.i32("generation").unwrap_or(0),
            pages: value.list("pages").map(|pages| pages.iter().filter_map(component_from).collect()).unwrap_or_default(),
            resolved: value.bool("resolved").unwrap_or(false),
        })
    }
}

/// The inverse of [`item_value`]. A text component given as JSON replaces the raw bytes of
/// the same component, unless they already encode the same text (so an unchanged stack keeps
/// its exact bytes and still stacks with its twins). Components are kept in type order.
pub fn slot_from(value: &Value) -> Result<Slot> {
    if value.is_null() {
        return Ok(Slot::EMPTY);
    }
    let name = value.str("item").ok_or_else(|| anyhow!("item stack without an item"))?;
    let item = items::id(name).ok_or_else(|| anyhow!("unknown item {name}"))? as i32;
    let count = value.i32("count").unwrap_or(1);
    if count <= 0 || item == 0 {
        return Ok(Slot::EMPTY);
    }
    let mut stack = paper_protocol::types::ItemStack::new(item, count.min(99));
    let component_id = |name: &str| paper_data::registries::id("minecraft:data_component_type", name).map(|i| i as i32);
    use paper_protocol::codec::WriteExt;
    let mut added: std::collections::BTreeMap<i32, Vec<u8>> = std::collections::BTreeMap::new();
    if let Some(raw) = value.list("raw") {
        for entry in raw {
            if let (Some(ty), Some(data)) = (entry.at(0).as_i32(), entry.at(1).as_bytes()) {
                added.insert(ty, data.to_vec());
            }
        }
    }
    for (key, component) in [("name", "minecraft:custom_name"), ("item_name", "minecraft:item_name")] {
        let (Some(c), Some(id)) = (component_from(value.get(key)), component_id(component)) else { continue };
        let same = added.get(&id).and_then(|bytes| paper_protocol::codec::Reader::new(bytes).component().ok()).is_some_and(|old| old == c);
        if !same {
            let mut data = Vec::new();
            data.put_component(&c);
            added.insert(id, data);
        }
    }
    if let (Some(lines), Some(id)) = (value.list("lore"), component_id("minecraft:lore")) {
        let lines: Vec<Component> = lines.iter().filter_map(component_from).collect();
        let same = added.get(&id).and_then(|bytes| paper_protocol::codec::Reader::new(bytes).list(256, |r| r.component()).ok()).is_some_and(|old| old == lines);
        if !same {
            let mut data = Vec::new();
            data.put_list(&lines, |w, c| w.put_component(c));
            added.insert(id, data);
        }
    }
    if let (Some(book), Some(id)) = (WrittenBook::from_value(value.get("book")), component_id("minecraft:written_book_content")) {
        if added.get(&id).and_then(|bytes| WrittenBook::read(bytes).ok()).as_ref() != Some(&book) {
            added.insert(id, book.write());
        }
    }
    stack.added = added.into_iter().collect();
    if let Some(removed) = value.list("removed") {
        stack.removed.extend(removed.iter().filter_map(Value::as_i32));
        stack.removed.sort_unstable();
        stack.removed.dedup();
    }
    Ok(Slot(Some(stack)))
}

fn player_id(server: &Server, args: &Value) -> Result<i32> {
    let uuid: Uuid = args.str("uuid").ok_or_else(|| anyhow!("missing uuid"))?.parse()?;
    server.by_uuid.get(&uuid).copied().ok_or_else(|| anyhow!("player {uuid} is not online"))
}

fn world_index(server: &Server, args: &Value) -> Result<usize> {
    match args.str("world") {
        Some(name) => server.worlds.iter().position(|w| w.name == name).ok_or_else(|| anyhow!("unknown world {name}")),
        None => Ok(0),
    }
}

fn block_xyz(args: &Value) -> Result<(i32, i32, i32)> {
    Ok((args.i32("x").ok_or_else(|| anyhow!("missing x"))?, args.i32("y").ok_or_else(|| anyhow!("missing y"))?, args.i32("z").ok_or_else(|| anyhow!("missing z"))?))
}

fn state_value(server: &Server, world: usize, x: i32, y: i32, z: i32) -> Value {
    let w = &server.worlds[world];
    match w.block_at(x, y, z) {
        Some(state) => bmap! {
            "state" => blocks::state_to_string(state),
            "id" => state,
            "loaded" => true,
            "sky_light" => w.chunks.get(&crate::world::chunk::ChunkPos::of_block(x, z)).map_or(0, |c| c.sky_light_at((x & 15) as usize, y, (z & 15) as usize)),
            "block_light" => w.chunks.get(&crate::world::chunk::ChunkPos::of_block(x, z)).map_or(0, |c| c.block_light_at((x & 15) as usize, y, (z & 15) as usize)),
        },
        None => bmap! { "state" => "minecraft:air", "id" => 0, "loaded" => false, "sky_light" => 0, "block_light" => 0 },
    }
}

/// Loads the chunk of a block if it is not loaded (Paper's block access loads chunks).
fn ensure_chunk(server: &mut Server, world: usize, x: i32, z: i32) {
    let pos = crate::world::chunk::ChunkPos::of_block(x, z);
    if !server.worlds[world].chunks.contains_key(&pos) {
        server.load_chunk_now(world, pos);
    }
}

/// Whether a chunk exists: loaded, or saved (`World.isChunkGenerated`).
fn chunk_generated(server: &Server, world: usize, pos: crate::world::chunk::ChunkPos) -> bool {
    let w = &server.worlds[world];
    w.chunks.contains_key(&pos) || w.storage.as_ref().is_some_and(|s| s.has_chunk(pos))
}

/// Executes one API call from the plugin host.
pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    match method {
        // --- players ---
        // `Player.getStatistic`/`setStatistic`: a stat type and key.
        // Online or offline players (by uuid).
        "player.statistic" => {
            let uuid: Uuid = args.str("uuid").ok_or_else(|| anyhow!("missing uuid"))?.parse()?;
            let (kind, key) = (args.str("type").unwrap_or(crate::server::stats::CUSTOM), args.str("key").unwrap_or(""));
            Ok(Value::Int(server.statistic(uuid, kind, key) as i64))
        }
        "player.set_statistic" => {
            let uuid: Uuid = args.str("uuid").ok_or_else(|| anyhow!("missing uuid"))?.parse()?;
            let (kind, key) = (args.str("type").unwrap_or(crate::server::stats::CUSTOM).to_owned(), args.str("key").unwrap_or("").to_owned());
            server.set_statistic(uuid, &kind, &key, args.i32("value").unwrap_or(0));
            Ok(Value::Null)
        }
        "player.state" => {
            let id = player_id(server, args)?;
            Ok(player_state(server, &server.players[&id]))
        }
        "player.send_message" => {
            let id = player_id(server, args)?;
            let message = component_from(args.get("message")).ok_or_else(|| anyhow!("missing message"))?;
            server.send(id, &play::SystemChat { content: message, overlay: args.bool("overlay").unwrap_or(false) });
            Ok(Value::Null)
        }
        "player.send_title" => {
            // Only the parts given: the times, the subtitle, then the title, which shows it.
            let id = player_id(server, args)?;
            if ["fade_in", "stay", "fade_out"].iter().any(|k| !args.get(k).is_null()) {
                let times = play::SetTitlesAnimation { fade_in: args.i32("fade_in").unwrap_or(10), stay: args.i32("stay").unwrap_or(70), fade_out: args.i32("fade_out").unwrap_or(20) };
                server.send(id, &times);
            }
            if let Some(sub) = component_from(args.get("subtitle")) {
                server.send(id, &play::SetSubtitleText { text: sub });
            }
            if let Some(title) = component_from(args.get("title")) {
                server.send(id, &play::SetTitleText { text: title });
            }
            Ok(Value::Null)
        }
        "player.clear_title" => {
            let id = player_id(server, args)?;
            server.send(id, &play::ClearTitles { reset: args.bool("reset").unwrap_or(false) });
            Ok(Value::Null)
        }
        "player.send_tab_list" => {
            let id = player_id(server, args)?;
            server.send(id, &play::TabList { header: component_from(args.get("header")).unwrap_or_default(), footer: component_from(args.get("footer")).unwrap_or_default() });
            Ok(Value::Null)
        }
        "player.kick" => {
            let id = player_id(server, args)?;
            let reason = component_from(args.get("reason")).unwrap_or_else(|| Component::translatable("multiplayer.disconnect.kicked", vec![]));
            server.kick(id, &reason);
            Ok(Value::Null)
        }
        "player.teleport" => {
            let id = player_id(server, args)?;
            let world = world_index(server, args)?;
            let current = server.players[&id].loc;
            let target = Location {
                x: args.f64("x").unwrap_or(current.x),
                y: args.f64("y").unwrap_or(current.y),
                z: args.f64("z").unwrap_or(current.z),
                yaw: args.f64("yaw").map_or(current.yaw, |v| v as f32),
                pitch: args.f64("pitch").map_or(current.pitch, |v| v as f32),
            };
            if server.players[&id].world == world {
                server.teleport(id, target);
            } else {
                // To another world, as portals take players.
                server.change_dimension(id, world, target);
            }
            Ok(player_state(server, &server.players[&id]))
        }
        "player.set_game_mode" => {
            let id = player_id(server, args)?;
            let mode = args.str("mode").and_then(GameMode::parse).ok_or_else(|| anyhow!("invalid game mode"))?;
            server.set_game_mode(id, mode);
            Ok(player_state(server, &server.players[&id]))
        }
        "player.set_op" => {
            let id = player_id(server, args)?;
            let (uuid, name) = { let p = &server.players[&id]; (p.uuid(), p.name().to_owned()) };
            let level = if args.bool("op").unwrap_or(false) {
                let level = server.config.properties.op_permission_level;
                server.shared.access.write().op(uuid, &name, level);
                level
            } else {
                server.shared.access.write().deop(uuid);
                0
            };
            server.set_op_level(id, level);
            Ok(Value::Null)
        }
        "player.set_display_name" | "player.set_list_name" => {
            let id = player_id(server, args)?;
            let name = component_from(args.get("name"));
            let player = server.players.get_mut(&id).unwrap();
            if method == "player.set_display_name" {
                player.display_name = name;
            } else {
                player.list_name = name;
                let entry = crate::server::join_info_entry(player);
                server.broadcast(&play::PlayerInfoUpdate { actions: play::player_info::UPDATE_DISPLAY_NAME, entries: vec![entry] });
            }
            Ok(Value::Null)
        }
        "player.set_health" => {
            let id = player_id(server, args)?;
            let player = server.players.get_mut(&id).unwrap();
            if let Some(h) = args.f64("health") {
                player.health = (h as f32).clamp(0.0, 1024.0);
            }
            if let Some(f) = args.i32("food") {
                player.food = f.clamp(0, 20);
            }
            if let Some(s) = args.f64("saturation") {
                player.saturation = s as f32;
            }
            player.conn.send(&play::SetHealth { health: player.health, food: VarInt(player.food), saturation: player.saturation });
            Ok(Value::Null)
        }
        "player.set_flight" => {
            let id = player_id(server, args)?;
            let player = server.players.get_mut(&id).unwrap();
            if let Some(allow) = args.bool("allow_flight") {
                player.allow_flight = allow;
                if !allow {
                    player.flying = false;
                }
            }
            if let Some(flying) = args.bool("flying") {
                if flying && !player.allow_flight {
                    bail!("the player is not allowed to fly");
                }
                player.flying = flying;
            }
            player.conn.send(&player.abilities_packet());
            Ok(Value::Null)
        }
        "player.perform_command" => {
            let id = player_id(server, args)?;
            let command = args.str("command").ok_or_else(|| anyhow!("missing command"))?.to_owned();
            server.dispatch_command(&CommandSource::Player(id), &command);
            Ok(Value::Bool(true))
        }
        "player.chat" => {
            let id = player_id(server, args)?;
            let message = args.str("message").ok_or_else(|| anyhow!("missing message"))?.to_owned();
            if let Some(command) = message.strip_prefix('/') {
                server.dispatch_command(&CommandSource::Player(id), command);
            } else {
                server.handle_player_chat(id, &message);
            }
            Ok(Value::Null)
        }
        "player.send_plugin_message" => {
            let id = player_id(server, args)?;
            let channel = args.str("channel").ok_or_else(|| anyhow!("missing channel"))?.to_owned();
            let data = args.get("data").as_bytes().unwrap_or(&[]).to_vec();
            server.send(id, &play::CustomPayload { channel, data: paper_protocol::codec::RemainingBytes(data) });
            Ok(Value::Null)
        }
        "player.update_commands" => {
            let id = player_id(server, args)?;
            let level = server.players[&id].op_level;
            let tree = server.command_tree_for(id, level);
            server.send(id, &tree);
            Ok(Value::Null)
        }
        "player.play_sound" => {
            let id = player_id(server, args)?;
            let player = &server.players[&id];
            let sound = args.str("sound").ok_or_else(|| anyhow!("missing sound"))?;
            let sound = match paper_data::registries::id("minecraft:sound_event", sound) {
                Some(i) => play::SoundRef::Id(i as i32),
                None => play::SoundRef::Named { name: sound.to_owned(), fixed_range: None },
            };
            let packet = play::Sound {
                sound,
                category: args.i32("category").unwrap_or(0),
                x: args.f64("x").unwrap_or(player.loc.x),
                y: args.f64("y").unwrap_or(player.loc.y),
                z: args.f64("z").unwrap_or(player.loc.z),
                volume: args.f64("volume").unwrap_or(1.0) as f32,
                pitch: args.f64("pitch").unwrap_or(1.0) as f32,
                seed: args.i64("seed").unwrap_or_else(rand::random),
            };
            player.conn.send(&packet);
            Ok(Value::Null)
        }
        "player.block_change" => {
            // Player#sendBlockChange: only this player sees the block (until the chunk resends it).
            let id = player_id(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            let state = match args.get("state") {
                Value::Str(s) => blocks::parse_state(s).ok_or_else(|| anyhow!("invalid block state {s}"))?,
                other => bail!("invalid block state {other}"),
            };
            server.players[&id].conn.send(&play::BlockUpdate { pos: paper_protocol::codec::BlockPos::new(x, y, z), state: paper_protocol::codec::VarInt(state as i32) });
            Ok(Value::Null)
        }
        "player.resource_pack_push" => {
            let id = player_id(server, args)?;
            let pack = args.str("id").and_then(|s| uuid::Uuid::parse_str(s).ok()).ok_or_else(|| anyhow!("missing pack id"))?;
            let packet = play::ResourcePackPush {
                uuid: pack,
                url: args.str("url").ok_or_else(|| anyhow!("missing url"))?.to_owned(),
                hash: args.str("hash").unwrap_or("").to_owned(),
                forced: args.bool("forced").unwrap_or(false),
                prompt: component_from(args.get("prompt")),
            };
            server.players[&id].conn.send(&packet);
            Ok(Value::Null)
        }
        "player.resource_pack_pop" => {
            let id = player_id(server, args)?;
            let pack = args.str("id").and_then(|s| uuid::Uuid::parse_str(s).ok());
            server.players[&id].conn.send(&play::ResourcePackPop { uuid: pack });
            Ok(Value::Null)
        }
        "player.open_book" => {
            // As CraftPlayer#openBook: the book goes into the held slot on the client only, the
            // client opens it, and the real item comes back.
            let id = player_id(server, args)?;
            let book = slot_from(args.get("item"))?;
            let p = &server.players[&id];
            let slot = (HOTBAR_START + p.held_slot as usize) as i16;
            let real = p.inventory[slot as usize].clone();
            p.conn.send(&play::ContainerSetSlot { window_id: 0, state_id: p.inventory_state, slot, item: book });
            p.conn.send(&play::OpenBook { hand: VarInt(0) });
            p.conn.send(&play::ContainerSetSlot { window_id: 0, state_id: p.inventory_state, slot, item: real });
            Ok(Value::Null)
        }
        "player.stop_sound" => {
            let id = player_id(server, args)?;
            server.players[&id].conn.send(&play::StopSound { category: args.i32("category"), sound: args.str("sound").map(str::to_owned) });
            Ok(Value::Null)
        }
        "player.inventory" => {
            let id = player_id(server, args)?;
            let player = &server.players[&id];
            Ok(bmap! {
                "slots" => Value::List(player.inventory.iter().map(item_value).collect()),
                "held_slot" => player.held_slot as i64,
            })
        }
        "player.set_slot" => {
            let id = player_id(server, args)?;
            let slot = args.i32("slot").ok_or_else(|| anyhow!("missing slot"))?;
            if !(0..INVENTORY_SLOTS as i32).contains(&slot) {
                bail!("slot {slot} is outside the player inventory");
            }
            let item = slot_from(args.get("item"))?;
            server.set_inventory_slot(id, slot as usize, item);
            Ok(Value::Null)
        }
        "player.set_held_slot" => {
            let id = player_id(server, args)?;
            let slot = args.i32("slot").filter(|s| (0..9).contains(s)).ok_or_else(|| anyhow!("held slot must be 0-8"))?;
            let player = server.players.get_mut(&id).unwrap();
            player.held_slot = slot as u8;
            player.equipment_dirty = true;
            player.conn.send(&play::SetHeldSlot { slot: VarInt(slot) });
            let _ = HOTBAR_START;
            Ok(Value::Null)
        }
        "player.open_inventory" => {
            let id = player_id(server, args)?;
            let inv = super::inventory::InvRef::parse(server, args.get("inventory"))?;
            let opened = super::inventory::open(server, id, &inv, component_from(args.get("title")))?;
            Ok(bmap! { "opened" => opened, "window" => super::inventory::window_value(server, id) })
        }
        "player.close_inventory" => {
            let id = player_id(server, args)?;
            let reason = args.str("reason").unwrap_or("PLUGIN").to_owned();
            server.close_menu_with_reason(id, true, &reason);
            Ok(Value::Null)
        }
        "player.cursor" => {
            let id = player_id(server, args)?;
            Ok(item_value(&server.players[&id].carried))
        }
        "player.set_cursor" => {
            let id = player_id(server, args)?;
            let item = slot_from(args.get("item"))?;
            let player = server.players.get_mut(&id).unwrap();
            player.carried = item.clone();
            player.conn.send(&play::SetCursorItem { item });
            Ok(Value::Null)
        }
        "player.set_scoreboard" => {
            let id = player_id(server, args)?;
            let board = args.i64("board").ok_or_else(|| anyhow!("missing board"))? as u64;
            server.set_player_scoreboard(id, board)?;
            Ok(Value::Null)
        }
        "player.give_exp" => {
            let id = player_id(server, args)?;
            server.give_experience(id, args.i32("amount").unwrap_or(0));
            Ok(Value::Null)
        }
        m if super::player::handles(m) => super::player::handle(server, m, args),
        m if m.starts_with("inventory.") => super::inventory::handle(server, m, args),
        m if m.starts_with("entity.") || m == "world.entities" || m == "world.spawn_entity" || m == "world.drop_item" => super::entity::handle(server, m, args),
        m if m.starts_with("scoreboard.") => super::scoreboard::handle(server, m, args),
        m if m.starts_with("bossbar.") => super::bossbar::handle(server, m, args),
        m if m.starts_with("attribute.") => super::attributes::handle(server, m, args),
        m if m.starts_with("merchant.") || m.starts_with("villager.") => super::merchant::handle(server, m, args),
        m if m.starts_with("offline.") => super::offline::handle(server, m, args),

        // --- worlds and blocks ---
        "world.get_block" => {
            // As on Paper, reading a block loads (or generates) its chunk.
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            Ok(state_value(server, world, x, y, z))
        }
        "world.block_entity" => {
            // A block entity as saved (named NBT, with its id and position), or null.
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            let chunk = crate::world::chunk::ChunkPos::of_block(x, z);
            let Some(nbt) = server.worlds[world].chunks.get(&chunk).and_then(|c| c.block_entities.get(&(y, (z & 15) as u8, (x & 15) as u8))) else { return Ok(Value::Null) };
            let mut nbt = nbt.clone();
            nbt.insert("x", paper_nbt::Tag::Int(x));
            nbt.insert("y", paper_nbt::Tag::Int(y));
            nbt.insert("z", paper_nbt::Tag::Int(z));
            let mut out = Vec::new();
            paper_nbt::write_named(&mut out, "", &nbt);
            Ok(Value::Bytes(out))
        }
        "world.set_block_entity" => {
            // Replaces a block entity's data (`id` and position come from the block) and shows
            // players what changed.
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            let bytes = args.get("nbt").as_bytes().ok_or_else(|| anyhow!("missing nbt"))?;
            let (_, mut nbt) = paper_nbt::read_named(&mut &bytes[..], 2 * 1024 * 1024)?;
            for key in ["x", "y", "z"] {
                nbt.remove(key);
            }
            let chunk = crate::world::chunk::ChunkPos::of_block(x, z);
            let key = (y, (z & 15) as u8, (x & 15) as u8);
            let Some(c) = server.worlds[world].chunks.get_mut(&chunk) else { return Ok(Value::Bool(false)) };
            let id = nbt.string("id").map(str::to_owned).or_else(|| c.block_entities.get(&key).and_then(|e| e.string("id")).map(str::to_owned));
            let Some(id) = id else { return Ok(Value::Bool(false)) };
            nbt.insert("id", paper_nbt::Tag::String(id.clone()));
            c.block_entities.insert(key, nbt.clone());
            c.dirty = true;
            if let Some(ty) = paper_data::registries::id("minecraft:block_entity_type", &id) {
                nbt.remove("id");
                let packet = play::BlockEntityData { pos: paper_protocol::codec::BlockPos::new(x, y, z), block_entity_type: VarInt(ty as i32), data: paper_protocol::codec::Nbt(Some(paper_nbt::Tag::Compound(nbt))) };
                server.broadcast_to_chunk_viewers(world, chunk, &packet);
            }
            Ok(Value::Bool(true))
        }
        "world.sign" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            Ok(server.sign_value(world, paper_protocol::codec::BlockPos::new(x, y, z)).unwrap_or(Value::Null))
        }
        "world.set_sign" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            Ok(Value::Bool(server.set_sign_value(world, paper_protocol::codec::BlockPos::new(x, y, z), args)))
        }
        "world.get_biome" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            Ok(Value::Str(server.biome_at(world, x, y, z).to_owned()))
        }
        "world.set_block" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            let state = match args.get("state") {
                Value::Str(s) => blocks::parse_state(s).ok_or_else(|| anyhow!("invalid block state {s}"))?,
                Value::Int(i) if blocks::is_valid_state(*i as u32) => *i as u32,
                other => bail!("invalid block state {other}"),
            };
            ensure_chunk(server, world, x, z);
            let Some(old) = server.worlds[world].block_at(x, y, z) else { bail!("the chunk at {x}, {z} could not be loaded") };
            // Bukkit's applyPhysics (default true): neighbours react like for any other change;
            // without physics nothing reacts, as in CraftBlock.
            use crate::world::block_updates::flags;
            let update = if args.bool("physics").unwrap_or(true) { flags::ALL } else { flags::CLIENTS | flags::KNOWN_SHAPE | flags::SKIP_ON_PLACE };
            let changed = server.set_block_with_flags(world, paper_protocol::codec::BlockPos::new(x, y, z), state, update);
            if args.bool("return_old") == Some(true) {
                // WorldEdit's adapter: the state before, if it changed.
                return Ok(if changed { Value::Int(old as i64) } else { Value::Null });
            }
            Ok(Value::Bool(changed))
        }
        "world.highest_block_y" => {
            let world = world_index(server, args)?;
            let (x, z) = (args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            Ok(server.worlds[world].top_y(x, z).map(|y| Value::Int(y as i64 - 1)).unwrap_or(Value::Null))
        }
        "world.set_time" => {
            let world = world_index(server, args)?;
            if let Some(t) = args.i64("time") {
                server.worlds[world].day_time = t.max(0);
            }
            if let Some(cycle) = args.bool("daylight_cycle") {
                server.worlds[world].daylight_cycle = cycle;
                server.worlds[world].rules.set("advance_time", paper_data::game_rules::Value::Bool(cycle));
            }
            server.resend_time(world);
            Ok(Value::Null)
        }
        "world.set_spawn" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            server.set_world_spawn(world, paper_protocol::codec::BlockPos::new(x, y, z), args.f64("yaw").unwrap_or(0.0) as f32);
            Ok(Value::Null)
        }
        "world.info" => {
            let world = world_index(server, args)?;
            Ok(world_info(&server.worlds[world]))
        }
        "world.set_weather" => {
            // `World.setStorm`, `setThundering` and the durations; the weather tick fades
            // the levels and tells the players.
            let world = world_index(server, args)?;
            let w = &mut server.worlds[world].weather;
            if let Some(b) = args.bool("raining") {
                w.raining = b;
            }
            if let Some(b) = args.bool("thundering") {
                w.thundering = b;
            }
            if let Some(t) = args.i32("rain_time") {
                w.rain_time = t.max(0);
            }
            if let Some(t) = args.i32("thunder_time") {
                w.thunder_time = t.max(0);
            }
            if let Some(t) = args.i32("clear_time") {
                w.clear_time = t.max(0);
            }
            Ok(world_info(&server.worlds[world]))
        }
        "world.set_border" => {
            // `WorldBorder`: each change goes to the world's players as vanilla's listeners send it.
            let world = world_index(server, args)?;
            let mut packets: Vec<bytes::Bytes> = Vec::new();
            let b = &mut server.worlds[world].border;
            if args.bool("reset") == Some(true) {
                *b = crate::world::Border::default();
            }
            if let (Some(x), Some(z)) = (args.f64("center_x"), args.f64("center_z")) {
                b.center_x = x.clamp(-crate::world::MAX_BORDER_CENTER, crate::world::MAX_BORDER_CENTER);
                b.center_z = z.clamp(-crate::world::MAX_BORDER_CENTER, crate::world::MAX_BORDER_CENTER);
                packets.push(paper_protocol::packets::encode_packet(&play::SetBorderCenter { center_x: b.center_x, center_z: b.center_z }).into());
            }
            if let Some(size) = args.f64("size") {
                let ticks = args.i64("ticks").unwrap_or(0);
                let old = b.size;
                b.set_size(size, ticks);
                if ticks > 0 {
                    packets.push(paper_protocol::packets::encode_packet(&play::SetBorderLerpSize { old_size: old, new_size: b.lerp_target, lerp_time: paper_protocol::codec::VarLong(ticks) }).into());
                } else {
                    packets.push(paper_protocol::packets::encode_packet(&play::SetBorderSize { size: b.size }).into());
                }
            }
            if let Some(v) = args.f64("damage_buffer") {
                b.damage_safe_zone = v.max(0.0);
            }
            if let Some(v) = args.f64("damage_amount") {
                b.damage_per_block = v.max(0.0);
            }
            if let Some(v) = args.i32("warning_distance") {
                b.warning_blocks = v.max(0);
                packets.push(paper_protocol::packets::encode_packet(&play::SetBorderWarningDistance { warning_blocks: VarInt(b.warning_blocks) }).into());
            }
            if let Some(v) = args.i32("warning_time") {
                b.warning_time = v.max(0);
                packets.push(paper_protocol::packets::encode_packet(&play::SetBorderWarningDelay { warning_time: VarInt(b.warning_time) }).into());
            }
            if args.bool("reset") == Some(true) {
                packets = vec![paper_protocol::packets::encode_packet(&server.border_packet(world)).into()];
            }
            for payload in packets {
                server.broadcast_payload_in_world(world, payload);
            }
            Ok(world_info(&server.worlds[world]))
        }
        // --- chunks (CraftWorld's chunk methods) ---
        "world.load_chunk" => {
            // `getChunkAt` and `loadChunk`: loaded now, generated unless told not to.
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            if args.bool("generate") == Some(false) && !chunk_generated(server, world, pos) {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(server.load_chunk_now(world, pos)))
        }
        "world.is_chunk_generated" => {
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            Ok(Value::Bool(chunk_generated(server, world, pos)))
        }
        "world.loaded_chunks" => {
            let world = world_index(server, args)?;
            let w = &server.worlds[world];
            Ok(Value::List(w.chunks.iter().filter(|(_, c)| c.lit).map(|(p, _)| Value::List(vec![p.x.into(), p.z.into()])).collect()))
        }
        "world.chunk_in_use" => {
            // Players (or plugins) keep it: a ticket on it.
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            Ok(Value::Bool(server.worlds[world].has_ticket(pos)))
        }
        "world.chunk_ticket" => {
            // A plugin's ticket (`addPluginChunkTicket`), or a forced chunk.
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            let w = &mut server.worlds[world];
            if args.bool("add") == Some(true) {
                w.add_ticket(pos);
            } else {
                w.remove_ticket(pos);
            }
            Ok(Value::Null)
        }
        "world.force_load" => {
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            let w = &mut server.worlds[world];
            match args.bool("forced") {
                Some(true) if w.forced.insert(pos) => w.add_ticket(pos),
                Some(false) if w.forced.remove(&pos) => w.remove_ticket(pos),
                _ => {}
            }
            Ok(Value::List(w.forced.iter().map(|p| Value::List(vec![p.x.into(), p.z.into()])).collect()))
        }
        "world.chunk_block_entities" => {
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            let Some(chunk) = server.worlds[world].chunks.get(&pos) else { return Ok(Value::List(Vec::new())) };
            Ok(Value::List(
                chunk.block_entities.keys().map(|(y, z, x)| Value::List(vec![(pos.x * 16 + *x as i32).into(), (*y).into(), (pos.z * 16 + *z as i32).into()])).collect(),
            ))
        }
        "world.players_seeing_chunk" => {
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            Ok(Value::List(server.players.values().filter(|p| p.world == world && p.view.sent.contains(&pos)).map(|p| Value::Str(p.uuid().to_string())).collect()))
        }
        "world.is_chunk_loaded" => {
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            Ok(Value::Bool(server.worlds[world].is_ready(pos)))
        }

        // --- server ---
        // `Bukkit.createWorld`: the world of that name, opened or made.
        "server.create_world" => {
            let name = args.str("name").ok_or_else(|| anyhow!("missing name"))?;
            let index = server.create_world(name, args.str("environment").unwrap_or("NORMAL"), args.i64("seed"), args.str("type").unwrap_or("NORMAL"), args.bool("structures").unwrap_or(true))?;
            Ok(world_info(&server.worlds[index]))
        }
        "server.dispatch_command" => {
            let command = args.str("command").ok_or_else(|| anyhow!("missing command"))?.to_owned();
            let source = match args.str("sender") {
                None | Some("console") => CommandSource::Console,
                Some(uuid) => {
                    let uuid: Uuid = uuid.parse()?;
                    CommandSource::Player(server.by_uuid.get(&uuid).copied().ok_or_else(|| anyhow!("player {uuid} is not online"))?)
                }
            };
            Ok(Value::Bool(server.dispatch_builtin_command(&source, &command)))
        }
        "server.broadcast" => {
            let message = component_from(args.get("message")).ok_or_else(|| anyhow!("missing message"))?;
            server.broadcast(&play::SystemChat { content: message, overlay: false });
            Ok(Value::Null)
        }
        "server.shutdown" => {
            server.running = false;
            Ok(Value::Null)
        }
        "server.status" => Ok(bmap! {
            "tps" => Value::List(vec![server.stats.tps(100).into(), server.stats.tps(300).into(), server.stats.tps(900).into()]),
            "mspt" => server.stats.mspt(100),
            "tick" => server.tick as i64,
            "online" => server.players.len(),
        }),
        "server.whitelist" => {
            let access = server.shared.access.read();
            Ok(bmap! {
                "enabled" => server.config.properties.white_list,
                "players" => Value::List(access.whitelist.iter().map(|e| bmap! { "uuid" => e.uuid.to_string(), "name" => e.name.clone() }).collect()),
                "ops" => Value::List(access.ops.iter().map(|e| bmap! { "uuid" => e.uuid.to_string(), "name" => e.name.clone(), "level" => e.level as i64 }).collect()),
            })
        }
        other => bail!("unknown plugin API method {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_protocol::codec::WriteExt;
    use paper_protocol::types::ItemStack;

    fn component(name: &str) -> i32 {
        paper_data::registries::id("minecraft:data_component_type", name).unwrap() as i32
    }

    fn sample() -> Slot {
        let mut stack = ItemStack::new(items::id("minecraft:diamond_sword").unwrap() as i32, 1);
        let mut name = Vec::new();
        name.put_component(&Component::text("Excalibur").color("gold"));
        stack.added.push((component("minecraft:custom_name"), name));
        // A component the plugin host does not understand, kept as bytes.
        stack.added.push((component("minecraft:food"), vec![4, 0, 0, 0, 0, 1]));
        stack.added.sort_by_key(|(t, _)| *t);
        stack.removed.push(component("minecraft:attribute_modifiers"));
        Slot(Some(stack))
    }

    #[test]
    fn items_round_trip_through_the_host_form() {
        let slot = sample();
        let value = item_value(&slot);
        assert!(value.str("name").is_some_and(|n| n.contains("Excalibur")), "{value:?}");
        assert_eq!(slot_from(&value).unwrap(), slot, "unchanged stacks keep their exact bytes");
    }

    #[test]
    fn a_changed_name_replaces_the_bytes() {
        let mut value = item_value(&sample());
        value.set("name", Value::Str(Component::text("Durendal").to_json_string()));
        let stack = slot_from(&value).unwrap().0.unwrap();
        let (_, bytes) = stack.added.iter().find(|(t, _)| *t == component("minecraft:custom_name")).unwrap();
        assert_eq!(paper_protocol::codec::Reader::new(bytes).component().unwrap().to_plain(), "Durendal");
        assert!(stack.added.iter().any(|(t, _)| *t == component("minecraft:food")), "other components stay");
        assert!(stack.added.windows(2).all(|w| w[0].0 < w[1].0), "components stay in type order");
    }
}
