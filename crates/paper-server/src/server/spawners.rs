//! Mob spawners (vanilla's `BaseSpawner`): with a player in range, a spawner counts down its
//! delay, then tries to put up to `SpawnCount` of its mob around itself, unless
//! `MaxNearbyEntities` of them are there already.

use super::mobs::SpawnReason;
use super::Server;
use crate::config::{Difficulty, GameMode};
use crate::world::entity::{Aabb, Vec3};
use paper_data::{blocks, entity_types};
use paper_nbt::{Compound, Tag};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use rand::Rng;

/// The settings of a spawner, with vanilla's defaults.
struct Settings {
    delay: i32,
    min_delay: i32,
    max_delay: i32,
    count: i32,
    max_nearby: i32,
    player_range: f64,
    spawn_range: f64,
}

impl Settings {
    fn of(nbt: &Compound) -> Self {
        let get = |key: &str, default: i32| nbt.i32(key).unwrap_or(default);
        Settings {
            delay: get("Delay", 20),
            min_delay: get("MinSpawnDelay", 200),
            max_delay: get("MaxSpawnDelay", 800),
            count: get("SpawnCount", 4),
            max_nearby: get("MaxNearbyEntities", 6),
            player_range: get("RequiredPlayerRange", 16) as f64,
            spawn_range: get("SpawnRange", 4) as f64,
        }
    }

    /// The next delay (`BaseSpawner.delay`).
    fn next_delay(&self, rng: &mut impl Rng) -> i32 {
        if self.max_delay <= self.min_delay {
            self.min_delay
        } else {
            self.min_delay + rng.gen_range(0..self.max_delay - self.min_delay)
        }
    }
}

impl Server {
    pub(super) fn tick_spawners(&mut self) {
        for world in 0..self.worlds.len() {
            let players: Vec<Vec3> = self.players.values().filter(|p| p.world == world && p.game_mode != GameMode::Spectator && !p.dead).map(|p| Vec3::new(p.loc.x, p.loc.y, p.loc.z)).collect();
            if players.is_empty() {
                continue;
            }
            let spawners: Vec<BlockPos> = self.worlds[world]
                .chunks
                .iter()
                .flat_map(|(cpos, chunk)| {
                    chunk.block_entities.iter().filter_map(move |((y, z, x), nbt)| (nbt.string("id") == Some("minecraft:mob_spawner")).then(|| BlockPos::new(cpos.x * 16 + *x as i32, *y, cpos.z * 16 + *z as i32)))
                })
                .collect();
            for pos in spawners {
                self.tick_spawner(world, pos, &players);
            }
        }
    }

    fn tick_spawner(&mut self, world: usize, pos: BlockPos, players: &[Vec3]) {
        if !self.worlds[world].block_at(pos.x, pos.y, pos.z).is_some_and(|s| blocks::block_of_state(s).name == "minecraft:spawner") {
            return;
        }
        let Some(nbt) = self.block_entity_data(world, pos).cloned() else { return };
        let settings = Settings::of(&nbt);
        let center = Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5);
        if !players.iter().any(|p| p.distance_squared(center) < settings.player_range * settings.player_range) {
            return;
        }
        let mut rng = rand::thread_rng();
        if settings.delay > 0 {
            self.set_spawner_delay(world, pos, settings.delay - 1);
            return;
        }
        if settings.delay < 0 {
            self.reset_spawner(world, pos, &settings);
            return;
        }
        let Some(name) = nbt.compound("SpawnData").and_then(|d| d.compound("entity")).and_then(|e| e.string("id")).map(str::to_owned) else {
            self.reset_spawner(world, pos, &settings);
            return;
        };
        let Some(info) = entity_types::by_name(&name) else {
            self.reset_spawner(world, pos, &settings);
            return;
        };
        let mut spawned = false;
        for _ in 0..settings.count {
            let at = Vec3::new(
                pos.x as f64 + (rng.gen::<f64>() - rng.gen::<f64>()) * settings.spawn_range + 0.5,
                (pos.y + rng.gen_range(0..3) - 1) as f64,
                pos.z as f64 + (rng.gen::<f64>() - rng.gen::<f64>()) * settings.spawn_range + 0.5,
            );
            let (hw, h) = (info.width as f64 / 2.0, info.height as f64);
            let space = Aabb::new(Vec3::new(at.x - hw, at.y, at.z - hw), Vec3::new(at.x + hw, at.y + h, at.z + hw));
            if crate::world::entity::collides(&self.worlds[world], &space) {
                continue;
            }
            // Monsters: not in peaceful, and only where it is dark enough
            // (`Monster.getWalkTargetValue` is negative above light 12).
            if info.category == "MONSTER" {
                let light = super::mobs::raw_brightness(&self.worlds[world], at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32);
                if self.difficulty == Difficulty::Peaceful || light > 12 {
                    continue;
                }
            }
            let r = settings.spawn_range;
            let area = Aabb::new(Vec3::new(pos.x as f64 - r, pos.y as f64 - r, pos.z as f64 - r), Vec3::new(pos.x as f64 + 1.0 + r, pos.y as f64 + 1.0 + r, pos.z as f64 + 1.0 + r));
            let nearby = self.worlds[world].entities_in(&area).into_iter().filter(|e| self.worlds[world].entities.get(e).is_some_and(|e| e.info.name == name)).count();
            if nearby as i32 >= settings.max_nearby {
                self.reset_spawner(world, pos, &settings);
                return;
            }
            let Some(id) = self.spawn_mob(world, &name, at, SpawnReason::Spawner) else {
                self.reset_spawner(world, pos, &settings);
                return;
            };
            if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                e.yaw = rng.gen::<f32>() * 360.0;
                e.head_yaw = e.yaw;
            }
            // The flames around the spawner.
            self.send_near(world, pos, 64.0, None, &play::LevelEvent { event: 2004, pos, data: 0, global: false });
            spawned = true;
        }
        if spawned {
            self.reset_spawner(world, pos, &settings);
        }
    }

    fn set_spawner_delay(&mut self, world: usize, pos: BlockPos, delay: i32) {
        let (chunk, key) = (crate::world::chunk::ChunkPos::of_block(pos.x, pos.z), (pos.y, (pos.z & 15) as u8, (pos.x & 15) as u8));
        if let Some(nbt) = self.worlds[world].chunks.get_mut(&chunk).and_then(|c| c.block_entities.get_mut(&key)) {
            nbt.insert("Delay", Tag::Short(delay.clamp(i16::MIN as i32, i16::MAX as i32) as i16));
        }
    }

    /// A new delay, and block event 1 so clients restart the spinning mob.
    fn reset_spawner(&mut self, world: usize, pos: BlockPos, settings: &Settings) {
        let delay = settings.next_delay(&mut rand::thread_rng());
        self.set_spawner_delay(world, pos, delay);
        if let Some(state) = self.worlds[world].block_at(pos.x, pos.y, pos.z) {
            let block = blocks::block_id_of_state(state) as i32;
            self.broadcast_to_chunk_viewers(world, crate::world::chunk::ChunkPos::of_block(pos.x, pos.z), &play::BlockEvent { pos, action: 1, param: 0, block: VarInt(block) });
        }
    }
}
