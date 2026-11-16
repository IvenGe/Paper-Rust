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
        "glowing" => e.flags.glowing,
        "silent" => e.flags.silent,
        "invisible" => e.flags.invisible,
        "invulnerable" => e.flags.invulnerable,
        "visual_fire" => e.flags.visual_fire,
        "portal_cooldown" => e.flags.portal_cooldown,
        "freeze_ticks" => e.flags.freeze_ticks,
        "tags" => Value::List(e.flags.tags.iter().map(|t| Value::Str(t.clone())).collect()),
        "spawn_reason" => e.flags.spawn_reason.clone(),
        "tracked_by" => Value::List(e.viewers.iter().filter_map(|v| server.players.get(v)).map(|p| Value::Str(p.uuid().to_string())).collect()),
    };
    // Where it is: in water, lava, rain, a bubble column, powder snow.
    {
        use crate::server::survival::{box_in_fluid, point_in_fluid, Fluid};
        let w = &server.worlds[world];
        let area = e.bounding_box();
        let in_water = box_in_fluid(w, &area, Fluid::Water);
        let (x, y, z) = (e.pos.x.floor() as i32, e.pos.y.floor() as i32, e.pos.z.floor() as i32);
        let at_feet = w.block_at(x, y, z).map_or("minecraft:air", |s| paper_data::blocks::block_of_state(s).name);
        let eye = e.eye_pos();
        out.set("in_water", in_water);
        out.set("under_water", point_in_fluid(w, Vec3::new(eye.x, eye.y - 0.11111111, eye.z), Fluid::Water));
        out.set("in_lava", box_in_fluid(w, &area, Fluid::Lava));
        out.set("in_rain", w.rain_at(x, y, z) || w.rain_at(x, (e.pos.y + e.info.height as f64).ceil() as i32, z));
        out.set("in_bubble_column", at_feet == "minecraft:bubble_column");
        out.set("in_powder_snow", at_feet == "minecraft:powder_snow");
    }
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
            out.set("last_damage", m.living.last_hurt);
            out.set("fall_distance", m.living.fall_distance);
            out.set("ai", !e.flags.no_ai);
            out.set("aware", !e.flags.unaware);
            out.set("left_handed", e.flags.left_handed);
            out.set("can_pick_up_loot", e.flags.can_pick_up_loot);
            // Animals and villagers (`AgeableMob`): how old, love mode, who fed it.
            out.set("ages", crate::world::entity_nbt::ages(&e.info.name));
            out.set("mob_age", m.age);
            out.set("age_locked", e.flags.age_locked);
            out.set("love_ticks", m.in_love);
            out.set("love_cause", m.love_cause.and_then(|p| server.players.get(&p)).map(|p| p.uuid().to_string()));
            out.set("sheared", m.sheared);
            if e.info.name == "minecraft:creeper" {
                out.set("powered", m.powered);
            }
            out.set("variant", m.variant);
            if let Some(registry) = crate::server::variants::registry_of(&e.info.name) {
                out.set("variant_key", crate::server::variants::key_of(registry, m.variant));
            }
            out.set("collar", m.collar);
            out.set("no_action_ticks", m.no_player_ticks);
            out.set("experience_reward", e.info.experience);
            // The player who hurt it in the last five seconds (`lastHurtByPlayer`).
            let killer = m.living.last_attacker.filter(|(_, t)| server.tick.saturating_sub(*t) < 100).and_then(|(a, _)| server.players.get(&a));
            out.set("killer", killer.map(|p| p.uuid().to_string()));
            out.set("hurt_sound", e.info.hurt_sound.clone());
            out.set("death_sound", e.info.death_sound.clone());
            out.set("ambient_sound", e.info.ambient_sound.clone());
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
        EntityKind::Thrown { item, owner, left_owner, .. } => {
            out.set("item", item_value(item));
            out.set("shooter", owner.map(|o| match server.players.get(&o) {
                Some(p) => bmap! { "player" => p.uuid().to_string() },
                None => bmap! { "entity" => o as i64 },
            }));
            out.set("left_shooter", *left_owner);
        }
        EntityKind::Cloud { potion, owner, radius, duration, wait, growth, on_use, .. } => {
            out.set("potion", potion.clone());
            out.set("source", owner.map(|o| match server.players.get(&o) {
                Some(p) => bmap! { "player" => p.uuid().to_string() },
                None => bmap! { "entity" => o as i64 },
            }));
            out.set("radius", *radius as f64);
            out.set("duration", *duration);
            out.set("wait_time", *wait);
            out.set("radius_per_tick", *growth as f64);
            out.set("radius_on_use", *on_use as f64);
        }
        // `Hanging`: the way it faces and its block; a frame's item, turn and settings; a
        // painting's variant (`Art`).
        EntityKind::Hanging(h) => {
            out.set("facing", ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"][h.facing as usize % 6]);
            out.set("hanging_block", Value::List(vec![h.block.x.into(), h.block.y.into(), h.block.z.into()]));
            if crate::world::hanging::is_frame(&e.info.name) {
                out.set("item", item_value(&h.item));
                out.set("rotation", h.rotation as i32);
                out.set("fixed", h.fixed);
                out.set("drop_chance", h.drop_chance as f64);
            } else {
                out.set("art", h.variant.clone());
            }
        }
        // `Firework`: its item, flight, what it is attached to.
        EntityKind::Firework(f) => {
            out.set("item", item_value(&f.item));
            out.set("life", f.life);
            out.set("lifetime", f.lifetime);
            out.set("shot_at_angle", f.shot_at_angle);
            out.set("attached", f.attached.map(|t| match server.players.get(&t) {
                Some(p) => bmap! { "player" => p.uuid().to_string() },
                None => bmap! { "entity" => t as i64 },
            }));
            out.set("shooter", f.owner.map(|o| match server.players.get(&o) {
                Some(p) => bmap! { "player" => p.uuid().to_string() },
                None => bmap! { "entity" => o as i64 },
            }));
            out.set("spawning", f.spawning.map(|u| u.to_string()));
        }
        // `FishHook`: its player, what it hooked, its state and waits.
        EntityKind::Fishing(h) => {
            out.set("shooter", match server.players.get(&h.owner) {
                Some(p) => bmap! { "player" => p.uuid().to_string() },
                None => bmap! { "entity" => h.owner as i64 },
            });
            out.set("hooked", h.hooked.map(|t| match server.players.get(&t) {
                Some(p) => bmap! { "player" => p.uuid().to_string() },
                None => bmap! { "entity" => t as i64 },
            }));
            let state = match h.state {
                crate::world::entity::HookState::Flying => "UNHOOKED",
                crate::world::entity::HookState::HookedInEntity => "HOOKED_ENTITY",
                crate::world::entity::HookState::Bobbing => "BOBBING",
            };
            out.set("hook_state", state);
            out.set("wait_time", h.time_until_lured);
            out.set("time_until_bite", h.time_until_hooked);
            out.set("min_wait", h.min_wait);
            out.set("max_wait", h.max_wait);
            out.set("min_lure", h.min_lure);
            out.set("max_lure", h.max_lure);
            out.set("min_lure_angle", h.min_lure_angle as f64);
            out.set("max_lure_angle", h.max_lure_angle as f64);
            out.set("apply_lure", h.apply_lure);
            out.set("rain_influenced", h.rain_influenced);
            out.set("sky_influenced", h.sky_influenced);
            out.set("open_water", h.open_water);
        }
        // `EnderCrystal`: its base and where its beam points (absent for none).
        EntityKind::Crystal { beam_target, show_bottom } => {
            out.set("show_bottom", *show_bottom);
            if let Some(p) = beam_target {
                out.set("beam_target", bmap! { "x" => p.x, "y" => p.y, "z" => p.z });
            }
        }
        EntityKind::Other if e.info.name == "minecraft:lightning_bolt" => out.set("effect", e.flags.visual_only),
        EntityKind::EyeOfEnder { .. } | EntityKind::Boat { .. } | EntityKind::Other => {}
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

pub(super) fn vec_arg(args: &Value, x: &str, y: &str, z: &str) -> Option<Vec3> {
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
            let found: Vec<&crate::world::entity::Entity> = ids
                .iter()
                .filter_map(|id| w.entities.get(id))
                .filter(|e| !e.removed)
                .filter(|e| types.as_ref().is_none_or(|t| t.contains(&e.info.name.as_str())))
                .filter(|e| area.as_ref().is_none_or(|a| e.bounding_box().intersects(a)))
                .collect();
            // Each state takes a while to work out (where the entity is, what it carries):
            // many are worked out in parallel.
            let list = if found.len() >= 16 {
                server.tick_pool.install(|| {
                    use rayon::prelude::*;
                    found.par_iter().map(|e| entity_value(server, world, e)).collect()
                })
            } else {
                found.iter().map(|e| entity_value(server, world, e)).collect()
            };
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
                "minecraft:firework_rocket" => {
                    // An empty rocket, as `CraftRegionAccessor` makes one, launched as any
                    // projectile (`ProjectileLaunchEvent`).
                    let e = server.new_firework(pos, paper_protocol::types::Slot::EMPTY, None).ok_or_else(|| anyhow!("cannot spawn {ty}"))?;
                    let id = server.spawn_entity(world, e);
                    if !server.launch(world, id) {
                        bail!("the entity was not spawned");
                    }
                    id
                }
                name if crate::world::hanging::is_hanging(name) => {
                    // Turned the way it hangs, whatever the location's yaw; plugins hear of it
                    // as of any new entity (`EntitySpawnEvent`).
                    let id = server.spawn_hanging(world, name, pos).ok_or_else(|| anyhow!("cannot spawn {ty}"))?;
                    if !server.keep_spawned(world, id) {
                        bail!("the entity was not spawned");
                    }
                    let e = &server.worlds[world].entities[&id];
                    return Ok(entity_value(server, world, e));
                }
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
        "entity.pull_hooked" => {
            let (world, id) = entity_arg(server, args)?;
            Ok(bmap! { "pulled" => server.pull_hooked(world, id) })
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
            // A chest boat spills its chest, as `discard` does.
            server.spill_entity_chest(world, id);
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
                    Ok(Value::Bool(server.add_effect(world, id, effect, "PLUGIN")))
                }
                "entity.remove_effect" => {
                    let effect = paper_data::effects::by_name(effect_name()?).ok_or_else(|| anyhow!("unknown effect"))?;
                    Ok(Value::Bool(server.remove_effect(world, id, effect.id, "PLUGIN")))
                }
                "entity.clear_effects" => Ok(Value::Int(server.clear_effects(world, id, "PLUGIN") as i64)),
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
        // `ProjectileSource.launchProjectile`: a player or mob shoots a projectile the way it
        // looks, as vanilla's items do (`CraftLivingEntity.launchProjectile`).
        "entity.launch" => {
            let shooter = target_id(server, args.get("shooter")).ok_or_else(|| anyhow!("no such shooter"))?;
            let kind = args.str("type").ok_or_else(|| anyhow!("no type"))?;
            let (world, eye, yaw, pitch, player) = match server.players.get(&shooter) {
                Some(p) => (p.world, Vec3::new(p.loc.x, p.loc.y + p.eye_height(), p.loc.z), p.loc.yaw, p.loc.pitch, true),
                None => {
                    let world = world_of(server, shooter).ok_or_else(|| anyhow!("no such shooter"))?;
                    let e = &server.worlds[world].entities[&shooter];
                    (world, e.eye_pos(), e.yaw, e.pitch, false)
                }
            };
            let look = |pitch_offset: f32| {
                let (y, p) = ((yaw as f64).to_radians(), ((pitch + pitch_offset) as f64).to_radians());
                Vec3::new(-y.sin() * (pitch as f64).to_radians().cos(), -p.sin(), y.cos() * (pitch as f64).to_radians().cos())
            };
            let id = match kind {
                "minecraft:arrow" | "minecraft:spectral_arrow" => Some(server.spawn_arrow(world, Vec3::new(eye.x, eye.y - 0.1, eye.z), yaw, pitch, 3.0, 1.0, Some(shooter), 2.0, false, player)),
                // A fireball starts slowly from the eyes and speeds up (`AbstractHurtingProjectile`).
                "minecraft:fireball" | "minecraft:small_fireball" | "minecraft:dragon_fireball" => server.spawn_thrown(world, kind, eye, look(0.0), 0.1, 0.0, 0.0, paper_protocol::types::Slot::EMPTY, Some(shooter)),
                _ => {
                    let (entity, pitch_offset, speed, gravity, _) = crate::server::throwing::throwable(kind).filter(|(e, ..)| *e == kind).ok_or_else(|| anyhow!("cannot launch {kind} yet"))?;
                    let item = paper_data::items::id(kind).map_or(paper_protocol::types::Slot::EMPTY, |i| paper_protocol::types::Slot::of(i as i32, 1));
                    server.spawn_thrown(world, entity, Vec3::new(eye.x, eye.y - 0.1, eye.z), look(pitch_offset), speed, 1.0, gravity, item, Some(shooter))
                }
            }
            .ok_or_else(|| anyhow!("could not launch {kind}"))?;
            if let Some(v) = vec_arg(args, "vx", "vy", "vz") {
                let e = server.worlds[world].entities.get_mut(&id).unwrap();
                e.vel = v;
                e.force_sync = true;
            }
            Ok(entity_value(server, world, &server.worlds[world].entities[&id]))
        }
        // `Entity.playEffect(EntityEffect)`: an entity event its viewers see.
        "entity.event" => {
            let (world, id) = entity_arg(server, args)?;
            let status = args.i32("status").ok_or_else(|| anyhow!("no status"))? as i8;
            let packet = paper_protocol::packets::play::EntityEvent { entity_id: id, status };
            server.broadcast_entity(world, id, bytes::Bytes::from(paper_protocol::packets::encode_packet(&packet)));
            Ok(Value::Null)
        }
        // `LivingEntity.swingMainHand`/`swingOffHand`.
        "entity.swing" => {
            let (world, id) = entity_arg(server, args)?;
            let packet = paper_protocol::packets::play::SwingAnimation { entity_id: paper_protocol::codec::VarInt(id), hand: paper_protocol::codec::VarInt(args.i32("hand").unwrap_or(0)), animation_type: paper_protocol::codec::VarInt(1), duration: paper_protocol::codec::VarInt(6) };
            server.broadcast_entity(world, id, bytes::Bytes::from(paper_protocol::packets::encode_packet(&packet)));
            Ok(Value::Null)
        }
        // `Shearable.shear` (sheep).
        "entity.shear" => {
            let (world, id) = entity_arg(server, args)?;
            if server.worlds[world].entities[&id].info.name != "minecraft:sheep" {
                bail!("only sheep can be sheared yet");
            }
            server.shear_sheep(world, id);
            Ok(entity_value(server, world, &server.worlds[world].entities[&id]))
        }
        // `Animals.isBreedItem`.
        "entity.breed_item" => {
            let (world, id) = entity_arg(server, args)?;
            let item = args.str("item").and_then(paper_data::items::id).ok_or_else(|| anyhow!("unknown item {:?}", args.get("item")))?;
            Ok(Value::Bool(crate::server::animals::is_food(&server.worlds[world].entities[&id].info.name, item as i32)))
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
            // `Projectile.setShooter` (an empty map for none).
            let shooter = target_id(server, args.get("shooter"));
            // `AreaEffectCloud.setSource` (an empty map for none).
            let source = target_id(server, args.get("source"));
            // `FishHook.setHookedEntity` (an empty map for none).
            let hooked = target_id(server, args.get("hooked"));
            // `Firework.setAttachedTo` (an empty map for none).
            let attached = target_id(server, args.get("attached"));
            // Players by uuid, for `Animals.setBreedCause`.
            let server_players: Vec<(i32, String)> = if args.get("love_cause").is_null() { Vec::new() } else { server.players.values().map(|p| (p.entity_id, p.uuid().to_string())).collect() };
            // `Painting.setArt` and `Hanging.setFacingDirection`: only where it can then still
            // hang, unless forced (the host reads whether it took from the state).
            if let Some(EntityKind::Hanging(h)) = server.worlds[world].entities.get(&id).map(|e| &e.kind) {
                let mut changed = (**h).clone();
                if let Some(art) = args.str("art") {
                    changed.variant = art.to_owned();
                }
                if let Some(face) = args.str("facing") {
                    if let Some(f) = ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"].iter().position(|n| *n == face) {
                        changed.facing = f as u8;
                    }
                }
                let name = server.worlds[world].entities[&id].info.name.clone();
                if (changed.variant != h.variant || changed.facing != h.facing) && (args.bool("force") == Some(true) || changed.survives(&server.worlds[world], &name, id)) {
                    let bb = changed.bounding_box(&name);
                    let e = server.worlds[world].entities.get_mut(&id).unwrap();
                    e.pos = Vec3::new((bb.min.x + bb.max.x) / 2.0, (bb.min.y + bb.max.y) / 2.0, (bb.min.z + bb.max.z) / 2.0);
                    (e.yaw, e.pitch) = crate::world::hanging::facing_rotation(changed.facing);
                    e.head_yaw = e.yaw;
                    e.kind = EntityKind::Hanging(Box::new(changed));
                    e.metadata_dirty = true;
                    server.worlds[world].reindex_entity(id);
                }
            }
            let frame_item = (!args.get("item").is_null() && server.worlds[world].entities.get(&id).is_some_and(|e| matches!(e.kind, EntityKind::Hanging(_)))).then(|| slot_from(args.get("item"))).transpose()?;
            if let Some(item) = frame_item {
                // `ItemFrame.setItem(item, playSound)`.
                server.set_frame_item(world, id, item, args.bool("sound") != Some(false));
            }
            // `setHealth(0)` kills (`CraftLivingEntity.setHealth` runs `die` with generic damage).
            let mut dies = false;
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
            // Flags any entity has; those clients see are sent again.
            let f = &mut e.flags;
            let shown = [("glowing", &mut f.glowing), ("silent", &mut f.silent), ("invisible", &mut f.invisible), ("visual_fire", &mut f.visual_fire), ("left_handed", &mut f.left_handed)]
                .into_iter()
                .fold(false, |changed, (key, flag)| match args.bool(key) {
                    Some(v) => {
                        *flag = v;
                        true
                    }
                    None => changed,
                });
            for (key, flag) in [("invulnerable", &mut f.invulnerable), ("can_pick_up_loot", &mut f.can_pick_up_loot), ("age_locked", &mut f.age_locked)] {
                if let Some(v) = args.bool(key) {
                    *flag = v;
                }
            }
            if let Some(ai) = args.bool("ai") {
                f.no_ai = !ai;
            }
            if let Some(aware) = args.bool("aware") {
                f.unaware = !aware;
            }
            if let Some(t) = args.i32("portal_cooldown") {
                f.portal_cooldown = t.max(0);
            }
            if let Some(t) = args.i32("freeze_ticks") {
                f.freeze_ticks = t.max(0);
            }
            if let Some(tag) = args.str("add_tag") {
                if f.tags.len() < 1024 && !f.tags.iter().any(|t| t == tag) {
                    f.tags.push(tag.to_owned());
                }
            }
            if let Some(tag) = args.str("remove_tag") {
                f.tags.retain(|t| t != tag);
            }
            if shown || args.bool("ai").is_some() || args.i32("freeze_ticks").is_some() {
                e.metadata_dirty = true;
            }
            if let Some(t) = args.i32("ticks_lived") {
                e.age = t.max(0);
            }
            if let Some(m) = e.mob_mut() {
                if let Some(t) = args.i32("no_action_ticks") {
                    m.no_player_ticks = t.max(0);
                }
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
                        // Not again while dying; the dragon dies its own way when out of health.
                        dies = h <= 0.0 && !m.living.is_dead() && m.living.death_time == 0 && e.info.name != crate::server::dragon::DRAGON;
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
                    // An animal is a baby while its age is below zero (`AgeableMob.setAge`);
                    // other mobs (zombies, piglins) just are one.
                    let ages = crate::world::entity_nbt::ages(&e.info.name);
                    if let Some(b) = args.bool("baby") {
                        if ages && b && m.age >= 0 {
                            m.age = -24000;
                        } else if ages && !b && m.age < 0 {
                            m.age = 0;
                        }
                        m.baby = b;
                        e.metadata_dirty = true;
                    }
                    if let Some(age) = args.i32("mob_age") {
                        if ages {
                            m.age = age;
                        }
                        if m.baby != (age < 0) {
                            m.baby = age < 0;
                            e.metadata_dirty = true;
                        }
                    }
                    if let Some(t) = args.i32("love_ticks") {
                        m.in_love = t.max(0);
                    }
                    match args.get("love_cause") {
                        Value::Str(uuid) => m.love_cause = server_players.iter().find(|(_, u)| u == uuid).map(|(id, _)| *id),
                        Value::Null => {}
                        _ => m.love_cause = None,
                    }
                    if let Some(d) = args.f64("fall_distance") {
                        m.living.fall_distance = d;
                    }
                    if let Some(d) = args.f64("last_damage") {
                        m.living.last_hurt = d as f32;
                    }
                    if e.info.name == "minecraft:sheep" {
                        if let Some(sheared) = args.bool("sheared") {
                            m.sheared = sheared;
                            e.metadata_dirty = true;
                        }
                    }
                    // `Creeper.setPowered`.
                    if e.info.name == "minecraft:creeper" {
                        if let Some(powered) = args.bool("powered") {
                            m.powered = powered;
                            e.metadata_dirty = true;
                        }
                    }
                    // A sheep's colour, a horse's coat and markings, a llama's colour, a
                    // rabbit's type, ... (as numbers the host checks), or an entry of the
                    // mob's variant registry (cats, cows, ...).
                    if let Some(v) = args.i32("variant") {
                        m.variant = if e.info.name == "minecraft:sheep" { v.clamp(0, 15) } else { v };
                        e.metadata_dirty = true;
                    }
                    if let Some(key) = args.str("variant_key") {
                        let registry = crate::server::variants::registry_of(&e.info.name).ok_or_else(|| anyhow!("{} has no variant of a registry", e.info.name))?;
                        m.variant = crate::server::variants::id_of(registry, key).ok_or_else(|| anyhow!("unknown {registry} entry {key}"))?;
                        e.metadata_dirty = true;
                    }
                    if let Some(c) = args.i32("collar") {
                        m.collar = c.clamp(0, 15);
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
                EntityKind::Arrow { damage, pickup, critical, owner, .. } => {
                    if !args.get("shooter").is_null() {
                        *owner = shooter;
                    }
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
                EntityKind::Thrown { item, owner, left_owner, .. } => {
                    // `Projectile.setShooter`, `setHasLeftShooter`, `ThrowableProjectile.setItem`.
                    if !args.get("shooter").is_null() {
                        *owner = shooter;
                    }
                    if let Some(left) = args.bool("left_shooter") {
                        *left_owner = left;
                    }
                    if !args.get("item").is_null() {
                        *item = slot_from(args.get("item"))?;
                        e.metadata_dirty = true;
                    }
                }
                EntityKind::ExperienceOrb { value } => {
                    if let Some(v) = args.i32("experience") {
                        *value = v.max(0);
                        e.metadata_dirty = true;
                    }
                }
                EntityKind::Firework(f) => {
                    // `Firework`'s setters: its item, flight and attachment; `detonate` ends
                    // its flight now.
                    if !args.get("item").is_null() {
                        f.item = slot_from(args.get("item"))?;
                        e.metadata_dirty = true;
                    }
                    if let Some(v) = args.i32("life") {
                        f.life = v.max(0);
                    }
                    if let Some(v) = args.i32("lifetime") {
                        f.lifetime = v.max(0);
                    }
                    if args.bool("detonate") == Some(true) {
                        f.lifetime = 0;
                    }
                    if let Some(v) = args.bool("shot_at_angle") {
                        f.shot_at_angle = v;
                        e.metadata_dirty = true;
                    }
                    if !args.get("attached").is_null() {
                        f.attached = attached;
                        e.metadata_dirty = true;
                    }
                }
                EntityKind::Fishing(h) => {
                    // `FishHook`'s setters: waits, lure times and angles, influences, and what it
                    // hooked (an empty map for nothing).
                    let int = |key: &str| args.i32(key);
                    if let Some(v) = int("wait_time") {
                        h.time_until_lured = v.max(0);
                    }
                    if let Some(v) = int("time_until_bite") {
                        h.time_until_hooked = v.max(0);
                    }
                    for (key, field) in [("min_wait", &mut h.min_wait), ("max_wait", &mut h.max_wait), ("min_lure", &mut h.min_lure), ("max_lure", &mut h.max_lure)] {
                        if let Some(v) = int(key) {
                            *field = v.max(0);
                        }
                    }
                    if let Some(v) = args.f64("min_lure_angle") {
                        h.min_lure_angle = v as f32;
                    }
                    if let Some(v) = args.f64("max_lure_angle") {
                        h.max_lure_angle = v as f32;
                    }
                    for (key, field) in [("apply_lure", &mut h.apply_lure), ("rain_influenced", &mut h.rain_influenced), ("sky_influenced", &mut h.sky_influenced)] {
                        if let Some(v) = args.bool(key) {
                            *field = v;
                        }
                    }
                    if args.bool("reset_fishing") == Some(true) {
                        // `resetFishingState`: back to waiting for a fish.
                        h.nibble = 0;
                        h.time_until_lured = 0;
                        h.time_until_hooked = 0;
                        h.biting = false;
                        e.metadata_dirty = true;
                    }
                    if !args.get("hooked").is_null() {
                        h.hooked = hooked;
                        h.state = if h.hooked.is_some() { crate::world::entity::HookState::HookedInEntity } else { crate::world::entity::HookState::Flying };
                        e.metadata_dirty = true;
                    }
                }
                // `EnderCrystal.setShowingBottom`, `setBeamTarget` (an empty map for none).
                EntityKind::Crystal { beam_target, show_bottom } => {
                    if let Some(v) = args.bool("show_bottom") {
                        *show_bottom = v;
                        e.metadata_dirty = true;
                    }
                    let target = args.get("beam_target");
                    if !target.is_null() {
                        *beam_target = match (target.i32("x"), target.i32("y"), target.i32("z")) {
                            (Some(x), Some(y), Some(z)) => Some(paper_protocol::codec::BlockPos::new(x, y, z)),
                            _ => None,
                        };
                        e.metadata_dirty = true;
                    }
                }
                EntityKind::Hanging(h) => {
                    // `ItemFrame.setRotation`, `setFixed`, `setItemDropChance`.
                    if let Some(r) = args.i32("rotation") {
                        h.rotation = r.rem_euclid(8) as u8;
                        e.metadata_dirty = true;
                    }
                    if let Some(fixed) = args.bool("fixed") {
                        h.fixed = fixed;
                    }
                    if let Some(chance) = args.f64("drop_chance") {
                        h.drop_chance = chance as f32;
                    }
                }
                EntityKind::Tnt { fuse } => {
                    if let Some(f) = args.i32("fuse") {
                        *fuse = f.max(0);
                        e.metadata_dirty = true;
                    }
                }
                EntityKind::Cloud { owner, radius, duration, wait, growth, on_use, .. } => {
                    // `AreaEffectCloud.setSource`, `setRadius`, `setDuration`, `setWaitTime`,
                    // `setRadiusPerTick`, `setRadiusOnUse`.
                    if !args.get("source").is_null() {
                        *owner = source;
                    }
                    if let Some(r) = args.f64("radius") {
                        *radius = r.max(0.0) as f32;
                        e.metadata_dirty = true;
                    }
                    if let Some(d) = args.i32("duration") {
                        *duration = d;
                    }
                    if let Some(w) = args.i32("wait_time") {
                        *wait = w.max(0);
                        e.metadata_dirty = true;
                    }
                    if let Some(g) = args.f64("radius_per_tick") {
                        *growth = g as f32;
                    }
                    if let Some(u) = args.f64("radius_on_use") {
                        *on_use = u as f32;
                    }
                }
                _ => {}
            }
            if dies {
                server.kill_mob(world, id, crate::server::survival::Damage::new("minecraft:generic"));
            }
            match server.worlds[world].entities.get(&id) {
                Some(e) => Ok(entity_value(server, world, e)),
                None => Ok(Value::Null),
            }
        }
        other => bail!("unknown plugin API method {other}"),
    }
}
