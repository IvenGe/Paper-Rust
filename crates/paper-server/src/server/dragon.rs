//! The ender dragon and its fight: vanilla's `EnderDragon` (flight, body parts, crystals,
//! death), the phases of its `DragonPhaseManager`, `DragonFireball`, and `EndDragonFight`
//! (the boss bar, spawning the dragon, the exit portal, the egg and the gateways).
//!
//! The dragon flies between 24 nodes above the main island (three rings, inner ones only
//! once the crystals are gone), now and then strafes a player with fireballs or charges at
//! one, and lands on the exit portal to breathe fire before taking off again. Crystals on
//! the pillars heal it; a hit on its head does full damage, elsewhere a quarter. Killed, it
//! flies to the portal and rises for ten seconds, giving 12000 experience the first time
//! (500 later), then the portal opens, the egg appears (the first time) and one of twenty
//! gateways opens around the island.
//!
//! Clients number the dragon's eight parts after the dragon's own id, so a dragon takes nine
//! ids; hits on a part reach the dragon.

use super::mobs::SpawnReason;
use super::survival::{category, Damage};
use super::Server;
use crate::config::GameMode;
use crate::world::dragon_fight::{gateway_position, DragonFight};
use crate::world::entity::{Aabb, EntityKind, Vec3};
use paper_data::blocks;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::types::LpVec3;
use paper_protocol::packets::play;
use paper_protocol::text::Component;
use rand::Rng;

pub const DRAGON: &str = "minecraft:ender_dragon";
/// The chunks of the main island and its pillars, held loaded during a fight.
fn arena_chunks() -> std::ops::RangeInclusive<i32> {
    -3..=2
}
const END: &str = "minecraft:the_end";

/// The parts in the order clients number them after the dragon (`subEntities`), with their
/// sizes.
pub const HEAD: usize = 0;
const NECK: usize = 1;
pub const BODY: usize = 2;
const TAIL: [usize; 3] = [3, 4, 5];
const WING1: usize = 6;
const WING2: usize = 7;
const PART_SIZE: [(f64, f64); 8] = [(1.0, 1.0), (3.0, 3.0), (5.0, 3.0), (2.0, 2.0), (2.0, 2.0), (2.0, 2.0), (4.0, 2.0), (4.0, 2.0)];

/// `EnderDragonPhase`, numbered as `DATA_PHASE` sends them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    HoldingPattern = 0,
    StrafePlayer = 1,
    LandingApproach = 2,
    Landing = 3,
    Takeoff = 4,
    SittingFlaming = 5,
    SittingScanning = 6,
    SittingAttacking = 7,
    ChargingPlayer = 8,
    Dying = 9,
    Hover = 10,
}

impl Phase {
    pub fn from_id(id: i32) -> Phase {
        match id {
            1 => Phase::StrafePlayer,
            2 => Phase::LandingApproach,
            3 => Phase::Landing,
            4 => Phase::Takeoff,
            5 => Phase::SittingFlaming,
            6 => Phase::SittingScanning,
            7 => Phase::SittingAttacking,
            8 => Phase::ChargingPlayer,
            9 => Phase::Dying,
            10 => Phase::Hover,
            _ => Phase::HoldingPattern,
        }
    }

    fn is_sitting(self) -> bool {
        matches!(self, Phase::SittingFlaming | Phase::SittingScanning | Phase::SittingAttacking)
    }

    /// How fast the dragon climbs or dives towards its target (`getFlySpeed`).
    fn fly_speed(self) -> f64 {
        match self {
            Phase::Landing => 1.5,
            Phase::ChargingPlayer | Phase::Dying => 3.0,
            _ => 0.6,
        }
    }
}

/// A path over the flight nodes (`Path`): node positions, and the next one to fly to.
#[derive(Clone, Debug, Default)]
pub struct NodePath {
    nodes: Vec<(i32, i32, i32)>,
    next: usize,
}

impl NodePath {
    fn is_done(&self) -> bool {
        self.next >= self.nodes.len()
    }

    fn advance(&mut self) {
        self.next += 1;
    }

    fn next_pos(&self) -> (i32, i32, i32) {
        self.nodes[self.next.min(self.nodes.len() - 1)]
    }
}

/// What a dragon knows besides what every mob does.
#[derive(Clone, Debug)]
pub struct DragonBrain {
    pub phase: Phase,
    /// Turning speed (`yRotA`).
    y_rot_a: f32,
    /// Recent yaw and height, newest at `pointer` (`positions`).
    positions: [[f64; 2]; 64],
    pointer: i32,
    in_wall: bool,
    sitting_damage: f32,
    pub death_time: i32,
    nearest_crystal: Option<i32>,
    /// Where each part is (feet centre).
    pub parts: [Vec3; 8],
    nodes: Option<Vec<(i32, i32, i32)>>,
    // Phase state. Every phase starts with a fresh path and target (`begin`); the holding
    // pattern's and strafing's directions and the flame count carry over.
    path: Option<NodePath>,
    target: Option<Vec3>,
    holding_clockwise: bool,
    strafe_clockwise: bool,
    strafe_player: Option<i32>,
    fireball_charge: i32,
    phase_ticks: i32,
    first_tick: bool,
    flame_count: i32,
    flame: Option<i32>,
}

impl DragonBrain {
    pub fn new(phase: Phase) -> DragonBrain {
        DragonBrain {
            phase,
            y_rot_a: 0.0,
            positions: [[0.0; 2]; 64],
            pointer: -1,
            in_wall: false,
            sitting_damage: 0.0,
            death_time: 0,
            nearest_crystal: None,
            parts: [Vec3::ZERO; 8],
            nodes: None,
            path: None,
            target: None,
            holding_clockwise: false,
            strafe_clockwise: false,
            strafe_player: None,
            fireball_charge: 0,
            phase_ticks: 0,
            first_tick: true,
            flame_count: 0,
            flame: None,
        }
    }

    /// `getLatencyPos`: yaw and height `offset` ticks ago.
    fn latency(&self, offset: i32) -> (f64, f64) {
        let i = ((self.pointer - offset) & 63) as usize;
        let j = ((self.pointer - offset - 1) & 63) as usize;
        let yaw = self.positions[i][0] + wrap_degrees(self.positions[j][0] - self.positions[i][0]);
        (yaw, self.positions[j][1])
    }

    pub fn part_box(&self, part: usize) -> Aabb {
        let (w, h) = PART_SIZE[part];
        Aabb::of_entity(self.parts[part], w, h)
    }
}

fn wrap_degrees(mut d: f64) -> f64 {
    d %= 360.0;
    if d >= 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    d
}

/// Bits of the flight nodes each node connects to (`nodeAdjacency`).
const ADJACENCY: [u32; 24] = [
    6146, 8197, 8202, 16404, 32808, 32848, 65696, 131392, 131712, 263424, 526848, 525313, 1581057, 3166214, 2138120, 6373424, 4358208, 12910976, 9044480, 9706496, 15216640, 13688832, 11763712, 8257536,
];

fn node_distance(a: (i32, i32, i32), b: (i32, i32, i32)) -> f32 {
    let (dx, dy, dz) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32, (b.2 - a.2) as f32);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// A* over the flight nodes (`EnderDragon.findPath`); `then` is appended as a last stop.
/// Without crystals only the inner nodes are used.
fn find_path(nodes: &[(i32, i32, i32)], start: usize, finish: usize, then: Option<(i32, i32, i32)>, crystals: bool) -> Option<NodePath> {
    let first = if crystals { 0 } else { 12 };
    let mut g = [f32::MAX; 24];
    let mut came_from = [usize::MAX; 24];
    let mut closed = [false; 24];
    let mut open: Vec<usize> = vec![start];
    g[start] = 0.0;
    let h = |i: usize| node_distance(nodes[i], nodes[finish]);
    let mut closest = start;
    let mut reached = false;
    while !open.is_empty() {
        let (pos, &current) = open.iter().enumerate().min_by(|(_, a), (_, b)| (g[**a] + h(**a)).total_cmp(&(g[**b] + h(**b)))).unwrap();
        open.swap_remove(pos);
        if current == finish {
            reached = true;
            break;
        }
        if h(current) < h(closest) {
            closest = current;
        }
        closed[current] = true;
        for next in first..24 {
            if ADJACENCY[current] & (1 << next) == 0 || closed[next] {
                continue;
            }
            let cost = g[current] + node_distance(nodes[current], nodes[next]);
            if !open.contains(&next) || cost < g[next] {
                came_from[next] = current;
                g[next] = cost;
                if !open.contains(&next) {
                    open.push(next);
                }
            }
        }
    }
    let end = if reached { finish } else { closest };
    if !reached && closest == start {
        return None;
    }
    let mut list = vec![nodes[end]];
    let mut at = end;
    while came_from[at] != usize::MAX && at != start {
        at = came_from[at];
        list.push(nodes[at]);
    }
    list.reverse();
    if let Some(t) = then {
        list.push(t);
    }
    Some(NodePath { nodes: list, next: 0 })
}

fn is_dragon(e: &crate::world::entity::Entity) -> bool {
    e.info.name == DRAGON
}

impl Server {
    // --- the dragon ------------------------------------------------------------------------------

    /// The dragon behind a part id the client attacked (parts follow the dragon's id), and
    /// which part it is.
    pub(super) fn dragon_of_part(&self, world: usize, id: i32) -> Option<(i32, usize)> {
        let w = self.worlds.get(world)?;
        if w.entities.contains_key(&id) || self.players.contains_key(&id) {
            return None;
        }
        (1..=8).find_map(|k| w.entities.get(&(id - k)).filter(|e| is_dragon(e)).map(|_| (id - k, (k - 1) as usize)))
    }

    fn dragon_brain(&self, world: usize, id: i32) -> Option<&DragonBrain> {
        self.worlds[world].entities.get(&id)?.mob()?.dragon.as_deref()
    }

    fn dragon_brain_mut(&mut self, world: usize, id: i32) -> Option<&mut DragonBrain> {
        self.worlds[world].entities.get_mut(&id)?.mob_mut()?.dragon.as_deref_mut()
    }

    fn fight_crystals(&self, world: usize) -> i32 {
        self.worlds[world].dragon_fight.as_ref().map_or(0, |f| f.crystals_alive)
    }

    /// Changes the dragon's phase (`DragonPhaseManager.setPhase`): the old phase ends, the
    /// new one begins, clients learn it.
    fn set_dragon_phase(&mut self, world: usize, id: i32, phase: Phase) {
        let Some(b) = self.dragon_brain_mut(world, id) else { return };
        if b.phase == phase {
            return;
        }
        // `end`: the flames go out.
        let flame = if b.phase == Phase::SittingFlaming { b.flame.take() } else { None };
        b.phase = phase;
        b.path = None;
        b.target = None;
        b.phase_ticks = 0;
        b.first_tick = true;
        match phase {
            Phase::StrafePlayer => {
                b.fireball_charge = 0;
                b.strafe_player = None;
            }
            Phase::SittingFlaming => b.flame_count += 1,
            _ => {}
        }
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            e.metadata_dirty = true;
        }
        if let Some(f) = flame {
            if let Some(cloud) = self.worlds[world].entities.get_mut(&f) {
                cloud.removed = true;
            }
        }
    }

    /// The flight nodes (`findClosestNode`'s first call): rings of 12, 8 and 4 at radius 60,
    /// 40 and 20, at least at y 73.
    fn dragon_nodes(&mut self, world: usize, id: i32) -> Vec<(i32, i32, i32)> {
        if let Some(nodes) = self.dragon_brain(world, id).and_then(|b| b.nodes.clone()) {
            return nodes;
        }
        let w = &self.worlds[world];
        let mut nodes = Vec::with_capacity(24);
        for i in 0..24 {
            let (radius, angle, lift) = if i < 12 {
                (60.0f32, std::f32::consts::PI / 12.0 * i as f32, 5)
            } else if i < 20 {
                (40.0, std::f32::consts::PI / 8.0 * (i - 12) as f32, 15)
            } else {
                (20.0, std::f32::consts::PI / 4.0 * (i - 20) as f32, 5)
            };
            let a = 2.0 * (-std::f32::consts::PI + angle);
            let (x, z) = ((radius * a.cos()).floor() as i32, (radius * a.sin()).floor() as i32);
            let y = (w.top_y(x, z).unwrap_or(w.min_y) + lift).max(73);
            nodes.push((x, y, z));
        }
        if let Some(b) = self.dragon_brain_mut(world, id) {
            b.nodes = Some(nodes.clone());
        }
        nodes
    }

    /// The node nearest a point; without crystals only the inner ones (`findClosestNode`).
    fn closest_node(&mut self, world: usize, id: i32, at: Vec3) -> usize {
        let nodes = self.dragon_nodes(world, id);
        let first = if self.fight_crystals(world) == 0 { 12 } else { 0 };
        let p = (at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32);
        let mut best = (f32::MAX, 0);
        for (i, n) in nodes.iter().enumerate().skip(first) {
            let (dx, dy, dz) = ((n.0 - p.0) as f32, (n.1 - p.1) as f32, (n.2 - p.2) as f32);
            let d = dx * dx + dy * dy + dz * dz;
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    }

    fn dragon_path(&mut self, world: usize, id: i32, start: usize, finish: usize, then: Option<(i32, i32, i32)>) -> Option<NodePath> {
        let nodes = self.dragon_nodes(world, id);
        find_path(&nodes, start, finish, then, self.fight_crystals(world) > 0)
    }

    /// Where the dragon lands: on top of the exit portal's pillar (the podium location's
    /// heightmap).
    fn podium_top(&self, world: usize) -> Vec3 {
        let y = self.worlds[world].top_y(0, 0).unwrap_or(64);
        Vec3::new(0.5, y as f64, 0.5)
    }

    /// The next node of the current path, at a random height up to 20 above it
    /// (`navigateToNextPathNode`).
    fn next_path_target(&mut self, world: usize, id: i32) {
        let mut rng = rand::thread_rng();
        let Some(b) = self.dragon_brain_mut(world, id) else { return };
        let Some(path) = b.path.as_mut().filter(|p| !p.is_done()) else { return };
        let (x, y, z) = path.next_pos();
        path.advance();
        b.target = Some(Vec3::new(x as f64, y as f64 + rng.gen::<f32>() as f64 * 20.0, z as f64));
    }

    /// One of the ring nodes next to the dragon's, round the island (`findNewTarget` of the
    /// holding pattern, strafing and taking off).
    fn ring_target(&self, world: usize, from: usize, clockwise: bool, flip: bool) -> usize {
        let mut j = from as i32;
        if flip {
            j += 6;
        }
        j += if clockwise { 1 } else { -1 };
        self.ring_index(world, j)
    }

    /// Wraps a node index onto the outer ring, or the middle one without crystals.
    fn ring_index(&self, world: usize, mut j: i32) -> usize {
        if self.fight_crystals(world) > 0 {
            j = j.rem_euclid(12);
        } else {
            j -= 12;
            j &= 7;
            j += 12;
        }
        j as usize
    }

    fn nearest_player(&self, world: usize, at: Vec3, range: f64, filter: impl Fn(&super::player::Player) -> bool) -> Option<i32> {
        self.players
            .values()
            .filter(|p| p.world == world && !p.dead && matches!(p.game_mode, GameMode::Survival | GameMode::Adventure) && filter(p))
            .map(|p| (p.entity_id, Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(at)))
            .filter(|(_, d)| *d <= range * range)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// The current phase's server tick (`doServerTick`).
    fn dragon_phase_tick(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (pos, yaw, collided) = (e.pos, e.yaw, e.horizontal_collision);
        let Some(b) = self.dragon_brain(world, id) else { return };
        let (phase, target) = (b.phase, b.target);
        let far = |t: Option<Vec3>| t.map_or(0.0, |t| t.distance_squared(pos));
        let mut rng = rand::thread_rng();
        match phase {
            Phase::HoldingPattern => {
                let d = far(target);
                if d < 100.0 || d > 22500.0 || collided {
                    self.holding_new_target(world, id);
                }
            }
            Phase::StrafePlayer => self.strafe_tick(world, id),
            Phase::LandingApproach => {
                let d = far(target);
                if d < 100.0 || d > 22500.0 || collided {
                    let path_done = self.dragon_brain(world, id).is_none_or(|b| b.path.as_ref().is_none_or(NodePath::is_done));
                    if path_done {
                        let start = self.closest_node(world, id, pos);
                        let podium = self.podium_top(world);
                        let finish = match self.nearest_player(world, podium, 128.0, |_| true) {
                            Some(p) => {
                                let p = &self.players[&p];
                                let v = Vec3::new(p.loc.x, 0.0, p.loc.z).normalize();
                                self.closest_node(world, id, Vec3::new(-v.x * 40.0, 105.0, -v.z * 40.0))
                            }
                            None => self.closest_node(world, id, Vec3::new(40.0, podium.y, 0.0)),
                        };
                        let then = (podium.x.floor() as i32, podium.y as i32, podium.z.floor() as i32);
                        let mut path = self.dragon_path(world, id, start, finish, Some(then));
                        if let Some(p) = path.as_mut() {
                            p.advance();
                        }
                        if let Some(b) = self.dragon_brain_mut(world, id) {
                            b.path = path;
                        }
                    }
                    self.next_path_target(world, id);
                    if self.dragon_brain(world, id).is_some_and(|b| b.path.as_ref().is_some_and(NodePath::is_done)) {
                        self.set_dragon_phase(world, id, Phase::Landing);
                    }
                }
            }
            Phase::Landing => {
                let podium = self.podium_top(world);
                let target = target.unwrap_or(podium);
                if let Some(b) = self.dragon_brain_mut(world, id) {
                    b.target = Some(target);
                }
                if target.distance_squared(pos) < 1.0 {
                    if let Some(b) = self.dragon_brain_mut(world, id) {
                        b.flame_count = 0;
                    }
                    self.set_dragon_phase(world, id, Phase::SittingScanning);
                }
            }
            Phase::SittingScanning => {
                let ticks = self.dragon_brain(world, id).map_or(0, |b| b.phase_ticks) + 1;
                if let Some(b) = self.dragon_brain_mut(world, id) {
                    b.phase_ticks = ticks;
                }
                let near = self.nearest_player(world, pos, 20.0, |p| (p.loc.y - pos.y).abs() <= 10.0);
                match near {
                    Some(p) if ticks > 25 => {
                        let _ = p;
                        self.set_dragon_phase(world, id, Phase::SittingAttacking);
                    }
                    Some(p) => {
                        // Turn to face the player.
                        let (px, pz) = (self.players[&p].loc.x, self.players[&p].loc.z);
                        let head = self.dragon_brain(world, id).map_or(pos, |b| b.parts[HEAD]);
                        let to = Vec3::new(px - pos.x, 0.0, pz - pos.z).normalize();
                        let facing = Vec3::new((yaw as f64).to_radians().sin(), 0.0, -(yaw as f64).to_radians().cos()).normalize();
                        let angle = (facing.x * to.x + facing.z * to.z).clamp(-1.0, 1.0).acos().to_degrees() + 0.5;
                        if !(0.0..=10.0).contains(&angle) {
                            let (dx, dz) = (px - head.x, pz - head.z);
                            let turn = wrap_degrees(180.0 - dx.atan2(dz).to_degrees() - yaw as f64).clamp(-100.0, 100.0);
                            let dist = ((dx * dx + dz * dz).sqrt() + 1.0) as f32;
                            let capped = dist.min(40.0);
                            let Some(b) = self.dragon_brain_mut(world, id) else { return };
                            b.y_rot_a *= 0.8;
                            b.y_rot_a += turn as f32 * (0.7 / capped / dist);
                            let delta = b.y_rot_a;
                            if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                                e.yaw += delta;
                            }
                        }
                    }
                    None if ticks >= 100 => {
                        let charge_at = self.nearest_player(world, pos, 150.0, |_| true);
                        self.set_dragon_phase(world, id, Phase::Takeoff);
                        if let Some(p) = charge_at {
                            let at = Vec3::new(self.players[&p].loc.x, self.players[&p].loc.y, self.players[&p].loc.z);
                            self.set_dragon_phase(world, id, Phase::ChargingPlayer);
                            if let Some(b) = self.dragon_brain_mut(world, id) {
                                b.target = Some(at);
                            }
                        }
                    }
                    None => {}
                }
            }
            Phase::SittingAttacking => {
                let Some(b) = self.dragon_brain_mut(world, id) else { return };
                b.phase_ticks += 1;
                if b.phase_ticks >= 40 {
                    self.set_dragon_phase(world, id, Phase::SittingFlaming);
                }
            }
            Phase::SittingFlaming => {
                let Some(b) = self.dragon_brain_mut(world, id) else { return };
                b.phase_ticks += 1;
                let (ticks, count, head) = (b.phase_ticks, b.flame_count, b.parts[HEAD]);
                if ticks >= 200 {
                    self.set_dragon_phase(world, id, if count >= 4 { Phase::Takeoff } else { Phase::SittingScanning });
                } else if ticks == 10 {
                    // The flames land on the ground in front of the head.
                    let dir = Vec3::new(head.x - pos.x, 0.0, head.z - pos.z).normalize();
                    let (x, z) = (head.x + dir.x * 5.0 / 2.0, head.z + dir.z * 5.0 / 2.0);
                    let top = head.y + PART_SIZE[HEAD].1 / 2.0;
                    let w = &self.worlds[world];
                    let mut y = top;
                    while w.block_at(x.floor() as i32, y.floor() as i32, z.floor() as i32).is_some_and(blocks::is_air) {
                        y -= 1.0;
                        if y < 0.0 {
                            y = top;
                            break;
                        }
                    }
                    let at = Vec3::new(x, y.floor() + 1.0, z);
                    let flame = self.spawn_breath_cloud(world, at, id, "minecraft:harming", 5.0, 200, 0.0);
                    if let Some(b) = self.dragon_brain_mut(world, id) {
                        b.flame = flame;
                    }
                }
            }
            Phase::Takeoff => {
                let first = self.dragon_brain(world, id).is_some_and(|b| b.first_tick || b.path.is_none());
                if !first {
                    let podium = self.podium_top(world);
                    if podium.distance_squared(pos) > 100.0 {
                        self.set_dragon_phase(world, id, Phase::HoldingPattern);
                    }
                    return;
                }
                if let Some(b) = self.dragon_brain_mut(world, id) {
                    b.first_tick = false;
                }
                let start = self.closest_node(world, id, pos);
                // Away from where the head looks (`getHeadLookVector` with the pitch raised).
                let podium = self.podium_top(world);
                let f = ((podium.distance_squared(pos)).sqrt() as f32 / 4.0).max(1.0);
                let pitch = -(6.0 / f) * 1.5 * 5.0;
                let look = Vec3::from_rotation(yaw, pitch);
                let near = self.closest_node(world, id, Vec3::new(-look.x * 40.0, 105.0, -look.z * 40.0)) as i32;
                let finish = self.ring_index(world, near);
                let mut path = self.dragon_path(world, id, start, finish, None);
                if let Some(p) = path.as_mut() {
                    p.advance();
                }
                if let Some(b) = self.dragon_brain_mut(world, id) {
                    b.path = path;
                }
                self.next_path_target(world, id);
            }
            Phase::ChargingPlayer => {
                let Some(target) = target else {
                    self.set_dragon_phase(world, id, Phase::HoldingPattern);
                    return;
                };
                let Some(b) = self.dragon_brain_mut(world, id) else { return };
                if b.phase_ticks > 0 {
                    b.phase_ticks += 1;
                    if b.phase_ticks > 10 {
                        self.set_dragon_phase(world, id, Phase::HoldingPattern);
                    }
                    return;
                }
                let d = target.distance_squared(pos);
                if d < 100.0 || d > 22500.0 || collided {
                    b.phase_ticks += 1;
                }
            }
            Phase::Dying => {
                let podium = self.podium_top(world);
                let target = target.unwrap_or(podium);
                if let Some(b) = self.dragon_brain_mut(world, id) {
                    b.target = Some(target);
                    b.phase_ticks += 1;
                }
                let d = target.distance_squared(pos);
                let health = if !(100.0..=22500.0).contains(&d) || collided { 0.0 } else { 1.0 };
                if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                    m.living.health = health;
                }
                if health <= 0.0 {
                    if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                        e.metadata_dirty = true;
                    }
                }
            }
            Phase::Hover => {
                if target.is_none() {
                    if let Some(b) = self.dragon_brain_mut(world, id) {
                        b.target = Some(pos);
                    }
                }
            }
        }
        let _ = &mut rng;
    }

    /// `DragonHoldingPatternPhase.findNewTarget`: at the end of a path the dragon may go
    /// to land (more likely with fewer crystals) or strafe a player; otherwise it flies on
    /// round the ring.
    fn holding_new_target(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let pos = e.pos;
        let done = self.dragon_brain(world, id).is_some_and(|b| b.path.as_ref().is_some_and(NodePath::is_done));
        let mut rng = rand::thread_rng();
        if done {
            let crystals = self.fight_crystals(world).max(0);
            if rng.gen_range(0..crystals + 3) == 0 {
                self.set_dragon_phase(world, id, Phase::LandingApproach);
                return;
            }
            let podium = self.podium_top(world);
            let player = self.nearest_player(world, podium, 128.0, |_| true);
            let d = player.map_or(64.0, |p| {
                let p = &self.players[&p];
                Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(podium) / 512.0
            });
            if let Some(p) = player {
                if rng.gen_range(0..(d + 2.0) as i32) == 0 || rng.gen_range(0..crystals + 2) == 0 {
                    self.strafe(world, id, p);
                    return;
                }
            }
        }
        let path_over = self.dragon_brain(world, id).is_none_or(|b| b.path.as_ref().is_none_or(NodePath::is_done));
        if path_over {
            let start = self.closest_node(world, id, pos);
            let flip = rng.gen_range(0..8) == 0;
            let clockwise = {
                let Some(b) = self.dragon_brain_mut(world, id) else { return };
                if flip {
                    b.holding_clockwise = !b.holding_clockwise;
                }
                b.holding_clockwise
            };
            let finish = self.ring_target(world, start, clockwise, flip);
            let mut path = self.dragon_path(world, id, start, finish, None);
            if let Some(p) = path.as_mut() {
                p.advance();
            }
            if let Some(b) = self.dragon_brain_mut(world, id) {
                b.path = path;
            }
        }
        self.next_path_target(world, id);
    }

    /// Starts strafing a player (`DragonStrafePlayerPhase.setTarget`).
    fn strafe(&mut self, world: usize, id: i32, player: i32) {
        self.set_dragon_phase(world, id, Phase::StrafePlayer);
        let Some(p) = self.players.get(&player) else { return };
        let at = Vec3::new(p.loc.x, p.loc.y, p.loc.z);
        let Some(pos) = self.worlds[world].entities.get(&id).map(|e| e.pos) else { return };
        let start = self.closest_node(world, id, pos);
        let finish = self.closest_node(world, id, at);
        let (bx, bz) = (at.x.floor(), at.z.floor());
        let dist = ((bx - pos.x).powi(2) + (bz - pos.z).powi(2)).sqrt();
        let lift = (0.4 + dist / 80.0 - 1.0).min(10.0);
        let then = (bx as i32, (at.y + lift).floor() as i32, bz as i32);
        let mut path = self.dragon_path(world, id, start, finish, Some(then));
        if let Some(b) = self.dragon_brain_mut(world, id) {
            b.strafe_player = Some(player);
            if let Some(p) = path.as_mut() {
                p.advance();
            }
            b.path = path;
        }
        self.next_path_target(world, id);
    }

    /// `DragonStrafePlayerPhase.doServerTick`: flies at the player and, once it looks at
    /// them from within 64 blocks for a quarter second, spits a fireball and goes back to
    /// circling.
    fn strafe_tick(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (pos, yaw) = (e.pos, e.yaw);
        let Some(b) = self.dragon_brain(world, id) else { return };
        let (player, target, path_done) = (b.strafe_player, b.target, b.path.as_ref().is_some_and(NodePath::is_done));
        let Some(p) = player.and_then(|p| self.players.get(&p)).filter(|p| p.world == world && !p.dead) else {
            self.set_dragon_phase(world, id, Phase::HoldingPattern);
            return;
        };
        let at = Vec3::new(p.loc.x, p.loc.y, p.loc.z);
        let eyes = Vec3::new(p.loc.x, p.loc.y + p.eye_height(), p.loc.z);
        if path_done {
            let dist = ((at.x - pos.x).powi(2) + (at.z - pos.z).powi(2)).sqrt();
            let lift = (0.4 + dist / 80.0 - 1.0).min(10.0);
            if let Some(b) = self.dragon_brain_mut(world, id) {
                b.target = Some(Vec3::new(at.x, at.y + lift, at.z));
            }
        }
        let d = self.dragon_brain(world, id).and_then(|b| b.target).map_or(0.0, |t| t.distance_squared(pos));
        if d < 100.0 || d > 22500.0 {
            let over = self.dragon_brain(world, id).is_none_or(|b| b.path.as_ref().is_none_or(NodePath::is_done));
            if over {
                let start = self.closest_node(world, id, pos);
                let flip = rand::thread_rng().gen_range(0..8) == 0;
                let clockwise = {
                    let Some(b) = self.dragon_brain_mut(world, id) else { return };
                    if flip {
                        b.strafe_clockwise = !b.strafe_clockwise;
                    }
                    b.strafe_clockwise
                };
                let finish = self.ring_target(world, start, clockwise, flip);
                let mut path = self.dragon_path(world, id, start, finish, None);
                if let Some(p) = path.as_mut() {
                    p.advance();
                }
                if let Some(b) = self.dragon_brain_mut(world, id) {
                    b.path = path;
                }
            }
            self.next_path_target(world, id);
        }
        let _ = target;
        let in_range = at.distance_squared(pos) < 4096.0;
        let sees = in_range && super::combat::clip_blocks(&self.worlds[world], Vec3::new(pos.x, pos.y + 4.0, pos.z), eyes).is_none();
        let Some(b) = self.dragon_brain_mut(world, id) else { return };
        if !sees {
            b.fireball_charge = (b.fireball_charge - 1).max(0);
            return;
        }
        b.fireball_charge += 1;
        let to = Vec3::new(at.x - pos.x, 0.0, at.z - pos.z).normalize();
        let facing = Vec3::new((yaw as f64).to_radians().sin(), 0.0, -(yaw as f64).to_radians().cos()).normalize();
        let angle = (facing.x * to.x + facing.z * to.z).clamp(-1.0, 1.0).acos().to_degrees() + 0.5;
        if b.fireball_charge < 5 || !(0.0..10.0).contains(&angle) {
            return;
        }
        b.fireball_charge = 0;
        if let Some(path) = b.path.as_mut() {
            path.next = path.nodes.len();
        }
        let head = b.parts[HEAD];
        let view = Vec3::from_rotation(yaw, 0.0);
        let from = Vec3::new(head.x - view.x, head.y + PART_SIZE[HEAD].1 / 2.0 + 0.5, head.z - view.z);
        let aim = Vec3::new(eyes.x - from.x, at.y + 0.9 - from.y, eyes.z - from.z);
        self.broadcast_level_event(world, 1017, BlockPos::new(pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32), 0, false);
        self.spawn_thrown(world, "minecraft:dragon_fireball", from, aim, 0.1, 0.0, 0.0, paper_protocol::types::Slot::EMPTY, Some(id));
        self.set_dragon_phase(world, id, Phase::HoldingPattern);
    }

    /// A cloud of dragon's breath (`AreaEffectCloud` with the dragon's breath particle).
    pub(super) fn spawn_breath_cloud(&mut self, world: usize, at: Vec3, owner: i32, potion: &str, radius: f32, duration: i32, growth: f32) -> Option<i32> {
        let info = paper_data::entity_types::by_name("minecraft:area_effect_cloud")?;
        let kind = EntityKind::Cloud { potion: potion.to_owned(), owner: Some(owner), radius, duration, wait: 20, uses: Vec::new(), growth, on_use: 0.0, breath: true };
        let e = crate::world::entity::Entity::new(self.next_entity_id(), info, at, kind);
        Some(self.spawn_entity(world, e))
    }

    /// A dragon's fireball bursts (`DragonFireball.onHit`) into a cloud of breath that grows
    /// from three to seven blocks over half a minute, on whoever stood within four blocks.
    pub(super) fn dragon_fireball_hits(&mut self, world: usize, id: i32, at: Vec3, owner: Option<i32>) {
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            e.removed = true;
        }
        let area = Aabb::of_entity(at, 1.0, 1.0).inflate(4.0, 2.0, 4.0);
        let mut place = at;
        let near_player = self.players.values().filter(|p| p.world == world && !p.dead).map(|p| Vec3::new(p.loc.x, p.loc.y, p.loc.z)).find(|p| area.intersects(&Aabb::of_entity(*p, 0.6, 1.8)) && p.distance_squared(at) < 16.0);
        if let Some(p) = near_player {
            place = p;
        }
        self.broadcast_level_event(world, 2006, BlockPos::new(at.x.floor() as i32, at.y.floor() as i32, at.z.floor() as i32), 1, false);
        if let Some(owner) = owner {
            self.spawn_breath_cloud(world, place, owner, "minecraft:strong_harming", 3.0, 600, (7.0 - 3.0) / 600.0);
        }
    }

    /// Healing from the nearest crystal: one point every half second, the crystal within
    /// 32 blocks looked for now and then (`checkCrystals`).
    fn dragon_crystals(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (age, area, pos) = (e.age, e.bounding_box().inflate(32.0, 32.0, 32.0), e.pos);
        let crystal = self.dragon_brain(world, id).and_then(|b| b.nearest_crystal);
        let w = &self.worlds[world];
        let alive = crystal.is_some_and(|c| w.entities.get(&c).is_some_and(|c| !c.removed));
        if crystal.is_some() && !alive {
            if let Some(b) = self.dragon_brain_mut(world, id) {
                b.nearest_crystal = None;
            }
        } else if alive && age % 10 == 0 {
            if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                if m.living.health < m.living.max_health {
                    m.living.health = (m.living.health + 1.0).min(m.living.max_health);
                }
            }
        }
        if rand::thread_rng().gen_range(0..10) == 0 {
            let w = &self.worlds[world];
            let nearest = w
                .entities_in(&area)
                .into_iter()
                .filter_map(|c| w.entities.get(&c))
                .filter(|c| c.info.name == "minecraft:end_crystal" && !c.removed)
                .map(|c| (c.id, c.pos.distance_squared(pos)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(c, _)| c);
            if let Some(b) = self.dragon_brain_mut(world, id) {
                b.nearest_crystal = nearest;
            }
        }
    }

    /// A crystal was destroyed (`EnderDragon.onCrystalDestroyed`): if it was healing the
    /// dragon, the dragon takes 10 damage; a circling dragon turns on the player.
    pub(super) fn crystal_destroyed(&mut self, world: usize, crystal: i32, at: Vec3, attacker: Option<i32>) {
        if self.worlds[world].dimension != END {
            return;
        }
        self.update_crystal_count(world);
        let Some(dragon) = self.worlds[world].entities.values().find(|e| is_dragon(e) && !e.removed).map(|e| e.id) else { return };
        let player = attacker.filter(|a| self.players.contains_key(a)).or_else(|| self.nearest_player(world, at, 64.0, |_| true));
        if self.dragon_brain(world, dragon).is_some_and(|b| b.nearest_crystal == Some(crystal)) {
            let mut damage = Damage::new("minecraft:explosion");
            damage.attacker = player;
            damage.direct = Some(crystal);
            self.hurt_dragon(world, dragon, HEAD, damage, 10.0);
        }
        if let Some(p) = player {
            if self.dragon_brain(world, dragon).is_some_and(|b| b.phase == Phase::HoldingPattern) {
                self.strafe(world, dragon, p);
            }
        }
    }

    /// `EnderDragon.hurt(part, ...)`: the head takes full damage, other parts a quarter
    /// (plus up to one); only players and explosions hurt it; arrows bounce off while it sits;
    /// a deadly blow in flight sends it into its death flight; enough damage while sitting
    /// makes it take off. Returns whether it was hit.
    pub(super) fn hurt_dragon(&mut self, world: usize, id: i32, part: usize, damage: Damage, mut amount: f32) -> bool {
        let Some(phase) = self.dragon_brain(world, id).map(|b| b.phase) else { return false };
        // `/kill` ends the fight at once (`EnderDragon.kill`).
        if damage.kind == "minecraft:generic_kill" {
            let Some(e) = self.worlds[world].entities.get_mut(&id) else { return false };
            e.removed = true;
            let uuid = e.uuid;
            self.dragon_killed(world, uuid);
            return true;
        }
        if phase == Phase::Dying || self.dragon_brain(world, id).is_some_and(|b| b.death_time > 0) {
            return false;
        }
        let arrow = damage.direct.is_some_and(|d| self.worlds[world].entities.get(&d).is_some_and(|e| matches!(e.kind, EntityKind::Arrow { .. })));
        if phase.is_sitting() && arrow {
            if let Some(d) = damage.direct {
                self.set_on_fire(world, d, 20);
            }
            amount = 0.0;
        }
        if part != HEAD {
            amount = amount / 4.0 + amount.min(1.0);
        }
        if amount < 0.01 {
            return false;
        }
        let by_player = damage.attacker.is_some_and(|a| self.players.contains_key(&a));
        if !by_player && !damage.is("always_hurts_ender_dragons") {
            return true;
        }
        if !crate::plugins::events::on_entity_damage(self, id, &damage, &mut amount) {
            return false;
        }
        let tick = self.tick;
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return false };
        let pos = e.pos;
        let Some(m) = e.mob_mut() else { return false };
        let living = &mut m.living;
        let before = living.health;
        let full = if living.invulnerable_time > 10 {
            if amount <= living.last_hurt {
                return false;
            }
            let extra = amount - living.last_hurt;
            living.last_hurt = amount;
            amount = extra;
            false
        } else {
            living.last_hurt = amount;
            living.invulnerable_time = 20;
            living.hurt_time = 10;
            true
        };
        living.health = (living.health - amount).max(0.0);
        if let Some(a) = damage.attacker {
            living.last_attacker = Some((a, tick));
        }
        let dead = living.health <= 0.0;
        let after = living.health;
        e.metadata_dirty = true;
        if full {
            let event = play::DamageEvent { entity_id: id, source_type: damage.network_id(), cause_id: damage.attacker, direct_id: damage.direct, source_position: damage.position.map(|v| (v.x, v.y, v.z)) };
            self.broadcast_entity(world, id, bytes::Bytes::from(paper_protocol::packets::encode_packet(&event)));
            if !dead {
                self.play_sound(world, pos, "minecraft:entity.ender_dragon.hurt", category::HOSTILE, 5.0, 1.0, None);
            }
        }
        if dead && !phase.is_sitting() {
            if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                m.living.health = 1.0;
            }
            self.set_dragon_phase(world, id, Phase::Dying);
        }
        if phase.is_sitting() {
            let max = self.worlds[world].entities.get(&id).and_then(|e| e.mob()).map_or(200.0, |m| m.living.max_health);
            let Some(b) = self.dragon_brain_mut(world, id) else { return true };
            b.sitting_damage += before - after;
            if b.sitting_damage > 0.25 * max {
                b.sitting_damage = 0.0;
                self.set_dragon_phase(world, id, Phase::Takeoff);
            }
        }
        true
    }

    /// A dragon's tick (`EnderDragon.aiStep` and `tickDeath`).
    pub(super) fn tick_dragon(&mut self, world: usize, id: i32) {
        let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) else { return };
        if m.dragon.is_none() {
            m.dragon = Some(Box::new(DragonBrain::new(Phase::HoldingPattern)));
        }
        let dying = m.living.health <= 0.0;
        if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
            if m.living.invulnerable_time > 0 {
                m.living.invulnerable_time -= 1;
            }
            if m.living.hurt_time > 0 {
                m.living.hurt_time -= 1;
            }
        }
        if dying {
            self.dragon_death_tick(world, id);
            return;
        }
        self.dragon_crystals(world, id);
        // Remember yaw and height (`positions`).
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            e.yaw = wrap_degrees(e.yaw as f64) as f32;
            let (yaw, y) = (e.yaw as f64, e.pos.y);
            if let Some(b) = e.mob_mut().and_then(|m| m.dragon.as_deref_mut()) {
                if b.pointer < 0 {
                    for p in b.positions.iter_mut() {
                        *p = [yaw, y];
                    }
                }
                b.pointer = (b.pointer + 1) & 63;
                b.positions[b.pointer as usize] = [yaw, y];
            }
        }
        let before = self.dragon_brain(world, id).map(|b| b.phase);
        self.dragon_phase_tick(world, id);
        if self.dragon_brain(world, id).map(|b| b.phase) != before {
            self.dragon_phase_tick(world, id);
        }
        self.dragon_fly(world, id);
        self.dragon_parts(world, id);
        // The fight follows its dragon.
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (uuid, health, max) = (e.uuid, e.mob().map_or(0.0, |m| m.living.health), e.mob().map_or(200.0, |m| m.living.max_health));
        if let Some(fight) = self.worlds[world].dragon_fight.as_mut() {
            if fight.dragon == Some(uuid) {
                fight.ticks_since_dragon_seen = 0;
                fight.progress = (health / max).clamp(0.0, 1.0);
            }
        }
    }

    /// Flies towards the phase's target (`aiStep`'s movement).
    fn dragon_fly(&mut self, world: usize, id: i32) {
        let Some(b) = self.dragon_brain(world, id) else { return };
        let (target, phase, in_wall) = (b.target, b.phase, b.in_wall);
        if phase.is_sitting() {
            return;
        }
        let Some(target) = target else { return };
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let (dx, mut dy, dz) = (target.x - e.pos.x, target.y - e.pos.y, target.z - e.pos.z);
        let dist2 = dx * dx + dy * dy + dz * dz;
        let speed = phase.fly_speed();
        let flat = (dx * dx + dz * dz).sqrt();
        if flat > 0.0 {
            dy = (dy / flat).clamp(-speed, speed);
        }
        e.vel.y += dy * 0.01;
        let yaw = wrap_degrees(e.yaw as f64);
        let to = Vec3::new(dx, target.y - e.pos.y, dz).normalize();
        let facing = Vec3::new(yaw.to_radians().sin(), e.vel.y, -yaw.to_radians().cos()).normalize();
        let align = ((facing.x * to.x + facing.y * to.y + facing.z * to.z + 0.5) / 1.5).max(0.0);
        let horizontal = (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt() as f32 + 1.0;
        let Some(b) = e.mob_mut().and_then(|m| m.dragon.as_deref_mut()) else { return };
        let mut new_yaw = yaw as f32;
        if dx.abs() > 1.0e-5 || dz.abs() > 1.0e-5 {
            let turn = wrap_degrees(180.0 - dx.atan2(dz).to_degrees() - yaw).clamp(-50.0, 50.0) as f32;
            let turn_speed = if phase == Phase::Landing { horizontal.min(40.0) / horizontal } else { 0.7 / horizontal.min(40.0) / horizontal };
            b.y_rot_a *= 0.8;
            b.y_rot_a += turn * turn_speed;
            new_yaw += b.y_rot_a * 0.1;
        }
        e.yaw = new_yaw;
        // `moveRelative(0.06 * ..., (0, 0, -1))`: forward is (sin yaw, -cos yaw) for a dragon.
        let n = (2.0 / (dist2 + 1.0)) as f64;
        let push = 0.06 * (align * n + (1.0 - n));
        let r = (new_yaw as f64).to_radians();
        e.vel.x += push * r.sin();
        e.vel.z += -push * r.cos();
        let step = if in_wall { e.vel.scale(0.8) } else { e.vel };
        e.pos = e.pos.add(step);
        let v = e.vel.normalize();
        let f = Vec3::new(r.sin(), e.vel.y, -r.cos()).normalize();
        let damping = 0.8 + 0.15 * ((v.x * f.x + v.y * f.y + v.z * f.z) + 1.0) / 2.0;
        e.vel = Vec3::new(e.vel.x * damping, e.vel.y * 0.91, e.vel.z * damping);
        let from_chunk = e.chunk;
        if e.pos.chunk() != from_chunk {
            self.worlds[world].reindex_entity(id);
        }
    }

    /// Places the parts around the body, knocks back what the wings touch, bites what the
    /// head touches and breaks through blocks (`aiStep`'s second half).
    fn dragon_parts(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (pos, yaw, hurt_time) = (e.pos, e.yaw, e.mob().map_or(0, |m| m.living.hurt_time));
        let Some(b) = self.dragon_brain(world, id) else { return };
        let sitting = b.phase.is_sitting();
        let y_rot_a = b.y_rot_a;
        let (l5, l10) = (b.latency(5), b.latency(10));
        let tilt = ((l5.1 - l10.1) * 10.0).to_radians();
        let (cos_t, sin_t) = (tilt.cos(), tilt.sin());
        let r = (yaw as f64).to_radians();
        let (sin_y, cos_y) = (r.sin(), r.cos());
        let head_y = if sitting { -1.0 } else { l5.1 - b.latency(0).1 };
        let mut parts = b.parts;
        parts[BODY] = Vec3::new(pos.x + sin_y * 0.5, pos.y, pos.z - cos_y * 0.5);
        parts[WING1] = Vec3::new(pos.x + cos_y * 4.5, pos.y + 2.0, pos.z + sin_y * 4.5);
        parts[WING2] = Vec3::new(pos.x - cos_y * 4.5, pos.y + 2.0, pos.z - sin_y * 4.5);
        let h = r - (y_rot_a as f64 * 0.01).to_radians();
        let (hs, hc) = (h.sin(), h.cos());
        parts[HEAD] = Vec3::new(pos.x + hs * 6.5 * cos_t, pos.y + head_y + sin_t * 6.5, pos.z - hc * 6.5 * cos_t);
        parts[NECK] = Vec3::new(pos.x + hs * 5.5 * cos_t, pos.y + head_y + sin_t * 5.5, pos.z - hc * 5.5 * cos_t);
        for (i, part) in TAIL.iter().enumerate() {
            let back = b.latency(12 + i as i32 * 2);
            let a = r + wrap_degrees(back.0 - l5.0).to_radians();
            let (sa, ca) = (a.sin(), a.cos());
            let len = (i + 1) as f64 * 2.0;
            parts[*part] = Vec3::new(pos.x - (sin_y * 1.5 + sa * len) * cos_t, pos.y + back.1 - l5.1 - (len + 1.5) * sin_t + 1.5, pos.z + (cos_y * 1.5 + ca * len) * cos_t);
        }
        if let Some(b) = self.dragon_brain_mut(world, id) {
            b.parts = parts;
        }
        let part_box = |p: usize| Aabb::of_entity(parts[p], PART_SIZE[p].0, PART_SIZE[p].1);
        if hurt_time == 0 {
            let body_center = parts[BODY];
            for wing in [WING1, WING2] {
                let area = part_box(wing).inflate(4.0, 2.0, 4.0).offset(Vec3::new(0.0, -2.0, 0.0));
                for victim in self.living_in(world, &area, id) {
                    let Some(at) = self.entity_position(world, victim) else { continue };
                    let (fx, fz) = (at.x - body_center.x, at.z - body_center.z);
                    let d = (fx * fx + fz * fz).max(0.1);
                    self.push_entity(world, victim, Vec3::new(fx / d * 4.0, 0.2, fz / d * 4.0));
                    if !sitting {
                        self.hurt_entity(world, victim, Damage::by("minecraft:mob_attack", id, pos), 5.0);
                    }
                }
            }
            for bite in [HEAD, NECK] {
                let area = part_box(bite).inflate(1.0, 1.0, 1.0);
                for victim in self.living_in(world, &area, id) {
                    self.hurt_entity(world, victim, Damage::by("minecraft:mob_attack", id, pos), 10.0);
                }
            }
        }
        let in_wall = [HEAD, NECK, BODY].iter().fold(false, |acc, p| self.dragon_breaks_blocks(world, part_box(*p)) | acc);
        if let Some(b) = self.dragon_brain_mut(world, id) {
            b.in_wall = in_wall;
        }
    }

    /// Players (not in creative or spectator) and mobs other than `except` touching `area`.
    fn living_in(&self, world: usize, area: &Aabb, except: i32) -> Vec<i32> {
        let mut out: Vec<i32> = self
            .players
            .values()
            .filter(|p| p.world == world && !p.dead && matches!(p.game_mode, GameMode::Survival | GameMode::Adventure) && Aabb::of_entity(Vec3::new(p.loc.x, p.loc.y, p.loc.z), 0.6, 1.8).intersects(area))
            .map(|p| p.entity_id)
            .collect();
        let w = &self.worlds[world];
        out.extend(w.entities_in(area).into_iter().filter(|e| *e != except && w.entities.get(e).is_some_and(|e| e.mob().is_some_and(|m| !m.living.is_dead()))));
        out
    }

    /// Adds to a player's or mob's motion (`Entity.push`).
    fn push_entity(&mut self, world: usize, id: i32, by: Vec3) {
        if self.players.contains_key(&id) {
            self.send(id, &play::SetEntityMotion { entity_id: VarInt(id), velocity: LpVec3(by.x, by.y, by.z) });
            return;
        }
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            e.vel = e.vel.add(by);
            e.force_sync = true;
        }
    }

    /// `checkWalls`: with mob griefing the dragon breaks what is not dragon-immune; returns
    /// whether something stopped it.
    fn dragon_breaks_blocks(&mut self, world: usize, b: Aabb) -> bool {
        let griefing = self.worlds[world].rules.bool("mob_griefing");
        let immune = paper_data::synced::tag("minecraft:block", "minecraft:dragon_immune");
        let transparent = paper_data::synced::tag("minecraft:block", "minecraft:dragon_transparent");
        let in_tag = |tag: Option<&[i32]>, state: u32| tag.is_some_and(|ids| ids.contains(&(blocks::block_id_of_state(state) as i32)));
        let (mut stopped, mut broke) = (false, Vec::new());
        for x in b.min.x.floor() as i32..=b.max.x.floor() as i32 {
            for y in b.min.y.floor() as i32..=b.max.y.floor() as i32 {
                for z in b.min.z.floor() as i32..=b.max.z.floor() as i32 {
                    let Some(state) = self.worlds[world].block_at(x, y, z) else { continue };
                    if blocks::is_air(state) || in_tag(transparent, state) {
                        continue;
                    }
                    if griefing && !in_tag(immune, state) {
                        broke.push(BlockPos::new(x, y, z));
                    } else {
                        stopped = true;
                    }
                }
            }
        }
        let any = !broke.is_empty();
        for pos in broke {
            self.set_block_and_update(world, pos, blocks::AIR);
        }
        if any {
            let mut rng = rand::thread_rng();
            let at = BlockPos::new(rng.gen_range(b.min.x.floor() as i32..=b.max.x.floor() as i32), rng.gen_range(b.min.y.floor() as i32..=b.max.y.floor() as i32), rng.gen_range(b.min.z.floor() as i32..=b.max.z.floor() as i32));
            self.broadcast_level_event(world, 2008, at, 0, false);
        }
        stopped
    }

    /// `tickDeath`: ten seconds rising and turning while experience pours out, then the
    /// fight is won.
    fn dragon_death_tick(&mut self, world: usize, id: i32) {
        let first = self.worlds[world].dragon_fight.as_ref().is_some_and(|f| !f.previously_killed);
        let total = if first { 12000 } else { 500 };
        let drops = self.worlds[world].rules.bool("mob_drops");
        let Some(b) = self.dragon_brain_mut(world, id) else { return };
        b.death_time += 1;
        let t = b.death_time;
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        e.pos.y += 0.1;
        e.yaw = wrap_degrees(e.yaw as f64 + 20.0) as f32;
        let (pos, uuid) = (e.pos, e.uuid);
        let block = BlockPos::new(pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32);
        if t == 1 {
            self.broadcast_level_event(world, 1028, block, 0, true);
        }
        if t > 150 && t % 5 == 0 && drops {
            self.award_experience(world, pos, (total as f32 * 0.08).floor() as i32);
        }
        if t >= 200 {
            if drops {
                self.award_experience(world, pos, (total as f32 * 0.2).floor() as i32);
            }
            if let Some(e) = self.worlds[world].entities.get_mut(&id) {
                e.removed = true;
            }
            self.dragon_killed(world, uuid);
        }
    }

    // --- the fight ------------------------------------------------------------------------

    /// Every End's fight (`EndDragonFight.tick`).
    pub(super) fn tick_dragon_fights(&mut self) {
        for world in 0..self.worlds.len() {
            if self.worlds[world].dimension == END && self.worlds[world].generator.name() != "flat" {
                self.tick_dragon_fight(world);
            }
        }
    }

    fn tick_dragon_fight(&mut self, world: usize) {
        let seed = self.worlds[world].seed;
        self.worlds[world].dragon_fight.get_or_insert_with(|| DragonFight::new(seed));
        self.finish_gateway_exits(world);
        let Some(fight) = self.worlds[world].dragon_fight.as_mut() else { return };
        // A gateway opens once its chunk is there.
        if let Some(at) = fight.pending_gateway {
            let chunk = crate::world::chunk::ChunkPos::of_block(at.x, at.z);
            if self.worlds[world].is_ready(chunk) {
                if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
                    f.pending_gateway = None;
                }
                self.place_gateway(world, at);
                self.worlds[world].remove_ticket(chunk);
            }
        }
        let Some(fight) = self.worlds[world].dragon_fight.as_mut() else { return };
        fight.ticks_since_player_scan += 1;
        if fight.ticks_since_player_scan >= 20 {
            fight.ticks_since_player_scan = 0;
            self.update_fight_players(world);
        }
        self.update_boss_bar(world);
        // The island stays loaded while anyone is near it.
        let Some(fight) = self.worlds[world].dragon_fight.as_mut() else { return };
        let near = !fight.viewers.is_empty();
        if near != fight.arena_ticket {
            fight.arena_ticket = near;
            for cx in arena_chunks() {
                for cz in arena_chunks() {
                    let pos = crate::world::chunk::ChunkPos::new(cx, cz);
                    if near {
                        self.worlds[world].add_ticket(pos);
                    } else {
                        self.worlds[world].remove_ticket(pos);
                    }
                }
            }
        }
        if !near {
            return;
        }
        let Some(fight) = self.worlds[world].dragon_fight.as_ref() else { return };
        let loaded = arena_chunks().all(|cx| arena_chunks().all(|cz| self.worlds[world].is_ready(crate::world::chunk::ChunkPos::new(cx, cz))));
        if fight.needs_scan && loaded {
            self.scan_fight(world);
        }
        let Some(fight) = self.worlds[world].dragon_fight.as_mut() else { return };
        if fight.dragon_killed {
            return;
        }
        fight.ticks_since_dragon_seen += 1;
        let missing = fight.dragon.is_none() || fight.ticks_since_dragon_seen >= 1200;
        fight.ticks_since_crystals += 1;
        let count = fight.ticks_since_crystals >= 100;
        if missing && loaded {
            let existing = self.worlds[world].entities.values().find(|e| is_dragon(e) && !e.removed).map(|e| e.uuid);
            let Some(fight) = self.worlds[world].dragon_fight.as_mut() else { return };
            fight.ticks_since_dragon_seen = 0;
            match existing {
                Some(uuid) => fight.dragon = Some(uuid),
                None => self.spawn_fight_dragon(world),
            }
        }
        if count && loaded {
            if let Some(fight) = self.worlds[world].dragon_fight.as_mut() {
                fight.ticks_since_crystals = 0;
            }
            self.update_crystal_count(world);
        }
    }

    /// `EndDragonFight.scanState`: an open exit portal means the dragon was killed before;
    /// a dragon without a portal goes.
    fn scan_fight(&mut self, world: usize) {
        let active = self.find_exit_portal(world, true).is_some();
        let dragons: Vec<(i32, uuid::Uuid)> = self.worlds[world].entities.values().filter(|e| is_dragon(e) && !e.removed).map(|e| (e.id, e.uuid)).collect();
        if !active && self.find_exit_portal(world, false).is_none() {
            self.place_exit_portal(world, false);
        }
        let Some(fight) = self.worlds[world].dragon_fight.as_mut() else { return };
        fight.needs_scan = false;
        fight.previously_killed = active;
        match dragons.first() {
            None => fight.dragon_killed = true,
            Some((id, uuid)) => {
                fight.dragon_killed = false;
                fight.dragon = Some(*uuid);
                if !active {
                    // `scanState` removes a dragon that has no portal; here every new world
                    // starts like that, so a dragon present before the first scan stays.
                    let _ = id;
                }
            }
        }
        if !fight.previously_killed && fight.dragon_killed {
            fight.dragon_killed = false;
        }
    }

    /// The exit portal's floor centre: saved, or found under the pillar at the origin
    /// (`findExitPortal`); `active` asks for an open one.
    fn find_exit_portal(&mut self, world: usize, active: bool) -> Option<BlockPos> {
        let w = &self.worlds[world];
        let portal = blocks::by_name("minecraft:end_portal").map(|b| b.default_state);
        let floor = match self.worlds[world].dragon_fight.as_ref().and_then(|f| f.exit_portal) {
            Some(p) => p,
            None => {
                let mut y = w.top_y(0, 0)? - 1;
                let bedrock = blocks::default_state("minecraft:bedrock");
                while y > 63 && w.block_at(0, y, 0) == bedrock {
                    y -= 1;
                }
                BlockPos::new(0, y.max(w.min_y + 1), 0)
            }
        };
        let is_portal = |x: i32, z: i32| w.block_at(x, floor.y, z).is_some_and(|s| Some(s) == portal);
        let rim = w.block_at(3, floor.y, 0).is_some_and(|s| blocks::block_of_state(s).name == "minecraft:bedrock");
        let found = if active { is_portal(1, 0) && is_portal(-1, 0) } else { rim };
        if !found {
            return None;
        }
        if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
            f.exit_portal = Some(floor);
        }
        Some(floor)
    }

    /// `EndPodiumFeature`: the bedrock bowl with the pillar and torches, its floor portal
    /// blocks when `active`.
    pub(super) fn place_exit_portal(&mut self, world: usize, active: bool) {
        let floor = match self.worlds[world].dragon_fight.as_ref().and_then(|f| f.exit_portal) {
            Some(p) => p,
            None => {
                let w = &self.worlds[world];
                let mut y = w.top_y(0, 0).unwrap_or(64) - 1;
                let bedrock = blocks::default_state("minecraft:bedrock");
                while y > 63 && w.block_at(0, y, 0) == bedrock {
                    y -= 1;
                }
                BlockPos::new(0, y.max(w.min_y + 1), 0)
            }
        };
        if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
            f.exit_portal = Some(floor);
        }
        let state = |n: &str| blocks::default_state(n).unwrap_or(blocks::AIR);
        let (bedrock, end_stone, portal) = (state("minecraft:bedrock"), state("minecraft:end_stone"), state("minecraft:end_portal"));
        let y = floor.y;
        for by in y - 1..=y + 32 {
            for z in -4..=4 {
                for x in -4..=4 {
                    let d2 = (x * x + (by - y) * (by - y) + z * z) as f64;
                    let inner = d2 < 2.5 * 2.5;
                    if !inner && d2 >= 3.5 * 3.5 {
                        continue;
                    }
                    let s = if by < y {
                        if inner {
                            bedrock
                        } else {
                            end_stone
                        }
                    } else if by > y {
                        blocks::AIR
                    } else if !inner {
                        bedrock
                    } else if active {
                        portal
                    } else {
                        blocks::AIR
                    };
                    self.set_block_with_flags(world, BlockPos::new(x, by, z), s, crate::world::block_updates::flags::CLIENTS);
                }
            }
        }
        for h in 0..4 {
            self.set_block_with_flags(world, BlockPos::new(0, y + h, 0), bedrock, crate::world::block_updates::flags::CLIENTS);
        }
        let torch = state("minecraft:wall_torch");
        for (dx, dz, facing) in [(0, -1, "north"), (0, 1, "south"), (-1, 0, "west"), (1, 0, "east")] {
            let s = blocks::with_property(torch, "facing", facing).unwrap_or(torch);
            self.set_block_with_flags(world, BlockPos::new(dx, y + 2, dz), s, crate::world::block_updates::flags::CLIENTS);
        }
    }

    /// `createNewDragon`: above the island, circling.
    fn spawn_fight_dragon(&mut self, world: usize) {
        let yaw = rand::thread_rng().gen::<f32>() * 360.0;
        let Some(id) = self.spawn_mob(world, DRAGON, Vec3::new(0.0, 128.0, 0.0), SpawnReason::Natural) else { return };
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        e.yaw = yaw;
        let uuid = e.uuid;
        if let Some(m) = e.mob_mut() {
            m.persistent = true;
            m.dragon = Some(Box::new(DragonBrain::new(Phase::HoldingPattern)));
        }
        if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
            f.dragon = Some(uuid);
        }
    }

    /// `updateCrystalCount`: the crystals still on top of the pillars.
    fn update_crystal_count(&mut self, world: usize) {
        let seed = self.worlds[world].seed;
        let w = &self.worlds[world];
        let mut count = 0;
        for spike in paper_worldgen::feature::spikes_for_seed(seed) {
            let r = spike.radius as f64;
            let area = Aabb::new(Vec3::new(spike.x as f64 - r, w.min_y as f64, spike.z as f64 - r), Vec3::new(spike.x as f64 + r, w.max_y() as f64, spike.z as f64 + r));
            count += w.entities_in(&area).into_iter().filter(|c| w.entities.get(c).is_some_and(|c| c.info.name == "minecraft:end_crystal" && !c.removed)).count() as i32;
        }
        if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
            f.crystals_alive = count;
        }
    }

    /// `setDragonKilled`: the bar goes, the portal opens, a gateway appears and, the first
    /// time, the egg.
    fn dragon_killed(&mut self, world: usize, uuid: uuid::Uuid) {
        let Some(fight) = self.worlds[world].dragon_fight.as_ref() else { return };
        if fight.dragon != Some(uuid) {
            return;
        }
        let first = !fight.previously_killed;
        if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
            f.progress = 0.0;
            f.dragon_killed = true;
        }
        self.update_boss_bar(world);
        self.place_exit_portal(world, true);
        self.open_next_gateway(world);
        if first {
            let y = self.worlds[world].top_y(0, 0).unwrap_or(68);
            if let Some(egg) = blocks::default_state("minecraft:dragon_egg") {
                self.set_block_and_update(world, BlockPos::new(0, y, 0), egg);
            }
        }
        if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
            f.previously_killed = true;
        }
    }

    /// `spawnNewGateway`: the next of the twenty gateways (`EndGatewayFeature` without an
    /// exit: bedrock above, below and around a gateway block).
    fn open_next_gateway(&mut self, world: usize) {
        let Some(index) = self.worlds[world].dragon_fight.as_mut().and_then(|f| f.gateways.pop()) else { return };
        let at = gateway_position(index);
        // Its chunk is loaded first (out beyond the island).
        let chunk = crate::world::chunk::ChunkPos::of_block(at.x, at.z);
        if !self.worlds[world].is_ready(chunk) {
            self.worlds[world].add_ticket(chunk);
            if let Some(f) = self.worlds[world].dragon_fight.as_mut() {
                f.pending_gateway = Some(at);
            }
            return;
        }
        self.place_gateway(world, at);
    }

    /// `EndGatewayFeature` without an exit: bedrock above, below and around a gateway block.
    fn place_gateway(&mut self, world: usize, at: BlockPos) {
        self.broadcast_level_event(world, 3000, at, 0, false);
        self.place_gateway_blocks(world, at, None);
    }

    /// The gateway structure, its block entity leading to `exit` (or nowhere yet).
    fn place_gateway_blocks(&mut self, world: usize, at: BlockPos, exit: Option<BlockPos>) {
        let (bedrock, gateway) = (blocks::default_state("minecraft:bedrock").unwrap_or(0), blocks::default_state("minecraft:end_gateway").unwrap_or(0));
        for dx in -1..=1 {
            for dy in -2i32..=2 {
                for dz in -1..=1 {
                    let (cx, cz, level, cap) = (dx == 0, dz == 0, dy == 0, dy.abs() == 2);
                    let state = if cx && level && cz {
                        gateway
                    } else if level || (cap && !(cx && cz)) {
                        blocks::AIR
                    } else if (cap && cx && cz) || ((cx || cz) && !cap) {
                        bedrock
                    } else {
                        blocks::AIR
                    };
                    self.set_block_with_flags(world, at.offset(dx, dy, dz), state, crate::world::block_updates::flags::CLIENTS);
                }
            }
        }
        self.set_gateway_exit(world, at, exit);
    }

    /// A gateway's block entity (`TheEndGatewayBlockEntity`): where it leads, if anywhere.
    fn set_gateway_exit(&mut self, world: usize, at: BlockPos, exit: Option<BlockPos>) {
        use paper_nbt::{Compound, Tag};
        let chunk = crate::world::chunk::ChunkPos::of_block(at.x, at.z);
        let Some(c) = self.worlds[world].chunks.get_mut(&chunk) else { return };
        let key = (at.y, (at.z & 15) as u8, (at.x & 15) as u8);
        let nbt = c.block_entities.entry(key).or_insert_with(|| Compound::new().with("id", Tag::String("minecraft:end_gateway".into())).with("Age", Tag::Long(0)));
        match exit {
            Some(e) => {
                nbt.insert("exit_portal", Tag::IntArray(vec![e.x, e.y, e.z]));
                nbt.insert("ExactTeleport", Tag::Byte(0));
            }
            None => {
                nbt.remove("exit_portal");
            }
        }
        c.dirty = true;
    }

    // --- where gateways lead --------------------------------------------------------------

    /// Someone stepped into a gateway that leads nowhere yet
    /// (`TheEndGatewayBlockEntity.findOrCreateValidTeleportPos`): out along the gateway's
    /// direction, from 1024 blocks back in to the gap and then out to the first land (from
    /// the noise alone); that land is loaded before the exit is made.
    pub(super) fn search_gateway_exit(&mut self, player: i32, world: usize, gateway: BlockPos) {
        if self.worlds[world].dimension != END {
            return;
        }
        let seed = self.worlds[world].seed;
        let fight = self.worlds[world].dragon_fight.get_or_insert_with(|| DragonFight::new(seed));
        if let Some(search) = fight.exit_searches.iter_mut().find(|s| s.gateway == gateway) {
            if !search.players.contains(&player) {
                search.players.push(player);
            }
            return;
        }
        let Some(g) = self.worlds[world].generator.staged() else { return };
        let dir = Vec3::new(gateway.x as f64, 0.0, gateway.z as f64).normalize();
        let land = |p: Vec3| g.inner.chunk_has_terrain(p.x.floor() as i32 >> 4, p.z.floor() as i32 >> 4);
        let mut at = dir.scale(1024.0);
        for _ in 0..16 {
            if !land(at) {
                break;
            }
            at = at.sub(dir.scale(16.0));
        }
        for _ in 0..16 {
            if land(at) {
                break;
            }
            at = at.add(dir.scale(16.0));
        }
        let (x, z) = (at.x.floor() as i32, at.z.floor() as i32);
        let mut chunks = Vec::new();
        for cx in (x - 16) >> 4..=(x + 16) >> 4 {
            for cz in (z - 16) >> 4..=(z + 16) >> 4 {
                chunks.push(crate::world::chunk::ChunkPos::new(cx, cz));
            }
        }
        for c in &chunks {
            self.worlds[world].add_ticket(*c);
        }
        if let Some(fight) = self.worlds[world].dragon_fight.as_mut() {
            fight.exit_searches.push(crate::world::dragon_fight::ExitSearch { gateway, at: (x, z), players: vec![player], chunks });
        }
    }

    /// Searches whose land has loaded get their exit: on end stone with room above near the
    /// middle of the world, or on a new little island; a return gateway ten blocks above the
    /// tallest block around it; then those who stepped in go through.
    fn finish_gateway_exits(&mut self, world: usize) {
        let Some(fight) = self.worlds[world].dragon_fight.as_mut() else { return };
        if fight.exit_searches.is_empty() {
            return;
        }
        let searches = std::mem::take(&mut fight.exit_searches);
        let mut waiting = Vec::new();
        for search in searches {
            if !search.chunks.iter().all(|c| self.worlds[world].is_ready(*c)) {
                waiting.push(search);
                continue;
            }
            let spawn = match self.valid_spawn_in_chunk(world, crate::world::chunk::ChunkPos::of_block(search.at.0, search.at.1)) {
                Some(p) => p,
                None => {
                    let p = BlockPos::new(search.at.0, 75, search.at.1);
                    self.place_end_island(world, p);
                    p
                }
            };
            let tallest = self.tallest_block(world, spawn, 16, true);
            let exit = tallest.offset(0, 10, 0);
            self.place_gateway_blocks(world, exit, Some(search.gateway));
            self.set_gateway_exit(world, search.gateway, Some(exit));
            for c in &search.chunks {
                self.worlds[world].remove_ticket(*c);
            }
            for p in search.players {
                if self.players.get(&p).is_some_and(|pl| pl.world == world && !pl.dead) {
                    self.use_end_gateway(p, world, search.gateway);
                }
            }
        }
        if let Some(fight) = self.worlds[world].dragon_fight.as_mut() {
            fight.exit_searches.extend(waiting);
        }
    }

    /// `findValidSpawnInChunk`: end stone with two free blocks above, nearest the world's
    /// middle.
    fn valid_spawn_in_chunk(&self, world: usize, chunk: crate::world::chunk::ChunkPos) -> Option<BlockPos> {
        let w = &self.worlds[world];
        let end_stone = blocks::default_state("minecraft:end_stone")?;
        let full = |x: i32, y: i32, z: i32| w.block_at(x, y, z).is_some_and(|s| blocks::flags(s) & blocks::FLAG_FULL_CUBE != 0);
        let mut best: Option<(f64, BlockPos)> = None;
        for x in chunk.x * 16..chunk.x * 16 + 16 {
            for z in chunk.z * 16..chunk.z * 16 + 16 {
                for y in 30..w.max_y() - 2 {
                    if w.block_at(x, y, z) != Some(end_stone) || full(x, y + 1, z) || full(x, y + 2, z) {
                        continue;
                    }
                    let d = (x as f64 + 0.5).powi(2) + (y as f64 + 0.5).powi(2) + (z as f64 + 0.5).powi(2);
                    if best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, BlockPos::new(x, y, z)));
                    }
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// `findTallestBlock`: the highest full block within `radius` columns.
    fn tallest_block(&self, world: usize, pos: BlockPos, radius: i32, bedrock: bool) -> BlockPos {
        let w = &self.worlds[world];
        let mut best: Option<BlockPos> = None;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx == 0 && dz == 0 && !bedrock {
                    continue;
                }
                let floor = best.map_or(w.min_y, |b| b.y);
                let mut y = w.max_y() - 1;
                while y > floor {
                    let at = BlockPos::new(pos.x + dx, y, pos.z + dz);
                    if let Some(s) = w.block_at(at.x, at.y, at.z) {
                        if blocks::flags(s) & blocks::FLAG_FULL_CUBE != 0 && (bedrock || blocks::block_of_state(s).name != "minecraft:bedrock") {
                            best = Some(at);
                            break;
                        }
                    }
                    y -= 1;
                }
            }
        }
        best.unwrap_or(pos)
    }

    /// `EndIslandFeature`: a little cone of end stone.
    fn place_end_island(&mut self, world: usize, at: BlockPos) {
        let Some(end_stone) = blocks::default_state("minecraft:end_stone") else { return };
        let mut rng = rand::thread_rng();
        let mut f = rng.gen_range(0..3) as f32 + 4.0;
        let mut dy = 0;
        while f > 0.5 {
            for dx in (-f).floor() as i32..=f.ceil() as i32 {
                for dz in (-f).floor() as i32..=f.ceil() as i32 {
                    if ((dx * dx + dz * dz) as f32) <= (f + 1.0) * (f + 1.0) {
                        self.set_block_with_flags(world, at.offset(dx, dy, dz), end_stone, crate::world::block_updates::flags::CLIENTS);
                    }
                }
            }
            f -= rng.gen_range(0..2) as f32 + 0.5;
            dy -= 1;
        }
    }

    /// Who sees the dragon's bar: players in the End within 192 blocks of the island's
    /// middle (`updatePlayers`).
    fn update_fight_players(&mut self, world: usize) {
        let valid: std::collections::HashSet<i32> = self
            .players
            .values()
            .filter(|p| p.world == world && !p.dead && {
                let (dx, dy, dz) = (p.loc.x, p.loc.y - 128.0, p.loc.z);
                dx * dx + dy * dy + dz * dz <= 192.0 * 192.0
            })
            .map(|p| p.entity_id)
            .collect();
        let Some(fight) = self.worlds[world].dragon_fight.as_mut() else { return };
        let gone: Vec<i32> = fight.viewers.difference(&valid).copied().collect();
        let added: Vec<i32> = valid.difference(&fight.viewers).copied().collect();
        fight.viewers = valid;
        let (bar, killed, progress) = (fight.boss_bar, fight.dragon_killed, fight.progress);
        for v in gone {
            self.send(v, &play::BossEvent { uuid: bar, action: play::BossEventAction::Remove });
        }
        if !killed {
            for v in added {
                self.send(v, &play::BossEvent { uuid: bar, action: play::BossEventAction::Add { title: Component::translatable("entity.minecraft.ender_dragon", vec![]), health: progress, color: 0, division: 0, flags: 2 | 4 } });
            }
        }
    }

    /// Keeps the bar's progress (and its visibility once the dragon is dead) up to date.
    fn update_boss_bar(&mut self, world: usize) {
        let Some(fight) = self.worlds[world].dragon_fight.as_mut() else { return };
        let (bar, viewers, killed, progress) = (fight.boss_bar, fight.viewers.clone(), fight.dragon_killed, fight.progress);
        if killed {
            if !viewers.is_empty() {
                fight.viewers.clear();
                for v in viewers {
                    self.send(v, &play::BossEvent { uuid: bar, action: play::BossEventAction::Remove });
                }
            }
            return;
        }
        if (fight.sent_progress - progress).abs() < 1.0e-4 {
            return;
        }
        fight.sent_progress = progress;
        for v in viewers {
            self.send(v, &play::BossEvent { uuid: bar, action: play::BossEventAction::UpdateHealth(progress) });
        }
    }
}
