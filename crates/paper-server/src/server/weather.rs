//! Weather (vanilla's `ServerLevel.advanceWeatherCycle`, `/weather`, `tickThunder` and
//! `tickPrecipitation`): rain and thunder come and go on random timers and fade in and out;
//! clients are told as they do. Thunderstorms strike lightning near players, and cold biomes
//! freeze still water and gather snow when it rains.

use super::commands::CommandSource;
use super::survival::Damage;
use super::Server;
use crate::config::Difficulty;
use crate::world::chunk::ChunkPos;
use crate::world::entity::{Aabb, Vec3};
use bytes::Bytes;
use paper_data::blocks;
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::{encode_packet, play};
use paper_protocol::text::Component;
use rand::Rng;
use std::collections::HashMap;
use std::sync::OnceLock;

/// `RAIN_DELAY`, `RAIN_DURATION`, `THUNDER_DELAY`, `THUNDER_DURATION`.
const RAIN_DELAY: (i32, i32) = (12000, 180000);
const RAIN_DURATION: (i32, i32) = (12000, 24000);
const THUNDER_DELAY: (i32, i32) = (12000, 180000);
const THUNDER_DURATION: (i32, i32) = (3600, 15600);

fn sample(range: (i32, i32)) -> i32 {
    rand::thread_rng().gen_range(range.0..=range.1)
}

/// Biome temperatures and whether they have precipitation, by biome name.
fn biome_climate(data: Option<&crate::vanilla::VanillaData>) -> &'static HashMap<String, (f32, bool)> {
    static TABLE: OnceLock<HashMap<String, (f32, bool)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        data.map(|d| d.registry("worldgen/biome").into_iter().map(|(name, v)| (name, (v["temperature"].as_f64().unwrap_or(0.8) as f32, v["has_precipitation"].as_bool().unwrap_or(true)))).collect()).unwrap_or_default()
    })
}

impl Server {
    /// Whether it is cold enough to snow at a block (`Biome.coldEnoughToSnow`; higher up
    /// is colder, without vanilla's temperature noise).
    pub fn cold_at(&self, world: usize, pos: BlockPos) -> bool {
        let biome = self.biome_at(world, pos.x, pos.y, pos.z);
        let (mut temperature, _) = biome_climate(self.vanilla.as_deref()).get(biome).copied().unwrap_or((0.8, true));
        let snow_line = self.worlds[world].generator.sea_level() + 17;
        if pos.y > snow_line {
            temperature -= (pos.y - snow_line) as f32 * 0.05 / 40.0;
        }
        temperature < 0.15
    }

    fn has_precipitation(&self, world: usize, pos: BlockPos) -> bool {
        let biome = self.biome_at(world, pos.x, pos.y, pos.z);
        biome_climate(self.vanilla.as_deref()).get(biome).is_none_or(|(_, p)| *p)
    }

    /// The weather events a player entering a world gets (`PlayerList.sendLevelInfo`).
    pub fn send_weather(&self, id: i32) {
        let Some(p) = self.players.get(&id) else { return };
        let w = &self.worlds[p.world];
        if w.weather.is_raining() {
            p.conn.send(&play::GameEvent { event: play::game_event::START_RAINING, value: 0.0 });
            p.conn.send(&play::GameEvent { event: play::game_event::RAIN_LEVEL_CHANGE, value: w.weather.rain_level });
            p.conn.send(&play::GameEvent { event: play::game_event::THUNDER_LEVEL_CHANGE, value: w.weather.thunder_level });
        }
    }

    fn broadcast_game_event(&self, world: usize, event: u8, value: f32) {
        let payload = Bytes::from(encode_packet(&play::GameEvent { event, value }));
        self.broadcast_payload_in_world(world, payload);
    }

    /// `setWeatherParameters` (what `/weather` does).
    pub fn set_weather(&mut self, world: usize, clear_time: i32, weather_time: i32, raining: bool, thundering: bool) {
        let w = &mut self.worlds[world].weather;
        w.clear_time = clear_time;
        w.rain_time = weather_time;
        w.thunder_time = weather_time;
        w.raining = raining;
        w.thundering = thundering;
    }

    /// Clears rain and thunder after a night slept through (`resetWeatherCycle`).
    pub fn reset_weather_cycle(&mut self, world: usize) {
        let w = &mut self.worlds[world].weather;
        w.rain_time = 0;
        w.raining = false;
        w.thunder_time = 0;
        w.thundering = false;
    }

    pub(super) fn tick_weather(&mut self) {
        for world in 0..self.worlds.len() {
            if !self.worlds[world].has_skylight {
                continue;
            }
            let advance = self.worlds[world].rules.bool("advance_weather");
            let w = &mut self.worlds[world].weather;
            let was_raining = w.is_raining();
            if advance {
                if w.clear_time > 0 {
                    w.clear_time -= 1;
                    w.thunder_time = if w.thundering { 0 } else { 1 };
                    w.rain_time = if w.raining { 0 } else { 1 };
                    w.thundering = false;
                    w.raining = false;
                } else {
                    if w.thunder_time > 0 {
                        w.thunder_time -= 1;
                        if w.thunder_time == 0 {
                            w.thundering = !w.thundering;
                        }
                    } else {
                        w.thunder_time = sample(if w.thundering { THUNDER_DURATION } else { THUNDER_DELAY });
                    }
                    if w.rain_time > 0 {
                        w.rain_time -= 1;
                        if w.rain_time == 0 {
                            w.raining = !w.raining;
                        }
                    } else {
                        w.rain_time = sample(if w.raining { RAIN_DURATION } else { RAIN_DELAY });
                    }
                }
            }
            let (old_rain, old_thunder) = (w.rain_level, w.thunder_level);
            w.thunder_level = (w.thunder_level + if w.thundering { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
            w.rain_level = (w.rain_level + if w.raining { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
            let (rain, thunder, now_raining) = (w.rain_level, w.thunder_level, w.is_raining());
            if rain != old_rain {
                self.broadcast_game_event(world, play::game_event::RAIN_LEVEL_CHANGE, rain);
            }
            if thunder != old_thunder {
                self.broadcast_game_event(world, play::game_event::THUNDER_LEVEL_CHANGE, thunder);
            }
            if was_raining != now_raining {
                self.broadcast_game_event(world, if was_raining { play::game_event::STOP_RAINING } else { play::game_event::START_RAINING }, 0.0);
                self.broadcast_game_event(world, play::game_event::RAIN_LEVEL_CHANGE, rain);
                self.broadcast_game_event(world, play::game_event::THUNDER_LEVEL_CHANGE, thunder);
            }
            self.weather_in_chunks(world);
        }
    }

    /// Lightning and precipitation in the chunks players keep ticking.
    fn weather_in_chunks(&mut self, world: usize) {
        let w = &self.worlds[world];
        let thundering = w.weather.is_thundering();
        let chunks: Vec<ChunkPos> = w.chunks.keys().copied().filter(|p| w.has_ticket(*p) && w.is_ready(*p)).collect();
        let mut rng = rand::thread_rng();
        for cp in chunks {
            if thundering && rng.gen_range(0..100000) == 0 {
                let (x, z) = (cp.x * 16 + rng.gen_range(0..16), cp.z * 16 + rng.gen_range(0..16));
                if let Some(y) = self.worlds[world].top_y(x, z) {
                    if self.worlds[world].rain_at(x, y, z) {
                        self.strike_lightning(world, BlockPos::new(x, y, z));
                    }
                }
            }
            if rng.gen_range(0..16) == 0 {
                let (x, z) = (cp.x * 16 + rng.gen_range(0..16), cp.z * 16 + rng.gen_range(0..16));
                self.precipitation_at(world, x, z);
            }
        }
    }

    /// `tickPrecipitation`: still water freezes in cold biomes, snow piles up when it rains.
    fn precipitation_at(&mut self, world: usize, x: i32, z: i32) {
        let Some(y) = self.worlds[world].top_y(x, z) else { return };
        let (top, below) = (BlockPos::new(x, y, z), BlockPos::new(x, y - 1, z));
        if !self.cold_at(world, below) {
            return;
        }
        let block_light = |s: &Self, p: BlockPos| s.worlds[world].chunks.get(&ChunkPos::of_block(p.x, p.z)).map_or(0, |c| c.block_light_at((p.x & 15) as usize, p.y, (p.z & 15) as usize));
        let water_source = |s: &Self, p: BlockPos| s.worlds[world].block_at(p.x, p.y, p.z).is_some_and(|b| blocks::block_of_state(b).name == "minecraft:water" && blocks::property(b, "level") == Some("0"));
        // `shouldFreeze` (at the edge of the water).
        if block_light(self, below) < 10 && water_source(self, below) {
            let surrounded = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().all(|(dx, dz)| water_source(self, below.offset(*dx, 0, *dz)));
            if !surrounded {
                if let Some(ice) = blocks::default_state("minecraft:ice") {
                    self.set_block_and_update(world, below, ice);
                }
            }
        }
        // `shouldSnow`.
        let max_height = self.worlds[world].rules.int("max_snow_accumulation_height");
        if !self.worlds[world].weather.is_raining() || max_height <= 0 || !self.has_precipitation(world, top) || block_light(self, top) >= 10 {
            return;
        }
        let Some(snow) = blocks::default_state("minecraft:snow") else { return };
        let here = self.worlds[world].block_at(top.x, top.y, top.z).unwrap_or(blocks::AIR);
        if blocks::block_of_state(here).name == "minecraft:snow" {
            let layers: i32 = blocks::property(here, "layers").and_then(|l| l.parse().ok()).unwrap_or(1);
            if layers < max_height.min(8) {
                if let Some(more) = blocks::with_property(here, "layers", &(layers + 1).to_string()) {
                    self.set_block_and_update(world, top, more);
                }
            }
        } else if blocks::is_air(here) {
            let fits = {
                let level = crate::world::block_updates::Level::new(&mut self.worlds[world]);
                level.can_survive(snow, top)
            };
            if fits {
                self.set_block_and_update(world, top, snow);
            }
        }
    }

    /// A lightning bolt strikes (`LightningBolt`): fire where it lands (on normal and hard),
    /// five damage and eight seconds of fire to everything close by.
    pub fn strike_lightning(&mut self, world: usize, pos: BlockPos) {
        let at = Vec3::new(pos.x as f64 + 0.5, pos.y as f64, pos.z as f64 + 0.5);
        let Some(bolt) = self.spawn_mob(world, "minecraft:lightning_bolt", at, super::mobs::SpawnReason::Natural) else { return };
        if matches!(self.difficulty, Difficulty::Normal | Difficulty::Hard) && self.worlds[world].updates.settings.fire_spreads {
            let mut rng = rand::thread_rng();
            let mut targets = vec![pos];
            for _ in 0..4 {
                targets.push(pos.offset(rng.gen_range(-1..=1), rng.gen_range(-1..=1), rng.gen_range(-1..=1)));
            }
            for p in targets {
                if !self.worlds[world].block_at(p.x, p.y, p.z).is_some_and(blocks::is_air) {
                    continue;
                }
                let fire = crate::world::block_updates::fire::fire_state_for(&self.worlds[world], p);
                let fits = {
                    let level = crate::world::block_updates::Level::new(&mut self.worlds[world]);
                    level.can_survive(fire, p)
                };
                if fits {
                    self.set_block_and_update(world, p, fire);
                }
            }
        }
        let area = Aabb::new(Vec3::new(at.x - 3.0, at.y - 3.0, at.z - 3.0), Vec3::new(at.x + 3.0, at.y + 9.0, at.z + 3.0));
        let mut victims: Vec<i32> = self.worlds[world].entities_in(&area).into_iter().filter(|e| *e != bolt).collect();
        victims.extend(self.players.values().filter(|p| p.world == world && Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8).intersects(&area)).map(|p| p.entity_id));
        for id in victims {
            self.set_on_fire(world, id, 8 * 20);
            self.hurt_entity(world, id, Damage::new("minecraft:lightning_bolt"), 5.0);
        }
    }

    /// `/weather clear|rain|thunder [duration]`.
    pub(super) fn weather_command(&mut self, source: &CommandSource, args: &[&str]) {
        let world = self.source_location(source).map_or(0, |(w, _)| w);
        let duration = match args.get(1) {
            None => None,
            Some(d) => match parse_duration(d) {
                Some(t) if t >= 1 => Some(t),
                _ => return self.fail(source, format!("Invalid duration '{d}'")),
            },
        };
        let (key, clear, time, raining, thundering) = match args.first().copied() {
            Some("clear") => ("commands.weather.set.clear", duration.unwrap_or_else(|| sample(RAIN_DELAY)), 0, false, false),
            Some("rain") => ("commands.weather.set.rain", 0, duration.unwrap_or_else(|| sample(RAIN_DURATION)), true, false),
            Some("thunder") => ("commands.weather.set.thunder", 0, duration.unwrap_or_else(|| sample(THUNDER_DURATION)), true, true),
            _ => return self.fail(source, "Usage: /weather (clear|rain|thunder) [duration]"),
        };
        self.set_weather(world, clear, time, raining, thundering);
        self.feedback(source, Component::translatable(key, vec![]));
    }
}

/// A `TimeArgument`: ticks, or with `t`, `s` or `d` (days of 24000 ticks).
fn parse_duration(s: &str) -> Option<i32> {
    let (number, unit) = match s.chars().last()? {
        't' => (&s[..s.len() - 1], 1.0),
        's' => (&s[..s.len() - 1], 20.0),
        'd' => (&s[..s.len() - 1], 24000.0),
        _ => (s, 1.0),
    };
    let value: f64 = number.parse().ok()?;
    Some((value * unit).round() as i32)
}
