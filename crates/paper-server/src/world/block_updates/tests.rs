//! Unit tests of block updates on a flat test world (grass at y = -61, air from y = -60).

use super::fluid::{fluid_state, Fluid};
use super::state::{get_bool, get_int, prop};
use super::*;
use crate::world::generator::FlatGenerator;
use paper_data::blocks;
use std::sync::Arc;

const Y: i32 = -60;

fn world() -> World {
    let mut w = World::new(0, "test", "minecraft:overworld", 1, Arc::new(FlatGenerator::new(-64)));
    for cx in -3..=3 {
        for cz in -3..=3 {
            let chunk = w.generator.generate(ChunkPos::new(cx, cz), w.min_y, w.height);
            w.chunks.insert(chunk.pos, chunk);
        }
    }
    w
}

fn s(name: &str) -> u32 {
    blocks::parse_state(name).unwrap_or_else(|| panic!("bad state {name}"))
}

fn p(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn set(w: &mut World, pos: BlockPos, state: u32) {
    Level::new(w).set_block(pos, state, flags::ALL);
}

/// Places a block the way a player would: vanilla's placement rules, then `set`.
fn place(w: &mut World, pos: BlockPos, state: u32) {
    let replaced = get(w, pos);
    let ctx = support::PlaceContext { facing: Dir::North, looking: Dir::North, replaced, click: (0.5, 0.5, 0.5) };
    let state = support::placement_state(&Level::new(w), state, pos, ctx).expect("the block can be placed here");
    set(w, pos, state);
}

fn get(w: &World, pos: BlockPos) -> u32 {
    w.block_at(pos.x, pos.y, pos.z).unwrap()
}

fn name(w: &World, pos: BlockPos) -> &'static str {
    blocks::block_of_state(get(w, pos)).name.strip_prefix("minecraft:").unwrap()
}

/// Advances the world by `n` game ticks.
fn run(w: &mut World, n: usize) {
    for _ in 0..n {
        w.tick_time();
        tick(w, &|_| true);
    }
}

fn use_block(w: &mut World, pos: BlockPos) -> Interaction {
    let state = get(w, pos);
    Level::new(w).use_without_item(state, pos, UseContext { player: 1, may_build: true, facing: Dir::North })
}

fn water_level(w: &World, pos: BlockPos) -> Option<i32> {
    let state = get(w, pos);
    (kind(state) == Kind::Water).then(|| get_int(state, "level"))
}

// --- infrastructure ---------------------------------------------------------------------------

#[test]
fn hash_set_order_matches_java() {
    let c = p(0, 64, 0);
    let mut around = vec![c];
    around.extend(Dir::ALL.iter().map(|d| c.rel(*d)));
    let order = java_hash_order(&around);
    assert_eq!(order, vec![c, c.below(), c.rel(Dir::South), c.rel(Dir::East), c.above(), c.rel(Dir::North), c.rel(Dir::West)]);
}

#[test]
fn neighbour_updates_run_depth_first_in_vanilla_order() {
    // A torch on a block next to another torch: breaking the support block of the first
    // torch removes it (shape update), and nothing else.
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("stone"));
    set(&mut w, p(0, Y + 1, 0), s("torch"));
    set(&mut w, p(1, Y, 0), s("wall_torch[facing=east]"));
    assert_eq!(name(&w, p(1, Y, 0)), "wall_torch");
    w.updates.effects.clear();
    set(&mut w, p(0, Y, 0), blocks::AIR);
    assert_eq!(name(&w, p(0, Y + 1, 0)), "air", "the standing torch lost its support");
    assert_eq!(name(&w, p(1, Y, 0)), "air", "the wall torch lost its support");
    let drops = w.updates.effects.iter().filter(|e| matches!(e, Effect::Drops { .. })).count();
    assert_eq!(drops, 2, "both torches drop");
    let breaks = w.updates.effects.iter().filter(|e| matches!(e, Effect::LevelEvent { event: 2001, .. })).count();
    assert_eq!(breaks, 2);
    // Without support a torch cannot be placed either.
    let level = Level::new(&mut w);
    assert!(!level.can_survive(s("torch"), p(5, Y + 3, 5)));
    assert!(level.can_survive(s("torch"), p(5, Y, 5)));
}

#[test]
fn scheduled_ticks_run_in_order_and_once() {
    let mut w = world();
    let torch = s("redstone_torch");
    set(&mut w, p(0, Y, 0), torch);
    let block = block_id(torch);
    let mut level = Level::new(&mut w);
    assert!(level.schedule_tick(p(0, Y, 0), block, 3));
    assert!(!level.schedule_tick(p(0, Y, 0), block, 1), "one pending tick per position and block");
    assert!(level.has_scheduled_tick(p(0, Y, 0), block));
    run(&mut w, 2);
    assert!(Level::new(&mut w).has_scheduled_tick(p(0, Y, 0), block));
    run(&mut w, 1);
    assert!(!Level::new(&mut w).has_scheduled_tick(p(0, Y, 0), block), "ran at its due time");
}

// --- fluids -------------------------------------------------------------------------------

#[test]
fn water_spreads_in_a_diamond_on_flat_ground() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("water"));
    run(&mut w, 60);
    assert_eq!(water_level(&w, p(0, Y, 0)), Some(0), "the source stays");
    for (x, z) in [(1i32, 0i32), (0, -3), (2, 2), (-4, 3), (7, 0), (0, -7), (3, -4)] {
        let distance = x.abs() + z.abs();
        assert_eq!(water_level(&w, p(x, Y, z)), Some(distance), "flowing water at {x},{z}");
    }
    for (x, z) in [(8, 0), (4, 4), (-1, -7), (0, 9)] {
        assert_eq!(name(&w, p(x, Y, z)), "air", "no water at {x},{z}");
    }
    assert_eq!(name(&w, p(0, Y + 1, 0)), "air");
}

#[test]
fn water_flows_towards_the_nearest_hole() {
    let mut w = world();
    // A hole two blocks east: water only flows that way.
    set(&mut w, p(2, Y - 1, 0), blocks::AIR);
    set(&mut w, p(0, Y, 0), s("water"));
    run(&mut w, 30);
    assert_eq!(water_level(&w, p(1, Y, 0)), Some(1));
    assert_eq!(name(&w, p(-1, Y, 0)), "air", "not away from the hole");
    assert_eq!(name(&w, p(0, Y, 1)), "air");
    assert!(water_level(&w, p(2, Y - 1, 0)).is_some(), "falls into the hole");
}

#[test]
fn falling_water_and_drying_up() {
    let mut w = world();
    let top = Y + 8;
    set(&mut w, p(0, top, 0), s("water"));
    run(&mut w, 6);
    assert_eq!(water_level(&w, p(0, top - 1, 0)), Some(8), "first it falls");
    assert_eq!(name(&w, p(1, top, 0)), "air");
    // With water below, the source's next tick spreads it sideways: a plus-shaped waterfall.
    run(&mut w, 74);
    for y in Y + 1..top {
        assert_eq!(water_level(&w, p(0, y, 0)), Some(8), "falling water at y {y}");
        assert_eq!(water_level(&w, p(0, y, 1)), Some(8), "falling water beside at y {y}");
    }
    assert_eq!(water_level(&w, p(1, top, 0)), Some(1));
    assert_eq!(name(&w, p(1, top, 1)), "air");
    // Where it lands it spreads: amount 7 next to a column, one less per block.
    assert_eq!(water_level(&w, p(0, Y, 0)), Some(8));
    assert_eq!(water_level(&w, p(2, Y, 0)), Some(1));
    assert_eq!(water_level(&w, p(0, Y, 8)), Some(7));
    assert_eq!(name(&w, p(0, Y, 9)), "air");
    // Without the source the waterfall and the pool disappear. Neighbouring columns feed
    // each other sideways, so each layer takes a while to drain.
    set(&mut w, p(0, top, 0), blocks::AIR);
    run(&mut w, 500);
    for y in Y..top {
        assert_eq!(name(&w, p(0, y, 0)), "air", "water left at y {y}");
    }
    assert_eq!(name(&w, p(3, Y, 0)), "air");
}

#[test]
fn two_water_sources_make_a_third() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("water"));
    set(&mut w, p(2, Y, 0), s("water"));
    run(&mut w, 20);
    assert_eq!(water_level(&w, p(1, Y, 0)), Some(0), "a new source between two sources on solid ground");
    // Lava does not do that.
    set(&mut w, p(10, Y, 10), s("lava"));
    set(&mut w, p(12, Y, 10), s("lava"));
    run(&mut w, 100);
    let between = get(&w, p(11, Y, 10));
    assert_eq!(kind(between), Kind::Lava);
    assert_ne!(get_int(between, "level"), 0);
}

#[test]
fn lava_spreads_less_and_slower_than_water_except_in_the_nether() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("lava"));
    run(&mut w, 29);
    assert_eq!(name(&w, p(1, Y, 0)), "air", "lava waits 30 ticks");
    run(&mut w, 200);
    let level_at = |w: &World, x: i32| get_int(get(w, p(x, Y, 0)), "level");
    assert_eq!((level_at(&w, 1), level_at(&w, 2), level_at(&w, 3)), (2, 4, 6), "drop-off 2");
    assert_eq!(name(&w, p(4, Y, 0)), "air", "lava reaches 3 blocks in the overworld");

    let mut nether = world();
    nether.updates.settings.fast_lava = true;
    set(&mut nether, p(0, Y, 0), s("lava"));
    run(&mut nether, 10);
    assert_eq!(kind(get(&nether, p(1, Y, 0))), Kind::Lava, "nether lava ticks every 10 ticks");
    run(&mut nether, 200);
    assert_eq!(get_int(get(&nether, p(7, Y, 0)), "level"), 7, "and reaches 7 blocks");
}

#[test]
fn lava_and_water_make_obsidian_cobblestone_stone_and_basalt() {
    // Water next to a lava source: obsidian.
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("lava"));
    set(&mut w, p(1, Y, 0), s("water"));
    assert_eq!(name(&w, p(0, Y, 0)), "obsidian");
    assert!(w.updates.effects.iter().any(|e| matches!(e, Effect::LevelEvent { event: 1501, .. })), "the fizz");

    // Water next to flowing lava: cobblestone.
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("lava"));
    run(&mut w, 100);
    assert_eq!(get_int(get(&w, p(2, Y, 0)), "level"), 4);
    set(&mut w, p(3, Y, 0), s("water"));
    assert_eq!(name(&w, p(2, Y, 0)), "cobblestone");

    // Lava flowing down onto water: stone.
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("water"));
    set(&mut w, p(0, Y + 2, 0), s("lava"));
    run(&mut w, 70);
    assert_eq!(name(&w, p(0, Y, 0)), "stone");

    // Lava over soul soil next to blue ice: basalt.
    let mut w = world();
    set(&mut w, p(0, Y - 1, 0), s("soul_soil"));
    set(&mut w, p(1, Y, 0), s("blue_ice"));
    set(&mut w, p(0, Y, 0), s("lava"));
    assert_eq!(name(&w, p(0, Y, 0)), "basalt");
}

#[test]
fn water_washes_away_torches_and_wire_but_not_signs() {
    let mut w = world();
    set(&mut w, p(1, Y, 0), s("torch"));
    set(&mut w, p(0, Y, 1), s("redstone_wire"));
    set(&mut w, p(-1, Y, 0), s("oak_sign"));
    w.updates.effects.clear();
    set(&mut w, p(0, Y, 0), s("water"));
    run(&mut w, 6);
    assert!(water_level(&w, p(1, Y, 0)).is_some(), "the torch was washed away");
    assert!(water_level(&w, p(0, Y, 1)).is_some(), "the wire was washed away");
    assert_eq!(name(&w, p(-1, Y, 0)), "oak_sign", "signs block water");
    let dropped: Vec<&str> = w.updates.effects.iter().filter_map(|e| if let Effect::Drops { state, .. } = e { Some(blocks::block_of_state(*state).name) } else { None }).collect();
    assert!(dropped.contains(&"minecraft:torch") && dropped.contains(&"minecraft:redstone_wire"), "{dropped:?}");
}

#[test]
fn waterlogged_blocks_are_water_sources() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("oak_stairs[waterlogged=true]"));
    run(&mut w, 1);
    // Placing it does not schedule anything, a neighbour change does.
    set(&mut w, p(5, Y, 5), s("stone"));
    set(&mut w, p(0, Y + 1, 0), s("stone"));
    run(&mut w, 20);
    assert!(fluid_state(get(&w, p(0, Y, 0))).source);
    assert!(water_level(&w, p(2, Y, 0)).is_some(), "the stairs' water spread");
    // Flowing water does not waterlog a slab.
    set(&mut w, p(-2, Y, 0), s("stone_slab"));
    run(&mut w, 20);
    assert!(!get_bool(get(&w, p(-2, Y, 0)), "waterlogged"));
    assert_eq!(fluid_state(get(&w, p(-2, Y, 0))).fluid, Fluid::Empty);
}

#[test]
fn breaking_a_waterlogged_block_leaves_water() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("oak_slab[waterlogged=true]"));
    let mut level = Level::new(&mut w);
    assert!(level.remove_block(p(0, Y, 0), false));
    assert_eq!(water_level(&w, p(0, Y, 0)), Some(0));
}

// --- redstone -----------------------------------------------------------------------------

fn wire_power(w: &World, pos: BlockPos) -> i32 {
    let state = get(w, pos);
    assert_eq!(kind(state), Kind::RedstoneWire, "wire at {pos:?}");
    get_int(state, "power")
}

#[test]
fn wire_power_decays_along_a_line() {
    let mut w = world();
    for x in 0..17 {
        set(&mut w, p(x, Y, 0), s("redstone_wire"));
    }
    assert_eq!(prop(get(&w, p(5, Y, 0)), "east"), Some("side"), "a line connects east-west");
    assert_eq!(prop(get(&w, p(5, Y, 0)), "north"), Some("none"));
    set(&mut w, p(-1, Y, 0), s("redstone_block"));
    for x in 0..16 {
        assert_eq!(wire_power(&w, p(x, Y, 0)), 15 - x, "power at x {x}");
    }
    assert_eq!(wire_power(&w, p(16, Y, 0)), 0);
    assert_eq!(prop(get(&w, p(0, Y, 0)), "west"), Some("side"), "connects to the redstone block");
    set(&mut w, p(-1, Y, 0), blocks::AIR);
    for x in 0..17 {
        assert_eq!(wire_power(&w, p(x, Y, 0)), 0, "power at x {x} after removal");
    }
}

#[test]
fn wire_shapes_climb_blocks_and_power_does_not_pass_through_blocks() {
    let mut w = world();
    // A lamp beside the block, then a step up onto the block.
    set(&mut w, p(1, Y, 1), s("redstone_lamp"));
    set(&mut w, p(1, Y, 0), s("stone"));
    place(&mut w, p(0, Y, 0), s("redstone_wire"));
    place(&mut w, p(1, Y + 1, 0), s("redstone_wire"));
    assert_eq!(prop(get(&w, p(0, Y, 0)), "east"), Some("up"), "climbs the side of the block");
    set(&mut w, p(-1, Y, 0), s("redstone_block"));
    assert_eq!(wire_power(&w, p(0, Y, 0)), 15);
    assert_eq!(wire_power(&w, p(1, Y + 1, 0)), 14, "power climbs the step");
    place(&mut w, p(2, Y, 0), s("redstone_wire"));
    assert_eq!(wire_power(&w, p(2, Y, 0)), 13, "the wire on the block reaches down the other side");
    // Without the wire on top, the wire next to the stone powers the stone only weakly: the
    // wire on the other side stays off, the lamp beside the stone lights.
    set(&mut w, p(1, Y + 1, 0), blocks::AIR);
    assert_eq!(prop(get(&w, p(0, Y, 0)), "east"), Some("side"), "the wire end points into the stone");
    assert_eq!(wire_power(&w, p(2, Y, 0)), 0, "no wire power through the stone");
    assert!(get_bool(get(&w, p(1, Y, 1)), "lit"), "a weakly powered block powers a lamp");
}

#[test]
fn torch_inverts_and_burns_out() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("stone"));
    set(&mut w, p(1, Y + 1, 0), s("redstone_lamp"));
    set(&mut w, p(0, Y + 1, 0), s("redstone_torch"));
    assert!(get_bool(get(&w, p(1, Y + 1, 0)), "lit"), "a lit torch powers the lamp beside it");
    // A lever on the stone powers it strongly: the torch turns off after 2 ticks.
    set(&mut w, p(-1, Y, 0), s("lever[face=wall,facing=west]"));
    assert_eq!(use_block(&mut w, p(-1, Y, 0)), Interaction::Success);
    run(&mut w, 1);
    assert!(get_bool(get(&w, p(0, Y + 1, 0)), "lit"));
    run(&mut w, 1);
    assert!(!get_bool(get(&w, p(0, Y + 1, 0)), "lit"), "inverted after 2 ticks");
    run(&mut w, 4);
    assert!(!get_bool(get(&w, p(1, Y + 1, 0)), "lit"), "the lamp went out 4 ticks later");
    use_block(&mut w, p(-1, Y, 0));
    run(&mut w, 2);
    assert!(get_bool(get(&w, p(0, Y + 1, 0)), "lit"), "back on");

    // A torch toggled more than 8 times within 60 ticks burns out.
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("stone"));
    set(&mut w, p(0, Y + 1, 0), s("redstone_torch"));
    for i in 0..20 {
        let lever = p(-1, Y, 0);
        if i == 0 {
            set(&mut w, lever, s("lever[face=wall,facing=west]"));
        }
        use_block(&mut w, lever);
        run(&mut w, 3);
    }
    assert!(w.updates.effects.iter().any(|e| matches!(e, Effect::LevelEvent { event: 1502, .. })), "burnout smoke");
}

/// A repeater facing west reads power from the west and outputs east.
fn repeater_line(w: &mut World, delay: i32) {
    set(w, p(0, Y, 0), s(&format!("repeater[facing=west,delay={delay}]")));
    set(w, p(1, Y, 0), s("redstone_lamp"));
}

#[test]
fn repeater_delays_by_its_setting() {
    for delay in [1, 4] {
        let mut w = world();
        repeater_line(&mut w, delay);
        set(&mut w, p(-1, Y, 0), s("redstone_block"));
        run(&mut w, (delay * 2 - 1) as usize);
        assert!(!get_bool(get(&w, p(1, Y, 0)), "lit"), "delay {delay}: not yet");
        run(&mut w, 1);
        assert!(get_bool(get(&w, p(0, Y, 0)), "powered"));
        assert!(get_bool(get(&w, p(1, Y, 0)), "lit"), "delay {delay}: on after {} ticks", delay * 2);
    }
    // Right-clicking cycles the delay.
    let mut w = world();
    repeater_line(&mut w, 4);
    use_block(&mut w, p(0, Y, 0));
    assert_eq!(get_int(get(&w, p(0, Y, 0)), "delay"), 1);
}

#[test]
fn repeater_locked_by_a_side_repeater() {
    let mut w = world();
    repeater_line(&mut w, 1);
    set(&mut w, p(-1, Y, 0), s("redstone_block"));
    run(&mut w, 2);
    assert!(get_bool(get(&w, p(1, Y, 0)), "lit"));
    // A second repeater points into the side of the first one.
    set(&mut w, p(0, Y, 1), s("repeater[facing=south,delay=1]"));
    set(&mut w, p(0, Y, 2), s("redstone_block"));
    run(&mut w, 2);
    assert!(get_bool(get(&w, p(0, Y, 0)), "locked"), "locked by the powered side repeater");
    // While locked, removing the input changes nothing.
    set(&mut w, p(-1, Y, 0), blocks::AIR);
    run(&mut w, 10);
    assert!(get_bool(get(&w, p(0, Y, 0)), "powered"), "a locked repeater keeps its output");
    assert!(get_bool(get(&w, p(1, Y, 0)), "lit"));
    // Unlocking lets it catch up.
    set(&mut w, p(0, Y, 2), blocks::AIR);
    run(&mut w, 10);
    assert!(!get_bool(get(&w, p(0, Y, 0)), "locked"));
    assert!(!get_bool(get(&w, p(0, Y, 0)), "powered"));
    assert!(!get_bool(get(&w, p(1, Y, 0)), "lit"));
}

#[test]
fn comparator_compares_and_subtracts() {
    let mut w = world();
    // Comparator facing west: rear input from the west, sides north and south, output east.
    set(&mut w, p(0, Y, 0), s("comparator[facing=west]"));
    for x in 1..4 {
        set(&mut w, p(x, Y, 0), s("redstone_wire"));
    }
    // Rear: a redstone block (15). Side: a wire powered to 13.
    set(&mut w, p(-1, Y, 0), s("redstone_block"));
    run(&mut w, 2);
    assert_eq!(wire_power(&w, p(1, Y, 0)), 15);
    set(&mut w, p(0, Y, 2), s("redstone_block"));
    set(&mut w, p(0, Y, 1), s("redstone_wire"));
    run(&mut w, 2);
    assert_eq!(wire_power(&w, p(0, Y, 1)), 15);
    assert_eq!(wire_power(&w, p(1, Y, 0)), 15, "compare: rear 15 >= side 15");
    use_block(&mut w, p(0, Y, 0));
    assert_eq!(prop(get(&w, p(0, Y, 0)), "mode"), Some("subtract"));
    run(&mut w, 2);
    assert_eq!(wire_power(&w, p(1, Y, 0)), 0, "subtract: 15 - 15");
    assert!(!get_bool(get(&w, p(0, Y, 0)), "powered"));
    // A full cauldron behind the comparator reads 3.
    set(&mut w, p(0, Y, 1), blocks::AIR);
    set(&mut w, p(-1, Y, 0), s("water_cauldron[level=3]"));
    run(&mut w, 4);
    assert_eq!(wire_power(&w, p(1, Y, 0)), 3);
}

#[test]
fn observer_pulses_when_its_face_sees_a_change() {
    let mut w = world();
    // Observer facing north looks at z = -1 and outputs south into the lamp.
    set(&mut w, p(0, Y, 0), s("observer[facing=north]"));
    set(&mut w, p(0, Y, 1), s("redstone_lamp"));
    run(&mut w, 5);
    set(&mut w, p(0, Y, -1), s("stone"));
    run(&mut w, 2);
    assert!(get_bool(get(&w, p(0, Y, 0)), "powered"), "pulse starts 2 ticks after the change");
    assert!(get_bool(get(&w, p(0, Y, 1)), "lit"));
    run(&mut w, 2);
    assert!(!get_bool(get(&w, p(0, Y, 0)), "powered"), "2-tick pulse");
    run(&mut w, 4);
    assert!(!get_bool(get(&w, p(0, Y, 1)), "lit"));
}

#[test]
fn buttons_release_after_their_press_time() {
    for (button, ticks) in [("stone_button", 20), ("oak_button", 30)] {
        let mut w = world();
        set(&mut w, p(0, Y, 0), s("stone"));
        set(&mut w, p(1, Y, 0), s(&format!("{button}[face=wall,facing=east]")));
        set(&mut w, p(-1, Y, 0), s("redstone_lamp"));
        assert_eq!(use_block(&mut w, p(1, Y, 0)), Interaction::Success);
        assert_eq!(use_block(&mut w, p(1, Y, 0)), Interaction::Consume, "already pressed");
        assert!(get_bool(get(&w, p(-1, Y, 0)), "lit"), "strongly powers the block it is on");
        assert!(w.updates.effects.iter().any(|e| matches!(e, Effect::Sound { sound, .. } if sound.ends_with("button.click_on"))));
        run(&mut w, ticks - 1);
        assert!(get_bool(get(&w, p(1, Y, 0)), "powered"));
        run(&mut w, 1);
        assert!(!get_bool(get(&w, p(1, Y, 0)), "powered"), "{button} released after {ticks} ticks");
        run(&mut w, 4);
        assert!(!get_bool(get(&w, p(-1, Y, 0)), "lit"));
    }
}

#[test]
fn lever_opens_doors_and_lights_lamps() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("oak_door[half=lower,facing=north]"));
    set(&mut w, p(0, Y + 1, 0), s("oak_door[half=upper,facing=north]"));
    set(&mut w, p(2, Y, 0), s("redstone_lamp"));
    set(&mut w, p(1, Y, 0), s("lever[face=floor,facing=north]"));
    use_block(&mut w, p(1, Y, 0));
    for y in [Y, Y + 1] {
        assert!(get_bool(get(&w, p(0, y, 0)), "open"), "both halves open");
        assert!(get_bool(get(&w, p(0, y, 0)), "powered"));
    }
    assert!(get_bool(get(&w, p(2, Y, 0)), "lit"), "the lamp lights at once");
    use_block(&mut w, p(1, Y, 0));
    assert!(!get_bool(get(&w, p(0, Y, 0)), "open"));
    assert!(!get_bool(get(&w, p(0, Y + 1, 0)), "open"));
    run(&mut w, 3);
    assert!(get_bool(get(&w, p(2, Y, 0)), "lit"), "lamps stay on 4 ticks");
    run(&mut w, 1);
    assert!(!get_bool(get(&w, p(2, Y, 0)), "lit"));
}

#[test]
fn doors_trapdoors_and_gates_open_by_hand_except_iron() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("oak_door[half=lower]"));
    set(&mut w, p(0, Y + 1, 0), s("oak_door[half=upper]"));
    assert_eq!(use_block(&mut w, p(0, Y + 1, 0)), Interaction::Success);
    assert!(get_bool(get(&w, p(0, Y, 0)), "open"), "the other half follows");
    set(&mut w, p(3, Y, 0), s("iron_door[half=lower]"));
    set(&mut w, p(3, Y + 1, 0), s("iron_door[half=upper]"));
    assert_eq!(use_block(&mut w, p(3, Y, 0)), Interaction::Pass);
    set(&mut w, p(5, Y, 0), s("spruce_trapdoor"));
    use_block(&mut w, p(5, Y, 0));
    assert!(get_bool(get(&w, p(5, Y, 0)), "open"));
    set(&mut w, p(7, Y, 0), s("oak_fence_gate[facing=south]"));
    use_block(&mut w, p(7, Y, 0));
    assert!(get_bool(get(&w, p(7, Y, 0)), "open"));
    assert_eq!(prop(get(&w, p(7, Y, 0)), "facing"), Some("north"), "opens away from the player");
    // Breaking the lower half of a door removes the upper half.
    set(&mut w, p(0, Y, 0), blocks::AIR);
    assert_eq!(name(&w, p(0, Y + 1, 0)), "air");
}

#[test]
fn note_block_plays_on_a_rising_edge() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("note_block"));
    set(&mut w, p(1, Y, 0), s("lever[face=floor]"));
    use_block(&mut w, p(1, Y, 0));
    assert!(get_bool(get(&w, p(0, Y, 0)), "powered"));
    w.updates.effects.clear();
    run(&mut w, 1);
    assert!(w.updates.effects.iter().any(|e| matches!(e, Effect::BlockEvent { .. })), "block event for clients");
    assert!(w.updates.effects.iter().any(|e| matches!(e, Effect::Sound { sound, .. } if sound == "minecraft:block.note_block.harp")));
    // On stone the instrument is the bass drum.
    set(&mut w, p(4, Y - 1, 0), s("stone"));
    set(&mut w, p(4, Y, 0), s("note_block"));
    let level = Level::new(&mut w);
    assert_eq!(redstone::note_instrument(&level, p(4, Y, 0)), "basedrum");
}

#[test]
fn ticks_wait_for_unticking_chunks() {
    let mut w = world();
    set(&mut w, p(0, Y, 0), s("water"));
    for _ in 0..20 {
        w.tick_time();
        tick(&mut w, &|_| false);
    }
    assert_eq!(name(&w, p(1, Y, 0)), "air", "chunks that do not tick keep their ticks");
    run(&mut w, 1);
    assert!(water_level(&w, p(1, Y, 0)).is_some(), "overdue ticks run once the chunk ticks");
}

