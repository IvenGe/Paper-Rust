//! Plants and the other blocks that change on random ticks (vanilla's `randomTick`s): crops,
//! farmland, grass and mycelium, sugar cane, cactus, pumpkin and melon stems, sweet berries,
//! nether wart, cocoa, saplings (their first stage), leaves, ice and snow, and lava lighting
//! fires. Also what keeps these blocks in place (`canSurvive`) and bone meal.

use super::fluid::{fluid_state, Fluid};
use super::state::{block_id, block_name, get_bool, get_dir, get_int, in_tag, is_air, set_bool, set_int, with};
use super::{flags, Dir, Kind, Level, PosExt};
use crate::world::chunk::ChunkPos;
use paper_data::{blocks, physics};
use paper_protocol::codec::BlockPos;

/// The plant families of this module, by block name (`None` for other blocks).
pub fn classify(n: &str) -> Option<Kind> {
    Some(match n {
        "wheat" | "carrots" | "potatoes" | "beetroots" | "torchflower_crop" => Kind::Crop,
        "farmland" => Kind::Farmland,
        "sugar_cane" => Kind::SugarCane,
        "cactus" => Kind::Cactus,
        "pumpkin_stem" | "melon_stem" => Kind::Stem,
        "attached_pumpkin_stem" | "attached_melon_stem" => Kind::AttachedStem,
        "sweet_berry_bush" => Kind::SweetBerryBush,
        "nether_wart" => Kind::NetherWart,
        "cocoa" => Kind::Cocoa,
        "ice" => Kind::Ice,
        "snow" => Kind::SnowLayer,
        _ if n.ends_with("_leaves") => Kind::Leaves,
        _ => return None,
    })
}

/// `getRawBrightness(pos, 0)`: block light or sky light dimmed by the time of day.
pub fn raw_brightness(level: &Level, pos: BlockPos) -> i32 {
    let Some(chunk) = level.world.chunks.get(&ChunkPos::of_block(pos.x, pos.z)).filter(|c| c.lit) else { return 0 };
    let (x, z) = ((pos.x & 15) as usize, (pos.z & 15) as usize);
    let sky = chunk.sky_light_at(x, pos.y, z) as i32 - crate::server::mobs::sky_darken(level.world);
    sky.max(chunk.block_light_at(x, pos.y, z) as i32)
}

fn block_light(level: &Level, pos: BlockPos) -> i32 {
    level.world.chunks.get(&ChunkPos::of_block(pos.x, pos.z)).filter(|c| c.lit).map_or(0, |c| c.block_light_at((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize) as i32)
}

fn random_int(level: &mut Level, bound: u32) -> u32 {
    if bound <= 1 {
        0
    } else {
        (level.random() % bound as u64) as u32
    }
}

fn max_age(state: u32) -> i32 {
    match block_name(state) {
        "minecraft:beetroots" | "minecraft:sweet_berry_bush" | "minecraft:nether_wart" => 3,
        "minecraft:torchflower_crop" => 2,
        "minecraft:cocoa" => 2,
        _ => 7,
    }
}

/// `canSurvive` of this module's blocks.
pub fn can_survive(level: &Level, state: u32, pos: BlockPos) -> bool {
    let below = level.state(pos.below());
    match super::kind(state) {
        Kind::Crop => in_tag!(below, "supports_crops"),
        Kind::Stem | Kind::AttachedStem => in_tag!(below, "supports_stem_crops"),
        Kind::SweetBerryBush => in_tag!(below, "dirt") || in_tag!(below, "supports_crops"),
        Kind::NetherWart => in_tag!(below, "supports_nether_wart"),
        // Nothing solid on top (`FarmBlock.canSurvive`).
        Kind::Farmland => {
            let above = level.state(pos.above());
            physics::flags(above) & physics::SOLID == 0 || in_tag!(above, "fence_gates") || block_name(above) == "minecraft:moving_piston"
        }
        Kind::SugarCane => {
            if block_id(below) == block_id(state) {
                return true;
            }
            if !in_tag!(below, "supports_sugar_cane") {
                return false;
            }
            Dir::HORIZONTAL.iter().any(|d| {
                let side = level.state(pos.below().rel(*d));
                fluid_state(side).fluid == Fluid::Water || in_tag!(side, "supports_sugar_cane_adjacently")
            })
        }
        Kind::Cactus => {
            for d in Dir::HORIZONTAL {
                let side = level.state(pos.rel(d));
                if physics::flags(side) & physics::SOLID != 0 || fluid_state(side).fluid == Fluid::Lava {
                    return false;
                }
            }
            let above = level.state(pos.above());
            (block_id(below) == block_id(state) || in_tag!(below, "supports_cactus")) && fluid_state(above).fluid == Fluid::Empty
        }
        Kind::Cocoa => {
            let facing = get_dir(state, "facing").unwrap_or(Dir::North);
            in_tag!(level.state(pos.rel(facing)), "supports_cocoa")
        }
        Kind::SnowLayer => {
            if in_tag!(below, "cannot_support_snow_layer") {
                return false;
            }
            in_tag!(below, "support_override_snow_layer") || physics::is_face_sturdy(below, Dir::Up.index()) || block_id(below) == block_id(state) && get_int(below, "layers") == 8
        }
        _ => true,
    }
}

/// `updateShape` of this module's blocks.
pub fn update_shape(level: &mut Level, state: u32, pos: BlockPos, dir: Dir, neighbor_state: u32) -> u32 {
    match super::kind(state) {
        Kind::Crop | Kind::Stem | Kind::SweetBerryBush | Kind::NetherWart | Kind::SnowLayer if dir == Dir::Down && !can_survive(level, state, pos) => blocks::AIR,
        Kind::Cocoa if get_dir(state, "facing") == Some(dir) && !can_survive(level, state, pos) => blocks::AIR,
        // Sugar cane and cactus break a tick later (`scheduleTick`).
        Kind::SugarCane | Kind::Cactus => {
            if !can_survive(level, state, pos) {
                level.schedule_tick(pos, block_id(state), 1);
            }
            state
        }
        Kind::Farmland => {
            if dir == Dir::Up && !can_survive(level, state, pos) {
                level.schedule_tick(pos, block_id(state), 1);
            }
            state
        }
        Kind::AttachedStem => {
            // The fruit went: back to a fully grown stem.
            let facing = get_dir(state, "facing").unwrap_or(Dir::North);
            let fruit = if block_name(state) == "minecraft:attached_pumpkin_stem" { "minecraft:pumpkin" } else { "minecraft:melon" };
            if dir == facing && block_name(neighbor_state) != fruit {
                let stem = if fruit == "minecraft:pumpkin" { "minecraft:pumpkin_stem" } else { "minecraft:melon_stem" };
                return blocks::default_state(stem).map_or(state, |s| set_int(s, "age", 7));
            }
            if dir == Dir::Down && !can_survive(level, state, pos) {
                return blocks::AIR;
            }
            state
        }
        Kind::Leaves => {
            let distance = distance_at(neighbor_state) + 1;
            if distance != 1 || get_int(state, "distance") != distance {
                level.schedule_tick(pos, block_id(state), 1);
            }
            state
        }
        _ => state,
    }
}

/// A scheduled tick of this module's blocks.
pub fn tick(level: &mut Level, state: u32, pos: BlockPos) {
    match super::kind(state) {
        Kind::SugarCane | Kind::Cactus if !can_survive(level, state, pos) => {
            level.destroy_block(pos, true);
        }
        Kind::Farmland if !can_survive(level, state, pos) => turn_to_dirt(level, pos),
        Kind::Leaves => {
            let updated = with_distance(level, state, pos);
            if updated != state {
                level.set_block(pos, updated, flags::ALL);
            }
        }
        _ => {}
    }
}

/// `LeavesBlock.getDistanceAt`: logs are 0 away, leaves say how far they are, others 7.
fn distance_at(state: u32) -> i32 {
    if in_tag!(state, "logs") {
        0
    } else if super::kind(state) == Kind::Leaves {
        get_int(state, "distance")
    } else {
        7
    }
}

fn with_distance(level: &Level, state: u32, pos: BlockPos) -> u32 {
    let distance = Dir::ALL.iter().map(|d| distance_at(level.state(pos.rel(*d))) + 1).min().unwrap_or(7).min(7);
    set_int(state, "distance", distance)
}

/// The distance a leaf placed at `pos` starts with (`getStateForPlacement`); placed by a
/// player it never decays.
pub fn leaves_placement(level: &Level, state: u32, pos: BlockPos) -> u32 {
    set_bool(with_distance(level, state, pos), "persistent", true)
}

fn turn_to_dirt(level: &mut Level, pos: BlockPos) {
    if let Some(dirt) = blocks::default_state("minecraft:dirt") {
        level.set_block(pos, dirt, flags::ALL);
    }
}

/// Farmland trampled by an entity falling on it (`FarmBlock.fallOn`).
pub fn trample(level: &mut Level, pos: BlockPos, fall_distance: f64, may_grief: bool) {
    if super::kind(level.state(pos)) != Kind::Farmland || !may_grief {
        return;
    }
    if (level.random_float() as f64) < fall_distance - 0.5 {
        turn_to_dirt(level, pos);
    }
}

/// `CropBlock.getGrowthSpeed`: wet farmland under and around helps; the same crop on
/// neighbouring rows slows it down.
fn growth_speed(level: &Level, state: u32, pos: BlockPos) -> f32 {
    let mut f = 1.0f32;
    let below = pos.below();
    for dx in -1..=1 {
        for dz in -1..=1 {
            let s = level.state(below.offset(dx, 0, dz));
            let mut g = 0.0f32;
            if block_name(s) == "minecraft:farmland" {
                g = if get_int(s, "moisture") > 0 { 3.0 } else { 1.0 };
            }
            if dx != 0 || dz != 0 {
                g /= 4.0;
            }
            f += g;
        }
    }
    let same = |p: BlockPos| block_id(level.state(p)) == block_id(state);
    let (n, s, w, e) = (pos.rel(Dir::North), pos.rel(Dir::South), pos.rel(Dir::West), pos.rel(Dir::East));
    let east_west = same(w) || same(e);
    let north_south = same(n) || same(s);
    if east_west && north_south {
        f /= 2.0;
    } else if same(w.rel(Dir::North)) || same(e.rel(Dir::North)) || same(e.rel(Dir::South)) || same(w.rel(Dir::South)) {
        f /= 2.0;
    }
    f
}

fn grow_to(level: &mut Level, state: u32, pos: BlockPos, age: i32) {
    // A torchflower crop fully grown is a torchflower.
    if block_name(state) == "minecraft:torchflower_crop" && age >= 2 {
        if let Some(flower) = blocks::default_state("minecraft:torchflower") {
            level.set_block(pos, flower, flags::CLIENTS);
        }
        return;
    }
    level.set_block(pos, set_int(state, "age", age.min(max_age(state))), flags::CLIENTS);
}

/// A random tick (`randomTick`) of any block. Returns quickly for blocks that do nothing.
pub fn random_tick(level: &mut Level, state: u32, pos: BlockPos) {
    match super::kind(state) {
        Kind::Crop => {
            // Beetroots grow at a third of the speed.
            if block_name(state) == "minecraft:beetroots" && random_int(level, 3) != 0 {
                return;
            }
            if raw_brightness(level, pos) >= 9 {
                let age = get_int(state, "age");
                if age < max_age(state) {
                    let speed = growth_speed(level, state, pos);
                    if random_int(level, (25.0 / speed) as u32 + 1) == 0 {
                        grow_to(level, state, pos, age + 1);
                    }
                }
            }
        }
        Kind::Stem => {
            if raw_brightness(level, pos) < 9 {
                return;
            }
            let speed = growth_speed(level, state, pos);
            if random_int(level, (25.0 / speed) as u32 + 1) != 0 {
                return;
            }
            let age = get_int(state, "age");
            if age < 7 {
                level.set_block(pos, set_int(state, "age", age + 1), flags::CLIENTS);
                return;
            }
            grow_fruit(level, state, pos);
        }
        Kind::Farmland => {
            let moisture = get_int(state, "moisture");
            let above = pos.above();
            if !near_water(level, pos) && !level.world.rain_at(above.x, above.y, above.z) {
                if moisture > 0 {
                    level.set_block(pos, set_int(state, "moisture", moisture - 1), flags::CLIENTS);
                } else if !in_tag!(level.state(pos.above()), "maintains_farmland") {
                    turn_to_dirt(level, pos);
                }
            } else if moisture < 7 {
                level.set_block(pos, set_int(state, "moisture", 7), flags::CLIENTS);
            }
        }
        Kind::SnowyDirt => spread_grass(level, state, pos),
        Kind::SugarCane => {
            if !is_air(level.state(pos.above())) {
                return;
            }
            let mut height = 1;
            while block_id(level.state(pos.offset(0, -height, 0))) == block_id(state) {
                height += 1;
            }
            if height < 3 {
                let age = get_int(state, "age");
                if age == 15 {
                    level.set_block_and_update(pos.above(), blocks::default_state("minecraft:sugar_cane").unwrap_or(state));
                    level.set_block(pos, set_int(state, "age", 0), flags::INVISIBLE);
                } else {
                    level.set_block(pos, set_int(state, "age", age + 1), flags::INVISIBLE);
                }
            }
        }
        Kind::Cactus => grow_cactus(level, state, pos),
        Kind::SweetBerryBush => {
            let age = get_int(state, "age");
            if age < 3 && random_int(level, 5) == 0 && raw_brightness(level, pos.above()) >= 9 {
                level.set_block(pos, set_int(state, "age", age + 1), flags::CLIENTS);
            }
        }
        Kind::NetherWart => {
            let age = get_int(state, "age");
            if age < 3 && random_int(level, 10) == 0 {
                level.set_block(pos, set_int(state, "age", age + 1), flags::CLIENTS);
            }
        }
        Kind::Cocoa => {
            let age = get_int(state, "age");
            if random_int(level, 5) == 0 && age < 2 {
                level.set_block(pos, set_int(state, "age", age + 1), flags::CLIENTS);
            }
        }
        Kind::Vegetation if block_name(state).ends_with("_sapling") => {
            if raw_brightness(level, pos.above()) >= 9 && random_int(level, 7) == 0 {
                advance_tree(level, state, pos);
            }
        }
        _ if block_name(state) == "minecraft:mangrove_propagule" => {
            // Hanging propagules ripen (`age` 0..4); planted ones grow like saplings.
            if get_bool(state, "hanging") {
                if get_int(state, "age") < 4 {
                    level.set_block(pos, set_int(state, "age", get_int(state, "age") + 1), flags::CLIENTS);
                }
            } else if random_int(level, 7) == 0 {
                advance_tree(level, state, pos);
            }
        }
        Kind::Leaves => {
            if !get_bool(state, "persistent") && get_int(state, "distance") == 7 {
                level.drop_resources(state, pos);
                level.remove_block(pos, false);
            }
        }
        Kind::Ice => {
            if block_light(level, pos) > 11 - blocks::opacity(state) as i32 {
                let melted = if level.world.dimension == "minecraft:the_nether" { blocks::AIR } else { blocks::default_state("minecraft:water").unwrap_or(blocks::AIR) };
                level.set_block_and_update(pos, melted);
            }
        }
        Kind::SnowLayer => {
            if block_light(level, pos) > 11 {
                level.drop_resources(state, pos);
                level.remove_block(pos, false);
            }
        }
        Kind::Lava => lava_ignites(level, pos),
        _ => {}
    }
}

/// `FarmBlock.isNearWater`: water within four blocks around, on its level or one up.
fn near_water(level: &Level, pos: BlockPos) -> bool {
    for dy in 0..=1 {
        for dz in -4..=4 {
            for dx in -4..=4 {
                if fluid_state(level.state(pos.offset(dx, dy, dz))).fluid == Fluid::Water {
                    return true;
                }
            }
        }
    }
    false
}

/// A grown stem puts a pumpkin or melon next to it (`StemBlock.randomTick`).
fn grow_fruit(level: &mut Level, state: u32, pos: BlockPos) {
    let dir = Dir::HORIZONTAL[random_int(level, 4) as usize];
    let at = pos.rel(dir);
    let below = level.state(at.below());
    if !is_air(level.state(at)) || !(block_name(below) == "minecraft:farmland" || in_tag!(below, "dirt")) {
        return;
    }
    let pumpkin = block_name(state) == "minecraft:pumpkin_stem";
    let (fruit, attached) = if pumpkin { ("minecraft:pumpkin", "minecraft:attached_pumpkin_stem") } else { ("minecraft:melon", "minecraft:attached_melon_stem") };
    let (Some(fruit), Some(attached)) = (blocks::default_state(fruit), blocks::default_state(attached)) else { return };
    level.set_block_and_update(at, fruit);
    level.set_block_and_update(pos, with(attached, "facing", dir.name()));
}

/// `CactusBlock.randomTick` (with 1.21.5's cactus flowers).
fn grow_cactus(level: &mut Level, state: u32, pos: BlockPos) {
    let above = pos.above();
    if !is_air(level.state(above)) {
        return;
    }
    let age = get_int(state, "age");
    let mut height = 1;
    while block_id(level.state(pos.offset(0, -height, 0))) == block_id(state) {
        height += 1;
        if height == 3 && age == 15 {
            return;
        }
    }
    if age == 8 && can_survive(level, state, above) {
        let chance = if height >= 3 { 0.25 } else { 0.1 };
        if (level.random_float() as f64) <= chance {
            if let Some(flower) = blocks::default_state("minecraft:cactus_flower") {
                level.set_block_and_update(above, flower);
            }
        }
    } else if age == 15 && height < 3 {
        level.set_block_and_update(above, blocks::default_state("minecraft:cactus").unwrap_or(state));
        level.set_block(pos, set_int(state, "age", 0), flags::INVISIBLE);
        return;
    }
    if age < 15 {
        level.set_block(pos, set_int(state, "age", age + 1), flags::INVISIBLE);
    }
}

/// `SpreadingSnowyDirtBlock.canBeGrass`: light gets to it from above.
fn can_be_grass(level: &Level, pos: BlockPos) -> bool {
    let above = level.state(pos.above());
    if block_name(above) == "minecraft:snow" && get_int(above, "layers") == 1 {
        return true;
    }
    let fluid = fluid_state(above);
    if fluid.amount == 8 {
        return false;
    }
    blocks::opacity(above) < 15
}

/// Grass and mycelium die in the dark and spread to dirt in the light.
fn spread_grass(level: &mut Level, state: u32, pos: BlockPos) {
    let name = block_name(state);
    if !matches!(name, "minecraft:grass_block" | "minecraft:mycelium") {
        return;
    }
    if !can_be_grass(level, pos) {
        if let Some(dirt) = blocks::default_state("minecraft:dirt") {
            level.set_block_and_update(pos, dirt);
        }
        return;
    }
    if raw_brightness(level, pos.above()) < 9 {
        return;
    }
    let Some(spread) = blocks::default_state(name) else { return };
    for _ in 0..4 {
        let at = pos.offset(random_int(level, 3) as i32 - 1, random_int(level, 5) as i32 - 3, random_int(level, 3) as i32 - 1);
        if block_name(level.state(at)) == "minecraft:dirt" && can_be_grass(level, at) && fluid_state(level.state(at.above())).fluid != Fluid::Water {
            let snowy = in_tag!(level.state(at.above()), "snow");
            level.set_block_and_update(at, set_bool(spread, "snowy", snowy));
        }
    }
}

fn flammable(state: u32) -> bool {
    physics::flags(state) & physics::BURNABLE != 0
}

/// `LavaFluid.randomTick`: lava sets fire to flammable blocks near and above it.
fn lava_ignites(level: &mut Level, pos: BlockPos) {
    if !level.settings().fire_spreads {
        return;
    }
    let fire = |level: &Level, at: BlockPos| super::fire::fire_state_for(level.world, at);
    let tries = random_int(level, 3);
    if tries > 0 {
        let mut at = pos;
        for _ in 0..tries {
            at = at.offset(random_int(level, 3) as i32 - 1, 1, random_int(level, 3) as i32 - 1);
            if !level.is_loaded(at) {
                return;
            }
            let s = level.state(at);
            if is_air(s) {
                if Dir::ALL.iter().any(|d| flammable(level.state(at.rel(*d)))) {
                    let f = fire(level, at);
                    level.set_block_and_update(at, f);
                    return;
                }
            } else if physics::flags(s) & physics::SOLID != 0 {
                return;
            }
        }
    } else {
        for _ in 0..3 {
            let at = pos.offset(random_int(level, 3) as i32 - 1, 0, random_int(level, 3) as i32 - 1);
            if !level.is_loaded(at) {
                return;
            }
            if is_air(level.state(at.above())) && flammable(level.state(at)) {
                let f = fire(level, at.above());
                level.set_block_and_update(at.above(), f);
            }
        }
    }
}

/// Bone meal on a block (`BonemealableBlock.performBonemeal` for the blocks here). Returns
/// whether it was used.
pub fn bone_meal(level: &mut Level, state: u32, pos: BlockPos) -> bool {
    let between = |level: &mut Level, lo: u32, hi: u32| lo + random_int(level, hi - lo + 1);
    match super::kind(state) {
        Kind::Crop => {
            let age = get_int(state, "age");
            if age >= max_age(state) {
                return false;
            }
            let grow = between(level, 2, 5) as i32;
            grow_to(level, state, pos, age + grow);
            true
        }
        Kind::Stem => {
            let age = get_int(state, "age");
            if age >= 7 {
                return false;
            }
            let next = (age + between(level, 2, 5) as i32).min(7);
            let grown = set_int(state, "age", next);
            level.set_block(pos, grown, flags::CLIENTS);
            if next == 7 {
                grow_fruit(level, grown, pos);
            }
            true
        }
        Kind::SweetBerryBush | Kind::Cocoa => {
            let age = get_int(state, "age");
            if age >= max_age(state) {
                return false;
            }
            level.set_block(pos, set_int(state, "age", age + 1), flags::CLIENTS);
            true
        }
        Kind::Vegetation if block_name(state).ends_with("_sapling") => {
            // `SaplingBlock.isBonemealSuccess`: 45% of the time a stage further or a tree.
            if level.random_float() < 0.45 {
                advance_tree(level, state, pos);
            }
            true
        }
        _ if block_name(state) == "minecraft:mangrove_propagule" => {
            if get_bool(state, "hanging") {
                if get_int(state, "age") >= 4 {
                    return false;
                }
                level.set_block(pos, set_int(state, "age", 4), flags::CLIENTS);
            } else if level.random_float() < 0.45 {
                advance_tree(level, state, pos);
            }
            true
        }
        _ if matches!(block_name(state), "minecraft:azalea" | "minecraft:flowering_azalea") => {
            // `AzaleaBlock`: the block above must be free; 45% of the time a tree.
            if !is_air(level.state(pos.above())) {
                return false;
            }
            if level.random_float() < 0.45 {
                grow_tree(level, state, pos);
            }
            true
        }
        _ if matches!(block_name(state), "minecraft:red_mushroom" | "minecraft:brown_mushroom") => {
            // `MushroomBlock.growMushroom`, 40% of the time.
            if level.random_float() < 0.4 {
                let feature = if block_name(state) == "minecraft:red_mushroom" { "huge_red_mushroom" } else { "huge_brown_mushroom" };
                crate::world::staged::place_grown_feature(level, feature, pos, &[(pos, blocks::AIR)]);
            }
            true
        }
        _ if matches!(block_name(state), "minecraft:crimson_fungus" | "minecraft:warped_fungus") => {
            // `FungusBlock`: only on its own nylium; 40% of the time a huge fungus.
            let crimson = block_name(state) == "minecraft:crimson_fungus";
            let nylium = if crimson { "minecraft:crimson_nylium" } else { "minecraft:warped_nylium" };
            if block_name(level.state(pos.below())) != nylium {
                return false;
            }
            if level.random_float() < 0.4 {
                crate::world::staged::place_grown_feature(level, if crimson { "crimson_fungus_planted" } else { "warped_fungus_planted" }, pos, &[]);
            }
            true
        }
        _ => false,
    }
}

/// `TreeGrower`: the trees a sapling can grow into (`secondary_chance` picks the second kind).
struct Grower {
    secondary_chance: f32,
    mega: Option<&'static str>,
    secondary_mega: Option<&'static str>,
    tree: Option<&'static str>,
    secondary_tree: Option<&'static str>,
    flowers: Option<&'static str>,
    secondary_flowers: Option<&'static str>,
}

const NO_GROWER: Grower = Grower { secondary_chance: 0.0, mega: None, secondary_mega: None, tree: None, secondary_tree: None, flowers: None, secondary_flowers: None };

/// Vanilla's growers by block. Poplar saplings are left out: how they choose among their
/// three tree colours is not in the data.
fn grower(block: &str) -> Option<Grower> {
    Some(match block {
        "minecraft:oak_sapling" => Grower { secondary_chance: 0.1, tree: Some("oak"), secondary_tree: Some("fancy_oak"), flowers: Some("oak_bees_005"), secondary_flowers: Some("fancy_oak_bees_005"), ..NO_GROWER },
        "minecraft:spruce_sapling" => Grower { secondary_chance: 0.5, mega: Some("mega_spruce"), secondary_mega: Some("mega_pine"), tree: Some("spruce"), ..NO_GROWER },
        "minecraft:mangrove_propagule" => Grower { secondary_chance: 0.85, tree: Some("mangrove"), secondary_tree: Some("tall_mangrove"), ..NO_GROWER },
        "minecraft:azalea" | "minecraft:flowering_azalea" => Grower { tree: Some("azalea_tree"), ..NO_GROWER },
        "minecraft:birch_sapling" => Grower { tree: Some("birch"), flowers: Some("birch_bees_005"), ..NO_GROWER },
        "minecraft:jungle_sapling" => Grower { mega: Some("mega_jungle_tree"), tree: Some("jungle_tree_no_vine"), ..NO_GROWER },
        "minecraft:acacia_sapling" => Grower { tree: Some("acacia"), ..NO_GROWER },
        "minecraft:cherry_sapling" => Grower { tree: Some("cherry"), flowers: Some("cherry_bees_005"), ..NO_GROWER },
        "minecraft:dark_oak_sapling" => Grower { mega: Some("dark_oak"), ..NO_GROWER },
        "minecraft:pale_oak_sapling" => Grower { mega: Some("pale_oak_bonemeal"), ..NO_GROWER },
        _ => return None,
    })
}

/// `SaplingBlock.advanceTree`: the first stage, then the tree.
fn advance_tree(level: &mut Level, state: u32, pos: BlockPos) {
    if get_int(state, "stage") == 0 {
        level.set_block(pos, set_int(state, "stage", 1), flags::INVISIBLE);
    } else {
        grow_tree(level, state, pos);
    }
}

/// `TreeGrower.growTree`: a 2x2 tree if four saplings stand in a square (dark oaks need
/// one), else a single tree, with bees when flowers are near.
pub fn grow_tree(level: &mut Level, state: u32, pos: BlockPos) -> bool {
    use crate::world::staged::place_grown_feature;
    let Some(g) = grower(block_name(state)) else { return false };
    let mega = match g.secondary_mega {
        Some(secondary) if level.random_float() < g.secondary_chance => Some(secondary),
        _ => g.mega,
    };
    if let Some(mega) = mega {
        for dx in [0, -1] {
            for dz in [0, -1] {
                let corner = pos.offset(dx, 0, dz);
                let square = [corner, corner.offset(1, 0, 0), corner.offset(0, 0, 1), corner.offset(1, 0, 1)];
                if square.iter().all(|p| block_id(level.state(*p)) == block_id(state)) {
                    let clear: Vec<(BlockPos, u32)> = square.iter().map(|p| (*p, blocks::AIR)).collect();
                    return place_grown_feature(level, mega, corner, &clear);
                }
            }
        }
    }
    let flowers = has_flowers(level, pos);
    let mut tree = None;
    if level.random_float() < g.secondary_chance {
        tree = if flowers && g.secondary_flowers.is_some() { g.secondary_flowers } else { g.secondary_tree };
    }
    let tree = tree.or(if flowers && g.flowers.is_some() { g.flowers } else { g.tree });
    let Some(tree) = tree else { return false };
    let fill = if get_bool(state, "waterlogged") { blocks::default_state("minecraft:water").unwrap_or(blocks::AIR) } else { blocks::AIR };
    place_grown_feature(level, tree, pos, &[(pos, fill)])
}

/// `TreeGrower.hasFlowers`: a flower within two blocks.
fn has_flowers(level: &mut Level, pos: BlockPos) -> bool {
    for y in -1..=1 {
        for z in -2..=2 {
            for x in -2..=2 {
                if in_tag!(level.state(pos.offset(x, y, z)), "flowers") {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn random_tick_flags() {
        let flag = |n: &str| paper_data::physics::flags(paper_data::blocks::parse_state(n).unwrap()) & paper_data::physics::RANDOM_TICKS != 0;
        for n in ["wheat", "grass_block", "lava", "sugar_cane", "farmland", "oak_sapling", "ice", "snow", "cactus", "pumpkin_stem", "oak_leaves[distance=7,persistent=false]"] {
            assert!(flag(n), "{n} ticks randomly");
        }
        for n in ["stone", "air", "water", "oak_log", "oak_leaves[distance=1,persistent=false]", "oak_leaves[distance=7,persistent=true]"] {
            assert!(!flag(n), "{n} does not");
        }
    }
}
