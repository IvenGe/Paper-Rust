//! Bottles and buckets: dragon's breath bottled from the dragon's clouds, fish scooped up in
//! water buckets and let out again as they were, and spawn eggs used on water.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::text::Component;
use paper_protocol::types::{LpVec3, Slot};
use paper_server::config::GameMode;
use paper_server::server::mobs::SpawnReason;
use paper_server::world::entity::{Entity, EntityKind, Vec3};
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(300);

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

/// Waits while reading what the server sends, so keep-alives are answered.
fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(50));
        client.clear_backlog();
    }
}

fn join(server: &paper_server::RunningServer) -> (Client, i32) {
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    alice.clear_backlog();
    let id = alice.entity_id;
    (alice, id)
}

fn hold(server: &paper_server::RunningServer, id: i32, stack: Slot) {
    server.handle.run(move |s| s.set_inventory_slot(id, 36, stack)).unwrap();
}

fn held(server: &paper_server::RunningServer, id: i32) -> Slot {
    server.handle.run(move |s| s.players[&id].inventory[36].clone()).unwrap()
}

/// Uses the held item looking straight down (at the block under Alice's feet).
fn use_down(alice: &mut Client, sequence: i32) {
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(sequence), yaw: 0.0, pitch: 90.0 });
}

fn survival() -> impl FnOnce(&mut paper_server::config::Config) {
    |c| c.properties.gamemode = GameMode::Survival
}

#[test]
fn dragons_breath_is_bottled_from_the_dragons_clouds() {
    let server = start_server(survival());
    let (mut alice, id) = join(&server);
    hold(&server, id, Slot::of(item("minecraft:glass_bottle"), 2));
    // A cloud of the dragon's within reach (two blocks around Alice), but not around her.
    let cloud = server
        .handle
        .run(|s| {
            let info = paper_data::entity_types::by_name("minecraft:area_effect_cloud").unwrap();
            let kind = EntityKind::Cloud { potion: String::new(), owner: None, radius: 1.0, duration: 600, wait: 0, uses: Vec::new(), growth: 0.0, on_use: 0.0, breath: true };
            let e = Entity::new(s.next_entity_id(), info, Vec3::new(11.0, -60.0, 8.5), kind);
            s.spawn_entity(0, e)
        })
        .unwrap();
    use_down(&mut alice, 1);
    let breath = item("minecraft:dragon_breath");
    wait_reading(&mut alice, "a bottle of dragon's breath", || server.handle.run(move |s| s.players[&id].inventory.iter().any(|i| i.0.as_ref().is_some_and(|i| i.item == breath))).unwrap());
    assert_eq!(held(&server, id), Slot::of(item("minecraft:glass_bottle"), 1), "one bottle filled");
    let radius = server.handle.run(move |s| s.worlds[0].entities.get(&cloud).map(|e| match &e.kind { EntityKind::Cloud { radius, .. } => *radius, _ => 0.0 })).unwrap();
    assert_eq!(radius, Some(0.5), "the cloud shrinks");
    server.stop();
}

#[test]
fn fish_go_into_water_buckets_and_come_out_again() {
    let server = start_server(survival());
    let (mut alice, id) = join(&server);
    let cod = server
        .handle
        .run(|s| {
            let cod = s.spawn_mob(0, "minecraft:cod", Vec3::new(10.5, -60.0, 8.5), SpawnReason::Command).unwrap();
            let e = s.worlds[0].entities.get_mut(&cod).unwrap();
            e.custom_name = Some(Component::text("Fishy".to_owned()));
            e.mob_mut().unwrap().living.health = 2.0;
            cod
        })
        .unwrap();
    hold(&server, id, Slot::of(item("minecraft:water_bucket"), 1));
    alice.send(&play::Interact { entity_id: VarInt(cod), hand: VarInt(0), target: LpVec3(0.0, 0.2, 0.0), sneaking: false });
    let bucket = item("minecraft:cod_bucket");
    wait_reading(&mut alice, "the cod in the bucket", || held(&server, id).0.as_ref().is_some_and(|s| s.item == bucket));
    assert!(server.handle.run(move |s| s.worlds[0].entities.get(&cod).is_none_or(|e| e.removed)).unwrap(), "the cod is gone");

    // Emptied at Alice's feet: water, and the cod as it went in, which now stays.
    use_down(&mut alice, 2);
    let let_out = |s: &mut paper_server::server::Server| s.worlds[0].entities.values().find(|e| e.info.name == "minecraft:cod" && !e.removed).map(|e| (e.custom_name.as_ref().map(|n| n.to_plain()), e.mob().unwrap().living.health, e.mob().unwrap().persistent));
    wait_reading(&mut alice, "the cod let out", || server.handle.run(move |s| let_out(s).is_some()).unwrap());
    assert_eq!(server.handle.run(move |s| let_out(s)).unwrap(), Some((Some("Fishy".to_owned()), 2.0, true)));
    assert_eq!(held(&server, id), Slot::of(item("minecraft:bucket"), 1));
    let water = server.handle.run(|s| s.worlds[0].block_at(8, -60, 8)).unwrap();
    assert_eq!(water.map(|b| blocks::block_of_state(b).name), Some("minecraft:water"));
    server.stop();
}

#[test]
fn axolotls_keep_their_variant_in_the_bucket() {
    let server = start_server(survival());
    let (mut alice, id) = join(&server);
    // A cyan axolotl (variant 3).
    let axolotl = server
        .handle
        .run(|s| {
            let a = s.spawn_mob(0, "minecraft:axolotl", Vec3::new(10.5, -60.0, 8.5), SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&a).unwrap().mob_mut().unwrap().variant = 3;
            a
        })
        .unwrap();
    hold(&server, id, Slot::of(item("minecraft:water_bucket"), 1));
    alice.send(&play::Interact { entity_id: VarInt(axolotl), hand: VarInt(0), target: LpVec3(0.0, 0.2, 0.0), sneaking: false });
    let bucket = item("minecraft:axolotl_bucket");
    wait_reading(&mut alice, "the axolotl in the bucket", || held(&server, id).0.as_ref().is_some_and(|s| s.item == bucket));
    // As vanilla keeps it: the `minecraft:axolotl/variant` component.
    let ty = paper_data::registries::id("minecraft:data_component_type", "minecraft:axolotl/variant").unwrap() as i32;
    let stack = held(&server, id).0.unwrap();
    assert_eq!(stack.added.iter().find(|(t, _)| *t == ty).map(|(_, bytes)| bytes.clone()), Some(vec![3]));
    use_down(&mut alice, 2);
    let let_out = |s: &mut paper_server::server::Server| s.worlds[0].entities.values().find(|e| e.info.name == "minecraft:axolotl" && !e.removed).map(|e| e.mob().unwrap().variant);
    wait_reading(&mut alice, "the axolotl let out", || server.handle.run(let_out).unwrap().is_some());
    assert_eq!(server.handle.run(let_out).unwrap(), Some(3));
    server.stop();
}

#[test]
fn spawn_eggs_work_on_water() {
    let server = start_server(survival());
    let (mut alice, id) = join(&server);
    let water = blocks::default_state("minecraft:water").unwrap();
    server.handle.run(move |s| s.set_block_and_update(0, BlockPos::new(8, -61, 8), water)).unwrap();
    hold(&server, id, Slot::of(item("minecraft:squid_spawn_egg"), 1));
    use_down(&mut alice, 1);
    let squid = |s: &mut paper_server::server::Server| s.worlds[0].entities.values().any(|e| e.info.name == "minecraft:squid" && (e.pos.x - 8.5).abs() < 1.0 && (e.pos.z - 8.5).abs() < 1.0 && (e.pos.y + 61.0).abs() < 1.0);
    wait_reading(&mut alice, "a squid in the water", || server.handle.run(squid).unwrap());
    assert!(held(&server, id).is_empty(), "the egg is used up");
    assert_eq!(server.handle.run(move |s| s.players[&id].stats.get("minecraft:used", "minecraft:squid_spawn_egg")).unwrap(), 1);
    server.stop();
}
