//! Sculk patches (`sculk_patch`, in the deep dark and in ancient cities).
//!
//! Written to vanilla 26.3's `SculkPatchFeature` (in 26.3 the catalyst and the shriekers
//! are separate steps of the data's `sequence`, so the feature only spreads charge), the
//! world generation `SculkSpreader` and its `ChargeCursor`, the `SculkBehaviour` of sculk,
//! sculk veins and other blocks, `SculkVeinBlock` (placing sculk, regrowing, discharging)
//! and the `MultifaceSpreader` with `SculkVeinSpreaderConfig`. Checked against
//! `pumpkin-world/src/generation/feature/features/sculk/` of Pumpkin
//! (https://github.com/Pumpkin-MC/Pumpkin), GPL-3.0, which departs from vanilla in a few
//! places this file follows vanilla in: the order of the non-corner neighbours before
//! they are shuffled (`BlockPos.betweenClosed` order), the cursor taking the faces of the
//! block it stays on, sculk veins discharging the state the cursor read, flowing water
//! and the `#fire` tag in `stateCanBeReplaced`, the collision face in `canAttachTo`, and
//! source water only above growths.
//!
//! Everything draws from the feature's random, so the result is deterministic. Sensors,
//! shriekers and catalysts get their block entities from the server, as vanilla's
//! world generation region leaves them for the chunk to create.

use crate::data::JsonExt;
use crate::feature::blockinfo::{BlockSet, Fluid};
use crate::feature::parse::Parser;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::Place;
use crate::random::{Random, WorldgenRandom};
use anyhow::Result;
use paper_data::blocks;
use serde_json::Value;
use std::sync::Arc;

pub(super) fn parse(p: &mut Parser, t: &str, v: &Value) -> Result<Option<Box<dyn Place>>> {
    if t != "sculk_patch" {
        return Ok(None);
    }
    Ok(Some(Box::new(SculkPatch {
        charge_count: v.i64_or("charge_count", 0) as i32,
        amount_per_charge: v.i64_or("amount_per_charge", 0) as i32,
        spread_attempts: v.i64_or("spread_attempts", 0) as i32,
        growth_rounds: v.i64_or("growth_rounds", 0) as i32,
        spread_rounds: v.i64_or("spread_rounds", 0) as i32,
        s: Arc::new(SculkBlocks::load(p)?),
    })))
}

/// The blocks and tags sculk spreading looks at.
struct SculkBlocks {
    /// Block ids.
    sculk: u32,
    vein: u32,
    catalyst: u32,
    moving_piston: u32,
    sensor: u32,
    shrieker: u32,
    water: u32,
    /// Default states.
    sculk_state: u32,
    vein_state: u32,
    sensor_state: u32,
    shrieker_state: u32,
    water_state: u32,
    fire: Arc<BlockSet>,
    /// `#sculk_replaceable` (substrate access).
    replaceable: Arc<BlockSet>,
    /// `#sculk_replaceable_world_gen` (the world generation spreader's blocks).
    replaceable_world_gen: Arc<BlockSet>,
}

impl SculkBlocks {
    fn load(p: &mut Parser) -> Result<Self> {
        let state = |name: &str| blocks::default_state(name).ok_or_else(|| anyhow::anyhow!("unknown block {name}"));
        let (sculk_state, vein_state, sensor_state, shrieker_state, water_state) =
            (state("minecraft:sculk")?, state("minecraft:sculk_vein")?, state("minecraft:sculk_sensor")?, state("minecraft:sculk_shrieker")?, state("minecraft:water")?);
        let id = blocks::block_id_of_state;
        Ok(Self {
            sculk: id(sculk_state),
            vein: id(vein_state),
            catalyst: id(state("minecraft:sculk_catalyst")?),
            moving_piston: id(state("minecraft:moving_piston")?),
            sensor: id(sensor_state),
            shrieker: id(shrieker_state),
            water: id(water_state),
            sculk_state,
            vein_state,
            sensor_state,
            shrieker_state,
            water_state,
            fire: p.tag("minecraft:fire")?,
            replaceable: p.tag("minecraft:sculk_replaceable")?,
            replaceable_world_gen: p.tag("minecraft:sculk_replaceable_world_gen")?,
        })
    }

    fn is(&self, state: u32, block: u32) -> bool {
        blocks::block_id_of_state(state) == block
    }

    fn is_vein(&self, state: u32) -> bool {
        self.is(state, self.vein)
    }

    /// The block's `SculkBehaviour`.
    fn behaviour(&self, state: u32) -> Behaviour {
        match blocks::block_id_of_state(state) {
            b if b == self.sculk => Behaviour::Sculk,
            b if b == self.vein => Behaviour::Vein,
            _ => Behaviour::Default,
        }
    }

    /// `MultifaceBlock.hasFace`.
    fn has_face(&self, state: u32, d: Dir) -> bool {
        self.is_vein(state) && blocks::property(state, d.name()) == Some("true")
    }

    fn with_face(&self, state: u32, d: Dir, value: bool) -> u32 {
        blocks::with_property(state, d.name(), if value { "true" } else { "false" }).unwrap_or(state)
    }

    fn faces(&self, state: u32) -> u8 {
        let mut set = 0;
        for d in Dir::ALL {
            if self.has_face(state, d) {
                set |= 1 << d.index();
            }
        }
        set
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Sculk,
    Vein,
    Default,
}

/// `SculkPatchFeature`.
struct SculkPatch {
    charge_count: i32,
    amount_per_charge: i32,
    spread_attempts: i32,
    growth_rounds: i32,
    spread_rounds: i32,
    s: Arc<SculkBlocks>,
}

impl Place for SculkPatch {
    fn place(&self, w: &mut World, r: &mut WorldgenRandom, pos: Pos) -> bool {
        let s = &*self.s;
        if !can_spread_from(w, s, pos) {
            return false;
        }
        let mut spreader = Spreader { cursors: Vec::new() };
        for round in 0..self.spread_rounds + self.growth_rounds {
            for _ in 0..self.charge_count {
                spreader.add_cursors(pos, self.amount_per_charge);
            }
            let convert = round < self.spread_rounds;
            for _ in 0..self.spread_attempts {
                spreader.update_cursors(w, s, pos, r, convert);
            }
            spreader.cursors.clear();
        }
        true
    }
}

/// `SculkPatchFeature.canSpreadFrom`: sculk, or air or a water source next to a full block.
fn can_spread_from(w: &World, s: &SculkBlocks, pos: Pos) -> bool {
    let state = w.get(pos);
    if s.behaviour(state) != Behaviour::Default {
        return true;
    }
    if !w.b.is_air(state) && !(s.is(state, s.water) && w.b.fluid(state) == Fluid::Water) {
        return false;
    }
    Dir::ALL.into_iter().any(|d| w.b.is_full_cube(w.get(pos.rel(d))))
}

// `SculkSpreader.createWorldGenSpreader()`.
const GROWTH_SPAWN_COST: i32 = 50;
const NO_GROWTH_RADIUS: i32 = 1;
const CHARGE_DECAY_RATE: i32 = 5;
const ADDITIONAL_DECAY_RATE: i32 = 10;
const MAX_CHARGE: i32 = 1000;
const MAX_CURSORS: usize = 32;
const MAX_CURSOR_DISTANCE: i32 = 1024;

/// The world generation `SculkSpreader`.
struct Spreader {
    cursors: Vec<Cursor>,
}

impl Spreader {
    /// `addCursors`: charges of at most 1000, at most 32 cursors.
    fn add_cursors(&mut self, pos: Pos, mut charge: i32) {
        while charge > 0 {
            let c = charge.min(MAX_CHARGE);
            if self.cursors.len() < MAX_CURSORS {
                self.cursors.push(Cursor { pos, charge: c, update_delay: 0, decay_delay: 1, facings: None });
            }
            charge -= c;
        }
    }

    /// `updateCursors`. World generation never merges cursors, so every cursor that keeps
    /// charge stays, in order.
    fn update_cursors(&mut self, w: &mut World, s: &SculkBlocks, origin: Pos, r: &mut WorldgenRandom, convert: bool) {
        let cursors = std::mem::take(&mut self.cursors);
        for mut c in cursors {
            let far = (c.pos.x - origin.x).abs().max((c.pos.y - origin.y).abs()).max((c.pos.z - origin.z).abs()) > MAX_CURSOR_DISTANCE;
            if far {
                continue;
            }
            c.update(w, s, origin, r, convert);
            if c.charge > 0 {
                self.cursors.push(c);
            }
        }
    }
}

/// `SculkSpreader.ChargeCursor`.
struct Cursor {
    pos: Pos,
    charge: i32,
    update_delay: i32,
    decay_delay: i32,
    /// `facings`: `None` for null, else the set of faces (bits by `Dir::index`).
    facings: Option<u8>,
}

impl Cursor {
    fn update(&mut self, w: &mut World, s: &SculkBlocks, origin: Pos, r: &mut WorldgenRandom, convert: bool) {
        if self.charge <= 0 {
            return;
        }
        if self.update_delay > 0 {
            self.update_delay -= 1;
            return;
        }
        let mut state = w.get(self.pos);
        let mut behaviour = s.behaviour(state);
        // Only sculk keeps its state when veins spread from it.
        if convert && attempt_spread_vein(w, s, behaviour, self.pos, state, self.facings) && behaviour != Behaviour::Sculk {
            state = w.get(self.pos);
            behaviour = s.behaviour(state);
        }
        self.charge = self.use_charge(w, s, behaviour, origin, r, convert);
        if self.charge <= 0 {
            on_discharged(w, s, behaviour, state, self.pos);
            return;
        }
        if let Some(next) = movement_pos(w, s, self.pos, r) {
            on_discharged(w, s, behaviour, state, self.pos);
            self.pos = next;
            // `closerThan(new Vec3i(origin.x, pos.y, origin.z), 15)`.
            let (dx, dz) = ((next.x - origin.x) as f64, (next.z - origin.z) as f64);
            if dx * dx + dz * dz >= 225.0 {
                self.charge = 0;
                return;
            }
            state = w.get(next);
        }
        if s.behaviour(state) != Behaviour::Default {
            // `MultifaceBlock.availableFaces`: empty for sculk.
            self.facings = Some(s.faces(state));
        }
        // `updateDecayDelay` and `getSculkSpreadDelay` of the behaviour before moving.
        self.decay_delay = if behaviour == Behaviour::Default { (self.decay_delay - 1).max(0) } else { 1 };
        self.update_delay = 1;
    }

    /// `attemptUseCharge` of the block's behaviour.
    fn use_charge(&self, w: &mut World, s: &SculkBlocks, behaviour: Behaviour, origin: Pos, r: &mut WorldgenRandom, convert: bool) -> i32 {
        let charge = self.charge;
        match behaviour {
            Behaviour::Vein => {
                if convert && attempt_place_sculk(w, s, self.pos, r) {
                    charge - 1
                } else if r.next_bounded(CHARGE_DECAY_RATE) == 0 {
                    (charge as f32 * 0.5).floor() as i32
                } else {
                    charge
                }
            }
            Behaviour::Sculk => {
                if charge == 0 || r.next_bounded(CHARGE_DECAY_RATE) != 0 {
                    return charge;
                }
                let p = self.pos;
                let near = p.dist_sq(origin) < (NO_GROWTH_RADIUS * NO_GROWTH_RADIUS) as f64;
                if !near && can_place_growth(w, s, p) {
                    if r.next_bounded(GROWTH_SPAWN_COST) < charge {
                        let at = p.up();
                        let growth = growth_state(w, s, at, r);
                        w.set(at, growth);
                    }
                    return (charge - GROWTH_SPAWN_COST).max(0);
                }
                if r.next_bounded(ADDITIONAL_DECAY_RATE) != 0 {
                    charge
                } else {
                    charge - if near { 1 } else { decay(p, origin, charge) }
                }
            }
            Behaviour::Default => {
                if self.decay_delay > 0 {
                    charge
                } else {
                    0
                }
            }
        }
    }
}

/// `SculkBlock.getDecay`.
fn decay(cursor: Pos, root: Pos, charge: i32) -> i32 {
    let distance = cursor.dist_sq(root).sqrt() as f32 - NO_GROWTH_RADIUS as f32;
    let f = distance * distance;
    let max = (24 - NO_GROWTH_RADIUS) * (24 - NO_GROWTH_RADIUS);
    let factor = (f / max as f32).min(1.0);
    ((charge as f32 * factor * 0.5) as i32).max(1)
}

/// `SculkBlock.canPlaceGrowth`: air or a water source above, at most two sensors or
/// shriekers around.
fn can_place_growth(w: &World, s: &SculkBlocks, pos: Pos) -> bool {
    let above = w.get(pos.up());
    if !(w.b.is_air(above) || (s.is(above, s.water) && w.b.fluid(above) == Fluid::Water)) {
        return false;
    }
    let mut n = 0;
    for z in -4..=4 {
        for y in 0..=2 {
            for x in -4..=4 {
                let st = w.get(pos.add(x, y, z));
                if s.is(st, s.sensor) || s.is(st, s.shrieker) {
                    n += 1;
                }
                if n > 2 {
                    return false;
                }
            }
        }
    }
    true
}

/// `SculkBlock.getRandomGrowthState`: a shrieker (that can summon wardens) one time in
/// eleven, a sensor otherwise, waterlogged in a fluid.
fn growth_state(w: &World, s: &SculkBlocks, pos: Pos, r: &mut WorldgenRandom) -> u32 {
    let state = if r.next_bounded(11) == 0 { blocks::with_property(s.shrieker_state, "can_summon", "true").unwrap_or(s.shrieker_state) } else { s.sensor_state };
    if w.b.fluid(w.get(pos)) != Fluid::Empty {
        blocks::with_property(state, "waterlogged", "true").unwrap_or(state)
    } else {
        state
    }
}

/// The 18 neighbours that are not corners, in `BlockPos.betweenClosed` order.
fn non_corner_neighbours() -> [(i32, i32, i32); 18] {
    let mut out = [(0, 0, 0); 18];
    let mut i = 0;
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                if (x == 0 || y == 0 || z == 0) && (x, y, z) != (0, 0, 0) {
                    out[i] = (x, y, z);
                    i += 1;
                }
            }
        }
    }
    out
}

/// `Util.shuffle`.
fn shuffle<T>(v: &mut [T], r: &mut WorldgenRandom) {
    for i in (2..=v.len()).rev() {
        let j = r.next_bounded(i as i32) as usize;
        v.swap(i - 1, j);
    }
}

/// `ChargeCursor.getValidMovementPos`: a sculk or vein neighbour the cursor can reach,
/// preferring the first with substrate access.
fn movement_pos(w: &World, s: &SculkBlocks, pos: Pos, r: &mut WorldgenRandom) -> Option<Pos> {
    let mut order = non_corner_neighbours();
    shuffle(&mut order, r);
    let mut found = pos;
    for (dx, dy, dz) in order {
        let n = pos.add(dx, dy, dz);
        let st = w.get(n);
        if s.behaviour(st) != Behaviour::Default && movement_unobstructed(w, pos, n) {
            found = n;
            if has_substrate_access(w, s, st, n) {
                break;
            }
        }
    }
    (found != pos).then_some(found)
}

/// `ChargeCursor.isMovementUnobstructed`.
fn movement_unobstructed(w: &World, from: Pos, to: Pos) -> bool {
    if from.manhattan(to) == 1 {
        return true;
    }
    let (dx, dy, dz) = (to.x - from.x, to.y - from.y, to.z - from.z);
    let x = if dx < 0 { Dir::West } else { Dir::East };
    let y = if dy < 0 { Dir::Down } else { Dir::Up };
    let z = if dz < 0 { Dir::North } else { Dir::South };
    let free = |d: Dir| {
        let p = from.rel(d);
        !w.b.sturdy(w.get(p), d.opposite())
    };
    if dx == 0 {
        free(y) || free(z)
    } else if dy == 0 {
        free(x) || free(z)
    } else {
        free(x) || free(y)
    }
}

/// `SculkVeinBlock.hasSubstrateAccess`.
fn has_substrate_access(w: &World, s: &SculkBlocks, state: u32, pos: Pos) -> bool {
    s.is_vein(state) && Dir::ALL.into_iter().any(|d| s.has_face(state, d) && s.replaceable.has(w.get(pos.rel(d))))
}

/// `SculkBehaviour.attemptSpreadVein` of the block's behaviour.
fn attempt_spread_vein(w: &mut World, s: &SculkBlocks, behaviour: Behaviour, pos: Pos, state: u32, facings: Option<u8>) -> bool {
    match (behaviour, facings) {
        (Behaviour::Sculk | Behaviour::Vein, _) => spread_all(w, s, state, pos, &SpreadType::ALL) > 0,
        (Behaviour::Default, None) => {
            let here = w.get(pos);
            spread_all(w, s, here, pos, &[SpreadType::SamePosition]) > 0
        }
        (Behaviour::Default, Some(0)) => spread_all(w, s, state, pos, &SpreadType::ALL) > 0,
        (Behaviour::Default, Some(faces)) => {
            if !w.b.is_air(state) && w.b.fluid(state) != Fluid::Water {
                return false;
            }
            regrow(w, s, pos, state, faces)
        }
    }
}

/// `SculkVeinBlock.regrow`: a vein with the given faces that can attach.
fn regrow(w: &mut World, s: &SculkBlocks, pos: Pos, state: u32, faces: u8) -> bool {
    let mut vein = s.vein_state;
    let mut any = false;
    for d in Dir::ALL {
        if faces & (1 << d.index()) != 0 && can_attach_to(w, w.get(pos.rel(d)), d) {
            vein = s.with_face(vein, d, true);
            any = true;
        }
    }
    if !any {
        return false;
    }
    if w.b.fluid(state) != Fluid::Empty {
        vein = blocks::with_property(vein, "waterlogged", "true").unwrap_or(vein);
    }
    w.set(pos, vein);
    true
}

/// `SculkVeinBlock.attemptPlaceSculk`: turns a supporting block into sculk and spreads
/// veins around it.
fn attempt_place_sculk(w: &mut World, s: &SculkBlocks, pos: Pos, r: &mut WorldgenRandom) -> bool {
    let state = w.get(pos);
    let mut dirs = Dir::ALL;
    shuffle(&mut dirs, r);
    for d in dirs {
        if !s.has_face(state, d) {
            continue;
        }
        let support = pos.rel(d);
        if !s.replaceable_world_gen.has(w.get(support)) {
            continue;
        }
        w.set(support, s.sculk_state);
        spread_all(w, s, s.sculk_state, support, &SpreadType::ALL);
        let back = d.opposite();
        for d1 in Dir::ALL {
            if d1 == back {
                continue;
            }
            let n = support.rel(d1);
            let ns = w.get(n);
            if s.is_vein(ns) {
                on_discharged(w, s, Behaviour::Vein, ns, n);
            }
        }
        return true;
    }
    false
}

/// `SculkBehaviour.onDischarged`: only veins react, dropping the faces on sculk (from the
/// state the caller read).
fn on_discharged(w: &mut World, s: &SculkBlocks, behaviour: Behaviour, state: u32, pos: Pos) {
    if behaviour != Behaviour::Vein || !s.is_vein(state) {
        return;
    }
    let mut st = state;
    for d in Dir::ALL {
        if s.has_face(st, d) && s.is(w.get(pos.rel(d)), s.sculk) {
            st = s.with_face(st, d, false);
        }
    }
    if s.faces(st) == 0 {
        st = if w.fluid(pos) == Fluid::Empty { w.b.known.air } else { s.water_state };
    }
    w.set(pos, st);
}

/// `MultifaceSpreader.SpreadType`.
#[derive(Clone, Copy)]
enum SpreadType {
    SamePosition,
    SamePlane,
    WrapAround,
}

impl SpreadType {
    /// `DEFAULT_SPREAD_ORDER`.
    const ALL: [SpreadType; 3] = [SpreadType::SamePosition, SpreadType::SamePlane, SpreadType::WrapAround];

    /// `getSpreadPos(pos, face, spreadDirection)`: the position and the face to place.
    fn spread_pos(self, pos: Pos, face: Dir, from: Dir) -> (Pos, Dir) {
        match self {
            SpreadType::SamePosition => (pos, face),
            SpreadType::SamePlane => (pos.rel(face), from),
            SpreadType::WrapAround => (pos.rel(face).rel(from), face.opposite()),
        }
    }
}

/// `MultifaceSpreader.spreadAll` for sculk veins: the number of faces placed.
fn spread_all(w: &mut World, s: &SculkBlocks, state: u32, pos: Pos, types: &[SpreadType]) -> u32 {
    // `isOtherBlockValidAsSource`: anything but a vein spreads from all its sides.
    let other = !s.is_vein(state);
    let mut placed = 0;
    for from in Dir::ALL {
        if !(other || s.has_face(state, from)) {
            continue;
        }
        for face in Dir::ALL {
            if face.axis() == from.axis() || !(other || (s.has_face(state, from) && !s.has_face(state, face))) {
                continue;
            }
            for t in types {
                let (target, target_face) = t.spread_pos(pos, face, from);
                let existing = w.get(target);
                if can_be_replaced(w, s, pos, target, target_face, existing) && valid_for_placement(w, s, existing, target, target_face) {
                    // `spreadToFace`: the first spread position that may be spread into.
                    if let Some(new) = state_for_placement(w, s, existing, target, target_face) {
                        if w.set(target, new) {
                            placed += 1;
                        }
                    }
                    break;
                }
            }
        }
    }
    placed
}

/// `SculkVeinSpreaderConfig.stateCanBeReplaced`.
fn can_be_replaced(w: &World, s: &SculkBlocks, pos: Pos, spread: Pos, face: Dir, state: u32) -> bool {
    let behind = w.get(spread.rel(face));
    if s.is(behind, s.sculk) || s.is(behind, s.catalyst) || s.is(behind, s.moving_piston) {
        return false;
    }
    if pos.manhattan(spread) == 2 {
        let p = pos.rel(face.opposite());
        if w.b.sturdy(w.get(p), face) {
            return false;
        }
    }
    let fluid = w.b.fluid(state);
    if fluid != Fluid::Empty && fluid != Fluid::Water {
        return false;
    }
    if s.fire.has(state) {
        return false;
    }
    w.b.is_replaceable(state) || w.b.is_air(state) || s.is_vein(state) || (s.is(state, s.water) && fluid == Fluid::Water)
}

/// `MultifaceBlock.canAttachTo`: the facing side of the support is full in its support
/// shape or in its collision shape.
fn can_attach_to(w: &World, support: u32, face: Dir) -> bool {
    let side = face.opposite();
    w.b.sturdy(support, side) || paper_data::physics::collision_face_full(support, side.index())
}

/// `MultifaceBlock.isValidStateForPlacement`.
fn valid_for_placement(w: &World, s: &SculkBlocks, state: u32, pos: Pos, face: Dir) -> bool {
    if s.has_face(state, face) {
        return false;
    }
    can_attach_to(w, w.get(pos.rel(face)), face)
}

/// `MultifaceBlock.getStateForPlacement`.
fn state_for_placement(w: &World, s: &SculkBlocks, state: u32, pos: Pos, face: Dir) -> Option<u32> {
    if !valid_for_placement(w, s, state, pos, face) {
        return None;
    }
    let base = if s.is_vein(state) {
        state
    } else if w.b.fluid(state) == Fluid::Water {
        blocks::with_property(s.vein_state, "waterlogged", "true").unwrap_or(s.vein_state)
    } else {
        s.vein_state
    };
    Some(s.with_face(base, face, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbours_in_between_closed_order() {
        let n = non_corner_neighbours();
        assert_eq!(n[0], (0, -1, -1));
        assert_eq!(n[1], (-1, 0, -1));
        assert_eq!(n[4], (0, 1, -1));
        assert_eq!(n[17], (0, 1, 1));
        assert!(n.iter().all(|&(x, y, z)| [x, y, z].contains(&0) && (x, y, z) != (0, 0, 0)));
    }

    #[test]
    fn decay_grows_with_distance() {
        let o = Pos::new(0, 0, 0);
        assert_eq!(decay(Pos::new(2, 0, 0), o, 100), 1);
        assert_eq!(decay(Pos::new(12, 0, 0), o, 100), 11);
        assert_eq!(decay(Pos::new(30, 0, 0), o, 100), 50);
    }
}
