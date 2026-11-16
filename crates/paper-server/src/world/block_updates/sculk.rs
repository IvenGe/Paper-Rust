//! What sculk blocks do on their own: a sensor's phases after it hears a vibration
//! (`SculkSensorBlock.activate`, `tick`, `deactivate`), a shrieker done shrieking
//! (`SculkShriekerBlock.tick`) and a catalyst done blooming. Hearing vibrations, shrieking
//! and spreading sculk are the server's (`server::sculk`).

use super::state::*;
use super::{flags, sound_source, Effect, Level};
use paper_protocol::codec::BlockPos;

pub const SENSOR: &str = "minecraft:sculk_sensor";
pub const CALIBRATED: &str = "minecraft:calibrated_sculk_sensor";
pub const SHRIEKER: &str = "minecraft:sculk_shrieker";
pub const CATALYST: &str = "minecraft:sculk_catalyst";

/// How long a sensor stays active, and then cools down.
pub const ACTIVE_TICKS: i32 = 30;
pub const CALIBRATED_ACTIVE_TICKS: i32 = 10;
pub const COOLDOWN_TICKS: i32 = 10;

/// A sculk block with ticks of its own (a sensor, shrieker or catalyst).
pub fn is_sculk_block(state: u32) -> bool {
    matches!(block_name(state), SENSOR | CALIBRATED | SHRIEKER | CATALYST)
}

pub fn is_sensor(state: u32) -> bool {
    matches!(block_name(state), SENSOR | CALIBRATED)
}

/// `SculkSensorBlock.canActivate`: the sensor is idle.
pub fn can_activate(state: u32) -> bool {
    is_sensor(state) && prop(state, "sculk_sensor_phase") == Some("inactive")
}

fn set_prop(state: u32, name: &str, value: &str) -> u32 {
    paper_data::blocks::with_property(state, name, value).unwrap_or(state)
}

/// The block below hears a sensor too: it is powered through it.
fn update_neighbours(level: &mut Level, pos: BlockPos, state: u32) {
    let block = block_id(state);
    level.update_neighbors_at(pos, block);
    level.update_neighbors_at(pos.below(), block);
}

fn click_pitch(level: &mut Level) -> f32 {
    level.random_float() * 0.2 + 0.8
}

/// `SculkSensorBlock.activate` (the block's part): active with `power` for a while, clicking.
pub fn activate(level: &mut Level, state: u32, pos: BlockPos, power: i32) {
    let active = set_prop(set_prop(state, "sculk_sensor_phase", "active"), "power", &power.clamp(0, 15).to_string());
    level.set_block(pos, active, flags::ALL);
    let ticks = if block_name(state) == CALIBRATED { CALIBRATED_ACTIVE_TICKS } else { ACTIVE_TICKS };
    level.schedule_tick(pos, block_id(state), ticks);
    update_neighbours(level, pos, active);
    if !get_bool(state, "waterlogged") {
        let pitch = click_pitch(level);
        level.play_sound(None, pos, "minecraft:block.sculk_sensor.clicking", sound_source::BLOCKS, 1.0, pitch);
    }
}

/// A sculk block's scheduled tick.
pub fn tick(level: &mut Level, state: u32, pos: BlockPos) {
    match block_name(state) {
        SENSOR | CALIBRATED => match prop(state, "sculk_sensor_phase") {
            // `deactivate`: unpowered, cooling down.
            Some("active") => {
                let cooling = set_prop(set_prop(state, "sculk_sensor_phase", "cooldown"), "power", "0");
                level.set_block(pos, cooling, flags::ALL);
                level.schedule_tick(pos, block_id(state), COOLDOWN_TICKS);
                update_neighbours(level, pos, cooling);
            }
            Some("cooldown") => {
                level.set_block(pos, set_prop(state, "sculk_sensor_phase", "inactive"), flags::ALL);
                if !get_bool(state, "waterlogged") {
                    let pitch = click_pitch(level);
                    level.play_sound(None, pos, "minecraft:block.sculk_sensor.clicking_stop", sound_source::BLOCKS, 1.0, pitch);
                }
            }
            _ => {}
        },
        // Done shrieking: the warden may answer (the server's part).
        SHRIEKER if get_bool(state, "shrieking") => {
            level.set_block(pos, set_prop(state, "shrieking", "false"), flags::ALL);
            level.world.updates.effects.push(Effect::ShriekerDone { pos });
        }
        CATALYST if get_bool(state, "bloom") => {
            level.set_block(pos, set_prop(state, "bloom", "false"), flags::ALL);
        }
        _ => {}
    }
}

// --- spreading sculk (`SculkSpreader`, `SculkBehaviour`, the vein's `MultifaceSpreader`) ---

use super::Dir;
use paper_data::physics;
use paper_nbt::{Compound, Tag};
use std::collections::HashMap;

/// A catalyst's spreading (`SculkSpreader.createLevelSpreader`): what growths cost, the radius
/// around the catalyst without growths, and how fast charge decays.
const GROWTH_SPAWN_COST: i32 = 10;
const NO_GROWTH_RADIUS: i32 = 4;
const CHARGE_DECAY_RATE: i32 = 10;
const ADDITIONAL_DECAY_RATE: i32 = 5;
const MAX_CHARGE: i32 = 1000;
const MAX_CURSORS: usize = 32;

/// Charge moving through sculk from where a mob died (`SculkSpreader.ChargeCursor`).
#[derive(Clone, Debug, PartialEq)]
pub struct Cursor {
    pub pos: BlockPos,
    pub charge: i32,
    pub decay_delay: i32,
    pub update_delay: i32,
    /// The faces of the vein it is on; `None` before it has been on sculk.
    pub facings: Option<Vec<Dir>>,
}

impl Cursor {
    pub fn new(pos: BlockPos, charge: i32) -> Cursor {
        Cursor { pos, charge, decay_delay: 1, update_delay: 0, facings: None }
    }

    /// `ChargeCursor.CODEC`.
    pub fn from_nbt(c: &Compound) -> Option<Cursor> {
        let pos = match c.get("pos") {
            Some(Tag::IntArray(v)) if v.len() == 3 => BlockPos::new(v[0], v[1], v[2]),
            _ => return None,
        };
        let facings = c.list("facings").map(|l| l.iter().filter_map(|t| t.as_str()).filter_map(dir_named).collect());
        Some(Cursor {
            pos,
            charge: c.i32("charge").unwrap_or(0).clamp(0, MAX_CHARGE),
            decay_delay: c.i32("decay_delay").unwrap_or(1).clamp(0, 1),
            update_delay: c.i32("update_delay").unwrap_or(0).max(0),
            facings,
        })
    }

    pub fn to_nbt(&self) -> Compound {
        let mut c = Compound::new()
            .with("pos", Tag::IntArray(vec![self.pos.x, self.pos.y, self.pos.z]))
            .with("charge", Tag::Int(self.charge))
            .with("decay_delay", Tag::Int(self.decay_delay))
            .with("update_delay", Tag::Int(self.update_delay));
        if let Some(f) = &self.facings {
            c.insert("facings", Tag::List(f.iter().map(|d| Tag::String(DIR_NAMES[d.index()].into())).collect()));
        }
        c
    }
}

const DIR_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

fn dir_named(name: &str) -> Option<Dir> {
    DIR_NAMES.iter().position(|n| *n == name).map(|i| Dir::ALL[i])
}

/// `SculkSpreader.addCursors`: the charge in cursors of at most 1000, at most 32 of them.
pub fn add_cursors(cursors: &mut Vec<Cursor>, pos: BlockPos, mut charge: i32) {
    while charge > 0 {
        let c = charge.min(MAX_CHARGE);
        if cursors.len() < MAX_CURSORS {
            cursors.push(Cursor::new(pos, c));
        }
        charge -= c;
    }
}

fn random_int(level: &mut Level, bound: i32) -> i32 {
    (level.random() % bound.max(1) as u64) as i32
}

fn in_block_tag(state: u32, tag: &str) -> bool {
    paper_data::synced::tag("minecraft:block", tag).is_some_and(|ids| ids.contains(&(block_id(state) as i32)))
}

fn is_vein(state: u32) -> bool {
    block_name(state) == "minecraft:sculk_vein"
}

fn has_face(state: u32, dir: Dir) -> bool {
    is_vein(state) && get_bool(state, DIR_NAMES[dir.index()])
}

fn faces(state: u32) -> Vec<Dir> {
    Dir::ALL.into_iter().filter(|d| has_face(state, *d)).collect()
}

/// Water as a fluid source here: a water source block or a waterlogged block.
fn water_source(state: u32) -> bool {
    get_bool(state, "waterlogged") || block_name(state) == "minecraft:water" && get_int(state, "level") == 0
}

/// Some fluid here (water or lava, still or flowing, or a waterlogged block).
fn any_fluid(state: u32) -> bool {
    get_bool(state, "waterlogged") || matches!(block_name(state), "minecraft:water" | "minecraft:lava" | "minecraft:bubble_column")
}

fn lava(state: u32) -> bool {
    block_name(state) == "minecraft:lava"
}

/// `MultifaceBlock.canAttachTo`: the neighbour's face towards the vein is full.
fn can_attach_to(state: u32, towards: Dir) -> bool {
    let face = towards.opposite().index();
    physics::is_face_sturdy(state, face) || physics::collision_face_full(state, face)
}

/// `MultifaceBlock.isValidStateForPlacement` for the vein.
fn valid_for_placement(level: &Level, state: u32, pos: BlockPos, dir: Dir) -> bool {
    if is_vein(state) && has_face(state, dir) {
        return false;
    }
    can_attach_to(level.state(pos.rel(dir)), dir)
}

/// `SculkVeinSpreaderConfig.stateCanBeReplaced`.
fn can_be_replaced(level: &Level, source: BlockPos, pos: BlockPos, dir: Dir, state: u32) -> bool {
    let behind = level.state(pos.rel(dir));
    if matches!(block_name(behind), "minecraft:sculk" | CATALYST | "minecraft:moving_piston") {
        return false;
    }
    if (source.x - pos.x).abs() + (source.y - pos.y).abs() + (source.z - pos.z).abs() == 2 {
        let p = source.rel(dir.opposite());
        if physics::is_face_sturdy(level.state(p), dir.index()) {
            return false;
        }
    }
    if any_fluid(state) && !(get_bool(state, "waterlogged") || block_name(state) == "minecraft:water") || lava(state) {
        return false;
    }
    if in_block_tag(state, "minecraft:fire") {
        return false;
    }
    paper_data::blocks::is_replaceable(state) || paper_data::blocks::is_air(state) || is_vein(state) || block_name(state) == "minecraft:water" && get_int(state, "level") == 0
}

/// `MultifaceBlock.getStateForPlacement` for the vein: the face added to what is there.
fn placement_state(level: &Level, old: u32, pos: BlockPos, dir: Dir) -> Option<u32> {
    if !valid_for_placement(level, old, pos, dir) {
        return None;
    }
    let vein = paper_data::blocks::default_state("minecraft:sculk_vein")?;
    let base = if is_vein(old) {
        old
    } else if water_source(old) {
        set_prop(vein, "waterlogged", "true")
    } else {
        vein
    };
    Some(set_prop(base, DIR_NAMES[dir.index()], "true"))
}

#[derive(Clone, Copy)]
enum SpreadType {
    SamePosition,
    SamePlane,
    WrapAround,
}

const DEFAULT_SPREAD_ORDER: [SpreadType; 3] = [SpreadType::SamePosition, SpreadType::SamePlane, SpreadType::WrapAround];

/// `MultifaceSpreader.spreadAll` with the vein's settings: from each face of `state` (any
/// face, for a block that is not a vein) towards each side; returns how many faces were added.
fn spread_all(level: &mut Level, state: u32, pos: BlockPos, types: &[SpreadType]) -> u32 {
    let mut count = 0;
    for from in Dir::ALL {
        if is_vein(state) && !has_face(state, from) {
            continue;
        }
        for to in Dir::ALL {
            if to.axis() == from.axis() {
                continue;
            }
            // `getSpreadFromFaceTowardDirection`: another block always spreads; a vein from a
            // face it has towards one it has not.
            if is_vein(state) && has_face(state, to) {
                continue;
            }
            for t in types {
                let (at, face) = match t {
                    SpreadType::SamePosition => (pos, to),
                    SpreadType::SamePlane => (pos.rel(to), from),
                    SpreadType::WrapAround => (pos.rel(to).rel(from), to.opposite()),
                };
                let there = level.state(at);
                if !(can_be_replaced(level, pos, at, face, there) && valid_for_placement(level, there, at, face)) {
                    continue;
                }
                if let Some(new) = placement_state(level, there, at, face) {
                    if new != there && level.set_block(at, new, flags::CLIENTS) {
                        count += 1;
                    }
                }
                break;
            }
        }
    }
    count
}

/// `SculkVeinBlock.regrow`: a vein on the faces of `faces` that can hold one.
fn regrow(level: &mut Level, pos: BlockPos, state: u32, faces: &[Dir]) -> bool {
    let Some(mut vein) = paper_data::blocks::default_state("minecraft:sculk_vein") else { return false };
    let mut any = false;
    for d in faces {
        if can_attach_to(level.state(pos.rel(*d)), *d) {
            vein = set_prop(vein, DIR_NAMES[d.index()], "true");
            any = true;
        }
    }
    if !any {
        return false;
    }
    if any_fluid(state) {
        vein = set_prop(vein, "waterlogged", "true");
    }
    level.set_block(pos, vein, flags::ALL);
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// Any other block (`SculkBehaviour.DEFAULT`).
    Default,
    Sculk,
    Vein,
}

fn behaviour(state: u32) -> Behaviour {
    match block_name(state) {
        "minecraft:sculk" => Behaviour::Sculk,
        "minecraft:sculk_vein" => Behaviour::Vein,
        _ => Behaviour::Default,
    }
}

/// `attemptSpreadVein`.
fn attempt_spread_vein(level: &mut Level, b: Behaviour, pos: BlockPos, state: u32, facings: Option<&[Dir]>) -> bool {
    match (b, facings) {
        (Behaviour::Default, None) => {
            let here = level.state(pos);
            spread_all(level, here, pos, &[SpreadType::SamePosition]) > 0
        }
        (Behaviour::Default, Some(f)) if !f.is_empty() => {
            if !paper_data::blocks::is_air(state) && !(block_name(state) == "minecraft:water" || get_bool(state, "waterlogged")) {
                return false;
            }
            regrow(level, pos, state, f)
        }
        _ => spread_all(level, state, pos, &DEFAULT_SPREAD_ORDER) > 0,
    }
}

/// `SculkBlock.canPlaceGrowth`: open above, and at most two sensors and shriekers around.
fn can_place_growth(level: &Level, pos: BlockPos) -> bool {
    let above = level.state(pos.above());
    if !(paper_data::blocks::is_air(above) || block_name(above) == "minecraft:water" && get_int(above, "level") == 0) {
        return false;
    }
    let mut count = 0;
    for y in 0..=2 {
        for z in -4..=4 {
            for x in -4..=4 {
                if in_block_tag(level.state(pos.offset(x, y, z)), "minecraft:sculk_growth_inhibitors") {
                    count += 1;
                    if count > 2 {
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let (dx, dy, dz) = ((a.x - b.x) as f64, (a.y - b.y) as f64, (a.z - b.z) as f64);
    dx * dx + dy * dy + dz * dz
}

/// `SculkBlock.getDecayPenalty`: charge lost far from the catalyst.
fn decay_penalty(pos: BlockPos, origin: BlockPos, charge: i32) -> i32 {
    let d = dist_sqr(pos, origin).sqrt() as f32 - NO_GROWTH_RADIUS as f32;
    let f = (d * d / ((24 - NO_GROWTH_RADIUS) * (24 - NO_GROWTH_RADIUS)) as f32).min(1.0);
    ((charge as f32 * f * 0.5) as i32).max(1)
}

/// `attemptUseCharge`: what is left of a cursor's charge after this block.
fn attempt_use_charge(level: &mut Level, b: Behaviour, cursor: &Cursor, origin: BlockPos) -> i32 {
    let charge = cursor.charge;
    match b {
        Behaviour::Default => {
            if cursor.decay_delay > 0 {
                charge
            } else {
                0
            }
        }
        Behaviour::Vein => {
            if place_sculk(level, cursor.pos) {
                charge - 1
            } else if random_int(level, CHARGE_DECAY_RATE) == 0 {
                (charge as f32 * 0.5).floor() as i32
            } else {
                charge
            }
        }
        Behaviour::Sculk => {
            if charge == 0 || random_int(level, CHARGE_DECAY_RATE) != 0 {
                return charge;
            }
            let pos = cursor.pos;
            let near = dist_sqr(pos, origin) < (NO_GROWTH_RADIUS * NO_GROWTH_RADIUS) as f64;
            if !near && can_place_growth(level, pos) {
                if random_int(level, GROWTH_SPAWN_COST) < charge {
                    // A shrieker now and then (one that cannot call the warden), else a sensor.
                    let growth = if random_int(level, 11) == 0 {
                        paper_data::blocks::default_state(SHRIEKER).map(|s| set_prop(s, "can_summon", "false"))
                    } else {
                        paper_data::blocks::default_state(SENSOR)
                    };
                    if let Some(mut growth) = growth {
                        let above = pos.above();
                        if any_fluid(level.state(above)) {
                            growth = set_prop(growth, "waterlogged", "true");
                        }
                        level.set_block(above, growth, flags::ALL);
                        let sound = if block_name(growth) == SHRIEKER { "minecraft:block.sculk_shrieker.place" } else { "minecraft:block.sculk_sensor.place" };
                        level.play_sound(None, pos, sound, sound_source::BLOCKS, 1.0, 1.0);
                    }
                }
                (charge - GROWTH_SPAWN_COST).max(0)
            } else if random_int(level, ADDITIONAL_DECAY_RATE) != 0 {
                charge
            } else {
                charge - if near { 1 } else { decay_penalty(pos, origin, charge) }
            }
        }
    }
}

/// `SculkVeinBlock.attemptPlaceSculk`: a block the vein lies on turns to sculk.
fn place_sculk(level: &mut Level, pos: BlockPos) -> bool {
    let state = level.state(pos);
    let mut dirs = Dir::ALL;
    shuffle(level, &mut dirs);
    for dir in dirs {
        if !has_face(state, dir) {
            continue;
        }
        let rel = pos.rel(dir);
        if !in_block_tag(level.state(rel), "minecraft:sculk_replaceable") {
            continue;
        }
        let Some(sculk) = paper_data::blocks::default_state("minecraft:sculk") else { return false };
        level.set_block(rel, sculk, flags::ALL);
        level.play_sound(None, rel, "minecraft:block.sculk.spread", sound_source::BLOCKS, 1.0, 1.0);
        spread_all(level, sculk, rel, &DEFAULT_SPREAD_ORDER);
        for d in Dir::ALL {
            if d == dir.opposite() {
                continue;
            }
            let p = rel.rel(d);
            let s = level.state(p);
            if is_vein(s) {
                discharge_vein(level, s, p);
            }
        }
        return true;
    }
    false
}

/// `SculkVeinBlock.onDischarged`: faces on sculk go; a vein without faces goes.
fn discharge_vein(level: &mut Level, state: u32, pos: BlockPos) {
    if !is_vein(state) {
        return;
    }
    let mut s = state;
    for d in Dir::ALL {
        if has_face(s, d) && block_name(level.state(pos.rel(d))) == "minecraft:sculk" {
            s = set_prop(s, DIR_NAMES[d.index()], "false");
        }
    }
    if faces(s).is_empty() {
        s = if get_bool(state, "waterlogged") { paper_data::blocks::default_state("minecraft:water").unwrap_or(paper_data::blocks::AIR) } else { paper_data::blocks::AIR };
    }
    level.set_block(pos, s, flags::ALL);
}

fn shuffle<T>(level: &mut Level, items: &mut [T]) {
    for i in (1..items.len()).rev() {
        let j = random_int(level, i as i32 + 1) as usize;
        items.swap(i, j);
    }
}

/// `isMovementUnobstructed`: a diagonal step needs one of its two corners open.
fn movement_unobstructed(level: &Level, from: BlockPos, to: BlockPos) -> bool {
    let (dx, dy, dz) = (to.x - from.x, to.y - from.y, to.z - from.z);
    if dx.abs() + dy.abs() + dz.abs() == 1 {
        return true;
    }
    let open = |d: Dir| !physics::is_face_sturdy(level.state(from.rel(d)), d.opposite().index());
    let x = if dx < 0 { Dir::West } else { Dir::East };
    let y = if dy < 0 { Dir::Down } else { Dir::Up };
    let z = if dz < 0 { Dir::North } else { Dir::South };
    if dx == 0 {
        open(y) || open(z)
    } else if dy == 0 {
        open(x) || open(z)
    } else {
        open(x) || open(y)
    }
}

/// `getValidMovementPos`: a neighbouring sculk block or vein (not a corner), one with a block
/// to turn into sculk first.
fn movement_pos(level: &mut Level, pos: BlockPos) -> Option<BlockPos> {
    let mut offsets: Vec<(i32, i32, i32)> = Vec::with_capacity(18);
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                if (x == 0 || y == 0 || z == 0) && (x, y, z) != (0, 0, 0) {
                    offsets.push((x, y, z));
                }
            }
        }
    }
    shuffle(level, &mut offsets);
    let mut chosen = pos;
    for (x, y, z) in offsets {
        let n = pos.offset(x, y, z);
        let state = level.state(n);
        if behaviour(state) != Behaviour::Default && movement_unobstructed(level, pos, n) {
            chosen = n;
            let substrate = is_vein(state) && Dir::ALL.into_iter().any(|d| has_face(state, d) && in_block_tag(level.state(n.rel(d)), "minecraft:sculk_replaceable"));
            if substrate {
                break;
            }
        }
    }
    (chosen != pos).then_some(chosen)
}

/// `ChargeCursor.update`.
fn update_cursor(level: &mut Level, origin: BlockPos, c: &mut Cursor) {
    if c.charge <= 0 || !level.is_loaded(c.pos) {
        return;
    }
    if c.update_delay > 0 {
        c.update_delay -= 1;
        return;
    }
    let mut state = level.state(c.pos);
    let mut b = behaviour(state);
    if attempt_spread_vein(level, b, c.pos, state, c.facings.as_deref()) {
        if b != Behaviour::Sculk {
            state = level.state(c.pos);
            b = behaviour(state);
        }
        level.play_sound(None, c.pos, "minecraft:block.sculk.spread", sound_source::BLOCKS, 1.0, 1.0);
    }
    c.charge = attempt_use_charge(level, b, c, origin);
    if c.charge <= 0 {
        if b == Behaviour::Vein {
            discharge_vein(level, state, c.pos);
        }
        return;
    }
    if let Some(next) = movement_pos(level, c.pos) {
        if b == Behaviour::Vein {
            discharge_vein(level, state, c.pos);
        }
        c.pos = next;
        state = level.state(next);
    }
    if behaviour(state) != Behaviour::Default {
        c.facings = Some(faces(state));
    }
    c.decay_delay = if b == Behaviour::Default { (c.decay_delay - 1).max(0) } else { 1 };
    c.update_delay = 1;
}

/// `SculkSpreader.updateCursors`: every cursor moves on; those in one place merge, and the
/// charge shows on the blocks it is in (level event 3006).
pub fn update_cursors(level: &mut Level, origin: BlockPos, cursors: &mut Vec<Cursor>) {
    if cursors.is_empty() {
        return;
    }
    let mut next: Vec<Cursor> = Vec::new();
    let mut merged: HashMap<BlockPos, usize> = HashMap::new();
    let mut charges: Vec<(BlockPos, i32)> = Vec::new();
    for mut cursor in std::mem::take(cursors) {
        let far = (cursor.pos.x - origin.x).abs().max((cursor.pos.y - origin.y).abs()).max((cursor.pos.z - origin.z).abs()) > 1024;
        if far {
            continue;
        }
        update_cursor(level, origin, &mut cursor);
        if cursor.charge <= 0 {
            level.level_event(3006, cursor.pos, 0);
            continue;
        }
        let pos = cursor.pos;
        match charges.iter_mut().find(|(p, _)| *p == pos) {
            Some((_, c)) => *c += cursor.charge,
            None => charges.push((pos, cursor.charge)),
        }
        match merged.get(&pos).copied() {
            None => {
                merged.insert(pos, next.len());
                next.push(cursor);
            }
            Some(i) if cursor.charge + next[i].charge <= MAX_CHARGE => {
                next[i].charge += cursor.charge;
                next[i].update_delay = next[i].update_delay.min(cursor.update_delay);
            }
            Some(i) => {
                let smaller = cursor.charge < next[i].charge;
                next.push(cursor);
                if smaller {
                    merged.insert(pos, next.len() - 1);
                }
            }
        }
    }
    for (pos, charge) in charges {
        let Some(facings) = merged.get(&pos).and_then(|i| next[*i].facings.as_ref()) else { continue };
        if charge > 0 {
            let c = ((charge as f64).ln_1p() / 2.3) as i32 + 1;
            let packed: i32 = facings.iter().map(|d| 1 << d.index()).fold(0, |a, b| a | b);
            level.level_event(3006, pos, (c << 6) + packed);
        }
    }
    *cursors = next;
}
