//! Item frames, glow item frames and paintings (vanilla's `HangingEntity`): where they hang,
//! the space they take up, and whether they can stay there.

use super::entity::{Aabb, Entity, EntityKind, Vec3};
use super::World;
use paper_data::blocks;
use paper_protocol::codec::BlockPos;
use paper_protocol::types::Slot;

pub const ITEM_FRAME: &str = "minecraft:item_frame";
pub const GLOW_ITEM_FRAME: &str = "minecraft:glow_item_frame";
pub const PAINTING: &str = "minecraft:painting";

/// How often a hanging entity checks that it can stay (`BlockAttachedEntity`'s
/// `CHECK_INTERVAL`, Spigot's `hanging-tick-frequency`).
pub const CHECK_INTERVAL: i32 = 100;

/// An item frame, glow item frame or painting: the block it takes up, and the way it faces
/// (`Direction`'s 3D data value: 0 down, 1 up, 2 north, 3 south, 4 west, 5 east), away from
/// the block that holds it up.
#[derive(Clone, Debug)]
pub struct Hanging {
    pub block: BlockPos,
    pub facing: u8,
    /// A frame's item, how far it is turned (0-7), whether it is fixed in place (only
    /// creative players and `/kill` break it then, and nothing changes its item), and the
    /// chance its item drops when it is knocked out.
    pub item: Slot,
    pub rotation: u8,
    pub fixed: bool,
    pub drop_chance: f32,
    /// A painting's variant (`minecraft:kebab`, ...).
    pub variant: String,
    /// Ticks since it last checked that it can stay (`ticksSinceLastCheck`).
    pub check: i32,
}

/// Offsets of the six directions, by 3D data value.
pub const STEP: [(i32, i32, i32); 6] = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];

pub fn is_frame(name: &str) -> bool {
    name == ITEM_FRAME || name == GLOW_ITEM_FRAME
}

pub fn is_hanging(name: &str) -> bool {
    is_frame(name) || name == PAINTING
}

/// A painting variant's width and height in blocks (vanilla's `PaintingVariants`).
pub fn painting_size(variant: &str) -> Option<(i32, i32)> {
    Some(match variant.strip_prefix("minecraft:").unwrap_or(variant) {
        "alban" | "aztec" | "aztec2" | "bomb" | "kebab" | "plant" | "wasteland" | "meditative" => (1, 1),
        "courbet" | "creebet" | "pool" | "sea" | "sunset" => (2, 1),
        "graham" | "wanderer" | "prairie_ride" => (1, 2),
        "bust" | "match" | "skull_and_roses" | "stage" | "void" | "wither" | "earth" | "fire" | "water" | "wind" | "baroque" | "humble" => (2, 2),
        "fighters" | "changing" | "finding" | "lowmist" | "passage" => (4, 2),
        "skeleton" | "donkey_kong" => (4, 3),
        "bouquet" | "cavebird" | "cotan" | "endboss" | "fern" | "owlemons" | "sunflowers" | "tides" | "dennis" => (3, 3),
        "backyard" | "pond" => (3, 4),
        "burning_skull" | "pigscene" | "pointer" | "orb" | "unpacked" => (4, 4),
        _ => return None,
    })
}

/// The way to the left of a wall's facing (`Direction.getCounterClockWise`).
fn counter_clockwise(facing: u8) -> u8 {
    match facing {
        2 => 4,
        4 => 3,
        3 => 5,
        5 => 2,
        other => other,
    }
}

/// The rotation `setDirection` gives: walls by yaw, floors and ceilings by pitch.
pub fn facing_rotation(facing: u8) -> (f32, f32) {
    match facing {
        0 => (0.0, 90.0),
        1 => (0.0, -90.0),
        2 => (180.0, 0.0),
        3 => (0.0, 0.0),
        4 => (90.0, 0.0),
        _ => (270.0, 0.0),
    }
}

/// Repeaters and comparators hold frames up on their side (`DiodeBlock.isDiode`).
fn is_diode(state: u32) -> bool {
    matches!(blocks::block_of_state(state).name, "minecraft:repeater" | "minecraft:comparator")
}

impl Hanging {
    pub fn new(block: BlockPos, facing: u8) -> Self {
        Hanging { block, facing: facing.min(5), item: Slot::EMPTY, rotation: 0, fixed: false, drop_chance: 1.0, variant: String::new(), check: 0 }
    }

    /// The space it takes up (`calculateBoundingBox`): a thin board against the block behind,
    /// a painting as many blocks wide and high as it is, to the left and up from its block.
    pub fn bounding_box(&self, name: &str) -> Aabb {
        let (sx, sy, sz) = STEP[self.facing as usize % 6];
        let center = Vec3::new(
            self.block.x as f64 + 0.5 - sx as f64 * 0.46875,
            self.block.y as f64 + 0.5 - sy as f64 * 0.46875,
            self.block.z as f64 + 0.5 - sz as f64 * 0.46875,
        );
        let (center, size) = if name == PAINTING {
            let (width, height) = painting_size(&self.variant).unwrap_or((1, 1));
            let offset = |n: i32| if n % 2 == 0 { 0.5 } else { 0.0 };
            let (lx, _, lz) = STEP[counter_clockwise(self.facing) as usize];
            let center = Vec3::new(center.x + lx as f64 * offset(width), center.y + offset(height), center.z + lz as f64 * offset(width));
            let across = |s: i32| if s != 0 { 0.0625 } else { width as f64 };
            (center, Vec3::new(across(sx), height as f64, across(sz)))
        } else {
            let size = |s: i32| if s != 0 { 0.0625 } else { 0.75 };
            (center, Vec3::new(size(sx), size(sy), size(sz)))
        };
        Aabb::new(Vec3::new(center.x - size.x / 2.0, center.y - size.y / 2.0, center.z - size.z / 2.0), Vec3::new(center.x + size.x / 2.0, center.y + size.y / 2.0, center.z + size.z / 2.0))
    }

    /// Whether it can stay where it is (`HangingEntity.survives`, `ItemFrame.survives`):
    /// nothing in its space, the blocks behind it solid (or, for a frame on a wall, a
    /// repeater or comparator), and no other hanging entity in the way. Fixed frames stay.
    pub fn survives(&self, world: &World, name: &str, id: i32) -> bool {
        if self.fixed && is_frame(name) {
            return true;
        }
        let bb = self.bounding_box(name);
        if super::entity::collides(world, &bb) {
            return false;
        }
        let (sx, sy, sz) = STEP[self.facing as usize % 6];
        let supported = if is_frame(name) {
            world.block_at(self.block.x - sx, self.block.y - sy, self.block.z - sz).is_some_and(|s| paper_data::physics::is_solid(s) || self.facing >= 2 && is_diode(s))
        } else {
            // Every block behind it (`calculateSupportBox`).
            let (dx, dy, dz) = (-0.5 * sx as f64, -0.5 * sy as f64, -0.5 * sz as f64);
            let (min, max) = (Vec3::new(bb.min.x + dx + 1.0e-7, bb.min.y + dy + 1.0e-7, bb.min.z + dz + 1.0e-7), Vec3::new(bb.max.x + dx - 1.0e-7, bb.max.y + dy - 1.0e-7, bb.max.z + dz - 1.0e-7));
            (min.x.floor() as i32..=max.x.floor() as i32).all(|x| {
                (min.y.floor() as i32..=max.y.floor() as i32).all(|y| (min.z.floor() as i32..=max.z.floor() as i32).all(|z| world.block_at(x, y, z).is_some_and(|s| paper_data::physics::is_solid(s) || is_diode(s))))
            })
        };
        supported && !world.entities_in(&bb).into_iter().any(|other| other != id && world.entities.get(&other).is_some_and(|o| matches!(o.kind, EntityKind::Hanging(_))))
    }
}

/// A new hanging entity of type `name` (not yet in a world), at the middle of its board and
/// turned the way it faces.
pub fn new_entity(id: i32, name: &str, hanging: Hanging) -> Option<Entity> {
    let info = paper_data::entity_types::by_name(name)?;
    let bb = hanging.bounding_box(name);
    let center = Vec3::new((bb.min.x + bb.max.x) / 2.0, (bb.min.y + bb.max.y) / 2.0, (bb.min.z + bb.max.z) / 2.0);
    let (yaw, pitch) = facing_rotation(hanging.facing);
    let mut e = Entity::new(id, info, center, EntityKind::Hanging(Box::new(hanging)));
    e.yaw = yaw;
    e.head_yaw = yaw;
    e.pitch = pitch;
    // Spread their checks over the interval, as Paper does.
    if let EntityKind::Hanging(h) = &mut e.kind {
        h.check = id.rem_euclid(CHECK_INTERVAL);
    }
    Some(e)
}

/// The variants a painting placed at `block` facing `facing` may take
/// (`Painting.create`): the placeable ones that fit, of the largest size that does.
pub fn fitting_variants(world: &World, block: BlockPos, facing: u8, id: i32) -> Vec<&'static str> {
    let Some(placeable) = paper_data::synced::tag("minecraft:painting_variant", "minecraft:placeable") else { return Vec::new() };
    let Some(entries) = paper_data::synced::entries("minecraft:painting_variant") else { return Vec::new() };
    let mut fitting: Vec<(&'static str, i32)> = Vec::new();
    for &entry in placeable {
        let Some(&name) = entries.get(entry as usize) else { continue };
        let Some((width, height)) = painting_size(name) else { continue };
        let mut h = Hanging::new(block, facing);
        h.variant = name.to_owned();
        if h.survives(world, PAINTING, id) {
            fitting.push((name, width * height));
        }
    }
    let largest = fitting.iter().map(|(_, area)| *area).max().unwrap_or(0);
    fitting.into_iter().filter(|(_, area)| *area == largest).map(|(name, _)| name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_painting_variant_has_a_size() {
        for name in paper_data::synced::entries("minecraft:painting_variant").unwrap() {
            assert!(painting_size(name).is_some(), "{name}");
        }
    }

    #[test]
    fn boards_sit_against_the_block_behind() {
        // A frame on the south side of a block at z = -1 hangs in block z = 0, at its north edge.
        let frame = Hanging::new(BlockPos::new(0, 64, 0), 3).bounding_box(ITEM_FRAME);
        assert_eq!((frame.min.z, frame.max.z), (0.0, 0.0625));
        assert_eq!((frame.min.x, frame.max.x, frame.min.y, frame.max.y), (0.125, 0.875, 64.125, 64.875));
        // A 2x2 painting facing north reaches into the block to its left (west) and above.
        let mut painting = Hanging::new(BlockPos::new(0, 64, 0), 2);
        painting.variant = "minecraft:bust".into();
        let b = painting.bounding_box(PAINTING);
        assert_eq!((b.min.x, b.max.x, b.min.y, b.max.y), (-1.0, 1.0, 64.0, 66.0));
        assert_eq!((b.min.z, b.max.z), (0.9375, 1.0));
    }
}
