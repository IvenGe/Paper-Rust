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
        "key" => w.key.clone(),
        "environment" => match w.dimension.as_str() {
            "minecraft:the_nether" => "NETHER",
            "minecraft:the_end" => "THE_END",
            _ => "NORMAL",
        },
        "min_y" => w.min_y,
        "height" => w.height,
        "seed" => w.seed,
        "sea_level" => w.generator.sea_level(),
        // What its dimension type says (`World#getCoordinateScale`, `hasCeiling`, ...).
        "dimension_type" => match paper_data::synced::dimension_type(&w.dimension) {
            Some(d) => bmap! {
                "coordinate_scale" => d.coordinate_scale,
                "has_skylight" => d.has_skylight,
                "has_ceiling" => d.has_ceiling,
                "ultrawarm" => d.water_evaporates && d.fast_lava,
                "fixed_time" => d.has_fixed_time,
                "piglin_safe" => !d.piglins_zombify,
                "respawn_anchor_works" => d.respawn_anchor_works,
                "raids" => d.can_start_raid,
                "bed_works" => d.bed_works,
            },
            None => Value::Null,
        },
        // The world's folder (`level.dat`, `region`...), for `World#getWorldPath`.
        "folder" => w.storage.as_ref().map_or(Value::Null, |s| Value::Str(s.dir.display().to_string())),
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
        "border" => border_value(&w.border),
    }
}

/// A border's settings for the host (`WorldBorder`'s getters).
pub fn border_value(b: &crate::world::Border) -> Value {
    bmap! {
        "center_x" => b.center_x,
        "center_z" => b.center_z,
        "size" => b.size,
        "target" => b.lerp_target,
        "lerp_left" => b.lerp_left,
        "damage_buffer" => b.damage_safe_zone,
        "damage_amount" => b.damage_per_block,
        "warning_distance" => b.warning_blocks,
        "warning_time" => b.warning_time,
    }
}

/// Applies a plugin's changes to a border (`WorldBorder`'s setters): `size` now, or over
/// `ticks` from `from` (else from where it is). Returns the packets vanilla's border listeners
/// send for them; damage settings send none.
pub(super) fn change_border(b: &mut crate::world::Border, args: &Value) -> Vec<bytes::Bytes> {
    use paper_protocol::packets::encode_packet;
    let mut packets: Vec<bytes::Bytes> = Vec::new();
    if let (Some(x), Some(z)) = (args.f64("center_x"), args.f64("center_z")) {
        b.center_x = x.clamp(-crate::world::MAX_BORDER_CENTER, crate::world::MAX_BORDER_CENTER);
        b.center_z = z.clamp(-crate::world::MAX_BORDER_CENTER, crate::world::MAX_BORDER_CENTER);
        packets.push(encode_packet(&play::SetBorderCenter { center_x: b.center_x, center_z: b.center_z }).into());
    }
    if let Some(size) = args.f64("size") {
        let ticks = args.i64("ticks").unwrap_or(0);
        if let Some(from) = args.f64("from").filter(|_| ticks > 0) {
            b.size = from.clamp(1.0, crate::world::MAX_BORDER_SIZE);
        }
        b.set_size(size, ticks);
        if ticks > 0 {
            packets.push(encode_packet(&play::SetBorderLerpSize { old_size: b.size, new_size: b.lerp_target, lerp_time: paper_protocol::codec::VarLong(ticks) }).into());
        } else {
            packets.push(encode_packet(&play::SetBorderSize { size: b.size }).into());
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
        packets.push(encode_packet(&play::SetBorderWarningDistance { warning_blocks: VarInt(b.warning_blocks) }).into());
    }
    if let Some(v) = args.i32("warning_time") {
        b.warning_time = v.max(0);
        packets.push(encode_packet(&play::SetBorderWarningDelay { warning_time: VarInt(b.warning_time) }).into());
    }
    packets
}

/// The state of a player the host caches. The host refreshes its copy whenever this changes.
pub fn player_state(server: &Server, p: &Player) -> Value {
    let mut out = player_state_fields(server, p);
    // Where it is, as for other entities: in water, lava, rain, a bubble column, powder snow.
    if let Some(w) = server.worlds.get(p.world) {
        use crate::server::survival::{box_in_fluid, point_in_fluid, Fluid};
        use crate::world::entity::{Aabb, Vec3};
        let height = if p.gliding { 0.6 } else if p.sneaking { 1.5 } else { 1.8 };
        let area = Aabb::new(Vec3::new(p.loc.x - 0.3, p.loc.y, p.loc.z - 0.3), Vec3::new(p.loc.x + 0.3, p.loc.y + height, p.loc.z + 0.3));
        let (x, y, z) = (p.loc.x.floor() as i32, p.loc.y.floor() as i32, p.loc.z.floor() as i32);
        let at_feet = w.block_at(x, y, z).map_or("minecraft:air", |s| blocks::block_of_state(s).name);
        let eye = Vec3::new(p.loc.x, p.loc.y + p.eye_height() - 0.11111111, p.loc.z);
        // In a boat that floats, the water is the boat's: `isInWater` is false for its riders.
        let afloat = p.vehicle.and_then(|v| w.entities.get(&v)).is_some_and(|b| {
            crate::server::boats::is_boat(&b.info.name) && crate::server::boats::water_level(w, &b.bounding_box()).is_none_or(|top| top <= b.pos.y + b.info.height as f64)
        });
        out.set("in_water", !afloat && box_in_fluid(w, &area, Fluid::Water));
        out.set("under_water", point_in_fluid(w, eye, Fluid::Water));
        out.set("in_lava", box_in_fluid(w, &area, Fluid::Lava));
        // `isRainingAt`: rain falls there, and the biome has rain (not none, nor snow).
        let rains = |y: i32| {
            let at = paper_protocol::codec::BlockPos::new(x, y, z);
            w.rain_at(x, y, z) && server.has_precipitation(p.world, at) && !server.cold_at(p.world, at)
        };
        out.set("in_rain", rains(y) || rains((p.loc.y + height).floor() as i32));
        out.set("in_bubble_column", at_feet == "minecraft:bubble_column");
        out.set("in_powder_snow", at_feet == "minecraft:powder_snow");
    }
    out
}

fn player_state_fields(server: &Server, p: &Player) -> Value {
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
        "gliding" => p.gliding,
        "blocking" => server.blocking_slot(p.entity_id).is_some(),
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
        "respawn_forced" => p.respawn_forced,
        // Ticks lived and since the last swing are worked out by the host from these and the
        // current tick: the state does not change every tick.
        "joined_tick" => p.api.joined_tick as i64,
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
        "last_swing" => p.last_swing as i64,
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

/// A chunk section's blocks for the host's section cache and chunk snapshots: a palette of
/// states and, unless uniform, two little-endian bytes of palette index per block (y, z, x).
fn section_value(section: &crate::world::chunk::Section) -> Value {
    match &section.blocks {
        crate::world::chunk::BlockStorage::Uniform(state) => bmap! {
            "palette" => Value::List(vec![Value::Str(blocks::state_to_string(*state as u32))]),
            "ids" => Value::List(vec![Value::Int(*state as i64)]),
        },
        crate::world::chunk::BlockStorage::Dense(data) => {
            let mut palette: Vec<u16> = Vec::new();
            let mut index: std::collections::HashMap<u16, u16> = std::collections::HashMap::new();
            let mut indices = Vec::with_capacity(8192);
            for state in data.iter() {
                let i = *index.entry(*state).or_insert_with(|| {
                    palette.push(*state);
                    (palette.len() - 1) as u16
                });
                indices.extend_from_slice(&i.to_le_bytes());
            }
            bmap! {
                "palette" => Value::List(palette.iter().map(|s| Value::Str(blocks::state_to_string(*s as u32))).collect()),
                "ids" => Value::List(palette.iter().map(|s| Value::Int(*s as i64)).collect()),
                "indices" => Value::Bytes(indices),
            }
        }
    }
}

/// Applies `world.edit_batch` entries in order; how many there were. The whole batch is
/// checked first, and refused whole if an entry is bad.
fn apply_edit_batch(server: &mut Server, world: usize, edits: &[u8]) -> Result<usize> {
    use crate::world::block_updates::{flags, Level};
    const SET: i32 = 0;
    const EFFECTS: i32 = 1;
    let int = |e: &[u8], i: usize| i32::from_le_bytes(e[i * 4..i * 4 + 4].try_into().unwrap());
    for (n, e) in edits.chunks_exact(28).enumerate() {
        let (kind, state, old) = (int(e, 0), int(e, 4) as u32, int(e, 5) as u32);
        if kind != SET && kind != EFFECTS {
            bail!("unknown edit kind {kind} in edit {n}");
        }
        if !blocks::is_valid_state(state) || kind == EFFECTS && !blocks::is_valid_state(old) {
            bail!("invalid block state in edit {n}");
        }
    }
    // The batch may come in the middle of something else the server does (a plugin's event
    // handler queued it): then its changes go after what that left, for it to publish in
    // order (clients end on the latest state), else out now.
    let in_the_middle = !server.worlds[world].updates.effects.is_empty();
    let mut last_chunk = None;
    let mut n = 0;
    for e in edits.chunks_exact(28) {
        let (kind, x, y, z, state) = (int(e, 0), int(e, 1), int(e, 2), int(e, 3), int(e, 4) as u32);
        let (old, effects) = (int(e, 5) as u32, int(e, 6));
        let chunk = crate::world::chunk::ChunkPos::of_block(x, z);
        if last_chunk != Some(chunk) {
            ensure_chunk(server, world, x, z);
            last_chunk = Some(chunk);
        }
        let pos = paper_protocol::codec::BlockPos::new(x, y, z);
        let mut level = Level::new(&mut server.worlds[world]);
        if kind == SET {
            level.set_block(pos, state, flags::CLIENTS | flags::KNOWN_SHAPE | flags::SKIP_ON_PLACE);
        } else {
            // On the block as it is now, which an earlier entry's side effects may have
            // changed (a wire's connections), as WorldEdit's `applySideEffects` reads it.
            let mut now = level.state(pos);
            if effects & 1 != 0 && now != old {
                level.on_place(now, pos, old, false);
                // `onPlace` replaced the block (TNT lit): the rest is for the new one, which
                // was placed with its own updates (`markAndNotifyBlock` stops here).
                if level.state(pos) != now {
                    n += 1;
                    continue;
                }
                now = level.state(pos);
            }
            if effects & 2 != 0 {
                level.update_neighbors_at(pos, blocks::block_id_of_state(old));
                if paper_data::physics::has_analog_output(now) {
                    level.update_neighbour_for_output_signal(pos, blocks::block_id_of_state(now));
                }
            }
            if effects & 4 != 0 {
                let limit = crate::world::block_updates::UPDATE_LIMIT - 1;
                level.update_indirect_neighbour_shapes(old, pos, flags::CLIENTS, limit);
                level.update_neighbour_shapes(now, pos, flags::CLIENTS, limit);
                level.update_indirect_neighbour_shapes(now, pos, flags::CLIENTS, limit);
            }
        }
        n += 1;
    }
    if !in_the_middle {
        server.publish_block_updates(world);
    }
    Ok(n)
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
            "collision" => blocks::has_collision(state),
            "block_entity" => blocks::flags(state) & blocks::FLAG_BLOCK_ENTITY != 0,
            "piston" => match paper_data::physics::push_reaction(state) {
                paper_data::physics::PushReaction::Normal => "MOVE",
                paper_data::physics::PushReaction::Destroy => "BREAK",
                paper_data::physics::PushReaction::Block => "BLOCK",
                paper_data::physics::PushReaction::PushOnly => "PUSH_ONLY",
            },
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
            server.kick(id, &reason, args.str("cause").unwrap_or("PLUGIN"));
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
            let mut dies = false;
            if let Some(h) = args.f64("health") {
                let alive = !player.dead && player.health > 0.0;
                player.health = (h as f32).clamp(0.0, 1024.0);
                // No health left kills (`CraftLivingEntity.setHealth` runs `die` with generic
                // damage): the death event, drops, the death screen. Not again while dying.
                dies = alive && player.health <= 0.0;
            }
            if let Some(f) = args.i32("food") {
                player.food = f.clamp(0, 20);
            }
            if let Some(s) = args.f64("saturation") {
                player.saturation = s as f32;
            }
            if dies {
                server.kill_player(id, crate::server::survival::Damage::new("minecraft:generic"));
            }
            // Sent even when unchanged (and scaled as `setHealthScale` asks).
            if let Some(player) = server.players.get_mut(&id) {
                player.sent_health = None;
            }
            server.send_health_if_changed(id);
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
        // `World#playSound`: everyone in the world within earshot hears it (16 blocks, or 16
        // times the volume for louder sounds, as `ServerLevel.playSeededSound`).
        "world.play_sound" => {
            let world = world_index(server, args)?;
            let sound = args.str("sound").ok_or_else(|| anyhow!("missing sound"))?;
            let sound = match paper_data::registries::id("minecraft:sound_event", sound) {
                Some(i) => play::SoundRef::Id(i as i32),
                None => play::SoundRef::Named { name: sound.to_owned(), fixed_range: None },
            };
            let (x, y, z) = (args.f64("x").unwrap_or(0.0), args.f64("y").unwrap_or(0.0), args.f64("z").unwrap_or(0.0));
            let volume = args.f64("volume").unwrap_or(1.0) as f32;
            let packet = play::Sound {
                sound,
                category: args.i32("category").unwrap_or(0),
                x,
                y,
                z,
                volume,
                pitch: args.f64("pitch").unwrap_or(1.0) as f32,
                seed: args.i64("seed").unwrap_or_else(rand::random),
            };
            let range = if volume > 1.0 { 16.0 * volume as f64 } else { 16.0 };
            for player in server.players.values().filter(|p| p.world == world) {
                let (dx, dy, dz) = (player.loc.x - x, player.loc.y - y, player.loc.z - z);
                if dx * dx + dy * dy + dz * dz < range * range {
                    player.conn.send(&packet);
                }
            }
            Ok(Value::Null)
        }
        // Particles (`World#spawnParticle`, `Player#spawnParticle`): a particle by name with its
        // options (`data`), shown to the world's players within 32 blocks (512 with `force`),
        // or only to the players in `to` (uuids). Answers how many were shown it.
        "world.particles" => {
            let world = world_index(server, args)?;
            let particle = particle_bytes(args.str("particle").unwrap_or(""), args.get("data"))?;
            let force = args.bool("force").unwrap_or(false);
            let f = |key: &str| args.f64(key).unwrap_or(0.0);
            let packet = play::LevelParticles {
                particle,
                override_limiter: force,
                always_show: force,
                x: f("x"),
                y: f("y"),
                z: f("z"),
                offset: (f("offset_x") as f32, f("offset_y") as f32, f("offset_z") as f32),
                max_speed: (f("speed_x") as f32, f("speed_y") as f32, f("speed_z") as f32),
                count: args.i32("count").unwrap_or(1),
                randomization: args.i32("randomization").unwrap_or(0),
            };
            let to: Option<Vec<String>> = args.list("to").map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect());
            let range = if force { 512.0 } else { 32.0 };
            let mut shown = 0;
            for player in server.players.values().filter(|p| p.world == world) {
                if to.as_ref().is_some_and(|to| !to.contains(&player.uuid().to_string())) {
                    continue;
                }
                let (dx, dy, dz) = (player.loc.x - packet.x, player.loc.y - packet.y, player.loc.z - packet.z);
                if dx * dx + dy * dy + dz * dz < range * range {
                    player.conn.send(&packet);
                    shown += 1;
                }
            }
            Ok(Value::Int(shown))
        }
        // `World#createExplosion`: blocks break (if `break_blocks`) and fire starts (if `fire`)
        // as for any explosion; `source` is the entity responsible.
        "world.explode" => {
            let world = world_index(server, args)?;
            let center = crate::world::entity::Vec3::new(args.f64("x").unwrap_or(0.0), args.f64("y").unwrap_or(0.0), args.f64("z").unwrap_or(0.0));
            let source = args.i32("source");
            server.explode_with(world, center, args.f64("power").unwrap_or(4.0) as f32, args.bool("break_blocks").unwrap_or(true), args.bool("fire").unwrap_or(false), source);
            Ok(Value::Bool(true))
        }
        // `World#strikeLightning`: a bolt that burns and hurts; with `effect`, only its flash
        // and thunder. Answers the bolt's entity.
        "world.strike_lightning" => {
            let world = world_index(server, args)?;
            let (x, y, z) = (args.f64("x").unwrap_or(0.0), args.f64("y").unwrap_or(0.0), args.f64("z").unwrap_or(0.0));
            let pos = paper_protocol::codec::BlockPos::new(x.floor() as i32, y.floor() as i32, z.floor() as i32);
            let bolt = if args.bool("effect").unwrap_or(false) {
                server.lightning_effect(world, crate::world::entity::Vec3::new(x, y, z), "CUSTOM")
            } else {
                server.strike_lightning(world, pos, "CUSTOM")
            };
            Ok(bolt.and_then(|id| server.worlds[world].entities.get(&id)).map_or(Value::Null, |e| crate::plugins::entity::entity_value(server, world, e)))
        }
        // `World#playEffect`: a level event (`LevelEvent`) at a block for the players within
        // `radius` blocks; its data a number, or a block state (`state`) for block effects.
        "world.level_event" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            let pos = paper_protocol::codec::BlockPos::new(x, y, z);
            let data = match args.str("state") {
                Some(state) => blocks::parse_state(state).ok_or_else(|| anyhow!("invalid block state {state}"))? as i32,
                None => args.i32("data").unwrap_or(0),
            };
            let packet = play::LevelEvent { event: args.i32("event").unwrap_or(0), pos, data, global: false };
            let radius = args.f64("radius").unwrap_or(64.0);
            let (cx, cy, cz) = (args.f64("at_x").unwrap_or(x as f64), args.f64("at_y").unwrap_or(y as f64), args.f64("at_z").unwrap_or(z as f64));
            for player in server.players.values().filter(|p| p.world == world) {
                let (dx, dy, dz) = (player.loc.x - cx, player.loc.y - cy, player.loc.z - cz);
                if dx * dx + dy * dy + dz * dz <= radius * radius {
                    player.conn.send(&packet);
                }
            }
            Ok(Value::Null)
        }
        // How many entities, players and block entities a world has (`World#getEntityCount`...).
        "world.counts" => {
            let world = world_index(server, args)?;
            let w = &server.worlds[world];
            let players = server.players.values().filter(|p| p.world == world).count();
            let block_entities: usize = w.chunks.values().map(|c| c.block_entities.len()).sum();
            Ok(bmap! { "entities" => (w.entities.len() + players) as i64, "players" => players as i64, "block_entities" => block_entities as i64 })
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
        // `HumanEntity.openWorkbench`, `openAnvil`, `openEnchanting`: the menu of a crafting
        // table, anvil or enchanting table at a block (which must be one, unless forced), or
        // where the player stands.
        "player.open_station" => {
            use crate::server::containers::MenuKind;
            let id = player_id(server, args)?;
            let (world, loc) = (server.players[&id].world, server.players[&id].loc);
            let pos = match (args.i32("x"), args.i32("y"), args.i32("z")) {
                (Some(x), Some(y), Some(z)) => paper_protocol::codec::BlockPos::new(x, y, z),
                _ => paper_protocol::codec::BlockPos::new(loc.x.floor() as i32, loc.y.floor() as i32, loc.z.floor() as i32),
            };
            let kind = args.str("kind").unwrap_or("");
            let (block, menu, title) = match kind {
                "crafting" => ("crafting_table", "minecraft:crafting", "container.crafting"),
                "anvil" => ("anvil", "minecraft:anvil", "container.repair"),
                "enchanting" => ("enchanting_table", "minecraft:enchantment", "container.enchant"),
                "stonecutter" => ("stonecutter", "minecraft:stonecutter", "container.stonecutter"),
                "smithing" => ("smithing_table", "minecraft:smithing", "container.upgrade"),
                "grindstone" => ("grindstone", "minecraft:grindstone", "container.grindstone_title"),
                "loom" => ("loom", "minecraft:loom", "container.loom"),
                other => bail!("no {other} menu"),
            };
            let there = server.worlds[world].block_at(pos.x, pos.y, pos.z).map_or("", |s| blocks::block_of_state(s).name);
            if args.bool("force") != Some(true) && !there.ends_with(block) {
                return Ok(bmap! { "opened" => false });
            }
            let menu_kind = match kind {
                "crafting" => MenuKind::CraftingTable,
                "anvil" => MenuKind::Anvil { world, pos },
                "stonecutter" => MenuKind::Stonecutter { world, pos },
                "smithing" => MenuKind::Smithing { world, pos },
                "grindstone" => MenuKind::Grindstone { world, pos },
                "loom" => MenuKind::Loom { world, pos },
                _ => MenuKind::Enchanting { world, pos },
            };
            let opened = server.open_menu(id, menu_kind, menu, paper_protocol::text::Component::translatable(title, vec![]));
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
        // A whole section of blocks at once, for the host's block cache: their states as a
        // palette and, unless they are all the same, an index per block (two bytes,
        // little-endian) in `y << 8 | z << 4 | x` order. Loads the chunk as a block read does.
        "world.section" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            let w = &server.worlds[world];
            let rel = y - w.min_y;
            let section = (0..w.height).contains(&rel).then(|| w.chunks.get(&crate::world::chunk::ChunkPos::of_block(x, z))).flatten().and_then(|c| c.sections.get((rel >> 4) as usize));
            let Some(section) = section else {
                // Outside the world: what reading a block there answers.
                let value = state_value(server, world, x, y, z);
                let state = value.str("state").unwrap_or("minecraft:void_air").to_owned();
                let id = blocks::parse_state(&state).unwrap_or(0);
                return Ok(bmap! { "palette" => Value::List(vec![Value::Str(state)]), "ids" => Value::List(vec![Value::Int(id as i64)]) });
            };
            let mut value = section_value(section);
            // Where the section has block entities (as y << 8 | z << 4 | x), so a reader asks
            // for those only.
            let base = w.min_y + (rel & !15);
            let chunk = w.chunks.get(&crate::world::chunk::ChunkPos::of_block(x, z));
            let entities: Vec<Value> = chunk
                .into_iter()
                .flat_map(|c| c.block_entities.keys())
                .filter(|(by, _, _)| (base..base + 16).contains(by))
                .map(|(by, bz, bx)| Value::Int((((by - base) << 8) | ((*bz as i32) << 4) | *bx as i32) as i64))
                .collect();
            if let Value::Map(m) = &mut value {
                m.insert("block_entities".into(), Value::List(entities));
            }
            Ok(value)
        }
        // Block changes the host made without waiting (WorldEdit's edits, Bukkit's changes
        // without physics), packed as seven little-endian ints each: kind, x, y, z, state,
        // old state, flags. Kind 0 sets a block without physics; kind 1 runs the side effects
        // WorldEdit applies after an edit (`WorldNativeAccess.applySideEffects`): the block's
        // `onPlace` (flag 1), its neighbours' updates (2) and their shapes (4), if the block is
        // still what the edit left. Plugins hear no events meanwhile unless `events` is set.
        "world.edit_batch" => {
            let world = world_index(server, args)?;
            let Value::Bytes(edits) = args.get("edits") else { bail!("missing edits") };
            if edits.len() % 28 != 0 {
                bail!("edits are 28 bytes each, not {} in all", edits.len());
            }
            let events = args.bool("events") == Some(true);
            if !events {
                server.events_suppressed += 1;
            }
            let applied = apply_edit_batch(server, world, edits);
            if !events {
                server.events_suppressed -= 1;
            }
            applied.map(|n| Value::Int(n as i64))
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
            // The chunk's cached packet would show the old data to players who come later.
            c.revision += 1;
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
        // A world's persistent data container (Paper's `paper:persistent_data_container`
        // saved data), as network NBT.
        "world.pdc" => {
            let world = world_index(server, args)?;
            Ok(match &server.worlds[world].plugin_data {
                Some(data) => {
                    let mut out = Vec::new();
                    paper_nbt::write_network_compound(&mut out, data);
                    Value::Bytes(out)
                }
                None => Value::Null,
            })
        }
        "world.set_pdc" => {
            let world = world_index(server, args)?;
            let data = match args.get("data") {
                Value::Bytes(b) if !b.is_empty() => match paper_nbt::read_network(&mut b.as_slice(), 2 * 1024 * 1024) {
                    Ok(Some(paper_nbt::Tag::Compound(c))) => c,
                    _ => bail!("invalid persistent data"),
                },
                _ => paper_nbt::Compound::new(),
            };
            server.worlds[world].plugin_data = Some(data);
            Ok(Value::Bool(true))
        }
        // Game rules by name (`keep_inventory` or `minecraft:keep_inventory`); null for a
        // rule there is not.
        "world.game_rule" => {
            let world = world_index(server, args)?;
            let name = args.str("rule").unwrap_or("");
            if paper_data::game_rules::get(name).is_none() {
                return Ok(Value::Null);
            }
            Ok(match server.worlds[world].rules.get(name) {
                paper_data::game_rules::Value::Bool(b) => Value::Bool(b),
                paper_data::game_rules::Value::Int(i) => Value::Int(i as i64),
            })
        }
        "world.set_game_rule" => {
            use paper_data::game_rules::Value as Rule;
            let world = world_index(server, args)?;
            let name = args.str("rule").unwrap_or("");
            let Some(rule) = paper_data::game_rules::get(name) else { return Ok(Value::Bool(false)) };
            let value = match (rule.default, args.get("value")) {
                (Rule::Bool(_), Value::Bool(b)) => Rule::Bool(*b),
                (Rule::Int(_), Value::Int(i)) if (rule.min as i64..=rule.max as i64).contains(i) => Rule::Int(*i as i32),
                (_, Value::Str(s)) => match rule.parse(s) {
                    Some(v) => v,
                    None => return Ok(Value::Bool(false)),
                },
                _ => return Ok(Value::Bool(false)),
            };
            server.set_game_rule(world, name, value);
            Ok(Value::Bool(true))
        }
        "world.game_rules" => Ok(Value::List(paper_data::game_rules::all().iter().map(|r| Value::Str(r.name.clone())).collect())),
        // `Block#breakNaturally`: what the block yields drops (with the given tool: nothing if
        // it is the wrong one; with none: whatever tool the block needs), with its experience if
        // asked, the break effect if asked, and the block is removed (leaving its water). True
        // if the block was removed and dropped something.
        "world.break_naturally" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            let pos = paper_protocol::codec::BlockPos::new(x, y, z);
            let Some(state) = server.worlds[world].block_at(x, y, z) else { return Ok(Value::Bool(false)) };
            if blocks::is_air(state) {
                return Ok(Value::Bool(false));
            }
            let tool = match args.get("tool") {
                Value::Null => None,
                v => Some(slot_from(v)?),
            };
            let drops = match &tool {
                None => true,
                Some(t) => !paper_data::physics::requires_correct_tool(state) || server.tool_is_correct(t, state),
            };
            if drops {
                server.roll_block_loot(world, pos, state, tool.as_ref(), None);
                if args.bool("experience") == Some(true) {
                    server.drop_block_experience(world, pos, state, tool.as_ref());
                }
            }
            if args.bool("effect") == Some(true) {
                let fire = matches!(blocks::block_of_state(state).name, "minecraft:fire" | "minecraft:soul_fire");
                let event = play::LevelEvent { event: if fire { 1009 } else { 2001 }, pos, data: if fire { 0 } else { state as i32 }, global: false };
                server.send_near(world, pos, 64.0, None, &event);
            }
            let removed = server.remove_block(world, pos);
            Ok(Value::Bool(removed && drops))
        }
        // `Block#getDrops`: what breaking the block would drop (with the tool, if one is
        // given: nothing if it is the wrong one).
        "world.block_drops" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            let Some(state) = server.worlds[world].block_at(x, y, z) else { return Ok(Value::List(Vec::new())) };
            let tool = match args.get("tool") {
                Value::Null => None,
                v => Some(slot_from(v)?),
            };
            if blocks::is_air(state) || tool.as_ref().is_some_and(|t| paper_data::physics::requires_correct_tool(state) && !server.tool_is_correct(t, state)) {
                return Ok(Value::List(Vec::new()));
            }
            Ok(Value::List(server.block_loot(state, tool.as_ref(), None).iter().map(item_value).collect()))
        }
        // A block entity for a block state snapshot (`CraftBlockEntityState`): its saved NBT
        // (network form) and its custom name as JSON. Null where there is none.
        "world.tile_data" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            let chunk = crate::world::chunk::ChunkPos::of_block(x, z);
            let Some(nbt) = server.worlds[world].chunks.get(&chunk).and_then(|c| c.block_entities.get(&(y, (z & 15) as u8, (x & 15) as u8))) else { return Ok(Value::Null) };
            let mut out = Vec::new();
            paper_nbt::write_network_compound(&mut out, nbt);
            let name = nbt.get("CustomName").map(|t| Component::from_nbt(t).to_json_string());
            Ok(bmap! { "nbt" => Value::Bytes(out), "name" => name.map_or(Value::Null, Value::Str) })
        }
        // Writes a snapshot's changes to the block entity: tags to replace (`set`, network
        // NBT), tags to remove (`remove`), and the custom name (`name` as JSON, or null to
        // clear it) when `name_changed`. Clients are shown blocks that look different for it.
        "world.set_tile_data" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            ensure_chunk(server, world, x, z);
            let set = match args.get("set") {
                Value::Bytes(b) if !b.is_empty() => match paper_nbt::read_network(&mut b.as_slice(), 2 * 1024 * 1024) {
                    Ok(Some(paper_nbt::Tag::Compound(c))) => Some(c),
                    _ => bail!("invalid block entity data"),
                },
                _ => None,
            };
            let name = if args.bool("name_changed") == Some(true) {
                Some(match args.str("name") {
                    Some(json) => Some(Component::from_json_str(json).map_err(|e| anyhow!("invalid custom name: {e}"))?.to_nbt()),
                    None => None,
                })
            } else {
                None
            };
            let pos = paper_protocol::codec::BlockPos::new(x, y, z);
            let chunk = crate::world::chunk::ChunkPos::of_block(x, z);
            let Some(c) = server.worlds[world].chunks.get_mut(&chunk) else { return Ok(Value::Bool(false)) };
            let Some(be) = c.block_entities.get_mut(&(y, (z & 15) as u8, (x & 15) as u8)) else { return Ok(Value::Bool(false)) };
            for (key, tag) in set.iter().flat_map(|c| c.iter()) {
                be.insert(key.clone(), tag.clone());
            }
            for key in args.list("remove").unwrap_or(&[]).iter().filter_map(Value::as_str) {
                be.remove(key);
            }
            match name {
                Some(Some(tag)) => {
                    be.insert("CustomName", tag);
                }
                Some(None) => {
                    be.remove("CustomName");
                }
                None => {}
            }
            let mut shown = be.clone();
            c.dirty = true;
            let id = shown.get("id").and_then(|t| t.as_str()).unwrap_or("").to_owned();
            if matches!(id.as_str(), "minecraft:banner" | "minecraft:mob_spawner" | "minecraft:skull" | "minecraft:sign" | "minecraft:hanging_sign") {
                // What clients see changed: the chunk's cached packet would show the old data
                // to players who come later. (Other data, such as a plugin's or a chest's
                // items, is not in it.)
                c.revision += 1;
                if let Some(ty) = paper_data::registries::id("minecraft:block_entity_type", &id) {
                    shown.remove("PublicBukkitValues");
                    let packet = play::BlockEntityData { pos, block_entity_type: VarInt(ty as i32), data: paper_protocol::codec::Nbt(Some(paper_nbt::Tag::Compound(shown))) };
                    server.broadcast_to_chunk_viewers(world, chunk, &packet);
                }
            }
            Ok(Value::Bool(true))
        }
        // `World#save`: the world's chunks, level.dat, game rules and plugin data, on disk
        // once this returns if `flush`.
        "world.save" => {
            let world = world_index(server, args)?;
            Ok(Value::Bool(server.save_world(world, args.bool("flush").unwrap_or(true))))
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
            // (Plugins' `WeatherChangeEvent`s may keep it as it is.)
            if let Some(b) = args.bool("raining") {
                let (_, raining) = server.weather_may_change(world, false, b, "PLUGIN");
                server.worlds[world].weather.raining = raining;
            }
            if let Some(b) = args.bool("thundering") {
                let (_, thundering) = server.weather_may_change(world, true, b, "PLUGIN");
                server.worlds[world].weather.thundering = thundering;
            }
            let w = &mut server.worlds[world].weather;
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
            // `WorldBorder`'s setters (the host has fired Paper's border events): each change
            // goes to the world's players who see its border, as vanilla's listeners send it.
            let world = world_index(server, args)?;
            let b = &mut server.worlds[world].border;
            let packets = if args.bool("reset") == Some(true) {
                *b = crate::world::Border::default();
                vec![paper_protocol::packets::encode_packet(&crate::server::borders::border_init(b)).into()]
            } else {
                change_border(b, args)
            };
            server.broadcast_border(world, &packets);
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
        // `Chunk#getPersistentDataContainer`: the chunk's plugin values (loading it if need
        // be), as network NBT; set again whole after each change.
        "world.chunk_pdc" => {
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            server.load_chunk_now(world, pos);
            Ok(match server.worlds[world].chunks.get(&pos).and_then(|c| c.plugin_data.as_ref()) {
                Some(data) => {
                    let mut out = Vec::new();
                    paper_nbt::write_network_compound(&mut out, data);
                    Value::Bytes(out)
                }
                None => Value::Null,
            })
        }
        "world.set_chunk_pdc" => {
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            let data = match args.get("data") {
                Value::Bytes(b) if !b.is_empty() => match paper_nbt::read_network(&mut b.as_slice(), 2 * 1024 * 1024) {
                    Ok(Some(paper_nbt::Tag::Compound(c))) => c,
                    _ => bail!("invalid persistent data"),
                },
                _ => paper_nbt::Compound::new(),
            };
            server.load_chunk_now(world, pos);
            let Some(chunk) = server.worlds[world].chunks.get_mut(&pos) else { bail!("chunk {},{} could not be loaded", pos.x, pos.z) };
            chunk.plugin_data = (!data.is_empty()).then_some(data);
            chunk.dirty = true;
            Ok(Value::Null)
        }
        // `Chunk#getChunkSnapshot`: everything a snapshot answers, at once: each section's blocks
        // (as `world.section`), sky and block light (a level, or 2048 nibble bytes), and its
        // 4x4x4 biomes; the highest block of each column (motion blocking); the world's time.
        "world.chunk_snapshot" => {
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            server.load_chunk_now(world, pos);
            let w = &server.worlds[world];
            let Some(chunk) = w.chunks.get(&pos) else { return Ok(Value::Null) };
            let light = |l: Option<&std::sync::Arc<crate::world::chunk::LightSection>>| match l.map(|a| &**a) {
                Some(crate::world::chunk::LightSection::Uniform(v)) => Value::Int(*v as i64),
                Some(crate::world::chunk::LightSection::Dense(d)) => Value::Bytes(d.to_vec()),
                None => Value::Int(0),
            };
            let names = paper_data::synced::entries("minecraft:worldgen/biome").unwrap_or(&[]);
            let sections: Vec<Value> = chunk
                .sections
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let mut v = section_value(s);
                    v.set("sky", light(chunk.sky_light.get(i + 1)));
                    v.set("block", light(chunk.block_light.get(i + 1)));
                    let biomes = (0..64).map(|j| Value::Str(names.get(s.biome(j & 3, j >> 4, (j >> 2) & 3) as usize).copied().unwrap_or("minecraft:plains").to_owned())).collect();
                    v.set("biomes", Value::List(biomes));
                    v
                })
                .collect();
            let heights = (0..256).map(|i| Value::Int(chunk.top_y(i & 15, i >> 4) as i64 - 1)).collect();
            Ok(bmap! { "min_y" => chunk.min_y, "sections" => Value::List(sections), "heights" => Value::List(heights), "time" => w.age })
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
            let index = server.create_world(name, args.str("environment").unwrap_or("NORMAL"), args.i64("seed"), args.str("type").unwrap_or("NORMAL"), args.bool("structures").unwrap_or(true), args.str("key"))?;
            Ok(world_info(&server.worlds[index]))
        }
        // `Bukkit.unloadWorld`, once the host has fired `WorldUnloadEvent`.
        "server.unload_world" => {
            let world = world_index(server, args)?;
            Ok(Value::Bool(server.unload_world(world, args.bool("save").unwrap_or(true))))
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
        // The server's difficulty, which all its worlds share (`World#getDifficulty`,
        // `World#setDifficulty`).
        "server.difficulty" => Ok(Value::Str(server.difficulty.name().to_owned())),
        "server.set_difficulty" => {
            let name = args.str("difficulty").unwrap_or("");
            let difficulty = crate::config::Difficulty::parse(name).ok_or_else(|| anyhow!("unknown difficulty {name}"))?;
            server.difficulty = difficulty;
            server.broadcast(&play::ChangeDifficulty { difficulty: difficulty.id(), locked: false });
            Ok(Value::Null)
        }
        // Plugins' recipes (`Server#addRecipe`): shaped, shapeless, transmute and cooking
        // recipes, whose ingredients name item types (`items`) or exact items (`exact`).
        // False if there is a recipe of that key already.
        "server.add_recipe" => {
            let recipe = recipe_from(args)?;
            let recipes = std::sync::Arc::make_mut(server.recipes.get_or_insert_with(Default::default));
            Ok(Value::Bool(match recipe {
                AnyRecipe::Crafting(r) => recipes.add_crafting(r),
                AnyRecipe::Cooking(r) => recipes.add_cooking(r),
            }))
        }
        "server.remove_recipe" => {
            let key = args.str("key").unwrap_or("");
            Ok(Value::Bool(server.recipes.as_mut().is_some_and(|r| std::sync::Arc::make_mut(r).remove(key))))
        }
        "server.has_recipe" => Ok(Value::Bool(server.recipes.as_ref().is_some_and(|r| r.has(args.str("key").unwrap_or(""))))),
        "server.clear_recipes" => {
            if let Some(r) = server.recipes.as_mut() {
                std::sync::Arc::make_mut(r).clear();
            }
            Ok(Value::Null)
        }
        "server.reset_recipes" => {
            if let Some(r) = server.recipes.as_mut() {
                std::sync::Arc::make_mut(r).reset();
            }
            Ok(Value::Null)
        }
        // Every recipe there is, as `server.add_recipe` takes them.
        "server.recipes" => Ok(Value::List(match &server.recipes {
            Some(r) => r.crafting_recipes().map(crafting_recipe_value).chain(r.cooking_recipes().map(cooking_recipe_value)).collect(),
            None => Vec::new(),
        })),
        // What a crafting grid (`grid`: 4 or 9 items, row by row) makes: the recipe's key and
        // the result, or null.
        "server.craft" => {
            let grid: Vec<Slot> = args.list("grid").unwrap_or(&[]).iter().map(slot_from).collect::<Result<_>>()?;
            let size = if grid.len() >= 9 { 3 } else { 2 };
            if grid.len() != size * size {
                bail!("a crafting grid of {} items", grid.len());
            }
            Ok(match server.recipes.as_ref().and_then(|r| r.craft(&grid, size)) {
                Some((key, result)) => bmap! { "key" => key.to_owned(), "result" => item_value(&Slot(Some(result))) },
                None => Value::Null,
            })
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

// --- recipes -----------------------------------------------------------------------------------

/// A recipe from a plugin, crafted or cooked.
enum AnyRecipe {
    Crafting(crate::server::recipes::CraftingRecipe),
    Cooking(crate::server::recipes::CookingRecipe),
}

fn ingredient_from(v: &Value) -> Result<crate::server::recipes::Ingredient> {
    let ids: Vec<i32> = v.list("items").unwrap_or(&[]).iter().filter_map(Value::as_str).filter_map(items::id).map(|i| i as i32).collect();
    let mut exact = Vec::new();
    for item in v.list("exact").unwrap_or(&[]) {
        if let Some(stack) = slot_from(item)?.0 {
            exact.push(stack);
        }
    }
    if ids.is_empty() && exact.is_empty() {
        bail!("an ingredient that takes no item");
    }
    Ok(crate::server::recipes::Ingredient(ids, exact))
}

fn ingredient_value(ingredient: &crate::server::recipes::Ingredient) -> Value {
    let mut out = bmap! { "items" => Value::List(ingredient.0.iter().filter_map(|id| items::get(*id as u32)).map(|i| Value::Str(i.name.to_owned())).collect()) };
    if !ingredient.1.is_empty() {
        out.set("exact", Value::List(ingredient.1.iter().map(|s| item_value(&Slot(Some(s.clone())))).collect()));
    }
    out
}

fn recipe_from(v: &Value) -> Result<AnyRecipe> {
    use crate::server::recipes::{CookingRecipe, CraftingRecipe};
    let id = v.str("key").ok_or_else(|| anyhow!("a recipe without a key"))?.to_owned();
    let result = slot_from(v.get("result"))?.0.ok_or_else(|| anyhow!("a recipe without a result"))?;
    Ok(match v.str("type").unwrap_or("") {
        "shaped" => {
            let rows: Vec<Vec<char>> = v.list("rows").unwrap_or(&[]).iter().filter_map(Value::as_str).map(|r| r.chars().collect()).collect();
            let mut keys = std::collections::HashMap::new();
            if let Value::Map(map) = v.get("keys") {
                for (key, ingredient) in map {
                    if let Some(c) = key.chars().next() {
                        keys.insert(c, ingredient_from(ingredient)?);
                    }
                }
            }
            let (height, width) = (rows.len(), rows.iter().map(Vec::len).max().unwrap_or(0));
            if !(1..=3).contains(&height) || !(1..=3).contains(&width) {
                bail!("a shape of {width} by {height}");
            }
            let cells = rows.iter().flat_map(|row| (0..width).map(|x| row.get(x).and_then(|c| keys.get(c).cloned()))).collect();
            AnyRecipe::Crafting(CraftingRecipe::Shaped { id, width, height, cells, result })
        }
        "shapeless" => {
            let ingredients = v.list("ingredients").unwrap_or(&[]).iter().map(ingredient_from).collect::<Result<Vec<_>>>()?;
            if !(1..=9).contains(&ingredients.len()) {
                bail!("a shapeless recipe of {} ingredients", ingredients.len());
            }
            AnyRecipe::Crafting(CraftingRecipe::Shapeless { id, ingredients, result })
        }
        "transmute" => AnyRecipe::Crafting(CraftingRecipe::Transmute { id, input: ingredient_from(v.get("input"))?, material: ingredient_from(v.get("material"))?, result }),
        kind @ ("smelting" | "blasting" | "smoking" | "campfire_cooking") => AnyRecipe::Cooking(CookingRecipe {
            id,
            kind: format!("minecraft:{kind}"),
            ingredient: ingredient_from(v.get("input"))?,
            result,
            experience: v.get("experience").as_f64().unwrap_or(0.0) as f32,
            cooking_time: v.i32("cooking_time").unwrap_or(200),
        }),
        other => bail!("recipes of type {other:?}"),
    })
}

/// A crafting recipe as `server.add_recipe` takes it (a letter per ingredient of a shape).
fn crafting_recipe_value(recipe: &crate::server::recipes::CraftingRecipe) -> Value {
    use crate::server::recipes::CraftingRecipe;
    match recipe {
        CraftingRecipe::Shaped { id, width, height, cells, result } => {
            let mut distinct: Vec<&crate::server::recipes::Ingredient> = Vec::new();
            let mut rows = Vec::new();
            for y in 0..*height {
                let mut row = String::new();
                for cell in &cells[y * width..(y + 1) * width] {
                    match cell {
                        None => row.push(' '),
                        Some(ingredient) => {
                            let i = distinct.iter().position(|d| *d == ingredient).unwrap_or_else(|| {
                                distinct.push(ingredient);
                                distinct.len() - 1
                            });
                            row.push((b'A' + i as u8) as char);
                        }
                    }
                }
                rows.push(Value::Str(row));
            }
            let keys = Value::Map(distinct.iter().enumerate().map(|(i, ingredient)| (((b'A' + i as u8) as char).to_string(), ingredient_value(ingredient))).collect());
            bmap! { "key" => id.clone(), "type" => "shaped", "rows" => Value::List(rows), "keys" => keys, "result" => item_value(&Slot(Some(result.clone()))) }
        }
        CraftingRecipe::Shapeless { id, ingredients, result } => {
            bmap! { "key" => id.clone(), "type" => "shapeless", "ingredients" => Value::List(ingredients.iter().map(ingredient_value).collect()), "result" => item_value(&Slot(Some(result.clone()))) }
        }
        CraftingRecipe::Transmute { id, input, material, result } => {
            bmap! { "key" => id.clone(), "type" => "transmute", "input" => ingredient_value(input), "material" => ingredient_value(material), "result" => item_value(&Slot(Some(result.clone()))) }
        }
    }
}

fn cooking_recipe_value(recipe: &crate::server::recipes::CookingRecipe) -> Value {
    bmap! {
        "key" => recipe.id.clone(),
        "type" => recipe.kind.strip_prefix("minecraft:").unwrap_or(&recipe.kind).to_owned(),
        "input" => ingredient_value(&recipe.ingredient),
        "result" => item_value(&Slot(Some(recipe.result.clone()))),
        "experience" => recipe.experience as f64,
        "cooking_time" => recipe.cooking_time,
    }
}

/// A particle and its options as `level_particles` carries them (`ParticleTypes.STREAM_CODEC`):
/// block particles a block state, dust a colour and size, ...; `data` holds what the API gave.
fn particle_bytes(name: &str, data: &Value) -> Result<Vec<u8>> {
    use paper_protocol::codec::WriteExt;
    let name = if name.contains(':') { name.to_owned() } else { format!("minecraft:{name}") };
    let id = paper_data::registries::id("minecraft:particle_type", &name).ok_or_else(|| anyhow!("unknown particle {name}"))?;
    let mut w = Vec::new();
    w.put_varint(id as i32);
    let f = |key: &str, default: f64| data.f64(key).unwrap_or(default);
    let color = |key: &str| data.i32(key).unwrap_or(-1);
    match name.trim_start_matches("minecraft:") {
        "block" | "block_marker" | "falling_dust" | "dust_pillar" | "block_crumble" => {
            let state = data.str("state").and_then(blocks::parse_state).unwrap_or_else(|| blocks::default_state("minecraft:stone").unwrap_or(blocks::AIR));
            w.put_varint(state as i32);
        }
        "dust" => {
            w.put_i32(color("color"));
            w.put_f32(f("scale", 1.0) as f32);
        }
        "dust_color_transition" => {
            w.put_i32(color("color"));
            w.put_i32(color("to_color"));
            w.put_f32(f("scale", 1.0) as f32);
        }
        "entity_effect" | "tinted_leaves" | "flash" => w.put_i32(color("color")),
        "trail" => {
            w.put_f64(f("target_x", 0.0));
            w.put_f64(f("target_y", 0.0));
            w.put_f64(f("target_z", 0.0));
            w.put_i32(color("color"));
            w.put_varint(data.i32("duration").unwrap_or(20));
        }
        "vibration" => {
            match data.i32("entity") {
                Some(entity) => {
                    w.put_varint(1);
                    w.put_varint(entity);
                    w.put_f32(f("y_offset", 0.0) as f32);
                }
                None => {
                    w.put_varint(0);
                    w.put_position(paper_protocol::codec::BlockPos::new(f("target_x", 0.0).floor() as i32, f("target_y", 0.0).floor() as i32, f("target_z", 0.0).floor() as i32));
                }
            }
            w.put_varint(data.i32("ticks").unwrap_or(20));
        }
        "sculk_charge" => w.put_f32(f("roll", 0.0) as f32),
        "shriek" => w.put_varint(data.i32("delay").unwrap_or(0)),
        "dragon_breath" => w.put_f32(f("power", 1.0) as f32),
        "effect" | "instant_effect" => {
            w.put_i32(color("color"));
            w.put_f32(f("power", 1.0) as f32);
        }
        "geyser" | "geyser_plume" => w.put_i32(data.i32("water_blocks").unwrap_or(1)),
        "geyser_base" | "geyser_poof" => {
            w.put_i32(data.i32("water_blocks").unwrap_or(1));
            w.put_f32(f("impulse", 1.0) as f32);
        }
        "item" => {
            // An item template: the item, its count and its components.
            let stack = slot_from(data.get("item"))?.0.unwrap_or_else(|| paper_protocol::types::ItemStack::new(items::id("minecraft:stone").unwrap_or(1) as i32, 1));
            stack.encode_template(&mut w);
        }
        _ => {}
    }
    Ok(w)
}

#[cfg(test)]
mod particle_tests {
    use super::*;

    /// Particles carry their options after their type: dust a colour and a size, block particles
    /// a block state.
    #[test]
    fn particle_options() {
        let id = |name: &str| paper_data::registries::id("minecraft:particle_type", name).unwrap() as u8;
        let dust = particle_bytes("dust", &bmap! { "color" => 0xFF0000_i64, "scale" => 2.0 }).unwrap();
        assert_eq!(dust, [&[id("minecraft:dust")][..], &0xFF0000_i32.to_be_bytes(), &2.0_f32.to_be_bytes()].concat());
        let stone = blocks::default_state("minecraft:stone").unwrap();
        let block = particle_bytes("minecraft:block", &bmap! { "state" => "minecraft:stone" }).unwrap();
        let mut expected = vec![id("minecraft:block")];
        paper_protocol::codec::WriteExt::put_varint(&mut expected, stone as i32);
        assert_eq!(block, expected);
        assert_eq!(particle_bytes("heart", &Value::Null).unwrap(), [id("minecraft:heart")]);
        assert!(particle_bytes("no_such_particle", &Value::Null).is_err());
    }
}
