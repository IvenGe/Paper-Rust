//! The plugin API's player calls beyond the basics in `api`: what Bukkit's player and entity
//! setters change (speeds, the time and weather one player sees, flags, tags, experience,
//! where the compass points, ...), hiding entities from a player, item cooldowns, and the
//! packets plugins send to one client only (`sendExperienceChange`, `sendHurtAnimation`, ...).

use super::api::slot_from;
use super::component_from;
use crate::server::Server;
use anyhow::{anyhow, bail, Result};
use paper_bridge::Value;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::LpVec3;
use uuid::Uuid;

fn player_id(server: &Server, args: &Value) -> Result<i32> {
    let uuid: Uuid = args.str("uuid").ok_or_else(|| anyhow!("missing uuid"))?.parse()?;
    server.by_uuid.get(&uuid).copied().ok_or_else(|| anyhow!("player {uuid} is not online"))
}

fn world_named(server: &Server, name: &str) -> Result<usize> {
    server.worlds.iter().position(|w| w.name == name).ok_or_else(|| anyhow!("unknown world {name}"))
}

/// `CraftPlayer.validateSpeed`.
fn speed(value: f64) -> Result<f32> {
    if !(-1.0..=1.0).contains(&value) {
        bail!("Speed value ({value}) need to be between -1f and 1f");
    }
    Ok(value as f32 / 2.0)
}

/// The calls this module answers.
pub fn handles(method: &str) -> bool {
    matches!(
        method,
        "player.set"
            | "player.update_inventory"
            | "player.hide_entity"
            | "player.show_entity"
            | "player.cooldown"
            | "player.game_event"
            | "player.transfer"
            | "player.swing"
            | "player.send_experience"
            | "player.send_health"
            | "player.hurt_animation"
            | "player.block_damage"
            | "player.equipment_change"
            | "player.camera"
    )
}

pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    let id = player_id(server, args)?;
    match method {
        "player.set" => {
            set(server, id, args)?;
            Ok(super::api::player_state(server, &server.players[&id]))
        }
        "player.update_inventory" => {
            // `containerMenu.sendAllDataToRemote`: the open window, or the inventory.
            server.send_menu_contents(id);
            Ok(Value::Null)
        }
        "player.hide_entity" | "player.show_entity" => {
            let target: Uuid = args.str("target").ok_or_else(|| anyhow!("missing target"))?.parse()?;
            let hide = method == "player.hide_entity";
            let p = server.players.get_mut(&id).unwrap();
            let changed = if hide { p.api.hidden.insert(target) } else { p.api.hidden.remove(&target) };
            if changed {
                // Players leave or rejoin this player's tab list (`CraftPlayer.hideEntity0`);
                // the trackers despawn or spawn the entity on their next tick.
                if let Some(other) = server.by_uuid.get(&target).and_then(|t| server.players.get(t)) {
                    let viewer = &server.players[&id];
                    if hide {
                        viewer.conn.send(&play::PlayerInfoRemove { uuids: vec![target] });
                    } else {
                        let actions = play::player_info::ADD_PLAYER | play::player_info::UPDATE_GAME_MODE | play::player_info::UPDATE_LISTED | play::player_info::UPDATE_LATENCY | play::player_info::UPDATE_DISPLAY_NAME;
                        viewer.conn.send(&play::PlayerInfoUpdate { actions, entries: vec![crate::server::join_info_entry(other)] });
                    }
                }
            }
            Ok(Value::Bool(changed))
        }
        "player.cooldown" => {
            // `HumanEntity.setCooldown`: an item cooldown group (the item's id by default).
            let group = args.str("group").ok_or_else(|| anyhow!("missing group"))?.to_owned();
            let tick = server.tick;
            let p = server.players.get_mut(&id).unwrap();
            if let Some(ticks) = args.i32("ticks") {
                if ticks < 0 {
                    bail!("Cannot have negative cooldown");
                }
                if ticks == 0 {
                    p.cooldowns.remove(&group);
                } else {
                    p.cooldowns.insert(group.clone(), tick + ticks as u64);
                }
                p.conn.send(&play::Cooldown { group, duration: VarInt(ticks) });
                return Ok(Value::Null);
            }
            Ok(Value::Int(p.cooldowns.get(&group).map_or(0, |until| until.saturating_sub(tick) as i64)))
        }
        "player.game_event" => {
            // The demo screen (5), the elder guardian (10), the credits (4).
            let event = args.i32("event").ok_or_else(|| anyhow!("missing event"))?;
            let value = args.f64("value").unwrap_or(0.0) as f32;
            server.players[&id].conn.send(&play::GameEvent { event: event as u8, value });
            Ok(Value::Null)
        }
        "player.transfer" => {
            let host = args.str("host").ok_or_else(|| anyhow!("missing host"))?.to_owned();
            let port = args.i32("port").ok_or_else(|| anyhow!("missing port"))?;
            server.players[&id].conn.send(&play::Transfer { host, port: VarInt(port) });
            Ok(Value::Null)
        }
        "player.swing" => {
            let hand = args.i32("hand").unwrap_or(0);
            let packet = play::SwingAnimation { entity_id: VarInt(id), hand: VarInt(hand), animation_type: VarInt(1), duration: VarInt(6) };
            server.broadcast_to_viewers(id, &packet);
            server.players[&id].conn.send(&packet);
            Ok(Value::Null)
        }
        "player.send_experience" => {
            let p = &server.players[&id];
            let bar = args.f64("progress").unwrap_or(p.xp_progress as f64) as f32;
            if !(0.0..=1.0).contains(&bar) {
                bail!("Experience progress must be between 0.0 and 1.0 ({bar})");
            }
            let level = args.i32("level").unwrap_or(p.xp_level);
            if level < 0 {
                bail!("Experience level must not be negative ({level})");
            }
            let total = args.i32("total").unwrap_or(p.xp_total);
            p.conn.send(&play::SetExperience { bar, level: VarInt(level), total: VarInt(total) });
            Ok(Value::Null)
        }
        "player.send_health" => {
            let p = &server.players[&id];
            let health = args.f64("health").unwrap_or(p.health as f64) as f32;
            let food = args.i32("food").unwrap_or(p.food);
            let saturation = args.f64("saturation").unwrap_or(p.saturation as f64) as f32;
            p.conn.send(&play::SetHealth { health, food: VarInt(food), saturation });
            Ok(Value::Null)
        }
        "player.hurt_animation" => {
            // `sendHurtAnimation(yaw)` shows this client its own hurt tilt;
            // `playHurtAnimation` shows everyone who sees the player.
            let yaw = args.f64("yaw").unwrap_or(0.0) as f32;
            let packet = play::HurtAnimation { entity_id: VarInt(id), yaw };
            if args.bool("everyone") == Some(true) {
                server.broadcast_to_viewers(id, &packet);
            }
            server.players[&id].conn.send(&packet);
            Ok(Value::Null)
        }
        "player.block_damage" => {
            // `sendBlockDamage`: a crack overlay (stage 0-9, or -1 / 10 to remove it).
            let pos = BlockPos::new(args.i32("x").unwrap_or(0), args.i32("y").unwrap_or(0), args.i32("z").unwrap_or(0));
            let stage = args.i32("stage").unwrap_or(-1);
            let source = args.i32("source").unwrap_or(id);
            server.players[&id].conn.send(&play::BlockDestruction { entity_id: VarInt(source), pos, stage: stage.clamp(-1, 255) as u8 });
            Ok(Value::Null)
        }
        "player.equipment_change" => {
            // `sendEquipmentChange`: what another entity seems to wear, for this client only.
            let entity = args.i32("entity").ok_or_else(|| anyhow!("missing entity"))?;
            let mut equipment = Vec::new();
            for entry in args.list("equipment").unwrap_or(&[]) {
                let slot = entry.at(0).as_i64().ok_or_else(|| anyhow!("missing slot"))? as u8;
                equipment.push((slot, slot_from(entry.at(1))?));
            }
            server.players[&id].conn.send(&play::SetEquipment { entity_id: entity, equipment });
            Ok(Value::Null)
        }
        "player.camera" => {
            // `setSpectatorTarget`: look through another entity (itself to stop).
            let target = args.i32("target").unwrap_or(id);
            server.players[&id].conn.send(&play::SetCamera { entity_id: VarInt(target) });
            Ok(Value::Null)
        }
        other => bail!("unknown plugin API method {other}"),
    }
}

/// `player.set`: every Bukkit setter of player and entity state that is plain data.
fn set(server: &mut Server, id: i32, args: &Value) -> Result<()> {
    let tick = server.tick;
    let mut abilities = false;
    let mut resend_time = false;
    let mut resend_weather = false;
    let mut experience = false;
    let mut compass = None;
    let mut velocity = None;
    let respawn = match args.get("respawn") {
        Value::Null => None,
        Value::List(l) if l.is_empty() => Some(None),
        v => {
            let world = world_named(server, v.at(0).as_str().ok_or_else(|| anyhow!("missing respawn world"))?)?;
            let pos = BlockPos::new(v.at(1).as_i64().unwrap_or(0) as i32, v.at(2).as_i64().unwrap_or(0) as i32, v.at(3).as_i64().unwrap_or(0) as i32);
            Some(Some((world, pos)))
        }
    };
    if let Value::List(l) = args.get("compass_target") {
        let world = world_named(server, l.first().and_then(Value::as_str).ok_or_else(|| anyhow!("missing compass world"))?)?;
        let pos = BlockPos::new(l.get(1).and_then(Value::as_i64).unwrap_or(0) as i32, l.get(2).and_then(Value::as_i64).unwrap_or(0) as i32, l.get(3).and_then(Value::as_i64).unwrap_or(0) as i32);
        compass = Some((world, pos, l.get(4).and_then(Value::as_f64).unwrap_or(0.0) as f32));
    }
    let p = server.players.get_mut(&id).unwrap();
    if let Some(v) = args.f64("walk_speed") {
        p.api.walk_speed = speed(v)?;
        abilities = true;
        // SPIGOT-5833: the movement speed attribute follows the walking speed.
        p.attributes.set_base("movement_speed", p.api.walk_speed as f64);
    }
    if let Some(v) = args.f64("fly_speed") {
        p.api.fly_speed = speed(v)?;
        abilities = true;
    }
    if let Some(b) = args.bool("sleeping_ignored") {
        p.api.sleeping_ignored = b;
    }
    if args.bool("reset_time") == Some(true) {
        p.api.time = None;
        resend_time = true;
    }
    if let Some(t) = args.i64("time") {
        p.api.time = Some((t, args.bool("relative").unwrap_or(true)));
        resend_time = true;
    }
    if args.bool("reset_weather") == Some(true) {
        p.api.weather = None;
        resend_weather = true;
    }
    if let Some(w) = args.str("weather") {
        p.api.weather = Some(w == "DOWNFALL");
        resend_weather = true;
    }
    if let Some(b) = args.bool("invulnerable") {
        p.api.invulnerable = b;
    }
    if let Some(b) = args.bool("glowing") {
        p.api.glowing = b;
        p.metadata_dirty = true;
    }
    if let Some(b) = args.bool("silent") {
        p.api.silent = b;
        p.metadata_dirty = true;
    }
    if let Some(b) = args.bool("gravity") {
        p.api.no_gravity = !b;
        p.metadata_dirty = true;
    }
    if let Some(b) = args.bool("invisible") {
        p.api.invisible = b;
        p.metadata_dirty = true;
    }
    if args.bool("clear_custom_name") == Some(true) {
        p.api.custom_name = None;
        p.metadata_dirty = true;
    }
    if let Some(name) = component_from(args.get("custom_name")) {
        p.api.custom_name = Some(name);
        p.metadata_dirty = true;
    }
    if let Some(b) = args.bool("custom_name_visible") {
        p.api.custom_name_visible = b;
        p.metadata_dirty = true;
    }
    if let Some(tag) = args.str("add_tag") {
        if !p.api.tags.iter().any(|t| t == tag) {
            p.api.tags.push(tag.to_owned());
        }
    }
    if let Some(tag) = args.str("remove_tag") {
        p.api.tags.retain(|t| t != tag);
    }
    if let Some(v) = args.i32("max_air") {
        p.api.max_air = v;
    }
    if let Some(v) = args.i32("air") {
        p.air = v.min(p.api.max_air);
        p.metadata_dirty = true;
    }
    if let Some(v) = args.i32("no_damage_ticks") {
        p.invulnerable_time = v.max(0);
    }
    if let Some(v) = args.i32("max_no_damage_ticks") {
        p.api.max_no_damage_ticks = v.max(0);
    }
    if let Some(v) = args.f64("last_damage") {
        p.last_hurt = v as f32;
    }
    if let Some(v) = args.f64("fall_distance") {
        p.fall_distance = v;
    }
    if let Some(v) = args.i32("fire_ticks") {
        p.fire_ticks = v.max(-20);
        p.metadata_dirty = true;
    }
    if let Some(v) = args.i32("freeze_ticks") {
        p.api.freeze_ticks = v.max(0);
        p.metadata_dirty = true;
    }
    if let Some(v) = args.i32("arrows_in_body") {
        p.api.arrows_in_body = v.max(0);
        p.metadata_dirty = true;
    }
    if let Some(v) = args.i32("stingers_in_body") {
        p.api.stingers_in_body = v.max(0);
        p.metadata_dirty = true;
    }
    if let Some(v) = args.i32("score") {
        p.api.score = v;
        p.metadata_dirty = true;
    }
    if let Some(v) = args.f64("absorption") {
        p.absorption = v.max(0.0) as f32;
        p.metadata_dirty = true;
    }
    if let Some(v) = args.f64("exhaustion") {
        p.exhaustion = v as f32;
    }
    if let Some(scaled) = args.bool("health_scaled") {
        let scale = args.f64("health_scale").or(p.api.health_scale).unwrap_or(20.0);
        p.api.health_scale = scaled.then_some(scale);
        p.sent_health = None;
    } else if let Some(scale) = args.f64("health_scale") {
        p.api.health_scale = Some(scale);
        p.sent_health = None;
    }
    if let Some(v) = args.i32("portal_cooldown") {
        p.portal_cooldown = v.max(0);
    }
    if let Some(v) = args.i32("ticks_lived") {
        if v <= 0 {
            bail!("Age value ({v}) must be greater than 0");
        }
        p.api.joined_tick = tick.saturating_sub(v as u64);
    }
    if let Some(b) = args.bool("sneaking") {
        p.sneaking = b;
        p.metadata_dirty = true;
    }
    if let Some(b) = args.bool("sprinting") {
        p.sprinting = b;
        p.metadata_dirty = true;
    }
    if let Some(v) = args.i32("xp_level") {
        p.xp_level = v.max(0);
        experience = true;
    }
    if let Some(v) = args.f64("xp_progress") {
        p.xp_progress = (v as f32).clamp(0.0, 1.0);
        experience = true;
    }
    if let Some(v) = args.i32("xp_total") {
        p.xp_total = v.max(0);
        experience = true;
    }
    if let Some(r) = respawn {
        p.respawn = r;
    }
    if let Some(b) = args.bool("seen_credits") {
        p.seen_credits = b;
    }
    if args.bool("reset_attack_cooldown") == Some(true) {
        p.last_swing = tick;
    }
    if args.bool("clear_active_item") == Some(true) && p.using_item.take().is_some() {
        p.metadata_dirty = true;
    }
    if let Some((world, pos, yaw)) = compass {
        p.api.compass_target = Some((world, pos, yaw));
    }
    if let Some(v) = super::entity::vec_arg(args, "vx", "vy", "vz") {
        velocity = Some(v);
    }
    if abilities {
        p.conn.send(&p.abilities_packet());
        let names = ["minecraft:movement_speed"];
        p.conn.send(&play::UpdateAttributes { entity_id: id, attributes: p.attributes.packet_entries(names.into_iter()) });
    }
    if experience {
        p.conn.send(&play::SetExperience { bar: p.xp_progress, level: VarInt(p.xp_level), total: VarInt(p.xp_total) });
    }
    if let Some((world, pos, yaw)) = compass {
        let dimension = server.worlds[world].dimension.clone();
        server.players[&id].conn.send(&play::SetDefaultSpawnPosition { dimension, pos, yaw, pitch: 0.0 });
    }
    if let Some(v) = velocity {
        // `Entity.setVelocity` on a player: the client moves itself, so it is told.
        let packet = play::SetEntityMotion { entity_id: VarInt(id), velocity: LpVec3(v.x, v.y, v.z) };
        server.players[&id].conn.send(&packet);
        server.broadcast_to_viewers(id, &packet);
    }
    if resend_time {
        server.send_time(id);
    }
    if resend_weather {
        send_own_weather(server, id);
    }
    Ok(())
}

/// After `setPlayerWeather` or `resetPlayerWeather`: rain starts or stops for this player.
fn send_own_weather(server: &Server, id: i32) {
    let Some(p) = server.players.get(&id) else { return };
    let w = &server.worlds[p.world].weather;
    let raining = p.api.weather.unwrap_or_else(|| w.is_raining());
    let event = if raining { play::game_event::START_RAINING } else { play::game_event::STOP_RAINING };
    p.conn.send(&play::GameEvent { event, value: 0.0 });
    if p.api.weather.is_none() && raining {
        p.conn.send(&play::GameEvent { event: play::game_event::RAIN_LEVEL_CHANGE, value: w.rain_level });
        p.conn.send(&play::GameEvent { event: play::game_event::THUNDER_LEVEL_CHANGE, value: w.thunder_level });
    } else {
        p.conn.send(&play::GameEvent { event: play::game_event::RAIN_LEVEL_CHANGE, value: if raining { 1.0 } else { 0.0 } });
    }
}
