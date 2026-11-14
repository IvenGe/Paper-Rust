//! The state of an End's dragon fight (vanilla's `EndDragonFight` and its saved `Data`),
//! kept with the End world and saved in its `level.dat` as `DragonFight`. The server runs the
//! fight (`server::dragon`).

use paper_nbt::{Compound, Tag};
use paper_protocol::codec::BlockPos;
use std::collections::HashSet;
use uuid::Uuid;

/// Where the twenty gateways a dragon's death opens stand: a ring of radius 96 at y 75
/// (`EndDragonFight.spawnNewGateway`).
pub fn gateway_position(index: i32) -> BlockPos {
    let angle = 2.0 * (-std::f64::consts::PI + std::f64::consts::PI / 20.0 * index as f64);
    BlockPos::new((96.0 * angle.cos()).floor() as i32, 75, (96.0 * angle.sin()).floor() as i32)
}

#[derive(Clone, Debug)]
pub struct DragonFight {
    /// The world has not been looked at yet (a world from before the fight was kept, or a
    /// new one): the first tick near the island works out whether a dragon was killed.
    pub needs_scan: bool,
    pub dragon_killed: bool,
    pub previously_killed: bool,
    pub dragon: Option<Uuid>,
    /// The exit portal's floor (its centre).
    pub exit_portal: Option<BlockPos>,
    /// The gateways still to open, the next one last (shuffled by the world seed).
    pub gateways: Vec<i32>,
    // Not saved:
    /// The boss bar's id and who sees it.
    pub boss_bar: Uuid,
    pub viewers: HashSet<i32>,
    pub progress: f32,
    pub sent_progress: f32,
    pub ticks_since_player_scan: i32,
    pub ticks_since_dragon_seen: i32,
    pub ticks_since_crystals: i32,
    pub crystals_alive: i32,
    /// The island's chunks are held loaded while players are near (the `DRAGON` ticket).
    pub arena_ticket: bool,
    /// A gateway waiting for its chunk to load before it opens.
    pub pending_gateway: Option<BlockPos>,
    /// Gateways whose exit is being looked for, with the land chunks loading for it.
    pub exit_searches: Vec<ExitSearch>,
}

/// Where a gateway without an exit leads is being worked out: the land found along its
/// direction, and who stepped in.
#[derive(Clone, Debug)]
pub struct ExitSearch {
    pub gateway: BlockPos,
    /// Block x and z of the land found.
    pub at: (i32, i32),
    pub players: Vec<i32>,
    pub chunks: Vec<crate::world::chunk::ChunkPos>,
}

impl DragonFight {
    /// A fight for a world without saved fight data (`EndDragonFight` with empty `Data`).
    pub fn new(seed: i64) -> DragonFight {
        DragonFight {
            needs_scan: true,
            dragon_killed: false,
            previously_killed: false,
            dragon: None,
            exit_portal: None,
            gateways: shuffled_gateways(seed),
            boss_bar: Uuid::new_v4(),
            viewers: HashSet::new(),
            progress: 1.0,
            sent_progress: 1.0,
            ticks_since_player_scan: 21,
            ticks_since_dragon_seen: 0,
            ticks_since_crystals: 0,
            crystals_alive: 0,
            arena_ticket: false,
            pending_gateway: None,
            exit_searches: Vec::new(),
        }
    }

    pub fn from_nbt(c: &Compound, seed: i64) -> DragonFight {
        let mut fight = DragonFight::new(seed);
        fight.needs_scan = c.bool("NeedsStateScanning").unwrap_or(true);
        fight.dragon_killed = c.bool("DragonKilled").unwrap_or(false);
        fight.previously_killed = c.bool("PreviouslyKilled").unwrap_or(false);
        fight.dragon = match c.get("Dragon") {
            Some(Tag::IntArray(v)) if v.len() == 4 => Some(Uuid::from_u128(v.iter().fold(0u128, |acc, i| (acc << 32) | (*i as u32) as u128))),
            _ => None,
        };
        fight.exit_portal = match c.get("ExitPortalLocation") {
            Some(Tag::IntArray(v)) if v.len() == 3 => Some(BlockPos::new(v[0], v[1], v[2])),
            Some(Tag::Compound(p)) => match (p.i32("X"), p.i32("Y"), p.i32("Z")) {
                (Some(x), Some(y), Some(z)) => Some(BlockPos::new(x, y, z)),
                _ => None,
            },
            _ => None,
        };
        match c.get("Gateways") {
            Some(Tag::IntArray(v)) => fight.gateways = v.clone(),
            Some(Tag::List(v)) => fight.gateways = v.iter().filter_map(|t| t.as_i32()).collect(),
            _ => {}
        }
        fight
    }

    pub fn to_nbt(&self) -> Compound {
        let mut c = Compound::new()
            .with("NeedsStateScanning", Tag::Byte(self.needs_scan as i8))
            .with("DragonKilled", Tag::Byte(self.dragon_killed as i8))
            .with("PreviouslyKilled", Tag::Byte(self.previously_killed as i8))
            .with("Gateways", Tag::IntArray(self.gateways.clone()));
        if let Some(uuid) = self.dragon {
            let v = uuid.as_u128();
            c.insert("Dragon", Tag::IntArray(vec![(v >> 96) as i32, (v >> 64) as i32, (v >> 32) as i32, v as i32]));
        }
        if let Some(p) = self.exit_portal {
            c.insert("ExitPortalLocation", Tag::IntArray(vec![p.x, p.y, p.z]));
        }
        c
    }
}

/// `0..20` shuffled with the world seed (`Util.shuffle` with `RandomSource.create(seed)`).
fn shuffled_gateways(seed: i64) -> Vec<i32> {
    use paper_worldgen::random::Random;
    let mut list: Vec<i32> = (0..20).collect();
    let mut random = paper_worldgen::random::LegacyRandom::new(seed);
    for j in (2..=list.len()).rev() {
        let k = random.next_bounded(j as i32) as usize;
        list.swap(j - 1, k);
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fight_state_round_trips() {
        let mut fight = DragonFight::new(42);
        fight.dragon = Some(Uuid::from_u128(0x0123_4567_89ab_cdef_0011_2233_4455_6677));
        fight.exit_portal = Some(BlockPos::new(0, 64, 0));
        fight.previously_killed = true;
        let back = DragonFight::from_nbt(&fight.to_nbt(), 42);
        assert_eq!((back.dragon, back.exit_portal, back.previously_killed, back.gateways.clone()), (fight.dragon, fight.exit_portal, true, fight.gateways.clone()));
        let mut sorted = back.gateways;
        sorted.sort();
        assert_eq!(sorted, (0..20).collect::<Vec<_>>());
    }
}
