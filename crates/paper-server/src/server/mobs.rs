//! Mob behaviour: AI, pathfinding, movement, attacks, despawning and natural spawning.
//!
//! The behaviours are simplified versions of vanilla's goals: animals wander, look at
//! players and panic when hurt; zombies and spiders chase and hit players; skeletons keep
//! their distance and shoot; creepers walk up and explode; undead burn in daylight. Movement
//! uses vanilla's walking physics (`LivingEntity.travel`), and spawning follows
//! `NaturalSpawner` with the biomes' spawner lists from the vanilla data.

use super::survival::{self, category, Damage, Fluid};
use super::Server;
use crate::config::{Difficulty, GameMode};
use crate::world::chunk::ChunkPos;
use crate::world::entity::{collides, friction_below, Aabb, Entity, EntityKind, LivingState, Mob, MobBrain, Vec3};
use crate::world::World;
use bytes::Bytes;
use paper_data::{blocks, entity_types, items};
use paper_protocol::codec::VarInt;
use paper_protocol::packets::{encode_packet, play};
use paper_protocol::types::Slot;
use rand::Rng;
use std::collections::{BinaryHeap, HashMap};

/// How a mob behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behavior {
    /// Wanders, looks at players, panics when hurt (cows, pigs, sheep, chickens...).
    Animal,
    /// Chases the nearest player and hits it (zombies, husks, spiders...).
    Melee,
    /// Keeps its distance and shoots arrows (skeletons, strays, bogged).
    Archer,
    /// Walks up to players and explodes.
    Creeper,
    /// Wanders; fights back only when hurt (endermen, wolves, piglins...).
    Neutral,
    /// Floats about in water.
    Swimmer,
    /// Flies about.
    Flyer,
    /// Stays where it is (armor stands, villagers without work...).
    Still,
}

pub fn behavior(name: &str, category: &str) -> Behavior {
    match name {
        "minecraft:zombie" | "minecraft:husk" | "minecraft:drowned" | "minecraft:zombie_villager" | "minecraft:spider" | "minecraft:cave_spider" | "minecraft:silverfish" | "minecraft:endermite" | "minecraft:vindicator" | "minecraft:zoglin" | "minecraft:hoglin" | "minecraft:piglin_brute" | "minecraft:ravager" | "minecraft:warden" => Behavior::Melee,
        "minecraft:skeleton" | "minecraft:stray" | "minecraft:bogged" | "minecraft:pillager" => Behavior::Archer,
        "minecraft:creeper" => Behavior::Creeper,
        "minecraft:enderman" | "minecraft:zombified_piglin" | "minecraft:wolf" | "minecraft:piglin" | "minecraft:iron_golem" | "minecraft:llama" | "minecraft:polar_bear" | "minecraft:panda" | "minecraft:bee" | "minecraft:goat" => Behavior::Neutral,
        "minecraft:bat" | "minecraft:ghast" | "minecraft:phantom" | "minecraft:parrot" | "minecraft:allay" | "minecraft:vex" | "minecraft:blaze" => Behavior::Flyer,
        "minecraft:armor_stand" | "minecraft:mannequin" | "minecraft:shulker" => Behavior::Still,
        _ => match category {
            "MONSTER" => Behavior::Melee,
            "WATER_CREATURE" | "WATER_AMBIENT" | "UNDERGROUND_WATER_CREATURE" | "AXOLOTLS" => Behavior::Swimmer,
            "AMBIENT" => Behavior::Flyer,
            _ => Behavior::Animal,
        },
    }
}

fn burns_in_daylight(name: &str) -> bool {
    matches!(name, "minecraft:zombie" | "minecraft:zombie_villager" | "minecraft:drowned" | "minecraft:skeleton" | "minecraft:stray" | "minecraft:bogged" | "minecraft:phantom")
}

fn is_fall_immune(name: &str) -> bool {
    let Some(info) = entity_types::by_name(name) else { return false };
    paper_data::synced::tag("minecraft:entity_type", "minecraft:fall_damage_immune").is_some_and(|ids| ids.contains(&info.id))
}

/// Why a mob is being spawned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnReason {
    Natural,
    ChunkGeneration,
    Command,
    Plugin,
    Egg,
}

// --- light and sky ---------------------------------------------------------------------------

/// How much the sky light is dimmed at the world's time of day (`Level.skyDarken`, clear
/// weather).
pub fn sky_darken(world: &World) -> i32 {
    if !world.has_skylight {
        return 0;
    }
    let d = ((world.day_time.rem_euclid(24000)) as f64 / 24000.0 - 0.25).rem_euclid(1.0);
    let e = 0.5 - (d * std::f64::consts::PI).cos() / 2.0;
    let time = (d * 2.0 + e) / 3.0;
    let f = 0.5 + 2.0 * (time * std::f64::consts::TAU).cos().clamp(-0.25, 0.25);
    ((1.0 - f) * 11.0) as i32
}

fn sky_light(world: &World, x: i32, y: i32, z: i32) -> i32 {
    world.chunks.get(&ChunkPos::of_block(x, z)).filter(|c| c.lit).map_or(15, |c| c.sky_light_at((x & 15) as usize, y, (z & 15) as usize) as i32)
}

fn block_light(world: &World, x: i32, y: i32, z: i32) -> i32 {
    world.chunks.get(&ChunkPos::of_block(x, z)).filter(|c| c.lit).map_or(0, |c| c.block_light_at((x & 15) as usize, y, (z & 15) as usize) as i32)
}

/// `getMaxLocalRawBrightness`: sky light dimmed by the time of day, or block light.
pub fn raw_brightness(world: &World, x: i32, y: i32, z: i32) -> i32 {
    (sky_light(world, x, y, z) - sky_darken(world)).max(block_light(world, x, y, z))
}

// --- pathfinding -----------------------------------------------------------------------------

fn passable(world: &World, x: i32, y: i32, z: i32) -> bool {
    let Some(state) = world.block_at(x, y, z) else { return false };
    if survival::fluid_of(state).is_some_and(|(f, _)| f == Fluid::Lava) {
        return false;
    }
    let name = blocks::block_of_state(state).name;
    if matches!(name, "minecraft:fire" | "minecraft:soul_fire" | "minecraft:cactus" | "minecraft:sweet_berry_bush" | "minecraft:powder_snow" | "minecraft:cobweb" | "minecraft:magma_block") {
        return false;
    }
    paper_data::physics::collision_boxes(state).iter().all(|b| b.max[1] <= 0.0 || b.min[1] >= 1.0)
}

fn standable(world: &World, x: i32, y: i32, z: i32) -> bool {
    let Some(state) = world.block_at(x, y - 1, z) else { return false };
    if survival::fluid_of(state).is_some() && paper_data::physics::collision_boxes(state).is_empty() {
        // Swimming across water is allowed.
        return survival::fluid_of(state).is_some_and(|(f, _)| f == Fluid::Water);
    }
    let boxes = paper_data::physics::collision_boxes(state);
    !boxes.is_empty() && boxes.iter().any(|b| b.max[1] >= 0.5) && !boxes.iter().any(|b| b.max[1] > 1.0)
        && !matches!(blocks::block_of_state(state).name, "minecraft:magma_block" | "minecraft:cactus" | "minecraft:campfire" | "minecraft:soul_campfire")
}

fn walkable(world: &World, x: i32, y: i32, z: i32, height: i32) -> bool {
    (0..height).all(|dy| passable(world, x, y + dy, z)) && standable(world, x, y, z)
}

#[derive(PartialEq)]
struct Open {
    cost: f64,
    node: (i32, i32, i32),
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.cost.total_cmp(&self.cost)
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A* over walkable blocks: steps, one-block climbs, drops of up to three blocks.
pub fn find_path(world: &World, start: (i32, i32, i32), goal: (i32, i32, i32), height: i32, max_nodes: usize) -> Option<Vec<(i32, i32, i32)>> {
    let h = |n: (i32, i32, i32)| {
        let (dx, dy, dz) = ((n.0 - goal.0) as f64, (n.1 - goal.1) as f64, (n.2 - goal.2) as f64);
        (dx * dx + dy * dy + dz * dz).sqrt()
    };
    let mut open = BinaryHeap::new();
    let mut came: HashMap<(i32, i32, i32), (i32, i32, i32)> = HashMap::new();
    let mut best: HashMap<(i32, i32, i32), f64> = HashMap::new();
    best.insert(start, 0.0);
    open.push(Open { cost: h(start), node: start });
    let mut closest = (h(start), start);
    let mut visited = 0;
    while let Some(Open { node, .. }) = open.pop() {
        if node == goal {
            closest = (0.0, node);
            break;
        }
        visited += 1;
        if visited > max_nodes {
            break;
        }
        let g = best[&node];
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let (nx, nz) = (node.0 + dx, node.2 + dz);
            if dx != 0 && dz != 0 && !(walkable(world, node.0 + dx, node.1, node.2, height) && walkable(world, node.0, node.1, node.2 + dz, height)) {
                continue;
            }
            let mut next = None;
            if walkable(world, nx, node.1, nz, height) {
                next = Some((nx, node.1, nz));
            } else if passable(world, node.0, node.1 + height, node.2) && walkable(world, nx, node.1 + 1, nz, height) {
                next = Some((nx, node.1 + 1, nz));
            } else if (0..height).all(|dy| passable(world, nx, node.1 + dy, nz)) {
                for drop in 1..=3 {
                    if !passable(world, nx, node.1 - drop, nz) {
                        break;
                    }
                    if walkable(world, nx, node.1 - drop, nz, height) {
                        next = Some((nx, node.1 - drop, nz));
                        break;
                    }
                }
            }
            let Some(n) = next else { continue };
            let step = if dx != 0 && dz != 0 { 1.414 } else { 1.0 } + if n.1 != node.1 { 0.5 } else { 0.0 };
            let cost = g + step;
            if best.get(&n).is_none_or(|b| cost < *b) {
                best.insert(n, cost);
                came.insert(n, node);
                let hn = h(n);
                if hn < closest.0 {
                    closest = (hn, n);
                }
                open.push(Open { cost: cost + hn, node: n });
            }
        }
    }
    // Walk back from the goal, or from the nearest point reached.
    let mut path = vec![closest.1];
    let mut cur = closest.1;
    while let Some(prev) = came.get(&cur) {
        if *prev == start {
            break;
        }
        path.push(*prev);
        cur = *prev;
    }
    path.reverse();
    (closest.1 != start).then_some(path)
}

// --- spawning data ---------------------------------------------------------------------------

/// One `spawners` entry of a biome.
#[derive(Clone, Debug)]
pub struct SpawnerEntry {
    pub entity: String,
    pub weight: i32,
    pub min: i32,
    pub max: i32,
}

/// Mob caps per category (`MobCategory`): (category, cap, friendly/persistent).
const CATEGORIES: [(&str, &str, i32, bool); 4] = [("MONSTER", "monster", 70, false), ("CREATURE", "creature", 10, true), ("AMBIENT", "ambient", 15, false), ("WATER_AMBIENT", "water_ambient", 20, false)];

impl Server {
    /// The spawner list of a biome for a category (`monster`, `creature`, ...), read from the
    /// vanilla data and cached.
    pub fn biome_spawners(&mut self, biome: &str, category: &str) -> Vec<SpawnerEntry> {
        let key = format!("{biome}|{category}");
        if let Some(list) = self.spawner_cache.get(&key) {
            return list.clone();
        }
        let path = format!("data/minecraft/worldgen/biome/{}.json", biome.strip_prefix("minecraft:").unwrap_or(biome));
        let list: Vec<SpawnerEntry> = self
            .vanilla
            .as_ref()
            .and_then(|v| v.read_json(&path))
            .and_then(|json| json.get("spawners").and_then(|s| s.get(category)).and_then(|l| l.as_array()).cloned())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| {
                        Some(SpawnerEntry {
                            entity: e["type"].as_str()?.to_owned(),
                            weight: e["weight"].as_i64().unwrap_or(1) as i32,
                            min: e["minCount"].as_i64().unwrap_or(1) as i32,
                            max: e["maxCount"].as_i64().unwrap_or(1) as i32,
                        })
                    })
                    .collect()
            })
            .unwrap_or_else(|| default_spawners(category));
        self.spawner_cache.insert(key, list.clone());
        list
    }

    /// The biome at a block position.
    pub fn biome_at(&self, world: usize, x: i32, y: i32, z: i32) -> &'static str {
        let w = &self.worlds[world];
        let Some(chunk) = w.chunks.get(&ChunkPos::of_block(x, z)) else { return "minecraft:plains" };
        let rel = (y - w.min_y).clamp(0, w.height - 1);
        let Some(section) = chunk.sections.get((rel >> 4) as usize) else { return "minecraft:plains" };
        let id = section.biome(((x & 15) >> 2) as usize, ((rel & 15) >> 2) as usize, ((z & 15) >> 2) as usize);
        paper_data::synced::entries("minecraft:worldgen/biome").and_then(|e| e.get(id as usize).copied()).unwrap_or("minecraft:plains")
    }

    // --- creating mobs -----------------------------------------------------------------------

    /// Spawns a mob (or any entity type) at a position. Returns its id.
    pub fn spawn_mob(&mut self, world: usize, name: &str, pos: Vec3, reason: SpawnReason) -> Option<i32> {
        let name = if name.contains(':') { name.to_owned() } else { format!("minecraft:{name}") };
        let info = entity_types::by_name(&name)?;
        if !info.mob {
            let kind = match name.as_str() {
                "minecraft:tnt" => EntityKind::Tnt { fuse: 80 },
                "minecraft:experience_orb" => EntityKind::ExperienceOrb { value: 1 },
                _ => EntityKind::Other,
            };
            let e = Entity::new(self.next_entity_id(), info, pos, kind);
            return Some(self.spawn_entity(world, e));
        }
        let mut rng = rand::thread_rng();
        let max_health = info.attribute("max_health").map_or(info.max_health, |h| h as f32).max(1.0);
        let mut mob = Mob {
            living: LivingState::new(max_health),
            equipment: Default::default(),
            brain: MobBrain { speed_factor: 1.0, idle_ticks: rng.gen_range(0..120), ..Default::default() },
            persistent: matches!(reason, SpawnReason::Command | SpawnReason::Plugin | SpawnReason::Egg),
            baby: false,
            variant: 0,
            sheared: false,
            no_player_ticks: 0,
        };
        // Equipment and variants picked when spawning (`finalizeSpawn`).
        match name.as_str() {
            "minecraft:skeleton" | "minecraft:stray" | "minecraft:bogged" => {
                if let Some(bow) = items::id("minecraft:bow") {
                    mob.equipment[0] = Slot::of(bow as i32, 1);
                }
            }
            "minecraft:zombie" | "minecraft:husk" | "minecraft:zombie_villager" | "minecraft:drowned" => {
                mob.baby = rng.gen::<f32>() < 0.05;
            }
            "minecraft:sheep" => {
                // White 81.8%, light grey, grey and black 5% each, brown 3%, pink 0.2%.
                let r = rng.gen_range(0..1000);
                mob.variant = match r {
                    0..50 => 15,
                    50..100 => 7,
                    100..150 => 8,
                    150..180 => 12,
                    180..182 => 6,
                    _ => 0,
                };
            }
            _ => {}
        }
        if reason == SpawnReason::ChunkGeneration || reason == SpawnReason::Natural {
            if matches!(name.as_str(), "minecraft:cow" | "minecraft:pig" | "minecraft:sheep" | "minecraft:chicken" | "minecraft:rabbit" | "minecraft:horse" | "minecraft:donkey" | "minecraft:llama" | "minecraft:goat" | "minecraft:camel") {
                mob.baby = rng.gen::<f32>() < 0.05;
            }
        }
        let mut e = Entity::new(self.next_entity_id(), info, pos, EntityKind::Mob(Box::new(mob)));
        e.yaw = rng.gen_range(0.0..360.0);
        e.head_yaw = e.yaw;
        let id = self.spawn_entity(world, e);
        crate::plugins::events::on_creature_spawn(self, world, id, reason);
        Some(id)
    }

    // --- per tick ------------------------------------------------------------------------------

    pub(super) fn tick_mob(&mut self, world: usize, id: i32) {
        if self.tick_dying(world, id) {
            return;
        }
        if self.mob_environment(world, id) {
            return;
        }
        self.mob_ai(world, id);
        self.mob_travel(world, id);
        self.mob_sounds(world, id);
    }

    /// Fire, lava, drowning, the void, sunlight and suffocation. Returns true if the mob died.
    fn mob_environment(&mut self, world: usize, id: i32) -> bool {
        let w = &self.worlds[world];
        let Some(e) = w.entities.get(&id) else { return true };
        let (info, pos, area) = (e.info, e.pos, e.bounding_box());
        let name = info.name.as_str();
        let in_water = survival::box_in_fluid(w, &area, Fluid::Water);
        let in_lava = survival::box_in_fluid(w, &area, Fluid::Lava);
        let eye = e.eye_pos();
        let eye_in_water = survival::point_in_fluid(w, Vec3::new(eye.x, eye.y - 0.11111111, eye.z), Fluid::Water);
        let suffocating = survival::in_wall(w, eye, info.width as f64);
        let below_world = pos.y < (w.min_y - 64) as f64;
        let breathes_water = matches!(behavior(name, &info.category), Behavior::Swimmer) || matches!(name, "minecraft:drowned" | "minecraft:axolotl" | "minecraft:turtle" | "minecraft:frog") || name.ends_with("skeleton") || name.ends_with("_golem") || name == "minecraft:stray" || name == "minecraft:bogged" || name == "minecraft:wither";
        // Daylight: undead without a helmet catch fire under the open sky.
        let sunburn = burns_in_daylight(name) && !in_water && {
            let (x, y, z) = (eye.x.floor() as i32, eye.y.floor() as i32, eye.z.floor() as i32);
            sky_darken(w) < 4 && w.has_skylight && sky_light(w, x, y, z) >= 15 && raw_brightness(w, x, y, z) > 11 && rand::thread_rng().gen::<f32>() * 30.0 < (raw_brightness(w, x, y, z) as f32 / 15.0 - 0.4) * 2.0
        };
        let tick_parity = e.age % 20 == 0;
        let e = self.worlds[world].entities.get_mut(&id).unwrap();
        let EntityKind::Mob(mob) = &mut e.kind else { return false };
        let living = &mut mob.living;
        if living.invulnerable_time > 0 {
            living.invulnerable_time -= 1;
        }
        if living.hurt_time > 0 {
            living.hurt_time -= 1;
        }
        let mut hurts: Vec<(Damage, f32)> = Vec::new();
        if below_world {
            hurts.push((Damage::new("minecraft:out_of_world"), 4.0));
        }
        if in_water && living.fire_ticks > 0 {
            living.fire_ticks = 0;
            e.metadata_dirty = true;
        }
        let living = &mut mob.living;
        if sunburn {
            if mob.equipment[5].is_empty() {
                if living.fire_ticks < 160 {
                    living.fire_ticks = 160;
                    e.metadata_dirty = true;
                }
            } else if survival::hurt_item(&mut mob.equipment[5], 1, true) {
                e.equipment_dirty = true;
            }
        }
        let living = &mut mob.living;
        if in_lava && !info.fire_immune {
            living.fire_ticks = living.fire_ticks.max(300);
            hurts.push((Damage::new("minecraft:lava"), 4.0));
        }
        if living.fire_ticks > 0 {
            if info.fire_immune {
                living.fire_ticks = (living.fire_ticks - 4).max(0);
            } else {
                if living.fire_ticks % 20 == 0 && !in_lava {
                    hurts.push((Damage::new("minecraft:on_fire"), 1.0));
                }
                living.fire_ticks -= 1;
            }
            if living.fire_ticks == 0 {
                e.metadata_dirty = true;
            }
        }
        let living = &mut mob.living;
        if eye_in_water && !breathes_water {
            living.air -= 1;
            if living.air <= -20 {
                living.air = 0;
                hurts.push((Damage::new("minecraft:drown"), 2.0));
            }
        } else if living.air < 300 {
            living.air = (living.air + 4).min(300);
        }
        if suffocating && tick_parity {
            hurts.push((Damage::new("minecraft:in_wall"), 1.0));
        }
        for (damage, amount) in hurts {
            self.hurt_entity(world, id, damage, amount);
            if self.worlds[world].entities.get(&id).is_none_or(|e| !e.is_alive()) {
                return true;
            }
        }
        false
    }

    /// Picks what the mob wants to do and where it wants to go.
    fn mob_ai(&mut self, world: usize, id: i32) {
        let difficulty = self.difficulty;
        let tick = self.tick;
        let w = &self.worlds[world];
        let Some(e) = w.entities.get(&id) else { return };
        let Some(mob) = e.mob() else { return };
        let info = e.info;
        let kind = behavior(&info.name, &info.category);
        let (pos, eye) = (e.pos, e.eye_pos());
        let follow = info.attribute("follow_range").unwrap_or(16.0);
        let height = (info.height.ceil() as i32).max(1);
        let mut brain = mob.brain.clone();
        let mut rng = rand::thread_rng();

        // Forget targets that left, died, went creative or out of range.
        if let Some(t) = brain.target {
            let keep = match self.players.get(&t) {
                Some(p) => p.world == world && !p.dead && matches!(p.game_mode, GameMode::Survival | GameMode::Adventure) && Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(pos) < follow * follow,
                None => w.entities.get(&t).is_some_and(|o| o.is_alive() && o.pos.distance_squared(pos) < follow * follow),
            };
            if !keep {
                brain.target = None;
                brain.path.clear();
                brain.charge = 0;
            }
        }
        // Hostile mobs look for players now and then (`NearestAttackableTargetGoal`).
        let hostile = matches!(kind, Behavior::Melee | Behavior::Archer | Behavior::Creeper);
        let spider_calm = matches!(info.name.as_str(), "minecraft:spider" | "minecraft:cave_spider") && raw_brightness(w, eye.x.floor() as i32, eye.y.floor() as i32, eye.z.floor() as i32) >= 12;
        if hostile && brain.target.is_none() && difficulty != Difficulty::Peaceful && rng.gen_range(0..10) == 0 && !spider_calm {
            let mut best: Option<(f64, i32)> = None;
            for p in self.players.values() {
                if p.world != world || p.dead || !matches!(p.game_mode, GameMode::Survival | GameMode::Adventure) {
                    continue;
                }
                let at = Vec3::new(p.loc.x, p.loc.y, p.loc.z);
                let range = if p.sneaking { follow * 0.8 } else { follow };
                let d = at.distance_squared(pos);
                if d > range * range || (at.y - pos.y).abs() > 4.0 + range.min(follow) {
                    continue;
                }
                let p_eye = Vec3::new(at.x, at.y + p.eye_height(), at.z);
                if super::combat::clip_blocks(w, eye, p_eye).is_some() {
                    continue;
                }
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, p.entity_id));
                }
            }
            if let Some((_, t)) = best {
                brain.target = Some(t);
                brain.path.clear();
            }
        }
        let target_pos = brain.target.and_then(|t| self.entity_position(world, t));
        let mut swing = false;
        let mut attack: Option<(i32, f32)> = None;
        let mut shoot: Option<(i32, Vec3)> = None;
        let mut explode = false;

        // Movement goal.
        brain.speed_factor = 1.0;
        let mut goal: Option<Vec3> = None;
        if brain.panic_ticks > 0 {
            brain.panic_ticks -= 1;
            brain.speed_factor = if info.name == "minecraft:chicken" { 1.4 } else { 2.0 };
            if brain.path.is_empty() {
                let d = Vec3::new(rng.gen_range(-5.0..5.0), 0.0, rng.gen_range(-5.0..5.0));
                goal = Some(pos.add(d));
            }
        } else if let (Some(t), Some(tp)) = (brain.target, target_pos) {
            let dist2 = tp.distance_squared(pos);
            brain.look_at = Some(Vec3::new(tp.x, tp.y + 1.5, tp.z));
            match kind {
                Behavior::Archer => {
                    let visible = super::combat::clip_blocks(w, eye, Vec3::new(tp.x, tp.y + 1.5, tp.z)).is_none();
                    if dist2 > 225.0 || !visible {
                        goal = Some(tp);
                    } else {
                        brain.path.clear();
                    }
                    // Draw for a second, then shoot (`RangedBowAttackGoal`).
                    if brain.attack_cooldown > 0 {
                        brain.attack_cooldown -= 1;
                    } else if visible {
                        brain.charge += 1;
                        if brain.charge >= 20 {
                            brain.charge = 0;
                            brain.attack_cooldown = if difficulty == Difficulty::Hard { 20 } else { 40 };
                            shoot = Some((t, tp));
                        }
                    } else {
                        brain.charge = 0;
                    }
                }
                Behavior::Creeper => {
                    let visible = super::combat::clip_blocks(w, eye, Vec3::new(tp.x, tp.y + 1.5, tp.z)).is_none();
                    let was_swelling = brain.swelling;
                    brain.swelling = dist2 < 9.0 && visible || brain.swelling && dist2 < 49.0 && visible;
                    if brain.swelling {
                        brain.path.clear();
                        brain.swell += 1;
                        if brain.swell >= 30 {
                            explode = true;
                        }
                    } else {
                        brain.swell = (brain.swell - 1).max(0);
                        goal = Some(tp);
                    }
                    if brain.swelling && !was_swelling && brain.swell == 1 {
                        self.play_sound(world, pos, "minecraft:entity.creeper.primed", category::HOSTILE, 1.0, 0.5, None);
                    }
                }
                _ => {
                    goal = Some(tp);
                    // Hit when the target is within reach (`isWithinMeleeAttackRange`).
                    if brain.attack_cooldown > 0 {
                        brain.attack_cooldown -= 1;
                    }
                    let reach = e.bounding_box().inflate(0.828_427, 0.0, 0.828_427);
                    let target_box = Aabb::of_entity(tp, 0.6, 1.8);
                    if brain.attack_cooldown <= 0 && reach.intersects(&target_box) {
                        brain.attack_cooldown = 20;
                        swing = true;
                        let mut amount = info.attribute("attack_damage").unwrap_or(2.0) as f32;
                        if let Some(weapon) = mob.equipment[0].0.as_ref() {
                            amount += paper_data::item_data::by_id(weapon.item as u32).modifier("minecraft:attack_damage", "mainhand") as f32;
                        }
                        attack = Some((t, amount));
                    }
                }
            }
        } else if matches!(kind, Behavior::Animal | Behavior::Neutral | Behavior::Melee | Behavior::Archer | Behavior::Creeper) {
            // Wander now and then (`RandomStrollGoal`), look at nearby players.
            if brain.path.is_empty() {
                brain.idle_ticks -= 1;
                if brain.idle_ticks <= 0 {
                    brain.idle_ticks = 120 + rng.gen_range(0..120);
                    let d = Vec3::new(rng.gen_range(-10.0..10.0), rng.gen_range(-3.0..3.0), rng.gen_range(-10.0..10.0));
                    goal = Some(pos.add(d));
                }
            }
            brain.look_at = self
                .players
                .values()
                .filter(|p| p.world == world && p.game_mode != GameMode::Spectator)
                .map(|p| Vec3::new(p.loc.x, p.loc.y + p.eye_height(), p.loc.z))
                .find(|at| at.distance_squared(pos) < 36.0);
        } else if matches!(kind, Behavior::Swimmer | Behavior::Flyer) && rng.gen_range(0..80) == 0 {
            let d = Vec3::new(rng.gen_range(-6.0..6.0), rng.gen_range(-2.0..2.0), rng.gen_range(-6.0..6.0));
            brain.look_at = Some(pos.add(d));
        }

        // Plan a path towards the goal (recomputed every few ticks while chasing).
        if let Some(g) = goal {
            let stale = brain.path.is_empty() || brain.path_ticks > 10 + rng.gen_range(0..10);
            if stale && !matches!(kind, Behavior::Swimmer | Behavior::Flyer | Behavior::Still) {
                let start = (pos.x.floor() as i32, (pos.y + 0.01).floor() as i32, pos.z.floor() as i32);
                let mut end = (g.x.floor() as i32, g.y.floor() as i32, g.z.floor() as i32);
                // Wander targets snap to the ground.
                if brain.target.is_none() {
                    for dy in [0, 1, -1, 2, -2, 3, -3] {
                        if walkable(w, end.0, end.1 + dy, end.2, height) {
                            end.1 += dy;
                            break;
                        }
                    }
                }
                let budget = (follow * 16.0).clamp(64.0, 600.0) as usize;
                brain.path = find_path(w, start, end, height, budget).unwrap_or_default();
                brain.path_ticks = 0;
            }
        }
        if !brain.path.is_empty() {
            brain.path_ticks += 1;
            if brain.path_ticks > 200 {
                brain.path.clear();
            }
        }
        let _ = tick;

        let e = self.worlds[world].entities.get_mut(&id).unwrap();
        let swelling_changed = e.mob().is_some_and(|m| m.brain.swelling != brain.swelling || m.brain.target.is_some() != brain.target.is_some());
        if let Some(m) = e.mob_mut() {
            m.brain = brain;
        }
        if swelling_changed {
            e.metadata_dirty = true;
        }
        if swing {
            let payload = Bytes::from(encode_packet(&play::SwingAnimation { entity_id: VarInt(id), hand: VarInt(0), animation_type: VarInt(1), duration: VarInt(6) }));
            self.broadcast_entity(world, id, payload);
        }
        if let Some((target, amount)) = attack {
            self.mob_hit(world, id, target, amount);
        }
        if let Some((_, at)) = shoot {
            self.mob_shoot(world, id, at);
        }
        if explode {
            self.creeper_explode(world, id);
        }
    }

    /// A melee hit (`Mob.doHurtTarget`).
    fn mob_hit(&mut self, world: usize, id: i32, target: i32, amount: f32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (pos, yaw, burning) = (e.pos, e.yaw, e.mob().is_some_and(|m| m.living.fire_ticks > 0));
        let is_zombie = e.info.name.contains("zombie") || e.info.name == "minecraft:husk" || e.info.name == "minecraft:drowned";
        let knockback = e.info.attribute("attack_knockback").unwrap_or(0.0);
        let difficulty = self.difficulty as i32;
        if self.hurt_entity(world, target, Damage::by("minecraft:mob_attack", id, pos), amount) {
            if knockback > 0.0 {
                let (s, c) = ((yaw as f64).to_radians().sin(), -(yaw as f64).to_radians().cos());
                self.knockback_entity(world, target, knockback * 0.5, s, c);
            }
            // Burning zombies set their victims alight.
            if is_zombie && burning && rand::thread_rng().gen::<f32>() < difficulty as f32 * 0.3 {
                self.set_on_fire(world, target, 40 * difficulty);
            }
        }
    }

    /// A skeleton's shot (`AbstractSkeleton.performRangedAttack`).
    fn mob_shoot(&mut self, world: usize, id: i32, target: Vec3) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let origin = Vec3::new(e.pos.x, e.pos.y + e.info.eye_height as f64 - 0.1, e.pos.z);
        let aim = Vec3::new(target.x, target.y + 1.8 / 3.0, target.z);
        let d = aim.sub(origin);
        let horizontal = d.horizontal_length();
        let dir = Vec3::new(d.x, d.y + horizontal * 0.2, d.z);
        let difficulty = self.difficulty as i32;
        let inaccuracy = (14 - difficulty * 4) as f64;
        let mut rng = rand::thread_rng();
        let damage = 2.0 + (rng.gen::<f64>() - rng.gen::<f64>()) * 0.57425 + difficulty as f64 * 0.11;
        let pos = e.pos;
        self.spawn_arrow_towards(world, origin, dir, 1.6, inaccuracy, Some(id), damage, false, false);
        let pitch = 1.0 / (rng.gen::<f32>() * 0.4 + 0.8);
        self.play_sound(world, pos, "minecraft:entity.skeleton.shoot", category::HOSTILE, 1.0, pitch, None);
    }

    fn creeper_explode(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let center = e.pos;
        e.removed = true;
        if let Some(m) = e.mob_mut() {
            m.living.health = 0.0;
        }
        let griefing = self.worlds[world].rules.bool("mob_griefing");
        self.explode(world, center, 3.0, griefing, Some(id));
    }

    /// Walking physics (`LivingEntity.travel` with the mob's move control).
    fn mob_travel(&mut self, world: usize, id: i32) {
        let Some(mut e) = self.worlds[world].entities.remove(&id) else { return };
        let w = &self.worlds[world];
        let info = e.info;
        let kind = behavior(&info.name, &info.category);
        let speed_attr = info.attribute("movement_speed").unwrap_or(0.25);
        let step = info.attribute("step_height").unwrap_or(0.6);
        let area = e.bounding_box();
        let in_water = survival::box_in_fluid(w, &area, Fluid::Water);
        let in_lava = survival::box_in_fluid(w, &area, Fluid::Lava);
        let was_on_ground = e.on_ground;
        let y_before = e.pos.y;
        let mut forward = 0.0;
        let mut jump = false;

        let eye = e.eye_pos();
        if let EntityKind::Mob(mob) = &mut e.kind {
            // Follow the path: aim at the centre of the next node.
            while let Some(&(nx, ny, nz)) = mob.brain.path.first() {
                let node = Vec3::new(nx as f64 + 0.5, ny as f64, nz as f64 + 0.5);
                let d = node.sub(e.pos);
                let reach = if info.width > 0.75 { info.width as f64 / 2.0 } else { 0.75 - info.width as f64 / 2.0 };
                if d.horizontal_length() < reach && (d.y.abs() < 1.0) {
                    mob.brain.path.remove(0);
                    continue;
                }
                let target_yaw = (d.z.atan2(d.x).to_degrees() as f32) - 90.0;
                e.yaw = rotate_towards(e.yaw, target_yaw, 90.0);
                forward = speed_attr * mob.brain.speed_factor;
                if d.y > step - 0.01 && d.horizontal_length() < info.width.max(1.0) as f64 + 0.5 || e.horizontal_collision && d.y > 0.0 {
                    jump = true;
                }
                break;
            }
            // Swimmers and fliers drift towards where they look.
            if matches!(kind, Behavior::Swimmer | Behavior::Flyer) {
                if let Some(at) = mob.brain.look_at {
                    let d = at.sub(e.pos);
                    if d.length_squared() > 1.0 {
                        e.yaw = rotate_towards(e.yaw, (d.z.atan2(d.x).to_degrees() as f32) - 90.0, 10.0);
                        forward = speed_attr * 0.5;
                        if kind == Behavior::Flyer {
                            e.vel.y += (d.y.signum() * 0.1 - e.vel.y) * 0.1;
                        }
                    }
                }
            }
            // Head and body look at what interests them.
            if let Some(at) = mob.brain.look_at {
                let d = at.sub(eye);
                e.head_yaw = rotate_towards(e.head_yaw, (d.z.atan2(d.x).to_degrees() as f32) - 90.0, 10.0);
                e.pitch = -(d.y.atan2(d.horizontal_length()).to_degrees() as f32).clamp(-40.0, 40.0);
            } else {
                e.head_yaw = rotate_towards(e.head_yaw, e.yaw, 10.0);
            }
            if forward > 0.0 {
                e.head_yaw = rotate_towards(e.head_yaw, e.yaw, 75.0);
            }
            // Creepers stand still while their fuse burns.
            if mob.brain.swelling || mob.living.is_dead() {
                forward = 0.0;
                jump = false;
            }
        }

        let gravity = if e.no_gravity || kind == Behavior::Flyer { 0.0 } else { 0.08 };
        let (sin, cos) = ((e.yaw as f64).to_radians().sin(), (e.yaw as f64).to_radians().cos());
        let input = Vec3::new(-sin * forward * 0.98, 0.0, cos * forward * 0.98);
        if in_water || in_lava {
            // Swimming: slow and buoyant; land mobs paddle upwards (`FloatGoal`).
            let accel = if kind == Behavior::Swimmer { 0.1 } else { 0.02 };
            e.vel = e.vel.add(input.scale(accel / forward.max(1.0e-6) * if forward > 0.0 { 1.0 } else { 0.0 }));
            if kind != Behavior::Swimmer && e.mob().is_some_and(|m| !m.living.is_dead()) {
                e.vel.y += 0.04;
            }
            let d = e.vel;
            e.move_by(w, d, step);
            let drag = if in_lava { 0.5 } else { 0.8 };
            e.vel = Vec3::new(e.vel.x * drag, e.vel.y * 0.8, e.vel.z * drag);
            e.vel.y -= gravity / 4.0;
        } else {
            let slip = if e.on_ground { friction_below(w, e.pos) } else { 1.0 };
            let friction = if e.on_ground { slip * 0.91 } else { 0.91 };
            let accel = if e.on_ground { forward * (0.216_000_02 / (slip * slip * slip)) } else { 0.02 };
            if forward > 0.0 {
                e.vel = e.vel.add(input.scale(accel / forward));
            }
            if jump && e.on_ground {
                e.vel.y = info.attribute("jump_strength").unwrap_or(0.42);
            }
            // Spiders climb walls.
            if e.horizontal_collision && matches!(info.name.as_str(), "minecraft:spider" | "minecraft:cave_spider") && forward > 0.0 {
                e.vel.y = 0.2;
            }
            let d = e.vel;
            e.move_by(w, d, step);
            e.vel = Vec3::new(e.vel.x * friction, (e.vel.y - gravity) * 0.98, e.vel.z * friction);
        }
        if e.vel.horizontal_length() < 0.003 && forward == 0.0 {
            e.vel.x = 0.0;
            e.vel.z = 0.0;
        }

        // Falling.
        let fall_immune = is_fall_immune(&info.name) || kind == Behavior::Flyer || info.name == "minecraft:chicken";
        let mut landed = None;
        let (pos_y, on_ground) = (e.pos.y, e.on_ground);
        if let EntityKind::Mob(mob) = &mut e.kind {
            let dy = pos_y - y_before;
            if in_water || in_lava {
                mob.living.fall_distance = 0.0;
            } else if on_ground {
                if mob.living.fall_distance > 3.0 && !was_on_ground && !fall_immune {
                    landed = Some(mob.living.fall_distance);
                }
                mob.living.fall_distance = 0.0;
            } else if dy < 0.0 {
                mob.living.fall_distance -= dy;
            }
        }
        // Chickens flutter down.
        if info.name == "minecraft:chicken" && !e.on_ground && e.vel.y < 0.0 {
            e.vel.y *= 0.6;
        }
        self.worlds[world].entities.insert(id, e);
        self.worlds[world].reindex_entity(id);
        if let Some(distance) = landed {
            let amount = (distance - 3.0).ceil() as f32;
            if amount > 0.0 {
                self.hurt_entity(world, id, Damage::new("minecraft:fall"), amount);
            }
        }
    }

    /// Ambient noises now and then (`Mob.baseTick`).
    fn mob_sounds(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let Some(sound) = e.info.ambient_sound.clone() else { return };
        let pos = e.pos;
        let category = if e.info.category == "MONSTER" { category::HOSTILE } else { category::NEUTRAL };
        let Some(mob) = e.mob_mut() else { return };
        let mut rng = rand::thread_rng();
        // `ambientSoundTime` counts up from -80; the chance grows each tick.
        mob.brain.ambient += 1;
        if rng.gen_range(0..1000) < mob.brain.ambient {
            mob.brain.ambient = -80;
            let pitch = (rng.gen::<f32>() - rng.gen::<f32>()) * 0.2 + if mob.baby { 1.5 } else { 1.0 };
            self.play_sound(world, pos, &sound, category, 1.0, pitch, None);
        }
    }

    // --- natural spawning ----------------------------------------------------------------------

    /// Spawns and despawns mobs around players (`NaturalSpawner.spawnForChunk` and
    /// `Mob.checkDespawn`).
    pub(super) fn tick_spawning(&mut self) {
        for world in 0..self.worlds.len() {
            self.despawn_mobs(world);
            if !self.worlds[world].rules.bool("spawn_mobs") {
                continue;
            }
            // Chunks within eight chunks of a player that are loaded and lit.
            let mut chunks: Vec<ChunkPos> = Vec::new();
            for p in self.players.values().filter(|p| p.world == world && p.game_mode != GameMode::Spectator) {
                let c = p.loc.chunk();
                for dz in -8..=8 {
                    for dx in -8..=8 {
                        let pos = ChunkPos::new(c.x + dx, c.z + dz);
                        if self.worlds[world].is_ready(pos) && !chunks.contains(&pos) {
                            chunks.push(pos);
                        }
                    }
                }
            }
            if chunks.is_empty() {
                continue;
            }
            // Mob counts per category against the caps scaled by the spawnable area.
            let mut counts: HashMap<&str, i32> = HashMap::new();
            for e in self.worlds[world].entities.values() {
                if e.mob().is_some() && e.is_alive() {
                    *counts.entry(category_key(&e.info.category)).or_default() += 1;
                }
            }
            let spawn_monsters = self.worlds[world].rules.bool("spawn_monsters") && self.difficulty != Difficulty::Peaceful;
            let friendly_tick = self.worlds[world].age % 400 == 0;
            let mut rng = rand::thread_rng();
            for (category, key, cap, persistent) in CATEGORIES {
                if category == "MONSTER" && !spawn_monsters || persistent && !friendly_tick {
                    continue;
                }
                let cap = cap * chunks.len() as i32 / 289;
                if counts.get(category).copied().unwrap_or(0) >= cap {
                    continue;
                }
                for &chunk in &chunks {
                    if rng.gen_range(0..2) == 0 && category == "MONSTER" {
                        continue;
                    }
                    let spawned = self.spawn_category_in_chunk(world, chunk, category, key);
                    *counts.entry(category).or_default() += spawned;
                    if counts[category] >= cap {
                        break;
                    }
                }
            }
        }
    }

    fn spawn_category_in_chunk(&mut self, world: usize, chunk: ChunkPos, category: &str, key: &str) -> i32 {
        let mut rng = rand::thread_rng();
        let w = &self.worlds[world];
        let (x0, z0) = (chunk.x * 16 + rng.gen_range(0..16), chunk.z * 16 + rng.gen_range(0..16));
        let Some(top) = w.chunks.get(&chunk).map(|c| c.min_y + c.world_surface[(((z0 & 15) << 4) | (x0 & 15)) as usize] as i32 + 1) else { return 0 };
        let y = rng.gen_range(w.min_y..=top.max(w.min_y));
        if y < w.min_y + 1 {
            return 0;
        }
        if w.block_at(x0, y, z0).is_none_or(|s| paper_data::physics::flags(s) & paper_data::physics::SOLID_BLOCK != 0) {
            return 0;
        }
        let biome = self.biome_at(world, x0, y, z0);
        let spawners = self.biome_spawners(biome, key);
        if spawners.is_empty() {
            return 0;
        }
        let mut count = 0;
        for _ in 0..3 {
            let (mut x, mut z) = (x0, z0);
            let mut pack = (rng.gen::<f32>() * 4.0).ceil() as i32;
            let mut chosen: Option<SpawnerEntry> = None;
            let mut in_pack = 0;
            let mut r = 0;
            while r < pack {
                r += 1;
                x += rng.gen_range(0..6) - rng.gen_range(0..6);
                z += rng.gen_range(0..6) - rng.gen_range(0..6);
                let at = Vec3::new(x as f64 + 0.5, y as f64, z as f64 + 0.5);
                let nearest = self.players.values().filter(|p| p.world == world).map(|p| Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(at)).fold(f64::MAX, f64::min);
                // Not too close to a player, not so far that it would despawn at once.
                if nearest <= 576.0 || nearest > 128.0 * 128.0 {
                    continue;
                }
                if chosen.is_none() {
                    let total: i32 = spawners.iter().map(|s| s.weight).sum();
                    if total <= 0 {
                        break;
                    }
                    let mut pick = rng.gen_range(0..total);
                    for s in &spawners {
                        pick -= s.weight;
                        if pick < 0 {
                            chosen = Some(s.clone());
                            break;
                        }
                    }
                    let Some(c) = &chosen else { break };
                    pack = c.min + rng.gen_range(0..=(c.max - c.min).max(0));
                }
                let entry = chosen.clone().unwrap();
                if !self.can_spawn_at(world, &entry.entity, x, y, z, category, false) {
                    continue;
                }
                if self.spawn_mob(world, &entry.entity, at, SpawnReason::Natural).is_some() {
                    count += 1;
                    in_pack += 1;
                    let limit = entity_types::by_name(&entry.entity).map_or(4, |i| i.limit_per_chunk.max(1));
                    if count >= limit || in_pack >= pack {
                        return count;
                    }
                }
            }
        }
        count
    }

    /// Spawn placement and light rules for a mob type at a block position.
    fn can_spawn_at(&self, world: usize, entity: &str, x: i32, y: i32, z: i32, category: &str, generation: bool) -> bool {
        let w = &self.worlds[world];
        let Some(info) = entity_types::by_name(entity) else { return false };
        let empty = |yy: i32| {
            w.block_at(x, yy, z).is_some_and(|s| {
                paper_data::physics::flags(s) & paper_data::physics::FULL_CUBE == 0 && survival::fluid_of(s).is_none() && !matches!(blocks::block_of_state(s).name, "minecraft:fire" | "minecraft:soul_fire" | "minecraft:powder_snow" | "minecraft:sweet_berry_bush" | "minecraft:cactus" | "minecraft:wither_rose" | "minecraft:magma_block") && !blocks::block_of_state(s).name.ends_with("_rail")
            })
        };
        let in_water_type = info.spawn_location == "IN_WATER";
        if in_water_type {
            let water = |yy: i32| w.block_at(x, yy, z).and_then(survival::fluid_of).is_some_and(|(f, _)| f == Fluid::Water);
            if !(water(y) && water(y - 1)) {
                return false;
            }
        } else {
            let Some(below) = w.block_at(x, y - 1, z) else { return false };
            let below_name = blocks::block_of_state(below).name;
            let sturdy = paper_data::physics::flags(below) & paper_data::physics::SOLID_BLOCK != 0;
            if !sturdy || matches!(below_name, "minecraft:bedrock" | "minecraft:barrier") || !empty(y) || !empty(y + 1) {
                return false;
            }
        }
        // The mob must fit.
        let area = Aabb::of_entity(Vec3::new(x as f64 + 0.5, y as f64, z as f64 + 0.5), info.width as f64, info.height as f64);
        if collides(w, &area) {
            return false;
        }
        match category {
            "MONSTER" => {
                // `isDarkEnoughToSpawn`: no block light, and dark sky light.
                let mut rng = rand::thread_rng();
                if sky_light(w, x, y, z) > rng.gen_range(0..32) {
                    return false;
                }
                if block_light(w, x, y, z) > 0 {
                    return false;
                }
                raw_brightness(w, x, y, z) <= rng.gen_range(0..=7)
            }
            "CREATURE" => {
                let below = w.block_at(x, y - 1, z).unwrap_or(blocks::AIR);
                let below_ok = paper_data::synced::tag("minecraft:block", "minecraft:animals_spawnable_on").is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(below) as i32)));
                below_ok && (generation || raw_brightness(w, x, y, z) > 8)
            }
            _ => true,
        }
    }

    /// Animals placed when a chunk is first generated (`spawnMobsForChunkGeneration`).
    pub(super) fn spawn_for_chunk_generation(&mut self, world: usize, chunk: ChunkPos) {
        if !self.worlds[world].rules.bool("spawn_mobs") {
            return;
        }
        let mut rng = rand::thread_rng();
        let (x0, z0) = (chunk.x * 16, chunk.z * 16);
        let biome = {
            let top = self.worlds[world].top_y(x0 + 8, z0 + 8).unwrap_or(64);
            self.biome_at(world, x0 + 8, top, z0 + 8)
        };
        let spawners = self.biome_spawners(biome, "creature");
        if spawners.is_empty() {
            return;
        }
        let total: i32 = spawners.iter().map(|s| s.weight).sum();
        while rng.gen::<f32>() < 0.1 && total > 0 {
            let mut pick = rng.gen_range(0..total);
            let Some(entry) = spawners.iter().find(|s| {
                pick -= s.weight;
                pick < 0
            }) else {
                break;
            };
            let count = entry.min + rng.gen_range(0..=(entry.max - entry.min).max(0));
            let (mut x, mut z) = (x0 + rng.gen_range(0..16), z0 + rng.gen_range(0..16));
            let (sx, sz) = (x, z);
            for _ in 0..count {
                for _ in 0..4 {
                    if let Some(top) = self.worlds[world].top_y(x, z) {
                        if self.can_spawn_at(world, &entry.entity, x, top, z, "CREATURE", true) {
                            self.spawn_mob(world, &entry.entity, Vec3::new(x as f64 + 0.5, top as f64, z as f64 + 0.5), SpawnReason::ChunkGeneration);
                            break;
                        }
                    }
                    x += rng.gen_range(0..5) - rng.gen_range(0..5);
                    z += rng.gen_range(0..5) - rng.gen_range(0..5);
                    while x < x0 || x >= x0 + 16 || z < z0 || z >= z0 + 16 {
                        x = sx + rng.gen_range(0..5) - rng.gen_range(0..5);
                        z = sz + rng.gen_range(0..5) - rng.gen_range(0..5);
                    }
                }
            }
        }
    }

    /// Hostile mobs vanish far from players and in peaceful (`Mob.checkDespawn`).
    fn despawn_mobs(&mut self, world: usize) {
        let peaceful = self.difficulty == Difficulty::Peaceful;
        let players: Vec<Vec3> = self.players.values().filter(|p| p.world == world && p.game_mode != GameMode::Spectator).map(|p| Vec3::new(p.loc.x, p.loc.y, p.loc.z)).collect();
        let mut rng = rand::thread_rng();
        for e in self.worlds[world].entities.values_mut() {
            let info = e.info;
            let pos = e.pos;
            let Some(mob) = e.mob_mut() else { continue };
            if mob.persistent || info.category == "CREATURE" || info.category == "MISC" {
                continue;
            }
            if peaceful && info.category == "MONSTER" {
                e.removed = true;
                continue;
            }
            let Some(nearest) = players.iter().map(|p| p.distance_squared(pos)).reduce(f64::min) else { continue };
            let far = if info.category == "WATER_AMBIENT" { 64.0 } else { 128.0 };
            if nearest > far * far {
                e.removed = true;
            } else if nearest > 32.0 * 32.0 {
                mob.no_player_ticks += 1;
                if mob.no_player_ticks > 600 && rng.gen_range(0..800) == 0 {
                    e.removed = true;
                }
            } else {
                mob.no_player_ticks = 0;
            }
        }
    }
}

fn category_key(category: &str) -> &'static str {
    match category {
        "MONSTER" => "MONSTER",
        "CREATURE" => "CREATURE",
        "AMBIENT" => "AMBIENT",
        "WATER_AMBIENT" => "WATER_AMBIENT",
        "WATER_CREATURE" => "WATER_CREATURE",
        _ => "MISC",
    }
}

/// Spawners used when the vanilla data is unavailable (plains).
fn default_spawners(category: &str) -> Vec<SpawnerEntry> {
    let e = |entity: &str, weight, min, max| SpawnerEntry { entity: entity.into(), weight, min, max };
    match category {
        "monster" => vec![e("minecraft:spider", 100, 4, 4), e("minecraft:zombie", 95, 4, 4), e("minecraft:skeleton", 100, 4, 4), e("minecraft:creeper", 100, 4, 4), e("minecraft:enderman", 10, 1, 4)],
        "creature" => vec![e("minecraft:sheep", 12, 4, 4), e("minecraft:pig", 10, 4, 4), e("minecraft:chicken", 10, 4, 4), e("minecraft:cow", 8, 4, 4)],
        _ => Vec::new(),
    }
}

/// Turns `from` towards `to` by at most `max` degrees.
pub fn rotate_towards(from: f32, to: f32, max: f32) -> f32 {
    let mut d = (to - from).rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    from + d.clamp(-max, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_takes_the_short_way() {
        assert_eq!(rotate_towards(350.0, 10.0, 90.0), 370.0);
        assert_eq!(rotate_towards(10.0, 350.0, 5.0), 5.0);
    }

    #[test]
    fn behaviours() {
        assert_eq!(behavior("minecraft:zombie", "MONSTER"), Behavior::Melee);
        assert_eq!(behavior("minecraft:cow", "CREATURE"), Behavior::Animal);
        assert_eq!(behavior("minecraft:skeleton", "MONSTER"), Behavior::Archer);
        assert_eq!(behavior("minecraft:cod", "WATER_AMBIENT"), Behavior::Swimmer);
    }
}
