//! End features: spikes, gateways, islands, the arrival platform and chorus plants.
//!
//! Derived from `pumpkin-world/src/generation/feature/features/{end_spike,end_gateway,
//! end_island,end_platform,chorus_plant}.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, and checked against vanilla 26.3
//! (`EndSpikeFeature`, `EndGatewayFeature`, `EndIslandFeature`, `EndPlatformFeature`,
//! `ChorusPlantFeature`, `ChorusFlowerBlock.generatePlant`).

use crate::data::JsonExt;
use crate::feature::blockinfo::BlockSet;
use crate::feature::parse::Parser;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::{BlockEntity, Entity, Place};
use crate::random::{LegacyRandom, Random, WorldgenRandom};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::Arc;

pub(super) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(Some(match t {
        "end_spike" => {
            let mut spikes = Vec::new();
            for s in v.get("spikes").and_then(Value::as_array).into_iter().flatten() {
                spikes.push(Spike {
                    x: s.i64_or("centerX", 0) as i32,
                    z: s.i64_or("centerZ", 0) as i32,
                    radius: s.i64_or("radius", 0) as i32,
                    height: s.i64_or("height", 0) as i32,
                    guarded: s.bool_or("guarded", false),
                });
            }
            Box::new(EndSpike {
                spikes,
                crystal_invulnerable: v.bool_or("crystal_invulnerable", false),
                beam_target: v.get("crystal_beam_target").map(block_pos).transpose()?,
            })
        }
        "end_gateway" => Box::new(EndGateway { exit: v.get("exit").map(block_pos).transpose()?, exact: v.bool_or("exact", false) }),
        "end_island" => Box::new(EndIsland),
        "end_platform" => Box::new(EndPlatform),
        "chorus_plant" => Box::new(ChorusPlant { supports: p.tag("minecraft:supports_chorus_plant")? }),
        _ => return Ok(None),
    }))
}

/// A `BlockPos` in its codec form, `[x, y, z]`.
fn block_pos(v: &Value) -> Result<(i32, i32, i32)> {
    let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| anyhow!("bad block position {v}"))?;
    let c = |i: usize| a[i].as_i64().map(|x| x as i32).ok_or_else(|| anyhow!("bad block position {v}"));
    Ok((c(0)?, c(1)?, c(2)?))
}

/// `EndSpikeFeature.EndSpike`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Spike {
    pub x: i32,
    pub z: i32,
    pub radius: i32,
    pub height: i32,
    pub guarded: bool,
}

/// `EndSpikeFeature.getSpikesForLevel`: the ten obsidian pillars of a seed.
pub(crate) fn spikes_for_seed(seed: i64) -> Vec<Spike> {
    let key = LegacyRandom::new(seed).next_i64() & 0xFFFF;
    let mut r = LegacyRandom::new(key);
    // `Util.toShuffledList(IntStream.range(0, 10), random)`.
    let mut sizes: Vec<i32> = (0..10).collect();
    for i in (2..=10).rev() {
        let j = r.next_bounded(i) as usize;
        sizes.swap(i as usize - 1, j);
    }
    (0..10)
        .map(|i| {
            let angle = 2.0 * (-std::f64::consts::PI + 0.314_159_265_358_979_3 * i as f64);
            let size = sizes[i];
            Spike {
                x: (42.0 * angle.cos()).floor() as i32,
                z: (42.0 * angle.sin()).floor() as i32,
                radius: 2 + size / 3,
                height: 76 + size * 3,
                guarded: size == 1 || size == 2,
            }
        })
        .collect()
}

/// `EndSpikeFeature`: the obsidian pillars around the main island, with their crystals.
struct EndSpike {
    spikes: Vec<Spike>,
    crystal_invulnerable: bool,
    beam_target: Option<(i32, i32, i32)>,
}

impl EndSpike {
    fn place_spike(&self, w: &mut World, r: &mut WorldgenRandom, s: &Spike) {
        let k = &w.b.known;
        let (obsidian, air, bedrock) = (k.obsidian, k.air, k.bedrock);
        let radius = s.radius;
        for z in s.z - radius..=s.z + radius {
            for y in w.min_y..=s.height + 10 {
                for x in s.x - radius..=s.x + radius {
                    let (dx, dz) = ((x - s.x) as f64, (z - s.z) as f64);
                    let p = Pos::new(x, y, z);
                    if dx * dx + dz * dz <= (radius * radius + 1) as f64 && y < s.height {
                        w.set(p, obsidian);
                    } else if y > 65 {
                        w.set(p, air);
                    }
                }
            }
        }
        if s.guarded {
            for dx in -2i32..=2 {
                for dz in -2i32..=2 {
                    for dy in 0..=3 {
                        let x_side = dx.abs() == 2;
                        let z_side = dz.abs() == 2;
                        let top = dy == 3;
                        if !(x_side || z_side || top) {
                            continue;
                        }
                        let ns = dx == -2 || dx == 2 || top;
                        let we = dz == -2 || dz == 2 || top;
                        let b = w.b;
                        let mut bars = b.known.iron_bars;
                        bars = b.with_bool(bars, "north", ns && dz != -2);
                        bars = b.with_bool(bars, "south", ns && dz != 2);
                        bars = b.with_bool(bars, "west", we && dx != -2);
                        bars = b.with_bool(bars, "east", we && dx != 2);
                        w.set(Pos::new(s.x + dx, s.height + dy, s.z + dz), bars);
                    }
                }
            }
        }
        let yaw = r.next_f32() * 360.0;
        w.add_entity(Entity::EndCrystal {
            x: s.x as f64 + 0.5,
            y: (s.height + 1) as f64,
            z: s.z as f64 + 0.5,
            yaw,
            beam_target: self.beam_target,
            invulnerable: self.crystal_invulnerable,
        });
        let crystal = Pos::new(s.x, s.height + 1, s.z);
        w.set(crystal.down(), bedrock);
        // `FireBlock.getState` over bedrock (a sturdy top face): plain fire.
        let fire = w.b.known.fire;
        w.set(crystal, fire);
    }
}

impl Place for EndSpike {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let spikes = if self.spikes.is_empty() { spikes_for_seed(w.seed()) } else { self.spikes.clone() };
        for s in &spikes {
            if s.x >> 4 == origin.x >> 4 && s.z >> 4 == origin.z >> 4 {
                self.place_spike(w, r, s);
            }
        }
        true
    }
}

/// `EndGatewayFeature`: a gateway in a bedrock frame.
struct EndGateway {
    exit: Option<(i32, i32, i32)>,
    exact: bool,
}

impl Place for EndGateway {
    fn place(&self, w: &mut World, _r: &mut WorldgenRandom, origin: Pos) -> bool {
        let k = &w.b.known;
        let (gateway, air, bedrock) = (k.end_gateway, k.air, k.bedrock);
        for z in origin.z - 1..=origin.z + 1 {
            for y in origin.y - 2..=origin.y + 2 {
                for x in origin.x - 1..=origin.x + 1 {
                    let p = Pos::new(x, y, z);
                    let same_x = x == origin.x;
                    let same_y = y == origin.y;
                    let same_z = z == origin.z;
                    let edge_y = (y - origin.y).abs() == 2;
                    if same_x && same_y && same_z {
                        w.set(p, gateway);
                        w.add_block_entity(p, BlockEntity::EndGateway { exit: self.exit, exact: self.exact });
                    } else if same_y {
                        w.set(p, air);
                    } else if (edge_y && same_x && same_z) || ((same_x || same_z) && !edge_y) {
                        w.set(p, bedrock);
                    } else {
                        w.set(p, air);
                    }
                }
            }
        }
        true
    }
}

/// `EndIslandFeature`: the small floating islands.
struct EndIsland;

impl Place for EndIsland {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let end_stone = w.b.known.end_stone;
        let mut size = r.next_bounded(3) as f32 + 4.0;
        let mut y = 0;
        while size > 0.5 {
            let (lo, hi) = ((-size).floor() as i32, size.ceil() as i32);
            for x in lo..=hi {
                for z in lo..=hi {
                    if ((x * x + z * z) as f32) <= (size + 1.0) * (size + 1.0) {
                        w.set(origin.add(x, y, z), end_stone);
                    }
                }
            }
            size -= r.next_bounded(2) as f32 + 0.5;
            y -= 1;
        }
        true
    }
}

/// `EndPlatformFeature`: the obsidian arrival platform.
struct EndPlatform;

impl Place for EndPlatform {
    fn place(&self, w: &mut World, _r: &mut WorldgenRandom, origin: Pos) -> bool {
        let (obsidian, air) = (w.b.known.obsidian, w.b.known.air);
        for dz in -2..=2 {
            for dx in -2..=2 {
                for dy in -1..3 {
                    let p = origin.add(dx, dy, dz);
                    let block = if dy == -1 { obsidian } else { air };
                    if !w.b.is_block(w.get(p), block) {
                        w.set(p, block);
                    }
                }
            }
        }
        true
    }
}

/// `ChorusPlantFeature`.
struct ChorusPlant {
    supports: Arc<BlockSet>,
}

impl ChorusPlant {
    /// `ChorusPlantBlock.getStateWithConnections`.
    fn state_at(&self, w: &World, p: Pos) -> u32 {
        let b = w.b;
        let (plant, flower) = (b.known.chorus_plant, b.known.chorus_flower);
        let connects = |q: Pos| {
            let s = w.get(q);
            b.is_block(s, plant) || b.is_block(s, flower)
        };
        let down = w.get(p.down());
        let mut s = plant;
        s = b.with_bool(s, "down", b.is_block(down, plant) || b.is_block(down, flower) || self.supports.has(down));
        s = b.with_bool(s, "up", connects(p.up()));
        s = b.with_bool(s, "north", connects(p.rel(Dir::North)));
        s = b.with_bool(s, "east", connects(p.rel(Dir::East)));
        s = b.with_bool(s, "south", connects(p.rel(Dir::South)));
        b.with_bool(s, "west", connects(p.rel(Dir::West)))
    }

    fn set_plant(&self, w: &mut World, p: Pos) {
        let s = self.state_at(w, p);
        w.set(p, s);
    }

    fn all_neighbors_empty(w: &World, p: Pos, ignore: Option<Dir>) -> bool {
        Dir::HORIZONTAL.iter().all(|&d| Some(d) == ignore || w.is_air(p.rel(d)))
    }

    /// `ChorusFlowerBlock.growTreeRecursive`.
    fn grow(&self, w: &mut World, r: &mut WorldgenRandom, current: Pos, start: Pos, spread: i32, depth: i32) {
        let mut height = r.next_bounded(4) + 1;
        if depth == 0 {
            height += 1;
        }
        for i in 0..height {
            let target = current.up_n(i + 1);
            if !Self::all_neighbors_empty(w, target, None) {
                return;
            }
            self.set_plant(w, target);
            self.set_plant(w, target.down());
        }
        let mut placed_stem = false;
        if depth < 4 {
            let mut stems = r.next_bounded(4);
            if depth == 0 {
                stems += 1;
            }
            for _ in 0..stems {
                let d = Dir::random_horizontal(r);
                let target = current.up_n(height).rel(d);
                if (target.x - start.x).abs() < spread
                    && (target.z - start.z).abs() < spread
                    && w.is_air(target)
                    && w.is_air(target.down())
                    && Self::all_neighbors_empty(w, target, Some(d.opposite()))
                {
                    placed_stem = true;
                    self.set_plant(w, target);
                    self.set_plant(w, target.rel(d.opposite()));
                    self.grow(w, r, target, start, spread, depth + 1);
                }
            }
        }
        if !placed_stem {
            let flower = w.b.with_int(w.b.known.chorus_flower, "age", 5);
            w.set(current.up_n(height), flower);
        }
    }
}

impl Place for ChorusPlant {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        if !w.is_air(origin) || !self.supports.has(w.get(origin.down())) {
            return false;
        }
        self.set_plant(w, origin);
        self.grow(w, r, origin, origin, 8, 0);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spike_positions() {
        // The pillar centers are the same for every seed; sizes are shuffled per seed.
        let centers: Vec<(i32, i32)> = spikes_for_seed(0).iter().map(|s| (s.x, s.z)).collect();
        assert_eq!(centers, [(42, 0), (33, 24), (12, 39), (-13, 39), (-34, 24), (-42, -1), (-34, -25), (-13, -40), (12, -40), (33, -25)]);
        let mut heights: Vec<i32> = spikes_for_seed(12345).iter().map(|s| s.height).collect();
        heights.sort();
        assert_eq!(heights, (0..10).map(|s| 76 + 3 * s).collect::<Vec<_>>());
    }
}
