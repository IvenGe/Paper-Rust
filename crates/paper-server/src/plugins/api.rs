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
        "root" => server.config.root.display().to_string(),
        "worlds" => Value::List(server.worlds.iter().map(world_info).collect()),
    }
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
    }
}

/// The state of a player the host caches. The host refreshes its copy whenever this changes.
pub fn player_state(server: &Server, p: &Player) -> Value {
    bmap! {
        "uuid" => p.uuid().to_string(),
        "name" => p.name(),
        "entity_id" => p.entity_id,
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
    }
}

/// An item stack as the host models it: item id, count and the components the server can
/// translate (`name`, `item_name`, `lore`), plus every other component as opaque bytes.
pub fn item_value(slot: &Slot) -> Value {
    let Some(stack) = slot.0.as_ref().filter(|s| s.count > 0) else { return Value::Null };
    let Some(info) = items::get(stack.item as u32) else { return Value::Null };
    let mut out = bmap! { "item" => info.name, "count" => stack.count };
    let mut raw = Vec::new();
    for (ty, data) in &stack.added {
        let name = paper_data::registries::name("minecraft:data_component_type", *ty as u32).unwrap_or("");
        match name {
            "minecraft:custom_name" | "minecraft:item_name" => {
                let mut r = paper_protocol::codec::Reader::new(data);
                if let Ok(c) = r.component() {
                    out.set(if name.ends_with("custom_name") { "name" } else { "item_name" }, component_value(&c));
                    continue;
                }
                raw.push(Value::List(vec![(*ty).into(), Value::Bytes(data.clone())]));
            }
            "minecraft:lore" => {
                let mut r = paper_protocol::codec::Reader::new(data);
                match r.list(256, |r| r.component()) {
                    Ok(lines) => out.set("lore", Value::List(lines.iter().map(component_value).collect())),
                    Err(_) => raw.push(Value::List(vec![(*ty).into(), Value::Bytes(data.clone())])),
                }
            }
            _ => raw.push(Value::List(vec![(*ty).into(), Value::Bytes(data.clone())])),
        }
    }
    if !raw.is_empty() {
        out.set("raw", Value::List(raw));
    }
    if !stack.removed.is_empty() {
        out.set("removed", Value::List(stack.removed.iter().map(|r| Value::from(*r)).collect()));
    }
    out
}

/// The inverse of [`item_value`].
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
    for (key, component) in [("name", "minecraft:custom_name"), ("item_name", "minecraft:item_name")] {
        if let Some(c) = component_from(value.get(key)) {
            if let Some(id) = component_id(component) {
                let mut data = Vec::new();
                data.put_component(&c);
                stack.added.push((id, data));
            }
        }
    }
    if let Some(lines) = value.list("lore") {
        if let Some(id) = component_id("minecraft:lore") {
            let lines: Vec<Component> = lines.iter().filter_map(component_from).collect();
            let mut data = Vec::new();
            data.put_list(&lines, |w, c| w.put_component(c));
            stack.added.push((id, data));
        }
    }
    if let Some(raw) = value.list("raw") {
        for entry in raw {
            if let (Some(ty), Some(data)) = (entry.at(0).as_i32(), entry.at(1).as_bytes()) {
                stack.added.push((ty, data.to_vec()));
            }
        }
    }
    if let Some(removed) = value.list("removed") {
        stack.removed.extend(removed.iter().filter_map(Value::as_i32));
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

/// Executes one API call from the plugin host.
pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    match method {
        // --- players ---
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
            let id = player_id(server, args)?;
            let times = play::SetTitlesAnimation { fade_in: args.i32("fade_in").unwrap_or(10), stay: args.i32("stay").unwrap_or(70), fade_out: args.i32("fade_out").unwrap_or(20) };
            server.send(id, &times);
            if let Some(sub) = component_from(args.get("subtitle")) {
                server.send(id, &play::SetSubtitleText { text: sub });
            }
            server.send(id, &play::SetTitleText { text: component_from(args.get("title")).unwrap_or_default() });
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
            if server.players[&id].world != world {
                bail!("teleporting between worlds is not supported yet");
            }
            let current = server.players[&id].loc;
            let target = Location {
                x: args.f64("x").unwrap_or(current.x),
                y: args.f64("y").unwrap_or(current.y),
                z: args.f64("z").unwrap_or(current.z),
                yaw: args.f64("yaw").map_or(current.yaw, |v| v as f32),
                pitch: args.f64("pitch").map_or(current.pitch, |v| v as f32),
            };
            server.teleport(id, target);
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
            player.conn.send(&play::PlayerAbilities { flags: player.ability_flags(), flying_speed: 0.05, fov_modifier: 0.1 });
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
                seed: rand::random(),
            };
            player.conn.send(&packet);
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

        // --- worlds and blocks ---
        "world.get_block" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            Ok(state_value(server, world, x, y, z))
        }
        "world.set_block" => {
            let world = world_index(server, args)?;
            let (x, y, z) = block_xyz(args)?;
            let state = match args.get("state") {
                Value::Str(s) => blocks::parse_state(s).ok_or_else(|| anyhow!("invalid block state {s}"))?,
                Value::Int(i) if blocks::is_valid_state(*i as u32) => *i as u32,
                other => bail!("invalid block state {other}"),
            };
            if server.worlds[world].block_at(x, y, z).is_none() {
                bail!("the chunk at {x}, {z} is not loaded");
            }
            match server.worlds[world].set_block(x, y, z, state) {
                Some(change) => {
                    server.publish_block_change(world, &change);
                    Ok(Value::Bool(true))
                }
                None => Ok(Value::Bool(false)),
            }
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
        "world.is_chunk_loaded" => {
            let world = world_index(server, args)?;
            let pos = crate::world::chunk::ChunkPos::new(args.i32("x").unwrap_or(0), args.i32("z").unwrap_or(0));
            Ok(Value::Bool(server.worlds[world].is_ready(pos)))
        }

        // --- server ---
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
