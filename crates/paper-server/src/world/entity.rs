//! Entities other than players: dropped items, mobs, projectiles, falling blocks, experience
//! orbs and primed TNT. Players are kept by the server; the shared living-entity state
//! ([`LivingState`]) is used by both.
//!
//! This module holds the data and the physics (movement with collisions against block
//! shapes, as vanilla's `Entity.move`). Behaviour that needs the rest of the server (AI,
//! damage, pickup, tracking) lives in `server::entities` and friends.

use super::chunk::ChunkPos;
use super::World;
use paper_data::entity_types::EntityTypeInfo;
use paper_data::physics;
use paper_protocol::text::Component;
use paper_protocol::types::Slot;
use std::collections::HashSet;
use uuid::Uuid;

/// A 3D vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    pub fn scale(self, f: f64) -> Vec3 {
        Vec3::new(self.x * f, self.y * f, self.z * f)
    }

    pub fn length_squared(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    pub fn length(self) -> f64 {
        self.length_squared().sqrt()
    }

    pub fn horizontal_length(self) -> f64 {
        (self.x * self.x + self.z * self.z).sqrt()
    }

    pub fn normalize(self) -> Vec3 {
        let l = self.length();
        if l < 1.0e-4 {
            Vec3::ZERO
        } else {
            self.scale(1.0 / l)
        }
    }

    pub fn distance_squared(self, o: Vec3) -> f64 {
        self.sub(o).length_squared()
    }

    pub fn chunk(self) -> ChunkPos {
        ChunkPos::new((self.x.floor() as i32) >> 4, (self.z.floor() as i32) >> 4)
    }

    /// The unit vector of a yaw/pitch rotation (degrees), as vanilla computes view vectors.
    pub fn from_rotation(yaw: f32, pitch: f32) -> Vec3 {
        let (yaw, pitch) = ((yaw as f64).to_radians(), (pitch as f64).to_radians());
        Vec3::new(-yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos())
    }
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// The box of an entity of the given size standing at `pos` (feet centre).
    pub fn of_entity(pos: Vec3, width: f64, height: f64) -> Self {
        let h = width / 2.0;
        Self { min: Vec3::new(pos.x - h, pos.y, pos.z - h), max: Vec3::new(pos.x + h, pos.y + height, pos.z + h) }
    }

    pub fn offset(self, d: Vec3) -> Aabb {
        Aabb { min: self.min.add(d), max: self.max.add(d) }
    }

    pub fn inflate(self, x: f64, y: f64, z: f64) -> Aabb {
        Aabb { min: Vec3::new(self.min.x - x, self.min.y - y, self.min.z - z), max: Vec3::new(self.max.x + x, self.max.y + y, self.max.z + z) }
    }

    /// The box grown in the direction of `d`.
    pub fn expand_towards(self, d: Vec3) -> Aabb {
        let mut b = self;
        if d.x < 0.0 { b.min.x += d.x } else { b.max.x += d.x }
        if d.y < 0.0 { b.min.y += d.y } else { b.max.y += d.y }
        if d.z < 0.0 { b.min.z += d.z } else { b.max.z += d.z }
        b
    }

    pub fn intersects(&self, o: &Aabb) -> bool {
        self.min.x < o.max.x && self.max.x > o.min.x && self.min.y < o.max.y && self.max.y > o.min.y && self.min.z < o.max.z && self.max.z > o.min.z
    }

    pub fn center(&self) -> Vec3 {
        Vec3::new((self.min.x + self.max.x) / 2.0, (self.min.y + self.max.y) / 2.0, (self.min.z + self.max.z) / 2.0)
    }

    /// How far this box may move along `axis` (0 x, 1 y, 2 z) by `d` before hitting `o`.
    fn clip(&self, o: &Aabb, axis: usize, mut d: f64) -> f64 {
        let (a, b) = match axis {
            0 => ((1, 2), (self.min.x, self.max.x, o.min.x, o.max.x)),
            1 => ((0, 2), (self.min.y, self.max.y, o.min.y, o.max.y)),
            _ => ((0, 1), (self.min.z, self.max.z, o.min.z, o.max.z)),
        };
        let get = |v: &Vec3, i: usize| [v.x, v.y, v.z][i];
        let overlaps = |i: usize| get(&self.min, i) < get(&o.max, i) - 1.0e-7 && get(&self.max, i) > get(&o.min, i) + 1.0e-7;
        if !overlaps(a.0) || !overlaps(a.1) {
            return d;
        }
        let (min, max, omin, omax) = b;
        if d > 0.0 && max <= omin + 1.0e-7 {
            d = d.min(omin - max);
        } else if d < 0.0 && min >= omax - 1.0e-7 {
            d = d.max(omax - min);
        }
        d
    }

    /// Where a ray from `from` to `to` enters this box, as a fraction of the segment.
    pub fn clip_ray(&self, from: Vec3, to: Vec3) -> Option<f64> {
        let d = to.sub(from);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for (o, dv, lo, hi) in [(from.x, d.x, self.min.x, self.max.x), (from.y, d.y, self.min.y, self.max.y), (from.z, d.z, self.min.z, self.max.z)] {
            if dv.abs() < 1.0e-12 {
                if o < lo || o > hi {
                    return None;
                }
            } else {
                let (mut a, mut b) = ((lo - o) / dv, (hi - o) / dv);
                if a > b {
                    std::mem::swap(&mut a, &mut b);
                }
                t0 = t0.max(a);
                t1 = t1.min(b);
                if t0 > t1 {
                    return None;
                }
            }
        }
        Some(t0)
    }
}

/// Collision boxes of the blocks around `region`, in world coordinates. Unloaded chunks
/// count as solid so entities do not fall out of the loaded world.
pub fn block_boxes(world: &World, region: &Aabb) -> Vec<Aabb> {
    let mut out = Vec::new();
    let (x0, x1) = (region.min.x.floor() as i32 - 1, region.max.x.floor() as i32 + 1);
    let (y0, y1) = ((region.min.y.floor() as i32 - 1).max(world.min_y), (region.max.y.floor() as i32 + 1).min(world.max_y() - 1));
    let (z0, z1) = (region.min.z.floor() as i32 - 1, region.max.z.floor() as i32 + 1);
    for x in x0..=x1 {
        for z in z0..=z1 {
            if !world.chunks.contains_key(&ChunkPos::of_block(x, z)) {
                let column = Aabb::new(Vec3::new(x as f64, world.min_y as f64, z as f64), Vec3::new(x as f64 + 1.0, world.max_y() as f64, z as f64 + 1.0));
                if column.intersects(region) {
                    out.push(column);
                }
                continue;
            }
            for y in y0..=y1 {
                let Some(state) = world.block_at(x, y, z) else { continue };
                for b in physics::collision_boxes(state) {
                    let bb = Aabb::new(Vec3::new(x as f64 + b.min[0], y as f64 + b.min[1], z as f64 + b.min[2]), Vec3::new(x as f64 + b.max[0], y as f64 + b.max[1], z as f64 + b.max[2]));
                    if bb.intersects(region) {
                        out.push(bb);
                    }
                }
            }
        }
    }
    out
}

/// Whether a box overlaps any block collision box.
pub fn collides(world: &World, b: &Aabb) -> bool {
    block_boxes(world, b).iter().any(|o| o.intersects(b))
}

/// The result of [`move_box`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Movement {
    pub delta: Vec3,
    pub on_ground: bool,
    pub collided_x: bool,
    pub collided_y: bool,
    pub collided_z: bool,
}

fn collide(boxes: &[Aabb], mut b: Aabb, d: Vec3) -> Vec3 {
    let mut out = Vec3::ZERO;
    // Vertical first, then the larger horizontal component last, as vanilla does.
    let order: [usize; 3] = if d.x.abs() < d.z.abs() { [1, 2, 0] } else { [1, 0, 2] };
    for axis in order {
        let mut v = [d.x, d.y, d.z][axis];
        if v == 0.0 {
            continue;
        }
        for o in boxes {
            v = b.clip(o, axis, v);
            if v == 0.0 {
                break;
            }
        }
        let step = match axis {
            0 => Vec3::new(v, 0.0, 0.0),
            1 => Vec3::new(0.0, v, 0.0),
            _ => Vec3::new(0.0, 0.0, v),
        };
        b = b.offset(step);
        out = out.add(step);
    }
    out
}

/// Moves a box through the world with collisions, stepping up ledges of at most
/// `step_height` when walking on the ground. Returns the allowed movement.
pub fn move_box(world: &World, b: Aabb, d: Vec3, step_height: f64, was_on_ground: bool) -> Movement {
    let region = b.expand_towards(d).inflate(1.0e-7, 1.0e-7, 1.0e-7);
    let boxes = block_boxes(world, &region.expand_towards(Vec3::new(0.0, step_height, 0.0)));
    let mut moved = collide(&boxes, b, d);
    let collided_x = (moved.x - d.x).abs() > 1.0e-7;
    let collided_z = (moved.z - d.z).abs() > 1.0e-7;
    let collided_y = (moved.y - d.y).abs() > 1.0e-7;
    let on_ground = collided_y && d.y < 0.0;
    if step_height > 0.0 && (on_ground || was_on_ground) && (collided_x || collided_z) {
        // Try stepping up: rise, move horizontally, settle back down.
        let up = collide(&boxes, b, Vec3::new(0.0, step_height, 0.0));
        let raised = b.offset(up);
        let across = collide(&boxes, raised, Vec3::new(d.x, 0.0, d.z));
        let down = collide(&boxes, raised.offset(across), Vec3::new(0.0, -(up.y) + d.y.min(0.0), 0.0));
        let stepped = up.add(across).add(down);
        if stepped.horizontal_length() > moved.horizontal_length() + 1.0e-7 {
            moved = stepped;
        }
    }
    Movement {
        delta: moved,
        on_ground: on_ground || (was_on_ground && (moved.y - d.y).abs() > 1.0e-7 && d.y <= 0.0),
        collided_x: (moved.x - d.x).abs() > 1.0e-7,
        collided_y,
        collided_z: (moved.z - d.z).abs() > 1.0e-7,
    }
}

/// State shared by every living entity (players and mobs).
#[derive(Clone, Debug)]
pub struct LivingState {
    pub health: f32,
    pub max_health: f32,
    pub absorption: f32,
    /// Ticks of the red hurt flash left.
    pub hurt_time: i32,
    /// Ticks during which only damage above `last_hurt` hurts.
    pub invulnerable_time: i32,
    pub last_hurt: f32,
    /// Ticks since death (0 while alive).
    pub death_time: i32,
    pub fall_distance: f64,
    /// Ticks left burning.
    pub fire_ticks: i32,
    /// Breath in ticks (300 when full).
    pub air: i32,
    /// Active effects: (effect id, amplifier, ticks left).
    pub effects: Vec<(i32, i32, i32)>,
    /// Who hurt this entity last and when (tick), for kill credit and retaliation.
    pub last_attacker: Option<(i32, u64)>,
}

impl LivingState {
    pub fn new(max_health: f32) -> Self {
        Self { health: max_health, max_health, absorption: 0.0, hurt_time: 0, invulnerable_time: 0, last_hurt: 0.0, death_time: 0, fall_distance: 0.0, fire_ticks: 0, air: 300, effects: Vec::new(), last_attacker: None }
    }

    pub fn is_dead(&self) -> bool {
        self.health <= 0.0
    }

    pub fn effect(&self, id: i32) -> Option<(i32, i32)> {
        self.effects.iter().find(|(e, _, _)| *e == id).map(|(_, a, t)| (*a, *t))
    }
}

/// What a mob is doing.
#[derive(Clone, Debug, Default)]
pub struct MobBrain {
    /// The entity it is attacking (or fleeing from when panicking).
    pub target: Option<i32>,
    /// Remaining path, next node first.
    pub path: Vec<(i32, i32, i32)>,
    /// Ticks until the next random stroll is considered.
    pub idle_ticks: i32,
    pub attack_cooldown: i32,
    pub panic_ticks: i32,
    /// Ticks the current path has been followed (to give up on stuck paths).
    pub path_ticks: i32,
    /// Where it is looking at, if anything.
    pub look_at: Option<Vec3>,
    /// Creeper fuse (0..=30) and whether it is swelling.
    pub swell: i32,
    pub swelling: bool,
    /// Skeleton bow draw time.
    pub charge: i32,
    pub speed_factor: f64,
    /// Ambient sound counter (`ambientSoundTime`).
    pub ambient: i32,
    /// Ticks spent near a breeding partner (`BreedGoal.loveTime`).
    pub love_time: i32,
    /// Where it wants to walk for its own reasons (a villager to its workstation).
    pub walk_target: Option<Vec3>,
}

/// A mob.
#[derive(Clone, Debug)]
pub struct Mob {
    /// Max health, speed, armour, attack damage, ... with equipment and effect modifiers.
    pub attributes: crate::world::attributes::Attributes,
    pub living: LivingState,
    /// Main hand, off hand, feet, legs, chest, head.
    pub equipment: [Slot; 6],
    pub brain: MobBrain,
    /// Never despawns (named, picked up items, spawned by commands or plugins).
    pub persistent: bool,
    pub baby: bool,
    /// Sheep wool colour, etc.
    pub variant: i32,
    pub sheared: bool,
    /// Ticks since a player was near (for despawning).
    pub no_player_ticks: i32,
    /// `AgeableMob` age: negative while a baby (ticks until grown), positive while it
    /// cannot breed again.
    pub age: i32,
    /// Ticks left of being in love (`Animal.inLove`), and the player who fed it.
    pub in_love: i32,
    pub love_cause: Option<i32>,
    /// Chickens: ticks until the next egg.
    pub egg_time: i32,
    /// Villagers and wandering traders: profession, trades, experience, gossip.
    pub merchant: Option<Box<crate::world::merchant::Merchant>>,
}

/// Behaviour-specific data.
#[derive(Clone, Debug)]
pub enum EntityKind {
    Item { stack: Slot, pickup_delay: i32, thrower: Option<Uuid> },
    Mob(Box<Mob>),
    Arrow { damage: f64, owner: Option<i32>, in_ground: bool, life: i32, pickup: bool, critical: bool },
    FallingBlock { state: u32, time: i32 },
    ExperienceOrb { value: i32 },
    Tnt { fuse: i32 },
    /// A thrown eye of ender flying towards `target` (`EyeOfEnder`); after 80 ticks it drops
    /// as an item if `survive`, else it shatters.
    EyeOfEnder { target: Vec3, life: i32, survive: bool },
    /// Any other type: exists and is shown, but does nothing.
    Other,
}

/// An entity in a world.
#[derive(Clone, Debug)]
pub struct Entity {
    pub id: i32,
    pub uuid: Uuid,
    pub info: &'static EntityTypeInfo,
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub age: i32,
    pub no_gravity: bool,
    pub custom_name: Option<Component>,
    pub custom_name_visible: bool,
    pub removed: bool,
    pub kind: EntityKind,
    /// The chunk it is indexed under.
    pub chunk: ChunkPos,
    // --- client tracking ---
    pub viewers: HashSet<i32>,
    pub sent_pos: Vec3,
    pub sent_yaw: f32,
    pub sent_pitch: f32,
    pub sent_head_yaw: f32,
    pub sent_vel: Vec3,
    pub sent_on_ground: bool,
    pub ticks_since_sync: i32,
    pub metadata_dirty: bool,
    pub equipment_dirty: bool,
    pub force_sync: bool,
}

impl Entity {
    pub fn new(id: i32, info: &'static EntityTypeInfo, pos: Vec3, kind: EntityKind) -> Self {
        Self {
            id,
            uuid: Uuid::new_v4(),
            info,
            pos,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: false,
            horizontal_collision: false,
            age: 0,
            no_gravity: false,
            custom_name: None,
            custom_name_visible: false,
            removed: false,
            kind,
            chunk: pos.chunk(),
            viewers: HashSet::new(),
            sent_pos: pos,
            sent_yaw: 0.0,
            sent_pitch: 0.0,
            sent_head_yaw: 0.0,
            sent_vel: Vec3::ZERO,
            sent_on_ground: false,
            ticks_since_sync: 0,
            metadata_dirty: false,
            equipment_dirty: false,
            force_sync: false,
        }
    }

    pub fn type_name(&self) -> &str {
        &self.info.name
    }

    pub fn bounding_box(&self) -> Aabb {
        let scale = if let EntityKind::Mob(m) = &self.kind { if m.baby { 0.5 } else { 1.0 } } else { 1.0 };
        Aabb::of_entity(self.pos, self.info.width as f64 * scale, self.info.height as f64 * scale)
    }

    pub fn eye_pos(&self) -> Vec3 {
        Vec3::new(self.pos.x, self.pos.y + self.info.eye_height as f64, self.pos.z)
    }

    pub fn mob(&self) -> Option<&Mob> {
        match &self.kind {
            EntityKind::Mob(m) => Some(m),
            _ => None,
        }
    }

    pub fn mob_mut(&mut self) -> Option<&mut Mob> {
        match &mut self.kind {
            EntityKind::Mob(m) => Some(m),
            _ => None,
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.removed && self.mob().is_none_or(|m| !m.living.is_dead())
    }

    /// Moves with collisions and updates the ground and collision flags.
    pub fn move_by(&mut self, world: &World, d: Vec3, step_height: f64) -> Movement {
        let m = move_box(world, self.bounding_box(), d, step_height, self.on_ground);
        self.pos = self.pos.add(m.delta);
        self.on_ground = m.on_ground;
        self.horizontal_collision = m.collided_x || m.collided_z;
        if m.collided_x {
            self.vel.x = 0.0;
        }
        if m.collided_z {
            self.vel.z = 0.0;
        }
        if m.collided_y {
            self.vel.y = 0.0;
        }
        m
    }
}

/// Slipperiness of the block under a position (0.6 for most blocks).
pub fn friction_below(world: &World, pos: Vec3) -> f64 {
    let (x, y, z) = (pos.x.floor() as i32, (pos.y - 0.5000001).floor() as i32, pos.z.floor() as i32);
    world.block_at(x, y, z).map_or(0.6, |state| physics::block(paper_data::blocks::block_of_state(state).name).slipperiness as f64)
}
