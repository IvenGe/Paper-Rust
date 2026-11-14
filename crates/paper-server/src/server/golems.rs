//! Golems built from blocks (`CarvedPumpkinBlock.trySpawnGolem`): a carved pumpkin or jack
//! o'lantern put on two snow blocks makes a snow golem, and on a T of four iron blocks (with
//! nothing beside its head or its leg) an iron golem. The blocks become the golem. As with
//! vanilla's `BlockPattern`, the shapes count in any orientation, sideways and upside down
//! too.

use super::mobs::SpawnReason;
use super::Server;
use crate::world::entity::Vec3;
use crate::world::World;
use paper_data::blocks;
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::play;

const DIRS: [(i32, i32, i32); 6] = [(0, 1, 0), (0, -1, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)];

fn is_head(name: &str) -> bool {
    matches!(name, "minecraft:carved_pumpkin" | "minecraft:jack_o_lantern")
}

fn step(p: BlockPos, (dx, dy, dz): (i32, i32, i32), n: i32) -> BlockPos {
    BlockPos::new(p.x + dx * n, p.y + dy * n, p.z + dz * n)
}

/// The golem the blocks around `head` make: its kind, its blocks, and the block it stands in.
/// Without `head_placed` the head may still be missing (a dispenser asking whether a pumpkin
/// would make one).
pub fn golem_at(world: &World, head: BlockPos, head_placed: bool) -> Option<(&'static str, Vec<BlockPos>, BlockPos)> {
    let name = |p: BlockPos| world.block_at(p.x, p.y, p.z).map(|s| blocks::block_of_state(s).name);
    if head_placed && !name(head).is_some_and(is_head) {
        return None;
    }
    let is = |p: BlockPos, block: &str| name(p) == Some(block);
    let air = |p: BlockPos| world.block_at(p.x, p.y, p.z).is_some_and(blocks::is_air);
    // Two snow blocks below the head make a snow golem (tried first).
    for up in DIRS {
        let down = (-up.0, -up.1, -up.2);
        let (body, base) = (step(head, down, 1), step(head, down, 2));
        if is(body, "minecraft:snow_block") && is(base, "minecraft:snow_block") {
            return Some(("minecraft:snow_golem", vec![head, body, base], base));
        }
    }
    // An iron body with arms either side and a leg below, with air beside the head and leg.
    for up in DIRS {
        let down = (-up.0, -up.1, -up.2);
        for side in DIRS {
            if side == up || side == down {
                continue;
            }
            let (body, leg) = (step(head, down, 1), step(head, down, 2));
            let arms = [step(body, side, 1), step(body, side, -1)];
            let iron = |p: BlockPos| is(p, "minecraft:iron_block");
            let clear = [step(head, side, 1), step(head, side, -1), step(leg, side, 1), step(leg, side, -1)];
            if iron(body) && iron(leg) && arms.iter().all(|a| iron(*a)) && clear.iter().all(|c| air(*c)) {
                return Some(("minecraft:iron_golem", vec![head, body, arms[0], arms[1], leg], leg));
            }
        }
    }
    None
}

impl Server {
    /// A carved pumpkin or jack o'lantern was placed: the golem its blocks make, if any
    /// (`spawnGolemInWorld`: the blocks break, and the golem stands where the lowest was).
    pub(super) fn try_spawn_golem(&mut self, world: usize, head: BlockPos) {
        let Some((kind, parts, stands)) = golem_at(&self.worlds[world], head, true) else { return };
        for p in &parts {
            let old = self.worlds[world].block_at(p.x, p.y, p.z).unwrap_or(blocks::AIR);
            self.send_near(world, *p, 64.0, None, &play::LevelEvent { event: 2001, pos: *p, data: old as i32, global: false });
            self.set_block_and_update(world, *p, blocks::AIR);
        }
        let at = Vec3::new(stands.x as f64 + 0.5, stands.y as f64 + 0.05, stands.z as f64 + 0.5);
        if let Some(id) = self.spawn_mob(world, kind, at, SpawnReason::Built) {
            if let Some(m) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                m.persistent = true;
            }
        }
    }
}
