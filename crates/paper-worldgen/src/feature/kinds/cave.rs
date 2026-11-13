//! Cave and plant features: vegetation patches, root systems, multiface growths (glow
//! lichen), vines, bamboo.
//!
//! Derived from `pumpkin-world/src/generation/feature/features/{vegetation_patch,
//! waterlogged_vegetation_patch,root_system,multiface_growth,vines,bamboo}.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, and checked against vanilla 26.3.

use crate::chunk::Heightmap;
use crate::data::JsonExt;
use crate::feature::blockinfo::{BlockSet, Fluid};
use crate::feature::javaset::JavaPosSet;
use crate::feature::parse::Parser;
use crate::feature::placement::PlacedFeature;
use crate::feature::predicate::BlockPredicate;
use crate::feature::provider::StateProvider;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::Place;
use crate::providers::IntProvider;
use crate::random::{Random, WorldgenRandom};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::sync::Arc;

pub(super) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(Some(match t {
        "vegetation_patch" => Box::new(VegetationPatch::parse(p, v, false)?),
        "waterlogged_vegetation_patch" => Box::new(VegetationPatch::parse(p, v, true)?),
        "root_system" => Box::new(RootSystem {
            tree: p.placed(v.field("feature")?)?,
            required_space: v.i64_or("required_vertical_space_for_tree", 0) as i32,
            level_test_distance: v.i64_or("level_test_distance", 0) as i32,
            max_level_deviation: v.i64_or("max_level_deviation", 0) as i32,
            root_radius: v.i64_or("root_radius", 1) as i32,
            root_replaceable: p.block_set(v.field("root_replaceable")?)?,
            root_provider: p.provider(v.field("root_state_provider")?)?,
            root_attempts: v.i64_or("root_placement_attempts", 0) as i32,
            root_column_max_height: v.i64_or("root_column_max_height", 0) as i32,
            hanging_radius: v.i64_or("hanging_root_radius", 1) as i32,
            hanging_span: v.i64_or("hanging_roots_vertical_span", 1) as i32,
            hanging_provider: p.provider(v.field("hanging_root_state_provider")?)?,
            hanging_attempts: v.i64_or("hanging_root_placement_attempts", 0) as i32,
            allowed_water: v.i64_or("allowed_vertical_water_for_tree", 0) as i32,
            allowed_position: p.predicate(v.field("allowed_tree_position")?)?,
        }),
        "multiface_growth" => {
            let block = p.state(v.get("block").unwrap_or(&Value::String("minecraft:glow_lichen".into())))?;
            Box::new(MultifaceGrowth {
                block,
                search_range: v.i64_or("search_range", 10) as i32,
                floor: v.bool_or("can_place_on_floor", false),
                ceiling: v.bool_or("can_place_on_ceiling", false),
                wall: v.bool_or("can_place_on_wall", false),
                spread_chance: v.f64_or("chance_of_spreading", 0.5) as f32,
                placed_on: p.block_set(v.field("can_be_placed_on")?)?,
            })
        }
        "vines" => Box::new(Vines),
        "bamboo" => Box::new(Bamboo { probability: v.f64_or("probability", 0.0) as f32, podzol_replaceable: p.tag("minecraft:beneath_bamboo_podzol_replaceable")? }),
        _ => return Ok(None),
    }))
}

struct VegetationPatch {
    replaceable: Arc<BlockSet>,
    ground: Arc<StateProvider>,
    vegetation: Arc<PlacedFeature>,
    ceiling: bool,
    depth: IntProvider,
    extra_bottom_chance: f32,
    vertical_range: i32,
    vegetation_chance: f32,
    xz_radius: IntProvider,
    extra_edge_chance: f32,
    waterlogged: bool,
}

impl VegetationPatch {
    fn parse(p: &mut Parser, v: &Value, waterlogged: bool) -> Result<Self> {
        let surface = v.str_field("surface")?;
        let ceiling = match surface {
            "floor" => false,
            "ceiling" => true,
            other => bail!("bad cave surface {other}"),
        };
        Ok(Self {
            replaceable: p.block_set(v.field("replaceable")?)?,
            ground: p.provider(v.field("ground_state")?)?,
            vegetation: p.placed(v.field("vegetation_feature")?)?,
            ceiling,
            depth: p.int_provider(v.field("depth")?)?,
            extra_bottom_chance: v.f64_or("extra_bottom_block_chance", 0.0) as f32,
            vertical_range: v.i64_or("vertical_range", 0) as i32,
            vegetation_chance: v.f64_or("vegetation_chance", 0.0) as f32,
            xz_radius: p.int_provider(v.field("xz_radius")?)?,
            extra_edge_chance: v.f64_or("extra_edge_column_chance", 0.0) as f32,
            waterlogged,
        })
    }

    /// The direction from the patch surface into the ground.
    fn inwards(&self) -> Dir {
        if self.ceiling {
            Dir::Up
        } else {
            Dir::Down
        }
    }

    fn ground_patch(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos, xr: i32, zr: i32) -> JavaPosSet {
        let inwards = self.inwards();
        let outwards = inwards.opposite();
        let mut surface = JavaPosSet::new();
        for dx in -xr..=xr {
            let x_edge = dx == -xr || dx == xr;
            for dz in -zr..=zr {
                let z_edge = dz == -zr || dz == zr;
                let corner = x_edge && z_edge;
                let edge = (x_edge || z_edge) && !corner;
                if corner {
                    continue;
                }
                if edge && (self.extra_edge_chance == 0.0 || r.next_f32() > self.extra_edge_chance) {
                    continue;
                }
                let mut p = origin.add(dx, 0, dz);
                let mut i = 0;
                while w.is_air(p) && i < self.vertical_range {
                    p = p.rel(inwards);
                    i += 1;
                }
                i = 0;
                while !w.is_air(p) && i < self.vertical_range {
                    p = p.rel(outwards);
                    i += 1;
                }
                let below = p.rel(inwards);
                if w.is_air(p) && w.b.sturdy(w.get(below), outwards) {
                    let depth = self.depth.sample(r) + i32::from(self.extra_bottom_chance > 0.0 && r.next_f32() < self.extra_bottom_chance);
                    if self.place_ground(w, r, below, depth) {
                        surface.insert(below);
                    }
                }
            }
        }
        surface
    }

    fn place_ground(&self, w: &mut World, r: &mut WorldgenRandom, mut p: Pos, depth: i32) -> bool {
        for i in 0..depth {
            let s = self.ground.get(w, r, p);
            let here = w.get(p);
            if w.b.is_block(s, here) {
                continue;
            }
            if !self.replaceable.has(here) {
                return i != 0;
            }
            w.set(p, s);
            p = p.rel(self.inwards());
        }
        true
    }

    fn exposed(w: &World, p: Pos) -> bool {
        [Dir::North, Dir::East, Dir::South, Dir::West, Dir::Down].iter().any(|d| !w.b.sturdy(w.get(p.rel(*d)), d.opposite()))
    }
}

impl Place for VegetationPatch {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let xr = self.xz_radius.sample(r) + 1;
        let zr = self.xz_radius.sample(r) + 1;
        let mut surface = self.ground_patch(w, r, pos, xr, zr);
        if self.waterlogged {
            let water: JavaPosSet = surface.iter().filter(|p| !Self::exposed(w, *p)).collect();
            for p in water.iter() {
                w.set(p, w.b.known.water);
            }
            surface = water;
        }
        for p in surface.iter() {
            if self.vegetation_chance > 0.0 && r.next_f32() < self.vegetation_chance {
                if self.waterlogged {
                    if self.vegetation.place(w, r, p) {
                        let s = w.get(p);
                        if w.b.prop_bool(s, "waterlogged") == Some(false) {
                            let ns = w.b.waterlogged(s, true);
                            w.set(p, ns);
                        }
                    }
                } else {
                    self.vegetation.place(w, r, p.rel(self.inwards().opposite()));
                }
            }
        }
        !surface.is_empty()
    }
}

struct RootSystem {
    tree: Arc<PlacedFeature>,
    required_space: i32,
    level_test_distance: i32,
    max_level_deviation: i32,
    root_radius: i32,
    root_replaceable: Arc<BlockSet>,
    root_provider: Arc<StateProvider>,
    root_attempts: i32,
    root_column_max_height: i32,
    hanging_radius: i32,
    hanging_span: i32,
    hanging_provider: Arc<StateProvider>,
    hanging_attempts: i32,
    allowed_water: i32,
    allowed_position: BlockPredicate,
}

impl RootSystem {
    fn space_for_tree(&self, w: &World, p: Pos) -> bool {
        let mut q = p;
        for i in 1..=self.required_space {
            q = q.up();
            let s = w.get(q);
            if !(w.b.is_air(s) || (i + 1 <= self.allowed_water && w.b.fluid(s).is_water())) {
                return false;
            }
        }
        if self.level_test_distance > 0 {
            for d in [Dir::South, Dir::West, Dir::North, Dir::East] {
                let m = p.rel_n(d, self.level_test_distance);
                let below = w.get(m.up_n(-self.max_level_deviation));
                let above = w.get(m.up_n(self.max_level_deviation));
                if w.b.is_air(below) || !w.b.is_air(above) {
                    return false;
                }
            }
        }
        true
    }
}

impl Place for RootSystem {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        if !w.is_air(pos) {
            return false;
        }
        let mut p = pos;
        let mut placed = false;
        for i in 0..self.root_column_max_height {
            p = p.up();
            if w.height_at(Heightmap::WorldSurface, p.x, p.z) < p.y {
                break;
            }
            if self.allowed_position.test(w, p) && self.space_for_tree(w, p) {
                let below = p.down();
                let bs = w.get(below);
                if w.b.fluid(bs).is_lava() || !w.b.is_solid(bs) {
                    break;
                }
                if self.tree.place(w, r, p) {
                    for y in pos.y..pos.y + i {
                        for _ in 0..self.root_attempts {
                            let q = Pos::new(
                                pos.x + r.next_bounded(self.root_radius) - r.next_bounded(self.root_radius),
                                y,
                                pos.z + r.next_bounded(self.root_radius) - r.next_bounded(self.root_radius),
                            );
                            if self.root_replaceable.has(w.get(q)) {
                                let s = self.root_provider.get(w, r, q);
                                w.set(q, s);
                            }
                        }
                    }
                    placed = true;
                    break;
                }
            }
        }
        if placed {
            for _ in 0..self.hanging_attempts {
                let q = pos.add(
                    r.next_bounded(self.hanging_radius) - r.next_bounded(self.hanging_radius),
                    r.next_bounded(self.hanging_span) - r.next_bounded(self.hanging_span),
                    r.next_bounded(self.hanging_radius) - r.next_bounded(self.hanging_radius),
                );
                if w.is_air(q) {
                    let s = self.hanging_provider.get(w, r, q);
                    if w.b.can_survive(w, s, q) && w.b.sturdy(w.get(q.up()), Dir::Down) {
                        w.set(q, s);
                    }
                }
            }
        }
        true
    }
}

struct MultifaceGrowth {
    block: u32,
    search_range: i32,
    floor: bool,
    ceiling: bool,
    wall: bool,
    spread_chance: f32,
    placed_on: Arc<BlockSet>,
}

impl MultifaceGrowth {
    fn valid_directions(&self) -> Vec<Dir> {
        let mut v = Vec::with_capacity(6);
        if self.ceiling {
            v.push(Dir::Up);
        }
        if self.floor {
            v.push(Dir::Down);
        }
        if self.wall {
            v.extend(Dir::HORIZONTAL);
        }
        v
    }

    fn shuffled(r: &mut WorldgenRandom, mut v: Vec<Dir>) -> Vec<Dir> {
        for i in (2..=v.len()).rev() {
            let j = r.next_bounded(i as i32) as usize;
            v.swap(i - 1, j);
        }
        v
    }

    fn air_or_water(w: &World, s: u32) -> bool {
        w.b.is_air(s) || w.b.is_block(s, w.b.known.water)
    }

    fn is_self(&self, w: &World, s: u32) -> bool {
        w.b.is_block(s, self.block)
    }

    /// `MultifaceBlock.isValidStateForPlacement`.
    fn valid_for_placement(&self, w: &World, old: u32, p: Pos, face: Dir) -> bool {
        if self.is_self(w, old) && w.b.prop_bool(old, face.name()) == Some(true) {
            return false;
        }
        w.b.can_attach_to(w.get(p.rel(face)), face.opposite())
    }

    /// `MultifaceBlock.getStateForPlacement`.
    fn state_for_placement(&self, w: &World, old: u32, p: Pos, face: Dir) -> Option<u32> {
        if !self.valid_for_placement(w, old, p, face) {
            return None;
        }
        let base = if self.is_self(w, old) {
            old
        } else if w.b.fluid(old) == Fluid::Water {
            w.b.waterlogged(self.block, true)
        } else {
            self.block
        };
        Some(w.b.with_bool(base, face.name(), true))
    }

    fn place_growth(&self, w: &mut World, r: &mut WorldgenRandom, p: Pos, old: u32, dirs: &[Dir]) -> bool {
        for d in dirs {
            if !self.placed_on.has(w.get(p.rel(*d))) {
                continue;
            }
            let Some(new) = self.state_for_placement(w, old, p, *d) else {
                return false;
            };
            w.set(p, new);
            if r.next_f32() < self.spread_chance {
                self.spread(w, r, new, p, *d);
            }
            return true;
        }
        false
    }

    /// `MultifaceSpreader.spreadFromFaceTowardRandomDirection`.
    fn spread(&self, w: &mut World, r: &mut WorldgenRandom, state: u32, p: Pos, from: Dir) {
        for d in Self::shuffled(r, Dir::ALL.to_vec()) {
            if d.axis() == from.axis() {
                continue;
            }
            if w.b.prop_bool(state, from.name()) != Some(true) || w.b.prop_bool(state, d.name()) == Some(true) {
                continue;
            }
            for (target, face) in [(p, d), (p.rel(d), from), (p.rel(d).rel(from), d.opposite())] {
                let existing = w.get(target);
                let replaceable = w.b.is_air(existing) || self.is_self(w, existing) || (w.b.is_block(existing, w.b.known.water) && w.b.fluid(existing) == Fluid::Water);
                if replaceable && self.valid_for_placement(w, existing, target, face) {
                    if let Some(s) = self.state_for_placement(w, existing, target, face) {
                        w.set(target, s);
                    }
                    return;
                }
            }
        }
    }
}

impl Place for MultifaceGrowth {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let here = w.get(pos);
        if !Self::air_or_water(w, here) {
            return false;
        }
        let dirs = Self::shuffled(r, self.valid_directions());
        if self.place_growth(w, r, pos, here, &dirs) {
            return true;
        }
        for d in dirs {
            let except: Vec<Dir> = self.valid_directions().into_iter().filter(|x| *x != d.opposite()).collect();
            let placement = Self::shuffled(r, except);
            let p = pos.rel(d);
            for _ in 0..self.search_range {
                let s = w.get(p);
                if !Self::air_or_water(w, s) && !self.is_self(w, s) {
                    break;
                }
                if self.place_growth(w, r, p, s, &placement) {
                    return true;
                }
            }
        }
        false
    }
}

struct Vines;

impl Place for Vines {
    fn place(&self, w: &mut World, _r: &mut WorldgenRandom, pos: Pos) -> bool {
        if !w.is_air(pos) {
            return false;
        }
        for d in Dir::ALL {
            if d == Dir::Down {
                continue;
            }
            if w.b.can_attach_to(w.get(pos.rel(d)), d.opposite()) {
                let s = w.b.with_bool(w.b.known.vine, d.name(), true);
                w.set(pos, s);
                return true;
            }
        }
        false
    }
}

struct Bamboo {
    probability: f32,
    podzol_replaceable: Arc<BlockSet>,
}

impl Place for Bamboo {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        if !w.is_air(pos) {
            return false;
        }
        let b = w.b;
        let bamboo = b.known.bamboo;
        if b.can_survive(w, bamboo, pos) {
            let height = r.next_bounded(12) + 5;
            if r.next_f32() < self.probability {
                let radius = r.next_bounded(4) + 1;
                for x in pos.x - radius..=pos.x + radius {
                    for z in pos.z - radius..=pos.z + radius {
                        let (dx, dz) = (x - pos.x, z - pos.z);
                        if dx * dx + dz * dz <= radius * radius {
                            let y = w.height_at(Heightmap::WorldSurface, x, z) - 1;
                            let q = Pos::new(x, y, z);
                            if self.podzol_replaceable.has(w.get(q)) {
                                w.set(q, b.known.podzol);
                            }
                        }
                    }
                }
            }
            let state = |leaves: &str, stage: i32| b.with_int(b.with(b.with_int(bamboo, "age", 1), "leaves", leaves), "stage", stage);
            let trunk = state("none", 0);
            let mut p = pos;
            let mut i = 0;
            while i < height && w.is_air(p) {
                w.set(p, trunk);
                p = p.up();
                i += 1;
            }
            if p.y - pos.y >= 3 {
                w.set(p, state("large", 1));
                w.set(p.down(), state("large", 0));
                w.set(p.down().down(), state("small", 0));
            }
        }
        true
    }
}

#[allow(dead_code)]
fn _unused(_: anyhow::Error) -> anyhow::Error {
    anyhow!("unused")
}
