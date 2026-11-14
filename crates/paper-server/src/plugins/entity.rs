//! Entities other than players for the plugin API: listing and spawning them, reading their
//! state and changing it. Entities are named by their entity id (unique across worlds and
//! shared with players); the host asks for the current state when plugin code reads it.

use super::api::{item_value, slot_from};
use super::{component_from, component_value};
use crate::server::mobs::SpawnReason;
use crate::server::survival::Damage;
use crate::server::Server;
use crate::world::entity::{Aabb, Entity, EntityKind, Vec3};
use anyhow::{anyhow, bail, Result};
use paper_bridge::{bmap, Value};
use rand::Rng;

/// The world an entity (not a player) is in.
pub fn world_of(server: &Server, id: i32) -> Option<usize> {
    server.worlds.iter().position(|w| w.entities.contains_key(&id))
}

/// What the host knows about an entity.
pub fn entity_value(server: &Server, world: usize, e: &Entity) -> Value {
    let mut out = bmap! {
        "id" => e.id,
        "uuid" => e.uuid.to_string(),
        "type" => e.info.name.clone(),
        "world" => server.worlds[world].name.clone(),
        "x" => e.pos.x, "y" => e.pos.y, "z" => e.pos.z,
        "yaw" => e.yaw, "pitch" => e.pitch,
        "vx" => e.vel.x, "vy" => e.vel.y, "vz" => e.vel.z,
        "on_ground" => e.on_ground,
        "age" => e.age,
        "valid" => !e.removed,
        "custom_name" => e.custom_name.as_ref().map(component_value),
        "custom_name_visible" => e.custom_name_visible,
        "no_gravity" => e.no_gravity,
        "width" => e.info.width as f64,
        "height" => e.info.height as f64,
        "eye_height" => e.info.eye_height as f64,
        "living" => matches!(e.kind, EntityKind::Mob(_)),
        "passengers" => Value::List(e.passengers.iter().map(|p| shallow_ref(server, *p)).collect()),
        "vehicle" => e.vehicle.map(|v| shallow_ref(server, v)),
    };
    match &e.kind {
        EntityKind::Mob(m) => {
            out.set("health", m.living.health);
            out.set("max_health", m.living.max_health);
            out.set("dead", m.living.is_dead() || m.living.death_time > 0);
            out.set("fire_ticks", m.living.fire_ticks);
            out.set("air", m.living.air);
            out.set("baby", m.baby);
            out.set("persistent", m.persistent);
            out.set("tamed", m.tamed);
            out.set("owner", m.owner.map(|o| o.to_string()));
            out.set("sitting", m.sitting);
            out.set("temper", m.temper);
            out.set("jump_strength", m.attributes.value("jump_strength"));
            out.set("no_damage_ticks", m.living.invulnerable_time);
            out.set("equipment", Value::List(m.equipment.iter().map(item_value).collect()));
            let target = m.brain.target.map(|t| match server.players.get(&t) {
                Some(p) => bmap! { "player" => p.uuid().to_string() },
                None => bmap! { "entity" => t as i64 },
            });
            out.set("target", target);
            let category = &e.info.category;
            out.set("category", category.clone());
            if let Some(merchant) = m.merchant.as_deref() {
                out.set("villager_type", merchant.villager_type.clone());
                out.set("profession", merchant.profession.clone());
                out.set("villager_level", merchant.level);
                out.set("villager_xp", merchant.xp);
                out.set("restocks_today", merchant.restocks_today);
                out.set("despawn_delay", merchant.despawn_delay);
                out.set("trader", merchant.trading_player.and_then(|p| server.players.get(&p)).map(|p| p.uuid().to_string()));
            }
        }
        EntityKind::Item { stack, pickup_delay, thrower, owner } => {
            out.set("item", item_value(stack));
            out.set("pickup_delay", *pickup_delay);
            out.set("thrower", thrower.map(|t| t.to_string()));
            out.set("item_owner", owner.map(|o| o.to_string()));
        }
        EntityKind::Arrow { damage, owner, in_ground, pickup, critical, .. } => {
            out.set("damage", *damage);
            out.set("shooter", owner.map(|o| match server.players.get(&o) {
                Some(p) => bmap! { "player" => p.uuid().to_string() },
                None => bmap! { "entity" => o as i64 },
            }));
            out.set("in_ground", *in_ground);
            out.set("pickup", *pickup);
            out.set("critical", *critical);
        }
        EntityKind::ExperienceOrb { value } => out.set("experience", *value),
        EntityKind::Tnt { fuse } => out.set("fuse", *fuse),
        EntityKind::FallingBlock { state, .. } => out.set("block", paper_data::blocks::state_to_string(*state)),
        EntityKind::Thrown { item, .. } => out.set("item", item_value(item)),
        EntityKind::EyeOfEnder { .. } | EntityKind::Cloud { .. } | EntityKind::Other => {}
    }
    out
}

fn entity_arg(server: &Server, args: &Value) -> Result<(usize, i32)> {
    let id = args.i32("id").ok_or_else(|| anyhow!("missing id"))?;
    let world = world_of(server, id).ok_or_else(|| anyhow!("entity {id} does not exist"))?;
    Ok((world, id))
}

fn world_arg(server: &Server, args: &Value) -> Result<usize> {
    match args.str("world") {
        Some(name) => server.worlds.iter().position(|w| w.name == name).ok_or_else(|| anyhow!("unknown world {name}")),
        None => Ok(0),
    }
}

fn vec_arg(args: &Value, x: &str, y: &str, z: &str) -> Option<Vec3> {
    Some(Vec3::new(args.f64(x)?, args.f64(y)?, args.f64(z)?))
}

/// `{player: uuid}` or `{entity: id}` without the entity's state (riders and vehicles name
/// each other).
pub fn shallow_ref(server: &Server, id: i32) -> Value {
    match server.players.get(&id) {
        Some(p) => bmap! { "player" => p.uuid().to_string() },
        None => bmap! { "entity" => id as i64 },
    }
}

/// A player or entity named by `{player: uuid}` or `{entity: id}`.
fn target_id(server: &Server, value: &Value) -> Option<i32> {
    if let Some(uuid) = value.str("player") {
        return uuid.parse().ok().and_then(|u| server.by_uuid.get(&u).copied());
    }
    value.i32("entity")
}

/// Executes an entity API call (`entity.*`, `world.entities`, `world.spawn_entity`,
/// `world.drop_item`).
pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    match method {
        "world.entities" => {
            let world = world_arg(server, args)?;
            let types: Option<Vec<&str>> = args.list("types").map(|l| l.iter().filter_map(Value::as_str).collect());
            let area = match (vec_arg(args.get("min"), "x", "y", "z"), vec_arg(args.get("max"), "x", "y", "z")) {
                (Some(min), Some(max)) => Some(Aabb::new(min, max)),
                _ => None,
            };
            let w = &server.worlds[world];
            let mut ids: Vec<i32> = match &area {
                Some(area) => w.entities_in(area),
                None => w.entities.keys().copied().collect(),
            };
            ids.sort_unstable();
            let list = ids
                .iter()
                .filter_map(|id| w.entities.get(id))
                .filter(|e| !e.removed)
                .filter(|e| types.as_ref().is_none_or(|t| t.contains(&e.info.name.as_str())))
                .filter(|e| area.as_ref().is_none_or(|a| e.bounding_box().intersects(a)))
                .map(|e| entity_value(server, world, e))
                .collect();
            Ok(Value::List(list))
        }
        "world.spawn_entity" => {
            let world = world_arg(server, args)?;
            let ty = args.str("type").ok_or_else(|| anyhow!("missing type"))?.to_owned();
            let pos = vec_arg(args, "x", "y", "z").ok_or_else(|| anyhow!("missing position"))?;
            let id = match ty.as_str() {
                "minecraft:item" => bail!("Cannot spawn an item entity without an item; use World#dropItem"),
                "minecraft:player" => bail!("Cannot spawn a player"),
                "minecraft:arrow" => server.spawn_arrow_towards(world, pos, Vec3::new(0.0, 0.0, 1.0), 0.0, 0.0, None, 2.0, false, true),
                _ => {
                    let reason = match args.str("reason") {
                        Some("COMMAND") => SpawnReason::Command,
                        Some("SPAWNER_EGG") => SpawnReason::Egg,
                        Some("NATURAL") => SpawnReason::Natural,
                        Some("SPAWNER") => SpawnReason::Spawner,
                        Some("BUCKET") => SpawnReason::Bucket,
                        Some("BUILD_SNOWMAN" | "BUILD_IRONGOLEM") => SpawnReason::Built,
                        _ => SpawnReason::Plugin,
                    };
                    server.spawn_mob(world, &ty, pos, reason).ok_or_else(|| anyhow!("cannot spawn {ty}"))?
                }
            };
            let Some(e) = server.worlds[world].entities.get_mut(&id) else { bail!("the entity was not spawned") };
            if let Some(yaw) = args.f64("yaw") {
                e.yaw = yaw as f32;
                e.head_yaw = yaw as f32;
            }
            if let Some(pitch) = args.f64("pitch") {
                e.pitch = pitch as f32;
            }
            let e = &server.worlds[world].entities[&id];
            Ok(entity_value(server, world, e))
        }
        "world.drop_item" => {
            let world = world_arg(server, args)?;
            let mut pos = vec_arg(args, "x", "y", "z").ok_or_else(|| anyhow!("missing position"))?;
            let stack = slot_from(args.get("item"))?;
            if stack.is_empty() {
                bail!("Cannot drop air");
            }
            let mut rng = rand::thread_rng();
            if args.bool("natural") == Some(true) {
                // World#dropItemNaturally: a random spot in the block, as vanilla pops items.
                pos = Vec3::new(pos.x + rng.gen_range(-0.25..0.25), pos.y + rng.gen_range(-0.25..0.25) - 0.125, pos.z + rng.gen_range(-0.25..0.25));
            }
            let vel = Vec3::new(rng.gen_range(-0.1..0.1), 0.2, rng.gen_range(-0.1..0.1));
            let id = server.spawn_item(world, pos, stack, vel, 10).ok_or_else(|| anyhow!("the item was not dropped"))?;
            let e = &server.worlds[world].entities[&id];
            Ok(entity_value(server, world, e))
        }
        "entity.state" => {
            let id = args.i32("id").ok_or_else(|| anyhow!("missing id"))?;
            match world_of(server, id) {
                Some(world) => Ok(entity_value(server, world, &server.worlds[world].entities[&id])),
                // Removed entities are gone; the host reports them as invalid.
                None => Ok(Value::Null),
            }
        }
        "entity.teleport" => {
            let (world, id) = entity_arg(server, args)?;
            let target_world = match args.str("world") {
                Some(_) => world_arg(server, args)?,
                None => world,
            };
            if target_world != world {
                bail!("teleporting entities between worlds is not supported yet");
            }
            let e = server.worlds[world].entities.get_mut(&id).unwrap();
            if let Some(pos) = vec_arg(args, "x", "y", "z") {
                e.pos = pos;
            }
            if let Some(yaw) = args.f64("yaw") {
                e.yaw = yaw as f32;
                e.head_yaw = yaw as f32;
            }
            if let Some(pitch) = args.f64("pitch") {
                e.pitch = pitch as f32;
            }
            e.force_sync = true;
            if let Some(m) = e.mob_mut() {
                m.brain.path.clear();
            }
            server.worlds[world].reindex_entity(id);
            Ok(Value::Bool(true))
        }
        "entity.remove" => {
            let (world, id) = entity_arg(server, args)?;
            server.worlds[world].entities.get_mut(&id).unwrap().removed = true;
            Ok(Value::Null)
        }
        "entity.add_effect" | "entity.remove_effect" | "entity.clear_effects" | "entity.effects" => {
            let id = args.i32("id").ok_or_else(|| anyhow!("missing id"))?;
            let world = match server.players.get(&id) {
                Some(p) => p.world,
                None => world_of(server, id).ok_or_else(|| anyhow!("entity {id} does not exist"))?,
            };
            let effect_name = || args.str("effect").ok_or_else(|| anyhow!("missing effect"));
            match method {
                "entity.add_effect" => {
                    let mut effect = crate::server::effects::ActiveEffect::new(effect_name()?, args.i32("duration").unwrap_or(600), args.i32("amplifier").unwrap_or(0)).ok_or_else(|| anyhow!("unknown effect {}", args.str("effect").unwrap_or("")))?;
                    effect.ambient = args.bool("ambient").unwrap_or(false);
                    effect.particles = args.bool("particles").unwrap_or(true);
                    effect.icon = args.bool("icon").unwrap_or(true);
                    Ok(Value::Bool(server.add_effect(world, id, effect)))
                }
                "entity.remove_effect" => {
                    let effect = paper_data::effects::by_name(effect_name()?).ok_or_else(|| anyhow!("unknown effect"))?;
                    Ok(Value::Bool(server.remove_effect(world, id, effect.id)))
                }
                "entity.clear_effects" => Ok(Value::Int(server.clear_effects(world, id) as i64)),
                _ => Ok(Value::List(
                    server
                        .effects_of(world, id)
                        .iter()
                        .map(|e| bmap! { "effect" => e.name(), "duration" => e.duration, "amplifier" => e.amplifier, "ambient" => e.ambient, "particles" => e.particles, "icon" => e.icon })
                        .collect(),
                )),
            }
        }
        "entity.damage" => {
            let id = args.i32("id").ok_or_else(|| anyhow!("missing id"))?;
            let world = match server.players.get(&id) {
                Some(p) => p.world,
                None => world_of(server, id).ok_or_else(|| anyhow!("entity {id} does not exist"))?,
            };
            let amount = args.f64("amount").unwrap_or(0.0) as f32;
            let attacker = target_id(server, args.get("source"));
            let damage = match attacker.and_then(|a| server.entity_position(world, a).map(|p| (a, p))) {
                Some((a, pos)) => Damage::by(if server.players.contains_key(&a) { "minecraft:player_attack" } else { "minecraft:mob_attack" }, a, pos),
                None => Damage::new("minecraft:generic"),
            };
            Ok(Value::Bool(server.hurt_entity(world, id, damage, amount)))
        }
        // A player's or entity's persistent data container, as network NBT.
        "entity.pdc" => {
            let Some(id) = target_id(server, args.get("target")) else { return Ok(Value::Null) };
            let pdc = match server.players.get(&id) {
                Some(p) => p.pdc.clone(),
                None => world_of(server, id).and_then(|w| server.worlds[w].entities.get(&id)).and_then(|e| e.pdc.clone()),
            };
            Ok(match pdc {
                Some(c) => {
                    let mut out = Vec::new();
                    paper_nbt::write_network_compound(&mut out, &c);
                    Value::Bytes(out)
                }
                None => Value::Null,
            })
        }
        "entity.set_pdc" => {
            let Some(id) = target_id(server, args.get("target")) else { return Ok(Value::Bool(false)) };
            let pdc = match args.get("data") {
                Value::Bytes(b) if !b.is_empty() => match paper_nbt::read_network(&mut b.as_slice(), 2 * 1024 * 1024) {
                    Ok(Some(paper_nbt::Tag::Compound(c))) => Some(c),
                    _ => return Ok(Value::Bool(false)),
                },
                _ => None,
            };
            if let Some(p) = server.players.get_mut(&id) {
                p.pdc = pdc;
                return Ok(Value::Bool(true));
            }
            let Some(world) = world_of(server, id) else { return Ok(Value::Bool(false)) };
            if let Some(e) = server.worlds[world].entities.get_mut(&id) {
                e.pdc = pdc;
            }
            Ok(Value::Bool(true))
        }
        // `Entity.addPassenger`, `removePassenger`, `eject`, `leaveVehicle`.
        "entity.add_passenger" => {
            let (world, id) = entity_arg(server, args)?;
            let Some(rider) = target_id(server, args.get("passenger")) else { return Ok(Value::Bool(false)) };
            if server.vehicle_of(rider).is_some_and(|(_, v)| v == id) {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(server.mount(world, rider, id)))
        }
        "entity.remove_passenger" => {
            let (world, id) = entity_arg(server, args)?;
            let Some(rider) = target_id(server, args.get("passenger")) else { return Ok(Value::Bool(false)) };
            if !server.passengers_of(world, id).contains(&rider) {
                return Ok(Value::Bool(false));
            }
            server.dismount(rider);
            Ok(Value::Bool(true))
        }
        "entity.eject" => {
            let (world, id) = entity_arg(server, args)?;
            let riders = server.passengers_of(world, id);
            for r in &riders {
                server.dismount(*r);
            }
            Ok(Value::Bool(!riders.is_empty()))
        }
        "entity.leave_vehicle" => {
            let Some(rider) = target_id(server, args.get("rider")) else { return Ok(Value::Bool(false)) };
            if server.vehicle_of(rider).is_none() {
                return Ok(Value::Bool(false));
            }
            server.dismount(rider);
            Ok(Value::Bool(true))
        }
        "entity.set" => {
            let (world, id) = entity_arg(server, args)?;
            let target = match args.get("target") {
                Value::Null => None,
                v => Some(target_id(server, v)),
            };
            let clear_target = args.bool("clear_target") == Some(true);
            if let Some(sitting) = args.bool("sitting") {
                server.set_sitting(world, id, sitting);
            }
            let e = server.worlds[world].entities.get_mut(&id).unwrap();
            if args.bool("clear_custom_name") == Some(true) {
                e.custom_name = None;
                e.metadata_dirty = true;
            }
            if let Some(name) = component_from(args.get("custom_name")) {
                e.custom_name = Some(name);
                e.metadata_dirty = true;
            }
            if let Some(visible) = args.bool("custom_name_visible") {
                e.custom_name_visible = visible;
                e.metadata_dirty = true;
            }
            if let Some(gravity) = args.bool("gravity") {
                e.no_gravity = !gravity;
                e.metadata_dirty = true;
            }
            let mut pet_changed = false;
            if let Some(m) = e.mob_mut() {
                if let Some(tamed) = args.bool("tamed") {
                    m.tamed = tamed;
                }
                match args.get("owner") {
                    Value::Str(o) if o.is_empty() => m.owner = None,
                    Value::Str(o) => m.owner = o.parse().ok(),
                    _ => {}
                }
                if let Some(temper) = args.i32("temper") {
                    m.temper = temper.clamp(0, 100);
                }
                if let Some(jump) = args.f64("jump_strength") {
                    m.attributes.set_base("jump_strength", jump);
                }
                pet_changed = args.bool("tamed").is_some() || matches!(args.get("owner"), Value::Str(_));
            }
            if pet_changed {
                e.metadata_dirty = true;
            }
            if let Some(v) = vec_arg(args, "vx", "vy", "vz") {
                e.vel = v;
                e.force_sync = true;
            }
            match &mut e.kind {
                EntityKind::Mob(m) => {
                    if let Some(h) = args.f64("max_health") {
                        if h <= 0.0 {
                            bail!("Max health amount ({h}) must be greater than 0");
                        }
                        // The max_health attribute's base, as CraftLivingEntity#setMaxHealth.
                        m.attributes.set_base("max_health", h);
                        m.living.max_health = m.attributes.value("max_health") as f32;
                        m.living.health = m.living.health.min(m.living.max_health);
                    }
                    if let Some(h) = args.f64("health") {
                        if h < 0.0 || h > m.living.max_health as f64 {
                            bail!("Health value ({h}) must be between 0 and {}", m.living.max_health);
                        }
                        m.living.health = h as f32;
                        e.metadata_dirty = true;
                    }
                    if let Some(t) = args.i32("fire_ticks") {
                        m.living.fire_ticks = t.max(0);
                        e.metadata_dirty = true;
                    }
                    if let Some(a) = args.i32("air") {
                        m.living.air = a;
                    }
                    if let Some(t) = args.i32("no_damage_ticks") {
                        m.living.invulnerable_time = t.max(0);
                    }
                    if let Some(p) = args.bool("persistent") {
                        m.persistent = p;
                    }
                    if let Some(b) = args.bool("baby") {
                        m.baby = b;
                        e.metadata_dirty = true;
                    }
                    if let Some(equipment) = args.list("equipment") {
                        for entry in equipment {
                            let slot = entry.at(0).as_i64().filter(|s| (0..8).contains(s)).ok_or_else(|| anyhow!("equipment slots are 0-7"))? as usize;
                            m.equipment[slot] = slot_from(entry.at(1))?;
                        }
                        e.equipment_dirty = true;
                    }
                    // Villagers and wandering traders (CraftVillager, CraftWanderingTrader).
                    if let Some(merchant) = m.merchant.as_deref_mut() {
                        if let Some(t) = args.str("villager_type") {
                            paper_data::registries::id("minecraft:villager_type", t).ok_or_else(|| anyhow!("unknown villager type {t}"))?;
                            merchant.villager_type = t.to_owned();
                            e.metadata_dirty = true;
                        }
                        if let Some(p) = args.str("profession") {
                            paper_data::registries::id("minecraft:villager_profession", p).ok_or_else(|| anyhow!("unknown profession {p}"))?;
                            // `setVillagerData`: another profession means other trades.
                            if merchant.profession != p {
                                merchant.profession = p.to_owned();
                                merchant.offers = None;
                            }
                            e.metadata_dirty = true;
                        }
                        if let Some(l) = args.i32("villager_level") {
                            merchant.level = l.clamp(1, 5);
                            e.metadata_dirty = true;
                        }
                        if let Some(x) = args.i32("villager_xp") {
                            merchant.xp = x.max(0);
                        }
                        if let Some(r) = args.i32("restocks_today") {
                            merchant.restocks_today = r;
                        }
                        if let Some(d) = args.i32("despawn_delay") {
                            merchant.despawn_delay = d;
                        }
                    }
                    if clear_target {
                        m.brain.target = None;
                        e.metadata_dirty = true;
                    } else if let Some(t) = target {
                        m.brain.target = t;
                        m.brain.path.clear();
                        e.metadata_dirty = true;
                    }
                }
                EntityKind::Item { stack, pickup_delay, thrower, owner } => {
                    // `Item.setOwner`/`setThrower` (an empty string clears them).
                    if let Some(o) = args.str("item_owner") {
                        *owner = o.parse().ok();
                    }
                    if let Some(t) = args.str("thrower") {
                        *thrower = t.parse().ok();
                    }
                    if !args.get("item").is_null() {
                        let item = slot_from(args.get("item"))?;
                        if item.is_empty() {
                            e.removed = true;
                        } else {
                            *stack = item;
                        }
                        e.metadata_dirty = true;
                    }
                    if let Some(d) = args.i32("pickup_delay") {
                        *pickup_delay = d.clamp(0, 32767);
                    }
                }
                EntityKind::Arrow { damage, pickup, critical, .. } => {
                    if let Some(d) = args.f64("damage") {
                        *damage = d;
                    }
                    if let Some(p) = args.bool("pickup") {
                        *pickup = p;
                    }
                    if let Some(c) = args.bool("critical") {
                        *critical = c;
                        e.metadata_dirty = true;
                    }
                }
                EntityKind::ExperienceOrb { value } => {
                    if let Some(v) = args.i32("experience") {
                        *value = v.max(0);
                        e.metadata_dirty = true;
                    }
                }
                EntityKind::Tnt { fuse } => {
                    if let Some(f) = args.i32("fuse") {
                        *fuse = f.max(0);
                        e.metadata_dirty = true;
                    }
                }
                _ => {}
            }
            Ok(entity_value(server, world, &server.worlds[world].entities[&id]))
        }
        other => bail!("unknown plugin API method {other}"),
    }
}
