//! Trees: trunk placers, foliage placers, root placers, decorators, the leaf distance
//! update and the edge shape update; fallen trees.
//!
//! Derived from `pumpkin-world/src/generation/feature/features/tree/**` and
//! `fallen_tree.rs` of Pumpkin (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, and checked
//! against vanilla 26.3 (`TreeFeature`, `TrunkPlacer`, `FoliagePlacer`, the decorators).

use crate::chunk::Heightmap;
use crate::data::JsonExt;
use crate::feature::blockinfo::{BlockSet, Fluid};
use crate::feature::javaset::JavaPosSet;
use crate::feature::parse::Parser;
use crate::feature::provider::StateProvider;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::{BlockEntity, Feature, Place};
use crate::math::mth_cos;
use crate::math::mth_sin;
use crate::providers::IntProvider;
use crate::random::{Random, WorldgenRandom};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

pub(super) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(Some(match t {
        "tree" => Box::new(Tree::parse(p, v)?),
        "fallen_tree" => Box::new(FallenTree {
            trunk: p.provider(v.field("trunk_provider")?)?,
            log_length: p.int_provider(v.field("log_length")?)?,
            stump_decorators: decorators(p, v.get("stump_decorators"))?,
            log_decorators: decorators(p, v.get("log_decorators"))?,
        }),
        _ => return Ok(None),
    }))
}

fn int_or(p: &Parser, v: &Value, key: &str) -> Result<IntProvider> {
    let x = v.field(key)?;
    // `UniformInt` may be written without a type.
    if x.is_object() && x.get("type").is_none() && x.get("min_inclusive").is_some() {
        return Ok(IntProvider::Uniform(x.i64_or("min_inclusive", 0) as i32, x.i64_or("max_inclusive", 0) as i32));
    }
    p.int_provider(x)
}

/// Positions a tree has placed, by role.
#[derive(Default)]
struct Parts {
    logs: Vec<Pos>,
    foliage: Vec<Pos>,
    foliage_set: HashSet<Pos>,
    roots: Vec<Pos>,
}

impl Parts {
    fn set_log(&mut self, w: &mut World, p: Pos, s: u32) {
        self.logs.push(p);
        w.set(p, s);
    }

    fn set_foliage(&mut self, w: &mut World, p: Pos, s: u32) {
        self.foliage.push(p);
        self.foliage_set.insert(p);
        w.set(p, s);
    }

    fn set_root(&mut self, w: &mut World, p: Pos, s: u32) {
        self.roots.push(p);
        w.set(p, s);
    }
}

/// `TreeFeature.validTreePos`: air or replaceable by trees.
fn valid_tree_pos(w: &World, p: Pos) -> bool {
    let s = w.get(p);
    w.b.is_air(s) || w.b.tags.replaceable_by_trees.has(s)
}

fn is_air_or_leaves(w: &World, p: Pos) -> bool {
    let s = w.get(p);
    w.b.is_air(s) || w.b.tags.leaves.has(s)
}

struct TreeEnv<'t> {
    trunk: &'t StateProvider,
    below_trunk: &'t StateProvider,
    can_grow_through: Option<&'t BlockSet>,
}

impl TreeEnv<'_> {
    /// `TrunkPlacer.isFree`.
    fn is_free(&self, w: &World, p: Pos) -> bool {
        valid_tree_pos(w, p) || w.b.tags.logs.has(w.get(p)) || self.can_grow_through.is_some_and(|c| c.has(w.get(p)))
    }

    /// `TrunkPlacer.placeLog` with a state modifier.
    fn place_log_with(&self, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, p: Pos, modify: impl FnOnce(u32) -> u32) -> bool {
        if valid_tree_pos(w, p) {
            let s = modify(self.trunk.get(w, r, p));
            parts.set_log(w, p, s);
            true
        } else {
            false
        }
    }

    fn place_log(&self, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, p: Pos) -> bool {
        self.place_log_with(w, r, parts, p, |s| s)
    }

    fn place_log_if_free(&self, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, p: Pos) {
        if self.is_free(w, p) {
            self.place_log(w, r, parts, p);
        }
    }

    /// `TrunkPlacer.placeBelowTrunkBlock`.
    fn below_trunk(&self, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, p: Pos) {
        if let Some(s) = self.below_trunk.get_optional(w, r, p) {
            parts.set_log(w, p, s);
        }
    }
}

#[derive(Clone, Copy)]
struct Attachment {
    pos: Pos,
    radius_offset: i32,
    double_trunk: bool,
}

impl Attachment {
    fn new(pos: Pos, radius_offset: i32, double_trunk: bool) -> Self {
        Self { pos, radius_offset, double_trunk }
    }
}

enum TrunkKind {
    Straight,
    Forking,
    Giant,
    MegaJungle,
    DarkOak,
    Fancy,
    Bending { min_height_for_leaves: i32, bend_length: IntProvider },
    UpwardsBranching { extra_branch_steps: IntProvider, branch_probability: f32, extra_branch_length: IntProvider, can_grow_through: Arc<BlockSet> },
    Cherry { count: IntProvider, horizontal_length: IntProvider, start_offset: (i32, i32), end_offset: IntProvider },
    Poplar { above_branches: IntProvider, branch_amount: IntProvider },
}

struct TrunkPlacer {
    base: i32,
    rand_a: i32,
    rand_b: i32,
    kind: TrunkKind,
}

impl TrunkPlacer {
    fn parse(p: &mut Parser, v: &Value) -> Result<Self> {
        let t = v.str_field("type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        let kind = match t {
            "straight_trunk_placer" => TrunkKind::Straight,
            "forking_trunk_placer" => TrunkKind::Forking,
            "giant_trunk_placer" => TrunkKind::Giant,
            "mega_jungle_trunk_placer" => TrunkKind::MegaJungle,
            "dark_oak_trunk_placer" => TrunkKind::DarkOak,
            "fancy_trunk_placer" => TrunkKind::Fancy,
            "bending_trunk_placer" => TrunkKind::Bending { min_height_for_leaves: v.i64_or("min_height_for_leaves", 1) as i32, bend_length: int_or(p, v, "bend_length")? },
            "upwards_branching_trunk_placer" => TrunkKind::UpwardsBranching {
                extra_branch_steps: int_or(p, v, "extra_branch_steps")?,
                branch_probability: v.f64_or("place_branch_per_log_probability", 0.0) as f32,
                extra_branch_length: int_or(p, v, "extra_branch_length")?,
                can_grow_through: p.block_set(v.field("can_grow_through")?)?,
            },
            "cherry_trunk_placer" => {
                let start = int_or(p, v, "branch_start_offset_from_top")?;
                let start_offset = match start {
                    IntProvider::Uniform(a, b) => (a, b),
                    IntProvider::Constant(c) => (c, c),
                    _ => bail!("cherry branch start offset must be uniform"),
                };
                TrunkKind::Cherry {
                    count: int_or(p, v, "branch_count")?,
                    horizontal_length: int_or(p, v, "branch_horizontal_length")?,
                    start_offset,
                    end_offset: int_or(p, v, "branch_end_offset_from_top")?,
                }
            }
            "poplar_trunk_placer" => TrunkKind::Poplar { above_branches: int_or(p, v, "trunk_height_above_branches")?, branch_amount: int_or(p, v, "branch_amount")? },
            other => bail!("unknown trunk placer {other}"),
        };
        Ok(Self { base: v.i64_or("base_height", 0) as i32, rand_a: v.i64_or("height_rand_a", 0) as i32, rand_b: v.i64_or("height_rand_b", 0) as i32, kind })
    }

    fn tree_height(&self, r: &mut WorldgenRandom) -> i32 {
        self.base + r.next_bounded(self.rand_a + 1) + r.next_bounded(self.rand_b + 1)
    }

    #[allow(clippy::too_many_lines)]
    fn place(&self, env: &TreeEnv, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, height: i32, origin: Pos) -> Vec<Attachment> {
        let b = w.b;
        match &self.kind {
            TrunkKind::Straight => {
                env.below_trunk(w, r, parts, origin.down());
                for i in 0..height {
                    env.place_log(w, r, parts, origin.up_n(i));
                }
                vec![Attachment::new(origin.up_n(height), 0, false)]
            }
            TrunkKind::Forking => {
                env.below_trunk(w, r, parts, origin.down());
                let mut out = Vec::new();
                let lean_dir = Dir::random_horizontal(r);
                let lean_height = height - r.next_bounded(4) - 1;
                let mut lean_steps = 3 - r.next_bounded(3);
                let (mut tx, mut tz) = (origin.x, origin.z);
                let mut end_y: Option<i32> = None;
                for dy in 0..height {
                    let yy = origin.y + dy;
                    if dy >= lean_height && lean_steps > 0 {
                        let (ox, _, oz) = lean_dir.offset();
                        tx += ox;
                        tz += oz;
                        lean_steps -= 1;
                    }
                    if env.place_log(w, r, parts, Pos::new(tx, yy, tz)) {
                        end_y = Some(yy + 1);
                    }
                }
                if let Some(ey) = end_y {
                    out.push(Attachment::new(Pos::new(tx, ey, tz), 1, false));
                }
                let (mut bx, mut bz) = (origin.x, origin.z);
                let branch_dir = Dir::random_horizontal(r);
                if branch_dir != lean_dir {
                    let branch_pos = lean_height - r.next_bounded(2) - 1;
                    let mut steps = 1 + r.next_bounded(3);
                    let mut end: Option<i32> = None;
                    let mut dy = branch_pos;
                    while dy < height && steps > 0 {
                        if dy >= 1 {
                            let yy = origin.y + dy;
                            let (ox, _, oz) = branch_dir.offset();
                            bx += ox;
                            bz += oz;
                            if env.place_log(w, r, parts, Pos::new(bx, yy, bz)) {
                                end = Some(yy + 1);
                            }
                        }
                        dy += 1;
                        steps -= 1;
                    }
                    if let Some(ey) = end {
                        out.push(Attachment::new(Pos::new(bx, ey, bz), 0, false));
                    }
                }
                out
            }
            TrunkKind::Giant => self.giant(env, w, r, parts, height, origin),
            TrunkKind::MegaJungle => {
                let mut out = self.giant(env, w, r, parts, height, origin);
                let mut branch_height = height - 2 - r.next_bounded(4);
                while branch_height > height / 2 {
                    let angle = r.next_f32() * (std::f64::consts::PI * 2.0) as f32;
                    let (mut bx, mut bz) = (0, 0);
                    for i in 0..5 {
                        bx = (1.5f32 + mth_cos(angle as f64) * i as f32) as i32;
                        bz = (1.5f32 + mth_sin(angle as f64) * i as f32) as i32;
                        env.place_log(w, r, parts, origin.add(bx, branch_height - 3 + i / 2, bz));
                    }
                    out.push(Attachment::new(origin.add(bx, branch_height, bz), -2, false));
                    branch_height -= 2 + r.next_bounded(4);
                }
                out
            }
            TrunkKind::DarkOak => {
                let below = origin.down();
                for p in [below, below.rel(Dir::East), below.rel(Dir::South), below.rel(Dir::South).rel(Dir::East)] {
                    env.below_trunk(w, r, parts, p);
                }
                let lean_dir = Dir::random_horizontal(r);
                let lean_height = height - r.next_bounded(4);
                let mut lean_steps = 2 - r.next_bounded(3);
                let (x, y, z) = (origin.x, origin.y, origin.z);
                let (mut tx, mut tz) = (x, z);
                let end_y = y + height - 1;
                for dy in 0..height {
                    if dy >= lean_height && lean_steps > 0 {
                        let (ox, _, oz) = lean_dir.offset();
                        tx += ox;
                        tz += oz;
                        lean_steps -= 1;
                    }
                    let p = Pos::new(tx, y + dy, tz);
                    if is_air_or_leaves(w, p) {
                        env.place_log(w, r, parts, p);
                        env.place_log(w, r, parts, p.rel(Dir::East));
                        env.place_log(w, r, parts, p.rel(Dir::South));
                        env.place_log(w, r, parts, p.rel(Dir::East).rel(Dir::South));
                    }
                }
                let mut out = vec![Attachment::new(Pos::new(tx, end_y, tz), 0, true)];
                for ox in -1..=2 {
                    for oz in -1..=2 {
                        if (!(0..=1).contains(&ox) || !(0..=1).contains(&oz)) && r.next_bounded(3) <= 0 {
                            let length = r.next_bounded(3) + 2;
                            for by in 0..length {
                                env.place_log(w, r, parts, Pos::new(x + ox, end_y - by - 1, z + oz));
                            }
                            out.push(Attachment::new(Pos::new(x + ox, end_y, z + oz), 0, false));
                        }
                    }
                }
                out
            }
            TrunkKind::Fancy => fancy_trunk(env, w, r, parts, height, origin),
            TrunkKind::Bending { min_height_for_leaves, bend_length } => {
                let dir = Dir::random_horizontal(r);
                let log_height = height - 1;
                let mut p = origin;
                env.below_trunk(w, r, parts, p.down());
                let mut out = Vec::new();
                for i in 0..=log_height {
                    if i + 1 >= log_height + r.next_bounded(2) {
                        p = p.rel(dir);
                    }
                    if env.place_log(w, r, parts, p) && i >= *min_height_for_leaves {
                        out.push(Attachment::new(p, 0, false));
                    }
                    p = p.up();
                }
                let bend = bend_length.sample(r);
                for _ in 0..=bend {
                    if env.place_log(w, r, parts, p) {
                        out.push(Attachment::new(p, 0, false));
                    }
                    p = p.rel(dir);
                }
                out
            }
            TrunkKind::UpwardsBranching { extra_branch_steps, branch_probability, extra_branch_length, .. } => {
                let mut out = Vec::new();
                for i in 0..height {
                    let y = origin.y + i;
                    let p = Pos::new(origin.x, y, origin.z);
                    if self.place_branch_log(env, w, r, parts, p) && i < height - 1 && r.next_f32() < *branch_probability {
                        let dir = Dir::random_horizontal(r);
                        let length = extra_branch_length.sample(r);
                        let branch_pos = (length - extra_branch_length.sample(r) - 1).max(0);
                        let steps = extra_branch_steps.sample(r);
                        self.place_branch(env, w, r, parts, height, &mut out, p, y, dir, branch_pos, steps);
                    }
                    if i == height - 1 {
                        out.push(Attachment::new(Pos::new(origin.x, y + 1, origin.z), 0, false));
                    }
                }
                out
            }
            TrunkKind::Cherry { count, horizontal_length, start_offset, end_offset } => {
                env.below_trunk(w, r, parts, origin.down());
                let first = (height - 1 + r.next_bounded(start_offset.1 - start_offset.0 + 1) + start_offset.0).max(0);
                let second_max = start_offset.1 - 1;
                let mut second = (height - 1 + r.next_bounded(second_max - start_offset.0 + 1) + start_offset.0).max(0);
                if second >= first {
                    second += 1;
                }
                let branch_count = count.sample(r);
                let has_middle = branch_count == 3;
                let both_sides = branch_count >= 2;
                let trunk_height = if has_middle {
                    height
                } else if both_sides {
                    first.max(second) + 1
                } else {
                    first + 1
                };
                for i in 0..trunk_height {
                    env.place_log(w, r, parts, origin.up_n(i));
                }
                let mut out = Vec::new();
                if has_middle {
                    out.push(Attachment::new(origin.up_n(trunk_height), 0, false));
                }
                let dir = Dir::random_horizontal(r);
                let side = |w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, d: Dir, off: i32, middle_continues: bool| {
                    let mut log = origin.up_n(off);
                    let end_off = height - 1 + end_offset.sample(r);
                    let extend = middle_continues || end_off < off;
                    let distance = horizontal_length.sample(r) + i32::from(extend);
                    let end = origin.rel_n(d, distance).up_n(end_off);
                    let steps = if extend { 2 } else { 1 };
                    let axis = d.axis();
                    for _ in 0..steps {
                        log = log.rel(d);
                        env.place_log_with(w, r, parts, log, |s| b.with(s, "axis", axis));
                    }
                    let vertical = if end.y > log.y { Dir::Up } else { Dir::Down };
                    loop {
                        let distance = log.manhattan(end);
                        if distance == 0 {
                            return Attachment::new(end.up(), 0, false);
                        }
                        let chance = (end.y - log.y).abs() as f32 / distance as f32;
                        if r.next_f32() < chance {
                            log = log.rel(vertical);
                            env.place_log(w, r, parts, log);
                        } else {
                            log = log.rel(d);
                            env.place_log_with(w, r, parts, log, |s| b.with(s, "axis", axis));
                        }
                    }
                };
                out.push(side(w, r, parts, dir, first, first < trunk_height - 1));
                if both_sides {
                    out.push(side(w, r, parts, dir.opposite(), second, second < trunk_height - 1));
                }
                out
            }
            TrunkKind::Poplar { above_branches, branch_amount } => {
                env.below_trunk(w, r, parts, origin.down());
                let branch_height = height - above_branches.sample(r);
                for i in 0..height {
                    let p = origin.up_n(i);
                    env.place_log(w, r, parts, p);
                    // All six directions are shuffled on every layer.
                    let mut dirs = Dir::ALL;
                    for k in (1..dirs.len()).rev() {
                        let j = r.next_bounded(k as i32 + 1) as usize;
                        dirs.swap(k, j);
                    }
                    if branch_height - 1 != i {
                        continue;
                    }
                    let n = branch_amount.sample(r).max(0) as usize;
                    for d in dirs.iter().filter(|d| d.is_horizontal()).take(n) {
                        let axis = d.axis();
                        env.place_log_with(w, r, parts, p.rel(*d), |s| b.with(s, "axis", axis));
                    }
                }
                vec![Attachment::new(origin.up_n(branch_height), 0, false)]
            }
        }
    }

    fn giant(&self, env: &TreeEnv, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, height: i32, origin: Pos) -> Vec<Attachment> {
        let below = origin.down();
        for p in [below, below.rel(Dir::East), below.rel(Dir::South), below.rel(Dir::South).rel(Dir::East)] {
            env.below_trunk(w, r, parts, p);
        }
        for i in 0..height {
            env.place_log_if_free(w, r, parts, origin.up_n(i));
            if i < height - 1 {
                env.place_log_if_free(w, r, parts, origin.add(1, i, 0));
                env.place_log_if_free(w, r, parts, origin.add(1, i, 1));
                env.place_log_if_free(w, r, parts, origin.add(0, i, 1));
            }
        }
        vec![Attachment::new(origin.up_n(height), 0, true)]
    }

    fn place_branch_log(&self, env: &TreeEnv, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, p: Pos) -> bool {
        if env.is_free(w, p) && !w.b.tags.logs.has(w.get(p)) || matches!(&self.kind, TrunkKind::UpwardsBranching { can_grow_through, .. } if can_grow_through.has(w.get(p))) {
            let s = env.trunk.get(w, r, p);
            parts.set_log(w, p, s);
            true
        } else {
            false
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place_branch(&self, env: &TreeEnv, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, height: i32, out: &mut Vec<Attachment>, origin: Pos, y: i32, dir: Dir, branch_pos: i32, mut steps: i32) {
        let mut height_along = y + branch_pos;
        let (mut lx, mut lz) = (origin.x, origin.z);
        let mut i = branch_pos;
        while i < height && steps > 0 {
            if i >= 1 {
                let py = y + i;
                let (ox, _, oz) = dir.offset();
                lx += ox;
                lz += oz;
                height_along = py;
                let p = Pos::new(lx, py, lz);
                if self.place_branch_log(env, w, r, parts, p) {
                    height_along = py + 1;
                }
                out.push(Attachment::new(p, 0, false));
            }
            i += 1;
            steps -= 1;
        }
        if height_along - y > 1 {
            let foliage = Pos::new(lx, height_along, lz);
            out.push(Attachment::new(foliage, 0, false));
            out.push(Attachment::new(foliage.down().down(), 0, false));
        }
    }
}

/// `FancyTrunkPlacer.placeTrunk`.
fn fancy_trunk(env: &TreeEnv, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, tree_height: i32, origin: Pos) -> Vec<Attachment> {
    let height = tree_height + 2;
    let trunk_height = (height as f64 * 0.618).floor() as i32;
    env.below_trunk(w, r, parts, origin.down());
    let clusters = 1.min((1.382 + (1.0 * height as f64 / 13.0).powf(2.0)).floor() as i32);
    let trunk_top = origin.y + trunk_height;
    let mut rel_y = height - 5;
    let mut coords: Vec<(Attachment, i32)> = vec![(Attachment::new(origin.up_n(rel_y), 0, false), trunk_top)];
    while rel_y >= 0 {
        let shape = tree_shape(height, rel_y);
        if shape >= 0.0 {
            for _ in 0..clusters {
                let length = 1.0 * shape as f64 * (r.next_f32() as f64 + 0.328);
                let angle = (r.next_f32() * 2.0) as f64 * std::f64::consts::PI;
                let x = length * angle.sin() + 0.5;
                let z = length * angle.cos() + 0.5;
                let check_start = origin.add(x.floor() as i32, rel_y - 1, z.floor() as i32);
                let check_end = check_start.up_n(5);
                if make_limb(env, w, r, parts, check_start, check_end, false) {
                    let dx = origin.x - check_start.x;
                    let dz = origin.z - check_start.z;
                    let branch_height = check_start.y as f64 - ((dx * dx + dz * dz) as f64).sqrt() * 0.381;
                    let branch_top = if branch_height > trunk_top as f64 { trunk_top } else { branch_height as i32 };
                    let base = Pos::new(origin.x, branch_top, origin.z);
                    if make_limb(env, w, r, parts, base, check_start, false) {
                        coords.push((Attachment::new(check_start, 0, false), base.y));
                    }
                }
            }
        }
        rel_y -= 1;
    }
    make_limb(env, w, r, parts, origin, origin.up_n(trunk_height), true);
    for (att, base_y) in &coords {
        let base = Pos::new(origin.x, *base_y, origin.z);
        if base != att.pos && trim_branches(height, base_y - origin.y) {
            make_limb(env, w, r, parts, base, att.pos, true);
        }
    }
    coords.into_iter().filter(|(_, base_y)| trim_branches(height, base_y - origin.y)).map(|(a, _)| a).collect()
}

fn tree_shape(height: i32, y: i32) -> f32 {
    if (y as f32) < height as f32 * 0.3 {
        return -1.0;
    }
    let radius = height as f32 / 2.0;
    let adjacent = radius - y as f32;
    let mut distance = (radius * radius - adjacent * adjacent).sqrt();
    if adjacent == 0.0 {
        distance = radius;
    } else if adjacent.abs() >= radius {
        return 0.0;
    }
    distance * 0.5
}

fn trim_branches(height: i32, local_y: i32) -> bool {
    local_y as f64 >= height as f64 * 0.2
}

fn make_limb(env: &TreeEnv, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, start: Pos, end: Pos, place: bool) -> bool {
    if !place && start == end {
        return true;
    }
    let (dx, dy, dz) = (end.x - start.x, end.y - start.y, end.z - start.z);
    let steps = dx.abs().max(dy.abs()).max(dz.abs());
    let fx = dx as f32 / steps as f32;
    let fy = dy as f32 / steps as f32;
    let fz = dz as f32 / steps as f32;
    for i in 0..=steps {
        let p = start.add((0.5 + i as f32 * fx).floor() as i32, (0.5 + i as f32 * fy).floor() as i32, (0.5 + i as f32 * fz).floor() as i32);
        if place {
            let (ax, az) = ((p.x - start.x).abs(), (p.z - start.z).abs());
            let max = ax.max(az);
            let axis = if max > 0 {
                if ax == max {
                    "x"
                } else {
                    "z"
                }
            } else {
                "y"
            };
            let b = w.b;
            env.place_log_with(w, r, parts, p, |s| b.with(s, "axis", axis));
        } else if !env.is_free(w, p) {
            return false;
        }
    }
    true
}

enum FoliageKind {
    Blob { height: i32 },
    Bush { height: i32 },
    Acacia,
    Fancy { height: i32 },
    Jungle { height: i32 },
    Spruce { trunk_height: IntProvider },
    Pine { height: IntProvider },
    MegaPine { crown_height: IntProvider },
    DarkOak,
    RandomSpread { foliage_height: IntProvider, attempts: i32 },
    Cherry { height: IntProvider, wide_bottom_hole: f32, corner_hole: f32, hanging: f32, hanging_extension: f32 },
    Poplar { height: IntProvider, side_hole: f32 },
}

struct FoliagePlacer {
    radius: IntProvider,
    offset: IntProvider,
    kind: FoliageKind,
}

impl FoliagePlacer {
    fn parse(p: &mut Parser, v: &Value) -> Result<Self> {
        let t = v.str_field("type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        let int = |k: &str| v.i64_or(k, 0) as i32;
        let kind = match t {
            "blob_foliage_placer" => FoliageKind::Blob { height: int("height") },
            "bush_foliage_placer" => FoliageKind::Bush { height: int("height") },
            "acacia_foliage_placer" => FoliageKind::Acacia,
            "fancy_foliage_placer" => FoliageKind::Fancy { height: int("height") },
            "jungle_foliage_placer" => FoliageKind::Jungle { height: int("height") },
            "spruce_foliage_placer" => FoliageKind::Spruce { trunk_height: int_or(p, v, "trunk_height")? },
            "pine_foliage_placer" => FoliageKind::Pine { height: int_or(p, v, "height")? },
            "mega_pine_foliage_placer" => FoliageKind::MegaPine { crown_height: int_or(p, v, "crown_height")? },
            "dark_oak_foliage_placer" => FoliageKind::DarkOak,
            "random_spread_foliage_placer" => FoliageKind::RandomSpread { foliage_height: int_or(p, v, "foliage_height")?, attempts: int("leaf_placement_attempts") },
            "cherry_foliage_placer" => FoliageKind::Cherry {
                height: int_or(p, v, "height")?,
                wide_bottom_hole: v.f64_or("wide_bottom_layer_hole_chance", 0.0) as f32,
                corner_hole: v.f64_or("corner_hole_chance", 0.0) as f32,
                hanging: v.f64_or("hanging_leaves_chance", 0.0) as f32,
                hanging_extension: v.f64_or("hanging_leaves_extension_chance", 0.0) as f32,
            },
            "poplar_foliage_placer" => FoliageKind::Poplar { height: int_or(p, v, "height")?, side_hole: v.f64_or("side_hole_chance", 0.0) as f32 },
            other => bail!("unknown foliage placer {other}"),
        };
        Ok(Self { radius: int_or(p, v, "radius")?, offset: int_or(p, v, "offset")?, kind })
    }

    /// `foliageHeight`.
    fn foliage_height(&self, r: &mut WorldgenRandom, tree_height: i32) -> i32 {
        match &self.kind {
            FoliageKind::Blob { height } | FoliageKind::Bush { height } | FoliageKind::Fancy { height } | FoliageKind::Jungle { height } => *height,
            FoliageKind::Acacia => 0,
            FoliageKind::Spruce { trunk_height } => (tree_height - trunk_height.sample(r)).max(4),
            FoliageKind::Pine { height } => height.sample(r),
            FoliageKind::MegaPine { crown_height } => crown_height.sample(r),
            FoliageKind::DarkOak => 4,
            FoliageKind::RandomSpread { foliage_height, .. } => foliage_height.sample(r),
            FoliageKind::Cherry { height, .. } | FoliageKind::Poplar { height, .. } => height.sample(r),
        }
    }

    /// `foliageRadius`.
    fn foliage_radius(&self, r: &mut WorldgenRandom, trunk_height: i32) -> i32 {
        match &self.kind {
            FoliageKind::Pine { .. } => self.radius.sample(r) + r.next_bounded((trunk_height + 1).max(1)),
            _ => self.radius.sample(r),
        }
    }

    /// The per-placer leaf test (`shouldSkipLocation` on folded or signed offsets).
    fn skip(&self, r: &mut WorldgenRandom, dx: i32, y: i32, dz: i32, radius: i32, double_trunk: bool) -> bool {
        if let FoliageKind::DarkOak = self.kind {
            if y == 0 && double_trunk && (dx == -radius || dx >= radius) && (dz == -radius || dz >= radius) {
                return true;
            }
        }
        let (x, z) = if double_trunk { (dx.abs().min((dx - 1).abs()), dz.abs().min((dz - 1).abs())) } else { (dx.abs(), dz.abs()) };
        match &self.kind {
            FoliageKind::Blob { .. } => x == radius && z == radius && (r.next_bounded(2) == 0 || y == 0),
            FoliageKind::Bush { .. } => x == radius && z == radius && r.next_bounded(2) == 0,
            FoliageKind::Acacia => {
                if y == 0 {
                    (x > 1 || z > 1) && x != 0 && z != 0
                } else {
                    x == radius && z == radius && radius > 0
                }
            }
            FoliageKind::Fancy { .. } => square(x as f32 + 0.5) + square(z as f32 + 0.5) > (radius * radius) as f32,
            FoliageKind::Jungle { .. } | FoliageKind::MegaPine { .. } => x + z >= 7 || x * x + z * z > radius * radius,
            FoliageKind::Spruce { .. } | FoliageKind::Pine { .. } => x == radius && z == radius && radius > 0,
            FoliageKind::DarkOak => {
                if y == -1 && !double_trunk {
                    x == radius && z == radius
                } else if y == 1 {
                    x + z > radius * 2 - 2
                } else {
                    false
                }
            }
            FoliageKind::Cherry { wide_bottom_hole, corner_hole, .. } => {
                if y == -1 && (x == radius || z == radius) && r.next_f32() < *wide_bottom_hole {
                    return true;
                }
                let corner = x == radius && z == radius;
                if radius > 2 {
                    corner || (x + z > radius * 2 - 2 && r.next_f32() < *corner_hole)
                } else {
                    corner && r.next_f32() < *corner_hole
                }
            }
            FoliageKind::RandomSpread { .. } | FoliageKind::Poplar { .. } => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn square(&self, w: &mut World, r: &mut WorldgenRandom, leaves: &StateProvider, parts: &mut Parts, center: Pos, radius: i32, y: i32, double_trunk: bool) {
        let extra = i32::from(double_trunk);
        for dx in -radius..=radius + extra {
            for dz in -radius..=radius + extra {
                if !self.skip(r, dx, y, dz, radius, double_trunk) {
                    try_place_leaf(w, r, leaves, parts, center.add(dx, y, dz));
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn square_with_hanging(&self, w: &mut World, r: &mut WorldgenRandom, leaves: &StateProvider, parts: &mut Parts, center: Pos, radius: i32, y: i32, double_trunk: bool, chance: f32, extension: f32) {
        self.square(w, r, leaves, parts, center, radius, y, double_trunk);
        let extra = i32::from(double_trunk);
        let log = center.down();
        for along in Dir::HORIZONTAL {
            let to_edge = along.clockwise();
            let off = if to_edge.positive() { radius + extra } else { radius };
            let mut p = center.add(0, y - 1, 0).rel_n(to_edge, off).rel_n(along, -radius);
            let mut i = -radius;
            while i < radius + extra {
                if parts.foliage_set.contains(&p.up()) && try_place_extension(w, r, leaves, parts, chance, log, p) {
                    try_place_extension(w, r, leaves, parts, extension, log, p.down());
                }
                p = p.rel(along);
                i += 1;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create(&self, w: &mut World, r: &mut WorldgenRandom, leaves: &StateProvider, parts: &mut Parts, tree_height: i32, att: Attachment, foliage_height: i32, leaf_radius: i32) {
        let offset = self.offset.sample(r);
        let pos = att.pos;
        let dt = att.double_trunk;
        match &self.kind {
            FoliageKind::Blob { .. } => {
                for y in (offset - foliage_height..=offset).rev() {
                    let radius = (leaf_radius + att.radius_offset - 1 - y / 2).max(0);
                    self.square(w, r, leaves, parts, pos, radius, y, dt);
                }
            }
            FoliageKind::Bush { .. } => {
                for y in (offset - foliage_height..=offset).rev() {
                    let radius = leaf_radius + att.radius_offset - 1 - y;
                    self.square(w, r, leaves, parts, pos, radius, y, dt);
                }
            }
            FoliageKind::Acacia => {
                let p = pos.up_n(offset);
                self.square(w, r, leaves, parts, p, leaf_radius + att.radius_offset, -1 - foliage_height, dt);
                self.square(w, r, leaves, parts, p, leaf_radius - 1, -foliage_height, dt);
                self.square(w, r, leaves, parts, p, leaf_radius + att.radius_offset - 1, 0, dt);
            }
            FoliageKind::Fancy { .. } => {
                for y in (offset - foliage_height..=offset).rev() {
                    let radius = leaf_radius + i32::from(y != offset && y != offset - foliage_height);
                    self.square(w, r, leaves, parts, pos, radius, y, dt);
                }
            }
            FoliageKind::Jungle { .. } => {
                let h = if dt { foliage_height } else { 1 + r.next_bounded(2) };
                for y in (offset - h..=offset).rev() {
                    let radius = leaf_radius + att.radius_offset + 1 - y;
                    self.square(w, r, leaves, parts, pos, radius, y, dt);
                }
            }
            FoliageKind::Spruce { .. } => {
                let mut radius = r.next_bounded(2);
                let mut max = 1;
                let mut next = 0;
                for y in (-foliage_height..=offset).rev() {
                    self.square(w, r, leaves, parts, pos, radius, y, dt);
                    if radius >= max {
                        radius = next;
                        next = 1;
                        max = (leaf_radius + att.radius_offset).min(max + 1);
                    } else {
                        radius += 1;
                    }
                }
            }
            FoliageKind::Pine { .. } => {
                let mut radius = 0;
                for y in (offset - foliage_height..=offset).rev() {
                    self.square(w, r, leaves, parts, pos, radius, y, dt);
                    if radius >= 1 && y == offset - foliage_height + 1 {
                        radius -= 1;
                    } else if radius < leaf_radius + att.radius_offset {
                        radius += 1;
                    }
                }
            }
            FoliageKind::MegaPine { .. } => {
                let mut current = 0;
                for y in pos.y - foliage_height + offset..=pos.y + offset {
                    let delta = pos.y - y;
                    let computed = leaf_radius + att.radius_offset + (delta as f32 / foliage_height as f32 * 3.5).floor() as i32;
                    let radius = if delta > 0 && computed == current && (y & 1) == 0 { computed + 1 } else { computed };
                    self.square(w, r, leaves, parts, Pos::new(pos.x, y, pos.z), radius, 0, dt);
                    current = computed;
                }
            }
            FoliageKind::DarkOak => {
                let p = pos.up_n(offset);
                self.square(w, r, leaves, parts, p, leaf_radius + 2, -1, dt);
                if dt {
                    self.square(w, r, leaves, parts, p, leaf_radius + 3, 0, dt);
                    self.square(w, r, leaves, parts, p, leaf_radius + 2, 1, dt);
                    if r.next_bool() {
                        self.square(w, r, leaves, parts, p, leaf_radius, 2, dt);
                    }
                } else {
                    self.square(w, r, leaves, parts, p, leaf_radius + 1, 0, dt);
                }
            }
            FoliageKind::RandomSpread { attempts, .. } => {
                for _ in 0..*attempts {
                    let dx = r.next_bounded(leaf_radius) - r.next_bounded(leaf_radius);
                    let dy = r.next_bounded(foliage_height) - r.next_bounded(foliage_height);
                    let dz = r.next_bounded(leaf_radius) - r.next_bounded(leaf_radius);
                    try_place_leaf(w, r, leaves, parts, pos.add(dx, dy, dz));
                }
            }
            FoliageKind::Cherry { hanging, hanging_extension, .. } => {
                let p = pos.up_n(offset);
                let current = leaf_radius + att.radius_offset - 1;
                self.square(w, r, leaves, parts, p, current - 2, foliage_height - 3, dt);
                self.square(w, r, leaves, parts, p, current - 1, foliage_height - 4, dt);
                for y in (0..=foliage_height - 5).rev() {
                    self.square(w, r, leaves, parts, p, current, y, dt);
                }
                self.square_with_hanging(w, r, leaves, parts, p, current, -1, dt, *hanging, *hanging_extension);
                self.square_with_hanging(w, r, leaves, parts, p, current - 1, -2, dt, *hanging, *hanging_extension);
            }
            FoliageKind::Poplar { side_hole, .. } => {
                let p = pos.up_n(offset);
                let current = leaf_radius + att.radius_offset - 1;
                let flipped = r.next_bool();
                let rhombus = |w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, radius: i32, y: i32| {
                    let extra = i32::from(dt);
                    for dx in -radius..=radius + extra {
                        for dz in -radius..=radius + extra {
                            if !poplar_skip(r, dx, y, dz, radius, foliage_height, flipped, *side_hole) {
                                try_place_leaf(w, r, leaves, parts, p.add(dx, y, dz));
                            }
                        }
                    }
                };
                rhombus(w, r, parts, current - 2, foliage_height - 1);
                rhombus(w, r, parts, current - 1, foliage_height - 2);
                rhombus(w, r, parts, current - 1, foliage_height - 3);
                for y in (1..=foliage_height - 4).rev() {
                    rhombus(w, r, parts, current, y);
                }
                rhombus(w, r, parts, current - 1, 0);
                rhombus(w, r, parts, (current - 2).clamp(1, 2), -1);
            }
        }
        let _ = tree_height;
    }
}

fn square(v: f32) -> f32 {
    v * v
}

#[allow(clippy::too_many_arguments)]
fn poplar_skip(r: &mut WorldgenRandom, dx: i32, y: i32, dz: i32, radius: i32, foliage_height: i32, flipped: bool, side_hole: f32) -> bool {
    let partial = foliage_height - 1 == y || foliage_height - 2 == y;
    let (ax, az) = (dx.abs(), dz.abs());
    if partial && (ax == radius || az == radius) {
        return true;
    }
    let small_corner = if flipped { dx > 0 && dz > 0 || dz < 0 && dx < 0 } else { dx > 0 && dz < 0 || dz > 0 && dx < 0 };
    let corner = if small_corner {
        radius - 1
    } else if partial {
        radius + 1
    } else {
        radius
    };
    let hole = i32::from(r.next_f32() <= side_hole);
    ax + az > radius * 2 - (corner + hole)
}

/// `FoliagePlacer.tryPlaceLeaf`.
fn try_place_leaf(w: &mut World, r: &mut WorldgenRandom, leaves: &StateProvider, parts: &mut Parts, p: Pos) -> bool {
    let here = w.get(p);
    if w.b.prop_bool(here, "persistent") == Some(true) || !valid_tree_pos(w, p) {
        return false;
    }
    let mut s = leaves.get(w, r, p);
    if w.b.has_prop(s, "waterlogged") {
        s = w.b.waterlogged(s, w.fluid(p) == Fluid::Water);
    }
    parts.set_foliage(w, p, s);
    true
}

fn try_place_extension(w: &mut World, r: &mut WorldgenRandom, leaves: &StateProvider, parts: &mut Parts, chance: f32, log: Pos, p: Pos) -> bool {
    if p.manhattan(log) >= 7 || r.next_f32() > chance {
        return false;
    }
    try_place_leaf(w, r, leaves, parts, p)
}

struct MangroveRoots {
    trunk_offset_y: IntProvider,
    root_provider: Arc<StateProvider>,
    above: Option<(Arc<StateProvider>, f32)>,
    can_grow_through: Arc<BlockSet>,
    muddy_roots_in: Arc<BlockSet>,
    muddy_provider: Arc<StateProvider>,
    max_width: i32,
    max_length: i32,
    skew_chance: f32,
}

impl MangroveRoots {
    fn parse(p: &mut Parser, v: &Value) -> Result<Self> {
        let placement = v.field("mangrove_root_placement")?;
        let above = match v.get("above_root_placement") {
            Some(a) => Some((p.provider(a.field("above_root_provider")?)?, a.f64_or("above_root_placement_chance", 0.0) as f32)),
            None => None,
        };
        Ok(Self {
            trunk_offset_y: int_or(p, v, "trunk_offset_y")?,
            root_provider: p.provider(v.field("root_provider")?)?,
            above,
            can_grow_through: p.block_set(placement.field("can_grow_through")?)?,
            muddy_roots_in: p.block_set(placement.field("muddy_roots_in")?)?,
            muddy_provider: p.provider(placement.field("muddy_roots_provider")?)?,
            max_width: placement.i64_or("max_root_width", 0) as i32,
            max_length: placement.i64_or("max_root_length", 0) as i32,
            skew_chance: placement.f64_or("random_skew_chance", 0.0) as f32,
        })
    }

    fn can_grow_through(&self, w: &World, p: Pos) -> bool {
        valid_tree_pos(w, p) || self.can_grow_through.has(w.get(p))
    }

    fn place(&self, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, origin: Pos, trunk: Pos) -> bool {
        let mut p = origin;
        while p.y < trunk.y {
            if !self.can_grow_through(w, p) {
                return false;
            }
            p = p.up();
        }
        let mut roots = vec![trunk.down()];
        for d in Dir::HORIZONTAL {
            let start = trunk.rel(d);
            let mut offshoots = Vec::new();
            if !self.simulate(w, r, start, d, trunk, &mut offshoots, 0) {
                return false;
            }
            roots.extend(offshoots);
            roots.push(start);
        }
        for p in roots {
            self.place_root(w, r, parts, p);
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn simulate(&self, w: &World, r: &mut WorldgenRandom, p: Pos, d: Dir, origin: Pos, out: &mut Vec<Pos>, length: i32) -> bool {
        if length == self.max_length || out.len() as i32 > self.max_length {
            return false;
        }
        for next in self.offshoots(r, p, d, origin) {
            if self.can_grow_through(w, next) {
                out.push(next);
                if !self.simulate(w, r, next, d, origin, out, length + 1) {
                    return false;
                }
            }
        }
        true
    }

    fn offshoots(&self, r: &mut WorldgenRandom, p: Pos, d: Dir, origin: Pos) -> Vec<Pos> {
        let down = p.down();
        let side = p.rel(d);
        let dist = p.manhattan(origin);
        if dist > self.max_width - 3 && dist <= self.max_width {
            if r.next_f32() < self.skew_chance {
                vec![down, side.down()]
            } else {
                vec![down]
            }
        } else if dist > self.max_width || r.next_f32() < self.skew_chance {
            vec![down]
        } else if r.next_bool() {
            vec![side]
        } else {
            vec![down]
        }
    }

    fn waterlog(w: &World, p: Pos, s: u32) -> u32 {
        if w.b.has_prop(s, "waterlogged") && w.is_water_at(p) {
            w.b.waterlogged(s, true)
        } else {
            s
        }
    }

    fn place_root(&self, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, p: Pos) {
        if self.muddy_roots_in.has(w.get(p)) {
            let s = self.muddy_provider.get(w, r, p);
            let s = Self::waterlog(w, p, s);
            parts.set_root(w, p, s);
            return;
        }
        if !self.can_grow_through(w, p) {
            return;
        }
        let s = self.root_provider.get(w, r, p);
        let s = Self::waterlog(w, p, s);
        parts.set_root(w, p, s);
        if let Some((provider, chance)) = &self.above {
            let up = p.up();
            if r.next_f32() < *chance && w.is_air(up) {
                let s = provider.get(w, r, up);
                let s = Self::waterlog(w, up, s);
                parts.set_root(w, up, s);
            }
        }
    }
}

enum Decorator {
    TrunkVine,
    LeaveVine(f32),
    Cocoa(f32),
    Beehive(f32),
    AlterGround(Arc<StateProvider>),
    AttachedToLeaves { probability: f32, xz: i32, y: i32, provider: Arc<StateProvider>, required_empty: i32, directions: Vec<Dir> },
    AttachedToLogs { probability: f32, provider: Arc<StateProvider>, directions: Vec<Dir> },
    PlaceOnGround { tries: i32, radius: i32, height: i32, provider: Arc<StateProvider> },
    PaleMoss { leaves: f32, trunk: f32, ground: f32, patch: Option<Arc<Feature>> },
    CreakingHeart(f32),
    ShelfMushroom(f32),
}

fn decorators(p: &mut Parser, v: Option<&Value>) -> Result<Vec<Decorator>> {
    let Some(list) = v.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for d in list {
        let t = d.str_field("type")?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        let prob = d.f64_or("probability", 0.0) as f32;
        let dirs = |d: &Value| -> Result<Vec<Dir>> {
            d.field("directions")?.as_array().ok_or_else(|| anyhow!("directions"))?.iter().map(|x| x.as_str().and_then(Dir::parse).ok_or_else(|| anyhow!("bad direction"))).collect()
        };
        out.push(match t {
            "trunk_vine" => Decorator::TrunkVine,
            "leave_vine" => Decorator::LeaveVine(prob),
            "cocoa" => Decorator::Cocoa(prob),
            "beehive" => Decorator::Beehive(prob),
            "alter_ground" => Decorator::AlterGround(p.provider(d.field("provider")?)?),
            "attached_to_leaves" => Decorator::AttachedToLeaves {
                probability: prob,
                xz: d.i64_or("exclusion_radius_xz", 0) as i32,
                y: d.i64_or("exclusion_radius_y", 0) as i32,
                provider: p.provider(d.field("block_provider")?)?,
                required_empty: d.i64_or("required_empty_blocks", 1) as i32,
                directions: dirs(d)?,
            },
            "attached_to_logs" => Decorator::AttachedToLogs { probability: prob, provider: p.provider(d.field("block_provider")?)?, directions: dirs(d)? },
            "place_on_ground" => Decorator::PlaceOnGround {
                tries: d.i64_or("tries", 128) as i32,
                radius: d.i64_or("radius", 2) as i32,
                height: d.i64_or("height", 1) as i32,
                provider: p.provider(d.field("block_state_provider")?)?,
            },
            "pale_moss" => Decorator::PaleMoss {
                leaves: d.f64_or("leaves_probability", 0.0) as f32,
                trunk: d.f64_or("trunk_probability", 0.0) as f32,
                ground: d.f64_or("ground_probability", 0.0) as f32,
                patch: p.feature(&Value::String("minecraft:pale_moss_patch".into())).ok(),
            },
            "creaking_heart" => Decorator::CreakingHeart(prob),
            "shelf_mushroom" => Decorator::ShelfMushroom(prob),
            other => bail!("unknown tree decorator {other}"),
        });
    }
    Ok(out)
}

/// `Util.shuffledCopy`.
fn shuffled(list: &[Pos], r: &mut WorldgenRandom) -> Vec<Pos> {
    let mut v = list.to_vec();
    for i in (2..=v.len()).rev() {
        let j = r.next_bounded(i as i32) as usize;
        v.swap(i - 1, j);
    }
    v
}

/// What decorators see: sorted copies of the tree's parts and a setter for decorations.
struct DecoratorContext {
    logs: Vec<Pos>,
    leaves: Vec<Pos>,
    roots: Vec<Pos>,
    decorations: Vec<Pos>,
}

impl DecoratorContext {
    fn new(logs: Vec<Pos>, leaves: Vec<Pos>, roots: Vec<Pos>) -> Self {
        let sort = |mut v: Vec<Pos>| {
            v.sort_by_key(|p| p.y);
            v
        };
        Self { logs: sort(logs), leaves: sort(leaves), roots: sort(roots), decorations: Vec::new() }
    }

    fn set(&mut self, w: &mut World, p: Pos, s: u32) {
        self.decorations.push(p);
        w.set(p, s);
    }

    fn place_vine(&mut self, w: &mut World, p: Pos, face: Dir) {
        let s = w.b.with_bool(w.b.known.vine, face.name(), true);
        self.set(w, p, s);
    }

    /// `TreeFeature.getLowestTrunkOrRootOfTree`.
    fn lowest_trunk_or_root(&self) -> Vec<Pos> {
        if self.roots.is_empty() {
            self.logs.clone()
        } else if !self.logs.is_empty() && self.roots[0].y == self.logs[0].y {
            let mut v = self.logs.clone();
            v.extend_from_slice(&self.roots);
            v
        } else {
            self.roots.clone()
        }
    }
}

impl Decorator {
    #[allow(clippy::too_many_lines)]
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, ctx: &mut DecoratorContext) {
        let b = w.b;
        match self {
            Decorator::TrunkVine => {
                for p in ctx.logs.clone() {
                    for (d, face) in [(Dir::West, Dir::East), (Dir::East, Dir::West), (Dir::North, Dir::South), (Dir::South, Dir::North)] {
                        if r.next_bounded(3) > 0 && w.is_air(p.rel(d)) {
                            ctx.place_vine(w, p.rel(d), face);
                        }
                    }
                }
            }
            Decorator::LeaveVine(prob) => {
                for p in ctx.leaves.clone() {
                    for (d, face) in [(Dir::West, Dir::East), (Dir::East, Dir::West), (Dir::North, Dir::South), (Dir::South, Dir::North)] {
                        if r.next_f32() < *prob {
                            let t = p.rel(d);
                            if w.is_air(t) {
                                ctx.place_vine(w, t, face);
                                let mut q = t.down();
                                let mut n = 4;
                                while w.is_air(q) && n > 0 {
                                    ctx.place_vine(w, q, face);
                                    q = q.down();
                                    n -= 1;
                                }
                            }
                        }
                    }
                }
            }
            Decorator::Cocoa(prob) => {
                if r.next_f32() >= *prob || ctx.logs.is_empty() {
                    return;
                }
                let tree_y = ctx.logs[0].y;
                for p in ctx.logs.clone().into_iter().filter(|p| p.y - tree_y <= 2) {
                    for d in Dir::HORIZONTAL {
                        if r.next_f32() <= 0.25 {
                            let opp = d.opposite();
                            let at = p.add(opp.offset().0, 0, opp.offset().2);
                            if w.is_air(at) {
                                let s = b.with(b.with_int(b.known.cocoa, "age", r.next_bounded(3)), "facing", d.name());
                                ctx.set(w, at, s);
                            }
                        }
                    }
                }
            }
            Decorator::Beehive(prob) => {
                if ctx.logs.is_empty() || r.next_f32() >= *prob {
                    return;
                }
                let hive_y = match ctx.leaves.first() {
                    Some(l) => (l.y - 1).max(ctx.logs[0].y + 1),
                    None => (ctx.logs[0].y + 1 + r.next_bounded(3)).min(ctx.logs[ctx.logs.len() - 1].y),
                };
                let mut spots: Vec<Pos> = Vec::new();
                for p in ctx.logs.iter().filter(|p| p.y == hive_y) {
                    for d in [Dir::East, Dir::South, Dir::West] {
                        spots.push(p.rel(d));
                    }
                }
                if spots.is_empty() {
                    return;
                }
                let spots = {
                    let mut v = spots;
                    for i in (2..=v.len()).rev() {
                        let j = r.next_bounded(i as i32) as usize;
                        v.swap(i - 1, j);
                    }
                    v
                };
                for p in spots {
                    if w.is_air(p) && w.is_air(p.rel(Dir::South)) {
                        let s = b.with(b.known.bee_nest, "facing", "south");
                        ctx.set(w, p, s);
                        let bees = 2 + r.next_bounded(2);
                        let occupants = (0..bees).map(|_| r.next_bounded(599)).collect();
                        w.add_block_entity(p, BlockEntity::Beehive { occupants });
                        break;
                    }
                }
            }
            Decorator::AlterGround(provider) => {
                let list = ctx.lowest_trunk_or_root();
                if list.is_empty() {
                    return;
                }
                let y = list[0].y;
                for p in list.iter().filter(|p| p.y == y) {
                    for c in [p.add(-1, 0, -1), p.add(2, 0, -1), p.add(-1, 0, 2), p.add(2, 0, 2)] {
                        alter_circle(w, r, ctx, provider, c);
                    }
                    for _ in 0..5 {
                        let placement = r.next_bounded(64);
                        let (xx, zz) = (placement % 8, placement / 8);
                        if xx == 0 || xx == 7 || zz == 0 || zz == 7 {
                            alter_circle(w, r, ctx, provider, p.add(-3 + xx, 0, -3 + zz));
                        }
                    }
                }
            }
            Decorator::AttachedToLeaves { probability, xz, y, provider, required_empty, directions } => {
                let mut blacklist: HashSet<Pos> = HashSet::new();
                for leaf in shuffled(&ctx.leaves, r) {
                    let d = directions[r.next_bounded(directions.len() as i32) as usize];
                    let at = leaf.rel(d);
                    if blacklist.contains(&at) || r.next_f32() >= *probability {
                        continue;
                    }
                    if !(1..=*required_empty).all(|i| w.is_air(leaf.rel_n(d, i))) {
                        continue;
                    }
                    for dx in -xz..=*xz {
                        for dy in -y..=*y {
                            for dz in -xz..=*xz {
                                blacklist.insert(at.add(dx, dy, dz));
                            }
                        }
                    }
                    let s = provider.get(w, r, at);
                    ctx.set(w, at, s);
                }
            }
            Decorator::AttachedToLogs { probability, provider, directions } => {
                for log in shuffled(&ctx.logs, r) {
                    let d = directions[r.next_bounded(directions.len() as i32) as usize];
                    let at = log.rel(d);
                    if r.next_f32() <= *probability && w.is_air(at) {
                        let s = provider.get(w, r, at);
                        ctx.set(w, at, s);
                    }
                }
            }
            Decorator::PlaceOnGround { tries, radius, height, provider } => {
                let list = ctx.lowest_trunk_or_root();
                let Some(first) = list.first().copied() else {
                    return;
                };
                let y = first.y;
                let (mut min_x, mut max_x, mut min_z, mut max_z) = (first.x, first.x, first.z, first.z);
                for p in list.iter().filter(|p| p.y == y) {
                    min_x = min_x.min(p.x);
                    max_x = max_x.max(p.x);
                    min_z = min_z.min(p.z);
                    max_z = max_z.max(p.z);
                }
                let (lx, hx, ly, hy, lz, hz) = (min_x - radius, max_x + radius, y - height, y + height, min_z - radius, max_z + radius);
                for _ in 0..*tries {
                    let p = Pos::new(r.next_between(lx, hx), r.next_between(ly, hy), r.next_between(lz, hz));
                    let above = p.up();
                    let a = w.get(above);
                    if (b.is_air(a) || b.is_block(a, b.known.vine))
                        && b.is_solid_render(w.get(p))
                        && w.height_at(Heightmap::MotionBlockingNoLeaves, p.x, p.z) <= above.y
                    {
                        let s = provider.get(w, r, above);
                        ctx.set(w, above, s);
                    }
                }
            }
            Decorator::PaleMoss { leaves, trunk, ground, patch } => {
                let logs = shuffled(&ctx.logs, r);
                if logs.is_empty() {
                    return;
                }
                let origin = *logs.iter().min_by_key(|p| p.y).expect("non-empty");
                if r.next_f32() < *ground {
                    if let Some(f) = patch {
                        f.place(w, r, origin.up());
                    }
                }
                for p in ctx.logs.clone() {
                    if r.next_f32() < *trunk && w.is_air(p.down()) {
                        add_moss_hanger(w, r, ctx, p.down());
                    }
                }
                for p in ctx.leaves.clone() {
                    if r.next_f32() < *leaves && w.is_air(p.down()) {
                        add_moss_hanger(w, r, ctx, p.down());
                    }
                }
            }
            Decorator::CreakingHeart(prob) => {
                if ctx.logs.is_empty() || r.next_f32() >= *prob {
                    return;
                }
                for p in shuffled(&ctx.logs, r) {
                    if Dir::ALL.iter().all(|d| b.tags.logs.has(w.get(p.rel(*d)))) {
                        let s = b.with(b.with(b.with(b.known.creaking_heart, "axis", "y"), "creaking_heart_state", "dormant"), "natural", "true");
                        ctx.set(w, p, s);
                        break;
                    }
                }
            }
            Decorator::ShelfMushroom(prob) => {
                if r.next_f32() >= *prob || ctx.logs.is_empty() {
                    return;
                }
                let logs = ctx.logs.clone();
                let mushroom = paper_data::blocks::default_state("minecraft:shelf_mushroom").unwrap_or(b.known.air);
                let is_mushroom = |w: &World, p: Pos| b.is_block(w.get(p), mushroom);
                let replaceable = |w: &World, p: Pos| {
                    let water = |q: Pos| b.is_block(w.get(q), b.known.water);
                    b.is_replaceable(w.get(p)) && !(water(p) || water(p.rel(Dir::East)) || water(p.rel(Dir::West)) || water(p.rel(Dir::North)) || water(p.rel(Dir::South)))
                };
                let adjacent = |w: &World, p: Pos| Dir::HORIZONTAL.iter().any(|d| is_mushroom(w, p.rel(*d)));
                let place = |w: &mut World, r: &mut WorldgenRandom, ctx: &mut DecoratorContext, p: Pos, d: Dir| {
                    let s = b.with(b.with_int(mushroom, "age", r.next_bounded(2)), "facing", d.name());
                    ctx.set(w, p, s);
                };
                let first = logs[0];
                let last = logs[logs.len() - 1];
                if first.y == last.y {
                    let dirs = if first.x != last.x { [Dir::North, Dir::South] } else { [Dir::East, Dir::West] };
                    for log in &logs {
                        for d in dirs {
                            if r.next_f32() > 0.25 {
                                continue;
                            }
                            let at = log.rel(d);
                            if !replaceable(w, at) || adjacent(w, at) || adjacent(w, *log) {
                                continue;
                            }
                            place(w, r, ctx, at, d);
                        }
                    }
                } else {
                    let d0 = Dir::random_horizontal(r);
                    let dirs = [d0, d0.clockwise()];
                    let base_y = first.y;
                    for log in &logs {
                        let dy = log.y - base_y;
                        if !(1..=4).contains(&dy) {
                            continue;
                        }
                        for d in dirs {
                            if r.next_f32() > 0.25 {
                                continue;
                            }
                            let at = log.rel(d);
                            if !replaceable(w, at) || is_mushroom(w, at.down()) {
                                continue;
                            }
                            place(w, r, ctx, at, d);
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn alter_circle(w: &mut World, r: &mut WorldgenRandom, ctx: &mut DecoratorContext, provider: &StateProvider, center: Pos) {
    for xx in -2..=2i32 {
        for zz in -2..=2i32 {
            if xx.abs() != 2 || zz.abs() != 2 {
                let p = center.add(xx, 0, zz);
                for dy in (-3..=2).rev() {
                    let q = p.up_n(dy);
                    if let Some(s) = provider.get_optional(w, r, q) {
                        ctx.set(w, q, s);
                        break;
                    }
                    if !w.is_air(q) && dy < 0 {
                        break;
                    }
                }
            }
        }
    }
}

fn add_moss_hanger(w: &mut World, r: &mut WorldgenRandom, ctx: &mut DecoratorContext, mut p: Pos) {
    let b = w.b;
    while w.is_air(p.down()) && r.next_f32() >= 0.5 {
        let s = b.with_bool(b.known.pale_hanging_moss, "tip", false);
        ctx.set(w, p, s);
        p = p.down();
    }
    let s = b.with_bool(b.known.pale_hanging_moss, "tip", true);
    ctx.set(w, p, s);
}

enum FeatureSize {
    Two { limit: i32, lower: i32, upper: i32 },
    Three { limit: i32, upper_limit: i32, lower: i32, middle: i32, upper: i32 },
}

impl FeatureSize {
    fn size_at(&self, height: i32, y: i32) -> i32 {
        match self {
            FeatureSize::Two { limit, lower, upper } => {
                if y < *limit {
                    *lower
                } else {
                    *upper
                }
            }
            FeatureSize::Three { limit, upper_limit, lower, middle, upper } => {
                if y < *limit {
                    *lower
                } else if y >= height - upper_limit {
                    *upper
                } else {
                    *middle
                }
            }
        }
    }
}

struct Tree {
    trunk: Arc<StateProvider>,
    trunk_placer: TrunkPlacer,
    foliage: Arc<StateProvider>,
    foliage_placer: FoliagePlacer,
    roots: Option<MangroveRoots>,
    size: FeatureSize,
    min_clipped: Option<i32>,
    decorators: Vec<Decorator>,
    ignore_vines: bool,
    below_trunk: Arc<StateProvider>,
}

impl Tree {
    fn parse(p: &mut Parser, v: &Value) -> Result<Self> {
        let size = v.field("minimum_size")?;
        let st = size.str_field("type")?;
        let fs = match st.strip_prefix("minecraft:").unwrap_or(st) {
            "two_layers_feature_size" => FeatureSize::Two { limit: size.i64_or("limit", 1) as i32, lower: size.i64_or("lower_size", 0) as i32, upper: size.i64_or("upper_size", 1) as i32 },
            "three_layers_feature_size" => FeatureSize::Three {
                limit: size.i64_or("limit", 1) as i32,
                upper_limit: size.i64_or("upper_limit", 1) as i32,
                lower: size.i64_or("lower_size", 0) as i32,
                middle: size.i64_or("middle_size", 1) as i32,
                upper: size.i64_or("upper_size", 1) as i32,
            },
            other => bail!("unknown feature size {other}"),
        };
        let roots = match v.get("root_placer") {
            Some(r) => Some(MangroveRoots::parse(p, r)?),
            None => None,
        };
        Ok(Self {
            trunk: p.provider(v.field("trunk_provider")?)?,
            trunk_placer: TrunkPlacer::parse(p, v.field("trunk_placer")?)?,
            foliage: p.provider(v.field("foliage_provider")?)?,
            foliage_placer: FoliagePlacer::parse(p, v.field("foliage_placer")?)?,
            roots,
            min_clipped: size.get("min_clipped_height").and_then(Value::as_i64).map(|x| x as i32),
            size: fs,
            decorators: decorators(p, v.get("decorators"))?,
            ignore_vines: v.bool_or("ignore_vines", false),
            below_trunk: p.provider(v.field("below_trunk_provider")?)?,
        })
    }

    fn max_free_height(&self, env: &TreeEnv, w: &World, height: i32, origin: Pos) -> i32 {
        for y in 0..=height + 1 {
            let r = self.size.size_at(height, y);
            for dx in -r..=r {
                for dz in -r..=r {
                    let p = origin.add(dx, y, dz);
                    if !env.is_free(w, p) || (!self.ignore_vines && w.b.is_block(w.get(p), w.b.known.vine)) {
                        return y - 2;
                    }
                }
            }
        }
        height
    }

    fn do_place(&self, env: &TreeEnv, w: &mut World, r: &mut WorldgenRandom, parts: &mut Parts, origin: Pos) -> bool {
        let height = self.trunk_placer.tree_height(r);
        let foliage_height = self.foliage_placer.foliage_height(r, height);
        let trunk_height = height - foliage_height;
        let leaf_radius = self.foliage_placer.foliage_radius(r, trunk_height);
        let trunk_origin = match &self.roots {
            Some(roots) => origin.up_n(roots.trunk_offset_y.sample(r)),
            None => origin,
        };
        let min_y = origin.y.min(trunk_origin.y);
        let max_y = origin.y.max(trunk_origin.y) + height + 1;
        if min_y < w.min_y + 1 || max_y > w.max_y() + 1 {
            return false;
        }
        let free = self.max_free_height(env, w, height, trunk_origin);
        if free < height && self.min_clipped.is_none_or(|m| free < m) {
            return false;
        }
        if let Some(roots) = &self.roots {
            if !roots.place(w, r, parts, origin, trunk_origin) {
                return false;
            }
        }
        let attachments = self.trunk_placer.place(env, w, r, parts, free, trunk_origin);
        for a in attachments {
            self.foliage_placer.create(w, r, &self.foliage, parts, free, a, foliage_height, leaf_radius);
        }
        true
    }
}

impl Place for Tree {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let can_grow_through = match &self.trunk_placer.kind {
            TrunkKind::UpwardsBranching { can_grow_through, .. } => Some(&**can_grow_through),
            _ => None,
        };
        let env = TreeEnv { trunk: &self.trunk, below_trunk: &self.below_trunk, can_grow_through };
        let mut parts = Parts::default();
        let ok = self.do_place(&env, w, r, &mut parts, pos);
        if !ok || (parts.logs.is_empty() && parts.foliage.is_empty()) {
            return false;
        }
        let logs: JavaPosSet = parts.logs.iter().copied().collect();
        let foliage: JavaPosSet = parts.foliage.iter().copied().collect();
        let roots: JavaPosSet = parts.roots.iter().copied().collect();
        let mut decorations = JavaPosSet::new();
        if !self.decorators.is_empty() {
            let mut ctx = DecoratorContext::new(logs.to_vec(), foliage.to_vec(), roots.to_vec());
            for d in &self.decorators {
                d.place(w, r, &mut ctx);
            }
            decorations = ctx.decorations.iter().copied().collect();
        }
        update_leaves(w, &logs, &roots, &foliage, &decorations);
        true
    }
}

/// `TreeFeature.updateLeaves` followed by `StructureTemplate.updateShapeAtEdge`.
fn update_leaves(w: &mut World, logs: &JavaPosSet, roots: &JavaPosSet, foliage: &JavaPosSet, decorations: &JavaPosSet) {
    let all = logs.iter().chain(roots.iter()).chain(foliage.iter()).chain(decorations.iter());
    let (mut min, mut max) = (Pos::new(i32::MAX, i32::MAX, i32::MAX), Pos::new(i32::MIN, i32::MIN, i32::MIN));
    let mut any = false;
    for p in all {
        any = true;
        min = Pos::new(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
        max = Pos::new(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
    }
    if !any {
        return;
    }
    let (sx, sy, sz) = ((max.x - min.x + 1) as usize, (max.y - min.y + 1) as usize, (max.z - min.z + 1) as usize);
    let mut shape = vec![false; sx * sy * sz];
    let index = |p: Pos| -> Option<usize> {
        if p.x < min.x || p.x > max.x || p.y < min.y || p.y > max.y || p.z < min.z || p.z > max.z {
            None
        } else {
            Some(((p.x - min.x) as usize * sy + (p.y - min.y) as usize) * sz + (p.z - min.z) as usize)
        }
    };
    for p in decorations.iter().chain(roots.iter()) {
        if let Some(i) = index(p) {
            shape[i] = true;
        }
    }
    let mut to_check: Vec<JavaPosSet> = (0..7).map(|_| JavaPosSet::new()).collect();
    for p in logs.iter() {
        to_check[0].insert(p);
    }
    let b = w.b;
    let mut smallest = 0usize;
    'outer: loop {
        while smallest >= 7 || !to_check[smallest].is_empty() {
            if smallest >= 7 {
                break 'outer;
            }
            let Some(p) = to_check[smallest].pop_first() else {
                break;
            };
            let Some(i) = index(p) else {
                continue;
            };
            if smallest != 0 {
                let s = w.get(p);
                let ns = b.with_int(s, "distance", smallest as i32);
                w.set(p, ns);
            }
            shape[i] = true;
            for d in Dir::ALL {
                let n = p.rel(d);
                let Some(ni) = index(n) else {
                    continue;
                };
                if shape[ni] {
                    continue;
                }
                if let Some(dist) = b.leaf_distance(w.get(n)) {
                    let nd = (dist as usize).min(smallest + 1);
                    if nd < 7 {
                        to_check[nd].insert(n);
                        smallest = smallest.min(nd);
                    }
                }
            }
        }
        smallest += 1;
    }
    update_shape_at_edge(w, &shape, min, sx, sy, sz);
}

/// `StructureTemplate.updateShapeAtEdge`: every face of the shape updates the block on
/// both sides (in `DiscreteVoxelShape.forAllFaces` order).
fn update_shape_at_edge(w: &mut World, shape: &[bool], min: Pos, sx: usize, sy: usize, sz: usize) {
    let full = |x: i32, y: i32, z: i32| -> bool {
        x >= 0 && y >= 0 && z >= 0 && (x as usize) < sx && (y as usize) < sy && (z as usize) < sz && shape[(x as usize * sy + y as usize) * sz + z as usize]
    };
    let mut faces: Vec<(Dir, i32, i32, i32)> = Vec::new();
    let (sx, sy, sz) = (sx as i32, sy as i32, sz as i32);
    // AxisCycle.NONE: z faces.
    for x in 0..sx {
        for y in 0..sy {
            let mut last = false;
            for z in 0..=sz {
                let f = z != sz && full(x, y, z);
                if !last && f {
                    faces.push((Dir::North, x, y, z));
                }
                if last && !f {
                    faces.push((Dir::South, x, y, z - 1));
                }
                last = f;
            }
        }
    }
    // AxisCycle.FORWARD: y faces.
    for z in 0..sz {
        for x in 0..sx {
            let mut last = false;
            for y in 0..=sy {
                let f = y != sy && full(x, y, z);
                if !last && f {
                    faces.push((Dir::Down, x, y, z));
                }
                if last && !f {
                    faces.push((Dir::Up, x, y - 1, z));
                }
                last = f;
            }
        }
    }
    // AxisCycle.BACKWARD: x faces.
    for y in 0..sy {
        for z in 0..sz {
            let mut last = false;
            for x in 0..=sx {
                let f = x != sx && full(x, y, z);
                if !last && f {
                    faces.push((Dir::West, x, y, z));
                }
                if last && !f {
                    faces.push((Dir::East, x - 1, y, z));
                }
                last = f;
            }
        }
    }
    let b = w.b;
    for (d, x, y, z) in faces {
        let p = min.add(x, y, z);
        let n = p.rel(d);
        let s = w.get(p);
        let ns = w.get(n);
        let new = b.update_shape(w, s, p, d, ns);
        if new != s {
            w.set(p, new);
        }
        let new_n = b.update_shape(w, ns, n, d.opposite(), new);
        if new_n != ns {
            w.set(n, new_n);
        }
    }
}

struct FallenTree {
    trunk: Arc<StateProvider>,
    log_length: IntProvider,
    stump_decorators: Vec<Decorator>,
    log_decorators: Vec<Decorator>,
}

impl FallenTree {
    fn over_solid_ground(w: &World, p: Pos) -> bool {
        w.b.sturdy(w.get(p.down()), Dir::Up)
    }

    fn may_place_on(w: &World, p: Pos) -> bool {
        valid_tree_pos(w, p) && Self::over_solid_ground(w, p)
    }

    fn log(&self, w: &mut World, r: &mut WorldgenRandom, p: Pos, axis: Option<&str>) -> Pos {
        let mut s = self.trunk.get(w, r, p);
        if let Some(a) = axis {
            s = w.b.with(s, "axis", a);
        }
        w.set(p, s);
        p
    }

    fn decorate(w: &mut World, r: &mut WorldgenRandom, logs: Vec<Pos>, decorators: &[Decorator]) {
        if decorators.is_empty() {
            return;
        }
        let mut ctx = DecoratorContext::new(logs, Vec::new(), Vec::new());
        for d in decorators {
            d.place(w, r, &mut ctx);
        }
    }
}

impl Place for FallenTree {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let stump = self.log(w, r, pos, None);
        Self::decorate(w, r, vec![stump], &self.stump_decorators);
        let dir = Dir::random_horizontal(r);
        let length = self.log_length.sample(r) - 2;
        let mut start = pos.rel_n(dir, 2 + r.next_bounded(2));
        start = start.up();
        for _ in 0..6 {
            if Self::may_place_on(w, start) {
                break;
            }
            start = start.down();
        }
        let mut gap = 0;
        let mut p = start;
        for _ in 0..length {
            if !valid_tree_pos(w, p) {
                return true;
            }
            if Self::over_solid_ground(w, p) {
                gap = 0;
            } else {
                gap += 1;
                if gap > 2 {
                    return true;
                }
            }
            p = p.rel(dir);
        }
        let mut set = JavaPosSet::new();
        let mut p = start;
        let axis = dir.axis();
        for _ in 0..length {
            set.insert(self.log(w, r, p, Some(axis)));
            p = p.rel(dir);
        }
        Self::decorate(w, r, set.to_vec(), &self.log_decorators);
        true
    }
}
