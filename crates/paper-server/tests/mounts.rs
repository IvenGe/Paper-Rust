//! Mounts' inventories (`HorseInventoryMenu`): a tamed horse's saddle and armour, and the
//! chests donkeys, mules and llamas carry, opened by sneaking or from the saddle, and saved
//! with them.

mod common;

use common::*;
use paper_data::items;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use paper_protocol::types::{HashedSlot, LpVec3, Slot};
use paper_server::config::GameMode;
use paper_server::server::mobs::SpawnReason;
use paper_server::world::entity::{Mob, Vec3};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(300);

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("paper-rs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Waits while reading what the server sends, so keep-alives are answered.
fn wait_reading(client: &mut Client, what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        client.drain(Duration::from_millis(50));
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

fn sneak(server: &paper_server::RunningServer, id: i32, on: bool) {
    server.handle.run(move |s| s.players.get_mut(&id).unwrap().sneaking = on).unwrap();
}

fn interact(entity: i32) -> play::Interact {
    play::Interact { entity_id: VarInt(entity), hand: VarInt(0), target: LpVec3(0.0, 1.0, 0.0), sneaking: false }
}

/// A shift-click (mode 1) on a window slot.
fn shift_click(window_id: i32, slot: i16) -> play::ContainerClick {
    play::ContainerClick { window_id, state_id: 0, slot, button: 0, mode: 1, changed_slots: Vec::new(), carried: HashedSlot(None) }
}

/// A tamed mob of the kind beside Alice.
fn tamed(server: &paper_server::RunningServer, name: &'static str) -> i32 {
    server
        .handle
        .run(move |s| {
            let id = s.spawn_mob(0, name, Vec3::new(10.5, -60.0, 8.5), SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&id).unwrap().mob_mut().unwrap().tamed = true;
            id
        })
        .unwrap()
}

fn mob<T: Send + 'static>(server: &paper_server::RunningServer, id: i32, f: impl Fn(&Mob) -> T + Send + Sync + 'static) -> T {
    server.handle.run(move |s| f(s.worlds[0].entities[&id].mob().unwrap())).unwrap()
}

#[test]
fn a_tamed_horse_opens_its_inventory_for_a_saddle_and_armour() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let (mut alice, id) = join(&server);
    let horse = tamed(&server, "minecraft:horse");
    hold(&server, id, Slot::of(item("minecraft:saddle"), 1));
    sneak(&server, id, true);
    alice.send(&interact(horse));
    let screen: play::MountScreenOpen = alice.expect();
    assert_eq!((screen.inventory_columns.0, screen.entity_id), (0, horse), "a horse has no chest");
    // Saddle, armour, then the inventory: the first hotbar slot is window slot 2 + 27.
    let window = screen.container_id.0;
    alice.send(&shift_click(window, 29));
    let saddle = Slot::of(item("minecraft:saddle"), 1);
    wait_reading(&mut alice, "the saddle on", || mob(&server, horse, |m| m.equipment[7].clone()) == saddle);
    assert!(held(&server, id).is_empty());
    hold(&server, id, Slot::of(item("minecraft:iron_horse_armor"), 1));
    alice.send(&shift_click(window, 29));
    let armour = Slot::of(item("minecraft:iron_horse_armor"), 1);
    wait_reading(&mut alice, "the armour on", || mob(&server, horse, |m| m.equipment[6].clone()) == armour);
    // A carpet is for llamas: it stays in the inventory.
    hold(&server, id, Slot::of(item("minecraft:white_carpet"), 1));
    alice.send(&shift_click(window, 29));
    wait_reading(&mut alice, "the carpet moved up", || held(&server, id).is_empty());
    assert_eq!(mob(&server, horse, |m| m.equipment[6].clone()), Slot::of(item("minecraft:iron_horse_armor"), 1));
    server.stop();
}

#[test]
fn donkeys_carry_chests_and_keep_their_items() {
    let root = temp_root("mounts");
    let config = |c: &mut paper_server::config::Config| {
        c.root = root.clone();
        c.properties.gamemode = GameMode::Survival;
    };
    let server = start_server(config);
    let (mut alice, id) = join(&server);
    let donkey = tamed(&server, "minecraft:donkey");
    hold(&server, id, Slot::of(item("minecraft:chest"), 1));
    alice.send(&interact(donkey));
    wait_reading(&mut alice, "a chest on the donkey", || mob(&server, donkey, |m| m.chest.as_ref().map(Vec::len)) == Some(15));
    assert!(held(&server, id).is_empty(), "the chest is used up");

    // Its window has the chest's five columns; dirt shift-clicked from the hotbar goes in
    // (the first hotbar slot is window slot 2 + 15 + 27).
    hold(&server, id, Slot::of(item("minecraft:dirt"), 10));
    sneak(&server, id, true);
    alice.send(&interact(donkey));
    let screen: play::MountScreenOpen = alice.find(TIMEOUT, |_: &play::MountScreenOpen| true).expect("the donkey's window");
    assert_eq!((screen.inventory_columns.0, screen.entity_id), (5, donkey));
    alice.send(&shift_click(screen.container_id.0, 44));
    let dirt = Slot::of(item("minecraft:dirt"), 10);
    let first = move |m: &Mob| m.chest.as_ref().map(|c| c[0].clone());
    wait_reading(&mut alice, "the dirt in the chest", || mob(&server, donkey, first) == Some(dirt.clone()));
    server.stop();

    // After a restart the donkey still carries the chest and the dirt.
    let server = start_server(config);
    let (mut alice, _) = join(&server);
    let chested = |s: &mut paper_server::server::Server| s.worlds[0].entities.values().filter(|e| e.info.name == "minecraft:donkey").find_map(|e| e.mob().and_then(|m| m.chest.as_ref().map(|c| (c.len(), c[0].clone()))));
    wait_reading(&mut alice, "the donkey back", || server.handle.run(chested).unwrap().is_some());
    assert_eq!(server.handle.run(chested).unwrap(), Some((15, Slot::of(item("minecraft:dirt"), 10))));
    server.stop();
}

#[test]
fn riders_open_their_mounts_inventory() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let (mut alice, id) = join(&server);
    let horse = tamed(&server, "minecraft:horse");
    alice.send(&interact(horse));
    wait_reading(&mut alice, "Alice on the horse", || server.handle.run(move |s| s.players[&id].vehicle).unwrap() == Some(horse));
    alice.send(&play::PlayerCommand { entity_id: VarInt(id), action: VarInt(play::player_command::OPEN_VEHICLE_INVENTORY), jump_boost: VarInt(0) });
    let screen: play::MountScreenOpen = alice.find(TIMEOUT, |_: &play::MountScreenOpen| true).expect("the horse's window");
    assert_eq!(screen.entity_id, horse);
    server.stop();
}
