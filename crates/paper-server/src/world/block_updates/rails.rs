//! Rails (`BaseRailBlock`, `RailState`): a rail bends and slopes to join the rails next to
//! it when placed, and a plain rail at a junction switches when a redstone signal next to it
//! changes. Rails need a block with a rigid top below them (and on the high side of a slope).
//! Powered and activator rails pass power on along a line of up to eight rails of their kind
//! (`PoweredRailBlock`); detector rails are pressed by minecarts (the server finds them, see
//! [`check_pressed`]).

use super::state::{block_id, block_name, default_of, get_bool, prop, set_bool, with};
use super::{flags, Dir, Kind, Level, PosExt};
use paper_data::physics;
use paper_protocol::codec::BlockPos;

/// `RailShape`: the `shape` property of rails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    NorthSouth,
    EastWest,
    AscendingEast,
    AscendingWest,
    AscendingNorth,
    AscendingSouth,
    SouthEast,
    SouthWest,
    NorthWest,
    NorthEast,
}

use Shape::*;

impl Shape {
    pub fn parse(name: &str) -> Option<Shape> {
        Some(match name {
            "north_south" => NorthSouth,
            "east_west" => EastWest,
            "ascending_east" => AscendingEast,
            "ascending_west" => AscendingWest,
            "ascending_north" => AscendingNorth,
            "ascending_south" => AscendingSouth,
            "south_east" => SouthEast,
            "south_west" => SouthWest,
            "north_west" => NorthWest,
            "north_east" => NorthEast,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            NorthSouth => "north_south",
            EastWest => "east_west",
            AscendingEast => "ascending_east",
            AscendingWest => "ascending_west",
            AscendingNorth => "ascending_north",
            AscendingSouth => "ascending_south",
            SouthEast => "south_east",
            SouthWest => "south_west",
            NorthWest => "north_west",
            NorthEast => "north_east",
        }
    }

    pub fn is_slope(self) -> bool {
        matches!(self, AscendingEast | AscendingWest | AscendingNorth | AscendingSouth)
    }

    /// `AbstractMinecart.exits`: the two ends of the rail, as offsets from its block (the low
    /// end of a slope one block down).
    pub fn exits(self) -> ((i32, i32, i32), (i32, i32, i32)) {
        let (w, e, n, s) = ((-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1));
        let below = |(x, y, z): (i32, i32, i32)| (x, y - 1, z);
        match self {
            NorthSouth => (n, s),
            EastWest => (w, e),
            AscendingEast => (below(w), e),
            AscendingWest => (w, below(e)),
            AscendingNorth => (n, below(s)),
            AscendingSouth => (below(n), s),
            SouthEast => (s, e),
            SouthWest => (s, w),
            NorthWest => (n, w),
            NorthEast => (n, e),
        }
    }
}

/// The shape of a rail state.
pub fn shape(state: u32) -> Shape {
    prop(state, "shape").and_then(Shape::parse).unwrap_or(NorthSouth)
}

/// `BaseRailBlock.isRail`.
pub fn is_rail(state: u32) -> bool {
    matches!(super::kind(state), Kind::Rail | Kind::PoweredRail | Kind::DetectorRail)
}

/// Powered, detector and activator rails only go straight.
fn is_straight(state: u32) -> bool {
    super::kind(state) != Kind::Rail
}

/// `canSupportRigidBlock`: a top face covering at least its outer ring, as a full top does
/// and the rims of hoppers, cauldrons and composters do.
pub fn rigid_top(state: u32) -> bool {
    physics::is_face_sturdy(state, Dir::Up.index())
        || matches!(block_name(state), "minecraft:hopper" | "minecraft:cauldron" | "minecraft:water_cauldron" | "minecraft:lava_cauldron" | "minecraft:powder_snow_cauldron" | "minecraft:composter")
}

/// `BaseRailBlock.canSurvive`.
pub fn can_survive(level: &Level, pos: BlockPos) -> bool {
    rigid_top(level.state(pos.below()))
}

/// `shouldBeRemoved`: nothing rigid below, or nothing on the high side of a slope.
fn should_be_removed(level: &Level, pos: BlockPos, shape: Shape) -> bool {
    let high_side = match shape {
        AscendingEast => Some(Dir::East),
        AscendingWest => Some(Dir::West),
        AscendingNorth => Some(Dir::North),
        AscendingSouth => Some(Dir::South),
        _ => None,
    };
    !can_survive(level, pos) || high_side.is_some_and(|d| !rigid_top(level.state(pos.rel(d))))
}

/// `RailState`: a rail and the positions of the rails it joins.
struct RailState {
    pos: BlockPos,
    state: u32,
    straight: bool,
    connections: Vec<BlockPos>,
}

/// The rail at `pos`, or just above or below it.
fn rail_at(level: &Level, pos: BlockPos) -> Option<RailState> {
    [pos, pos.above(), pos.below()].into_iter().find_map(|p| {
        let state = level.state(p);
        is_rail(state).then(|| RailState::new(p, state))
    })
}

fn has_rail(level: &Level, pos: BlockPos) -> bool {
    is_rail(level.state(pos)) || is_rail(level.state(pos.above())) || is_rail(level.state(pos.below()))
}

impl RailState {
    fn new(pos: BlockPos, state: u32) -> RailState {
        let mut rail = RailState { pos, state, straight: is_straight(state), connections: Vec::new() };
        rail.update_connections(shape(state));
        rail
    }

    fn update_connections(&mut self, shape: Shape) {
        let p = self.pos;
        let (n, s, w, e) = (p.rel(Dir::North), p.rel(Dir::South), p.rel(Dir::West), p.rel(Dir::East));
        self.connections = match shape {
            NorthSouth => vec![n, s],
            EastWest => vec![w, e],
            AscendingEast => vec![w, e.above()],
            AscendingWest => vec![w.above(), e],
            AscendingNorth => vec![n.above(), s],
            AscendingSouth => vec![n, s.above()],
            SouthEast => vec![e, s],
            SouthWest => vec![w, s],
            NorthWest => vec![w, n],
            NorthEast => vec![e, n],
        };
    }

    /// Keeps only the joins whose rail joins back (at that rail's real height).
    fn remove_soft_connections(&mut self, level: &Level) {
        let mut i = 0;
        while i < self.connections.len() {
            match rail_at(level, self.connections[i]) {
                Some(rail) if rail.has_connection(self.pos) => {
                    self.connections[i] = rail.pos;
                    i += 1;
                }
                _ => {
                    self.connections.remove(i);
                }
            }
        }
    }

    fn has_connection(&self, pos: BlockPos) -> bool {
        self.connections.iter().any(|c| c.x == pos.x && c.z == pos.z)
    }

    fn count_potential_connections(&self, level: &Level) -> usize {
        Dir::HORIZONTAL.iter().filter(|d| has_rail(level, self.pos.rel(**d))).count()
    }

    fn can_connect_to(&self, other: &RailState) -> bool {
        self.has_connection(other.pos) || self.connections.len() != 2
    }

    /// Turns towards a rail that joined it.
    fn connect_to(&mut self, level: &mut Level, other: BlockPos) {
        self.connections.push(other);
        let p = self.pos;
        let (north, south, west, east) = (p.rel(Dir::North), p.rel(Dir::South), p.rel(Dir::West), p.rel(Dir::East));
        let (n, s, w, e) = (self.has_connection(north), self.has_connection(south), self.has_connection(west), self.has_connection(east));
        let mut shape = None;
        if n || s {
            shape = Some(NorthSouth);
        }
        if w || e {
            shape = Some(EastWest);
        }
        if !self.straight {
            if s && e && !n && !w {
                shape = Some(SouthEast);
            }
            if s && w && !n && !e {
                shape = Some(SouthWest);
            }
            if n && w && !s && !e {
                shape = Some(NorthWest);
            }
            if n && e && !s && !w {
                shape = Some(NorthEast);
            }
        }
        let shape = slope(level, shape, north, south, west, east).unwrap_or(NorthSouth);
        self.state = with(self.state, "shape", shape.name());
        level.set_block(self.pos, self.state, flags::ALL);
    }

    fn has_neighbor_rail(&self, level: &Level, pos: BlockPos) -> bool {
        match rail_at(level, pos) {
            Some(mut neighbor) => {
                neighbor.remove_soft_connections(level);
                neighbor.can_connect_to(self)
            }
            None => false,
        }
    }

    /// `RailState.place`: picks the shape that joins the neighbouring rails (`default` when
    /// nothing decides) and turns them towards this one.
    fn place(mut self, level: &mut Level, signal: bool, first: bool, default: Shape) -> RailState {
        let p = self.pos;
        let (north, south, west, east) = (p.rel(Dir::North), p.rel(Dir::South), p.rel(Dir::West), p.rel(Dir::East));
        let n = self.has_neighbor_rail(level, north);
        let s = self.has_neighbor_rail(level, south);
        let w = self.has_neighbor_rail(level, west);
        let e = self.has_neighbor_rail(level, east);
        let mut shape = None;
        let (ns, ew) = (n || s, w || e);
        if ns && !ew {
            shape = Some(NorthSouth);
        }
        if ew && !ns {
            shape = Some(EastWest);
        }
        let (se, sw, ne, nw) = (s && e, s && w, n && e, n && w);
        if !self.straight {
            if se && !n && !w {
                shape = Some(SouthEast);
            }
            if sw && !n && !e {
                shape = Some(SouthWest);
            }
            if nw && !s && !e {
                shape = Some(NorthWest);
            }
            if ne && !s && !w {
                shape = Some(NorthEast);
            }
        }
        if shape.is_none() {
            if ns && ew {
                shape = Some(default);
            } else if ns {
                shape = Some(NorthSouth);
            } else if ew {
                shape = Some(EastWest);
            }
            // A junction: a signal flips which curve wins.
            if !self.straight {
                let order = if signal { [(se, SouthEast), (sw, SouthWest), (ne, NorthEast), (nw, NorthWest)] } else { [(nw, NorthWest), (ne, NorthEast), (sw, SouthWest), (se, SouthEast)] };
                for (joined, curve) in order {
                    if joined {
                        shape = Some(curve);
                    }
                }
            }
        }
        let shape = slope(level, shape, north, south, west, east).unwrap_or(default);
        self.update_connections(shape);
        self.state = with(self.state, "shape", shape.name());
        if first || level.state(self.pos) != self.state {
            level.set_block(self.pos, self.state, flags::ALL);
            for i in 0..self.connections.len() {
                if let Some(mut rail) = rail_at(level, self.connections[i]) {
                    rail.remove_soft_connections(level);
                    if rail.can_connect_to(&self) {
                        rail.connect_to(level, self.pos);
                    }
                }
            }
        }
        self
    }
}

/// A straight shape climbs towards a rail one block up.
fn slope(level: &Level, shape: Option<Shape>, north: BlockPos, south: BlockPos, west: BlockPos, east: BlockPos) -> Option<Shape> {
    let mut shape = shape;
    if shape == Some(NorthSouth) {
        if is_rail(level.state(north.above())) {
            shape = Some(AscendingNorth);
        }
        if is_rail(level.state(south.above())) {
            shape = Some(AscendingSouth);
        }
    }
    if shape == Some(EastWest) {
        if is_rail(level.state(east.above())) {
            shape = Some(AscendingEast);
        }
        if is_rail(level.state(west.above())) {
            shape = Some(AscendingWest);
        }
    }
    shape
}

/// `updateDir`: the rail joins its neighbours again.
fn update_dir(level: &mut Level, state: u32, pos: BlockPos, first: bool) -> u32 {
    let signal = level.has_neighbor_signal(pos);
    RailState::new(pos, state).place(level, signal, first, shape(state)).state
}

/// The rails a rail at `pos` joins (`RailState.getConnections`).
pub fn connections(pos: BlockPos, state: u32) -> Vec<BlockPos> {
    RailState::new(pos, state).connections
}

/// `BaseRailBlock.getStateForPlacement`: along the way the player looks.
pub fn placement_shape(looking_east_west: bool) -> &'static str {
    if looking_east_west {
        "east_west"
    } else {
        "north_south"
    }
}

/// `BaseRailBlock.onPlace` (`updateState`): a new rail joins its neighbours; straight rails
/// then check their power, and a detector rail whether a minecart is on it.
pub fn on_place(level: &mut Level, state: u32, pos: BlockPos, old: u32, moved: bool) {
    if block_id(old) == block_id(state) {
        return;
    }
    let state = update_dir(level, state, pos, true);
    if is_straight(state) {
        neighbor_changed(level, state, pos, block_id(state), moved);
    }
    if super::kind(state) == Kind::DetectorRail {
        level.world.updates.effects.push(super::Effect::CheckDetectorRail { pos });
    }
}

/// `BaseRailBlock.neighborChanged`: without support the rail pops off; otherwise a plain
/// rail at a junction re-picks its curve when a signal source next to it changes
/// (`RailBlock.updateState`), and powered and activator rails update their power.
pub fn neighbor_changed(level: &mut Level, state: u32, pos: BlockPos, source_block: u32, moved: bool) {
    if block_id(level.state(pos)) != block_id(state) {
        return;
    }
    if should_be_removed(level, pos, shape(state)) {
        level.drop_resources(state, pos);
        level.remove_block(pos, moved);
        return;
    }
    match super::kind(state) {
        Kind::Rail => {
            if level.is_signal_source(default_of(source_block)) && RailState::new(pos, state).count_potential_connections(level) == 3 {
                update_dir(level, state, pos, false);
            }
        }
        Kind::PoweredRail => update_power(level, state, pos),
        _ => {}
    }
}

/// `BaseRailBlock.affectNeighborsAfterRemoval`.
pub fn after_removal(level: &mut Level, old: u32, pos: BlockPos, moved: bool) {
    if moved {
        return;
    }
    let block = block_id(old);
    if shape(old).is_slope() {
        level.update_neighbors_at(pos.above(), block);
    }
    if is_straight(old) {
        level.update_neighbors_at(pos, block);
        level.update_neighbors_at(pos.below(), block);
    }
}

/// `PoweredRailBlock.updateState`: powered while a signal reaches the rail directly or
/// through up to eight powered rails of its kind in a line.
fn update_power(level: &mut Level, state: u32, pos: BlockPos) {
    let was = get_bool(state, "powered");
    let now = level.has_neighbor_signal(pos) || find_signal(level, pos, state, true, 0) || find_signal(level, pos, state, false, 0);
    if now != was {
        level.set_block(pos, set_bool(state, "powered", now), flags::ALL);
        level.update_neighbors_at(pos.below(), block_id(state));
        if shape(state).is_slope() {
            level.update_neighbors_at(pos.above(), block_id(state));
        }
    }
}

/// `findPoweredRailSignal`: the next rail along the line (`forward` or back) is a powered one
/// of the same kind, lined up, that is powered itself or passes power on.
fn find_signal(level: &Level, pos: BlockPos, state: u32, forward: bool, depth: i32) -> bool {
    if depth >= 8 {
        return false;
    }
    let (mut x, mut y, mut z) = (pos.x, pos.y, pos.z);
    let mut check_below = true;
    let mut line = shape(state);
    match line {
        NorthSouth => z += if forward { 1 } else { -1 },
        EastWest => x += if forward { -1 } else { 1 },
        AscendingEast => {
            if forward {
                x -= 1;
            } else {
                x += 1;
                y += 1;
                check_below = false;
            }
            line = EastWest;
        }
        AscendingWest => {
            if forward {
                x -= 1;
                y += 1;
                check_below = false;
            } else {
                x += 1;
            }
            line = EastWest;
        }
        AscendingNorth => {
            if forward {
                z += 1;
            } else {
                z -= 1;
                y += 1;
                check_below = false;
            }
            line = NorthSouth;
        }
        AscendingSouth => {
            if forward {
                z += 1;
                y += 1;
                check_below = false;
            } else {
                z -= 1;
            }
            line = NorthSouth;
        }
        _ => {}
    }
    let block = block_id(state);
    same_rail_with_power(level, BlockPos::new(x, y, z), block, forward, depth, line) || check_below && same_rail_with_power(level, BlockPos::new(x, y - 1, z), block, forward, depth, line)
}

/// `isSameRailWithPower`.
fn same_rail_with_power(level: &Level, pos: BlockPos, block: u32, forward: bool, depth: i32, line: Shape) -> bool {
    let state = level.state(pos);
    if block_id(state) != block {
        return false;
    }
    let across = match line {
        EastWest => matches!(shape(state), NorthSouth | AscendingNorth | AscendingSouth),
        NorthSouth => matches!(shape(state), EastWest | AscendingEast | AscendingWest),
        _ => false,
    };
    if across || !get_bool(state, "powered") {
        return false;
    }
    level.has_neighbor_signal(pos) || find_signal(level, pos, state, forward, depth + 1)
}

/// `DetectorRailBlock.checkPressed`, given whether a minecart is on the rail: the rail and
/// the rails it joins hear about a change, and it checks again in a second while pressed.
/// Comparators read it again either way.
pub fn check_pressed(level: &mut Level, pos: BlockPos, pressed: bool) {
    let state = level.state(pos);
    if super::kind(state) != Kind::DetectorRail || !can_survive(level, pos) {
        return;
    }
    let block = block_id(state);
    if pressed != get_bool(state, "powered") {
        let now = set_bool(state, "powered", pressed);
        level.set_block(pos, now, flags::ALL);
        // `updatePowerToConnected`.
        for c in connections(pos, now) {
            let joined = level.state(c);
            level.neighbor_changed_with(joined, c, block_id(joined), false);
        }
        level.update_neighbors_at(pos, block);
        level.update_neighbors_at(pos.below(), block);
    }
    if pressed {
        level.schedule_tick(pos, block, 20);
    }
    level.update_neighbour_for_output_signal(pos, block);
}
