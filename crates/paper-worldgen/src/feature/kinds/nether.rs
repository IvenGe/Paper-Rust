//! Nether features: huge fungi, deltas, blob replacement and basalt column clusters.
//!
//! Derived from `pumpkin-world/src/generation/feature/features/{huge_fungus,delta_feature,
//! netherrack_replace_blobs,stepped_column_cluster}.rs` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, and checked against vanilla 26.3.

use crate::data::JsonExt;
use crate::feature::blockinfo::BlockSet;
use crate::feature::parse::Parser;
use crate::feature::predicate::BlockPredicate;
use crate::feature::provider::StateProvider;
use crate::feature::world::{within_manhattan, Dir, Pos, World};
use crate::feature::Place;
use crate::providers::IntProvider;
use crate::random::{Random, WorldgenRandom};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub(super) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(Some(match t {
        "huge_fungus" => Box::new(HugeFungus {
            valid_base: p.state(v.field("valid_base_block")?)?,
            stem: p.state(v.field("stem_state")?)?,
            hat: p.state(v.field("hat_state")?)?,
            decor: p.state(v.field("decor_state")?)?,
            replaceable: p.predicate(v.field("replaceable_blocks")?)?,
            planted: v.bool_or("planted", false),
        }),
        "delta_feature" => Box::new(Delta {
            contents: p.state(v.field("contents")?)?,
            rim: p.state(v.field("rim")?)?,
            size: p.int_provider(v.field("size")?)?,
            rim_size: p.int_provider(v.field("rim_size")?)?,
            cannot_replace: BlockSet::from_ids(
                ["bedrock", "nether_bricks", "nether_brick_fence", "nether_brick_stairs", "nether_wart", "chest", "spawner"]
                    .iter()
                    .map(|b| p.block_id(&format!("minecraft:{b}")))
                    .collect::<Result<Vec<_>>>()?,
            ),
        }),
        "netherrack_replace_blobs" => Box::new(ReplaceBlobs {
            target: p.state(v.field("target")?)?,
            state: p.state(v.field("state")?)?,
            radius: p.int_provider(v.field("radius")?)?,
        }),
        "stepped_column_cluster" => Box::new(SteppedColumnCluster {
            block: p.provider(v.field("block")?)?,
            continue_through: p.predicate(v.field("continue_through")?)?,
            can_replace: p.predicate(v.field("can_replace")?)?,
            cannot_place_on: p.block_set(v.field("cannot_place_on")?)?,
            cluster_reach: p.int_provider(v.field("cluster_reach")?)?,
            column_count: p.int_provider(v.field("column_count")?)?,
            column_reach: p.int_provider(v.field("column_reach")?)?,
            height: p.int_provider(v.field("height")?)?,
        }),
        _ => return Ok(None),
    }))
}

/// `HugeFungusFeature`: crimson and warped fungi.
struct HugeFungus {
    valid_base: u32,
    stem: u32,
    hat: u32,
    decor: u32,
    replaceable: BlockPredicate,
    planted: bool,
}

impl HugeFungus {
    /// `isReplaceable`: replaceable blocks, plus the configured ones when `check` is set.
    fn is_replaceable(&self, w: &World, p: Pos, check: bool) -> bool {
        w.b.is_replaceable(w.get(p)) || (check && self.replaceable.test(w, p))
    }

    fn place_stem(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos, height: i32, huge: bool) {
        let radius: i32 = if huge { 1 } else { 0 };
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let corner = huge && dx.abs() == radius && dz.abs() == radius;
                for dy in 0..height {
                    let p = origin.add(dx, dy, dz);
                    if !self.is_replaceable(w, p, true) {
                        continue;
                    }
                    if self.planted {
                        // Only saplings grown by players are planted; destroying the block
                        // first makes no difference to the resulting state.
                        w.set(p, self.stem);
                    } else if corner {
                        if r.next_f32() < 0.1 {
                            w.set(p, self.stem);
                        }
                    } else {
                        w.set(p, self.stem);
                    }
                }
            }
        }
    }

    fn place_hat(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos, height: i32, huge: bool) {
        let vines = w.b.is_block(self.hat, w.b.known.nether_wart_block);
        let hat_height = (r.next_bounded(1 + height / 3) + 5).min(height);
        let start = height - hat_height;
        for dy in start..=height {
            let mut radius = if dy < height - r.next_bounded(3) { 2 } else { 1 };
            if hat_height > 8 && dy < start + 4 {
                radius = 3;
            }
            if huge {
                radius += 1;
            }
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let edge_x = dx == -radius || dx == radius;
                    let edge_z = dz == -radius || dz == radius;
                    let inside = !edge_x && !edge_z && dy != height;
                    let corner = edge_x && edge_z;
                    let lower = dy < start + 3;
                    let p = origin.add(dx, dy, dz);
                    if !self.is_replaceable(w, p, false) {
                        continue;
                    }
                    if lower {
                        if !inside {
                            self.place_hat_drop_block(w, r, p, vines);
                        }
                    } else if inside {
                        self.place_hat_block(w, r, p, 0.1, 0.2, if vines { 0.1 } else { 0.0 });
                    } else if corner {
                        self.place_hat_block(w, r, p, 0.01, 0.7, if vines { 0.083 } else { 0.0 });
                    } else {
                        self.place_hat_block(w, r, p, 5.0e-4, 0.98, if vines { 0.07 } else { 0.0 });
                    }
                }
            }
        }
    }

    fn place_hat_block(&self, w: &mut World, r: &mut WorldgenRandom, p: Pos, decor: f32, hat: f32, vines: f32) {
        if r.next_f32() < decor {
            w.set(p, self.decor);
        } else if r.next_f32() < hat {
            w.set(p, self.hat);
            if r.next_f32() < vines {
                place_weeping_vines(w, r, p);
            }
        }
    }

    fn place_hat_drop_block(&self, w: &mut World, r: &mut WorldgenRandom, p: Pos, vines: bool) {
        if w.b.is_block(w.get(p.down()), self.hat) {
            w.set(p, self.hat);
        } else if (r.next_f32() as f64) < 0.15 {
            w.set(p, self.hat);
            if vines && r.next_bounded(11) == 0 {
                place_weeping_vines(w, r, p);
            }
        }
    }
}

/// `HugeFungusFeature.tryPlaceWeepingVines` and `WeepingVinesFeature.placeWeepingVinesColumn`.
fn place_weeping_vines(w: &mut World, r: &mut WorldgenRandom, hat: Pos) {
    let mut p = hat.down();
    if !w.is_air(p) {
        return;
    }
    let mut height = r.next_int_range(1, 5);
    if r.next_bounded(7) == 0 {
        height *= 2;
    }
    for i in 0..=height {
        if w.is_air(p) {
            if i == height || !w.is_air(p.down()) {
                let age = r.next_int_range(23, 25);
                let state = w.b.with_int(w.b.known.weeping_vines, "age", age);
                w.set(p, state);
                break;
            }
            w.set(p, w.b.known.weeping_vines_plant);
        }
        p = p.down();
    }
}

impl Place for HugeFungus {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        if !w.b.is_block(w.get(pos.down()), self.valid_base) {
            return false;
        }
        let mut height = r.next_int_range(4, 13);
        if r.next_bounded(12) == 0 {
            height *= 2;
        }
        if !self.planted && pos.y + height + 1 >= w.gen.gen_height {
            return false;
        }
        let huge = !self.planted && r.next_f32() < 0.06;
        w.set(pos, w.b.known.air);
        self.place_stem(w, r, pos, height, huge);
        self.place_hat(w, r, pos, height, huge);
        true
    }
}

/// `DeltaFeature`: lava pools with magma rims in basalt deltas.
struct Delta {
    contents: u32,
    rim: u32,
    size: IntProvider,
    rim_size: IntProvider,
    cannot_replace: BlockSet,
}

impl Delta {
    fn is_clear(&self, w: &World, p: Pos) -> bool {
        let state = w.get(p);
        if w.b.is_block(state, self.contents) || self.cannot_replace.has(state) {
            return false;
        }
        for d in Dir::ALL {
            let air = w.is_air(p.rel(d));
            if air != (d == Dir::Up) {
                return false;
            }
        }
        true
    }
}

impl Place for Delta {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let mut placed = false;
        let rim = r.next_f64() < 0.9;
        let rim_x = if rim { self.rim_size.sample(r) } else { 0 };
        let rim_z = if rim { self.rim_size.sample(r) } else { 0 };
        let place_rim = rim && rim_x != 0 && rim_z != 0;
        let size_x = self.size.sample(r);
        let size_z = self.size.sample(r);
        let max = size_x.max(size_z);
        for p in within_manhattan(origin, size_x, 0, size_z) {
            if p.manhattan(origin) > max {
                break;
            }
            if self.is_clear(w, p) {
                if place_rim {
                    placed = true;
                    w.set(p, self.rim);
                }
                let contents = p.add(rim_x, 0, rim_z);
                if self.is_clear(w, contents) {
                    placed = true;
                    w.set(contents, self.contents);
                }
            }
        }
        placed
    }
}

/// `ReplaceBlobsFeature` (`netherrack_replace_blobs`).
struct ReplaceBlobs {
    target: u32,
    state: u32,
    radius: IntProvider,
}

impl Place for ReplaceBlobs {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        let mut cursor = origin.with_y(origin.y.clamp(w.min_y + 1, w.max_y()));
        let center = loop {
            if cursor.y <= w.min_y + 1 {
                return false;
            }
            if w.b.is_block(w.get(cursor), self.target) {
                break cursor;
            }
            cursor = cursor.down();
        };
        let rx = self.radius.sample(r);
        let ry = self.radius.sample(r);
        let rz = self.radius.sample(r);
        let max = rx.max(ry.max(rz));
        let mut replaced = false;
        for p in within_manhattan(center, rx, ry, rz) {
            if p.manhattan(center) > max {
                break;
            }
            if w.b.is_block(w.get(p), self.target) {
                w.set(p, self.state);
                replaced = true;
            }
        }
        replaced
    }
}

/// `SteppedColumnClusterFeature`: the basalt columns of basalt deltas.
struct SteppedColumnCluster {
    block: Arc<StateProvider>,
    continue_through: BlockPredicate,
    can_replace: BlockPredicate,
    cannot_place_on: Arc<BlockSet>,
    cluster_reach: IntProvider,
    column_count: IntProvider,
    column_reach: IntProvider,
    height: IntProvider,
}

impl SteppedColumnCluster {
    fn can_place_at(&self, w: &World, p: Pos) -> bool {
        if !self.can_replace.test(w, p) {
            return false;
        }
        let below = w.get(p.down());
        !w.b.is_air(below) && !self.cannot_place_on.has(below)
    }

    fn find_surface(&self, w: &World, mut p: Pos, mut limit: i32) -> Option<Pos> {
        while p.y > w.min_y + 1 && limit > 0 {
            limit -= 1;
            if self.can_place_at(w, p) {
                return Some(p);
            }
            p = p.down();
        }
        None
    }

    fn find_air(&self, w: &World, mut p: Pos, mut limit: i32) -> Option<Pos> {
        while p.y <= w.max_y() && limit > 0 {
            limit -= 1;
            let state = w.get(p);
            if self.cannot_place_on.has(state) {
                return None;
            }
            if w.b.is_air(state) {
                return Some(p);
            }
            p = p.up();
        }
        None
    }

    fn place_column(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos, height: i32, reach: i32) -> bool {
        let mut placed = false;
        for z in origin.z - reach..=origin.z + reach {
            for x in origin.x - reach..=origin.x + reach {
                let p = Pos::new(x, origin.y, z);
                let step = p.manhattan(origin);
                let column = if self.can_replace.test(w, p) { self.find_surface(w, p, step) } else { self.find_air(w, p, step) };
                let Some(mut cursor) = column else { continue };
                let mut blocks = height - step / 2;
                while blocks >= 0 {
                    if self.can_replace.test(w, cursor) {
                        let state = self.block.get(w, r, cursor);
                        w.set(cursor, state);
                        cursor = cursor.up();
                        placed = true;
                    } else {
                        if !self.continue_through.test(w, cursor) {
                            break;
                        }
                        cursor = cursor.up();
                    }
                    blocks -= 1;
                }
            }
        }
        placed
    }
}

impl Place for SteppedColumnCluster {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, origin: Pos) -> bool {
        if !self.can_place_at(w, origin) {
            return false;
        }
        let column_height = self.height.sample(r);
        let reach = column_height.min(self.cluster_reach.sample(r));
        let count = self.column_count.sample(r);
        let mut placed = false;
        let width = 2 * reach + 1;
        for _ in 0..count {
            let x = origin.x - reach + r.next_bounded(width);
            let y = origin.y + r.next_bounded(1);
            let z = origin.z - reach + r.next_bounded(width);
            let p = Pos::new(x, y, z);
            let blocks = column_height - p.manhattan(origin);
            if blocks >= 0 {
                let column_reach = self.column_reach.sample(r);
                placed |= self.place_column(w, r, p, blocks, column_reach);
            }
        }
        placed
    }
}
