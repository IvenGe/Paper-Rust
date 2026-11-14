//! Villagers and wandering traders: a workstation gives a villager its profession, right
//! clicking opens the trading window with its offers, picking a trade fills the payment,
//! taking the result pays for it, villagers gain experience and levels, and lose a job
//! nobody traded for when the workstation goes.

mod common;

use common::*;
use paper_data::{blocks, items};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play;
use paper_protocol::types::{HashedSlot, ItemStack, LpVec3, Slot};
use paper_server::config::{Difficulty, GameMode};
use paper_server::server::mobs::SpawnReason;
use paper_server::server::player::Location;
use paper_server::world::entity::Vec3;
use paper_server::world::merchant::{ItemCost, Merchant, Offer};
use std::time::{Duration, Instant};

fn server() -> paper_server::RunningServer {
    let server = start_server(|c| {
        c.properties.gamemode = GameMode::Survival;
        c.properties.difficulty = Difficulty::Peaceful;
    });
    server
        .handle
        .run(|s| {
            use paper_data::game_rules::Value;
            s.set_game_rule(0, "respawn_radius", Value::Int(0));
            s.set_game_rule(0, "spawn_mobs", Value::Bool(false));
            s.set_game_rule(0, "spawn_wandering_traders", Value::Bool(false));
        })
        .unwrap();
    server
}

fn join(server: &paper_server::RunningServer, name: &str) -> Client {
    let mut client = Client::join(server.port(), name);
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(Duration::from_millis(300));
    client.clear_backlog();
    client
}

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn merchant(server: &paper_server::RunningServer, id: i32) -> Option<Merchant> {
    server.handle.run(move |s| s.worlds[0].entities.get(&id).and_then(|e| e.mob()).and_then(|m| m.merchant.as_deref().cloned())).unwrap()
}

fn with_merchant(server: &paper_server::RunningServer, id: i32, f: impl FnOnce(&mut Merchant) + Send + 'static) {
    server.handle.run(move |s| f(s.worlds[0].entities.get_mut(&id).unwrap().mob_mut().unwrap().merchant.as_deref_mut().unwrap())).unwrap();
}

fn interact(entity: i32) -> play::Interact {
    play::Interact { entity_id: VarInt(entity), hand: VarInt(0), target: LpVec3(0.0, 1.0, 0.0), sneaking: false }
}

fn click(window_id: i32, slot: i16, button: i8, mode: i32) -> play::ContainerClick {
    play::ContainerClick { window_id, state_id: 0, slot, button, mode, changed_slots: Vec::new(), carried: HashedSlot(None) }
}

/// Puts the player next to an entity, looking at it.
fn stand_next_to(server: &paper_server::RunningServer, player: i32, entity: i32) {
    server
        .handle
        .run(move |s| {
            let at = s.worlds[0].entities[&entity].pos;
            s.teleport(player, Location { x: at.x + 1.5, y: at.y, z: at.z, yaw: 90.0, pitch: 0.0 });
        })
        .unwrap();
}

fn count_of(server: &paper_server::RunningServer, player: i32, item: i32) -> i32 {
    server.handle.run(move |s| s.players[&player].inventory.iter().filter_map(|i| i.0.as_ref()).filter(|i| i.item == item).map(|i| i.count).sum()).unwrap()
}

#[test]
fn villagers_take_workstations_trade_and_level_up() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let (wheat, emerald, bread) = (item("minecraft:wheat"), item("minecraft:emerald"), item("minecraft:bread"));

    // An unemployed villager walks to a free composter and becomes a farmer.
    let composter = BlockPos::new(9, -60, 3);
    let villager = server
        .handle
        .run(move |s| {
            s.set_block_and_update(0, composter, blocks::default_state("minecraft:composter").unwrap());
            s.spawn_mob(0, "minecraft:villager", Vec3::new(3.5, -60.0, 3.5), SpawnReason::Command).unwrap()
        })
        .unwrap();
    let born = merchant(&server, villager).unwrap();
    assert_eq!((born.villager_type.as_str(), born.profession.as_str(), born.level), ("minecraft:plains", "minecraft:none", 1));
    wait_until("the villager to become a farmer", Duration::from_secs(40), || merchant(&server, villager).is_some_and(|m| m.profession == "minecraft:farmer" && m.job_site == Some(composter)));
    // Clients see the new clothes: villager data (plains = 2, farmer = 5, level 1).
    alice.find(TIMEOUT, |m: &play::SetEntityData| m.entity_id.0 == villager && m.metadata.0.windows(5).any(|w| w == [19, 18, 2, 5, 1])).expect("the farmer's villager data");

    // Known offers make the rest repeatable.
    with_merchant(&server, villager, move |m| {
        m.offers = Some(vec![
            Offer::new(ItemCost::new(wheat, 20), None, ItemStack::new(emerald, 1), 16, 2, 0.05),
            Offer::new(ItemCost::new(emerald, 1), None, ItemStack::new(bread, 6), 16, 1, 0.05),
        ]);
    });
    server.handle.run(move |s| s.set_inventory_slot(id, 36, Slot::of(wheat, 64))).unwrap();
    stand_next_to(&server, id, villager);
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();

    // Right-click: the trading window and its offers.
    alice.send(&interact(villager));
    let merchant_menu = paper_data::registries::id("minecraft:menu", "minecraft:merchant").unwrap() as i32;
    let screen = alice.find(TIMEOUT, |o: &play::OpenScreen| o.menu_type.0 == merchant_menu).expect("the trading window");
    assert_eq!(screen.title.to_plain(), "entity.minecraft.villager.farmer");
    let window = screen.window_id.0;
    let offers = alice.find(TIMEOUT, |o: &play::MerchantOffers| o.window_id == window).expect("the offers");
    assert_eq!((offers.offers.len(), offers.level, offers.xp, offers.show_progress, offers.can_restock), (2, 1, 0, true, true));
    assert_eq!((offers.offers[0].cost_a.item, offers.offers[0].cost_a.count, offers.offers[0].result.clone()), (wheat, 20, Slot::of(emerald, 1)));
    assert_eq!(merchant(&server, villager).unwrap().trading_player, Some(id));

    // Picking the first trade moves the wheat into the payment slot.
    alice.send(&play::SelectTrade { item: VarInt(0) });
    alice.find(TIMEOUT, |c: &play::ContainerSetContent| c.window_id == window && c.slots[0] == Slot::of(wheat, 64) && c.slots[2] == Slot::of(emerald, 1)).expect("payment and result");

    // Shift-clicking the result trades as often as the payment allows: three times.
    alice.send(&click(window, 2, 0, 1));
    wait_until("three trades", TIMEOUT, || count_of(&server, id, emerald) == 3);
    let m = merchant(&server, villager).unwrap();
    assert_eq!((m.xp, m.offers.as_ref().unwrap()[0].uses), (6, 3));
    let alice_uuid = server.handle.run(move |s| s.players[&id].uuid()).unwrap();
    // The wheat left in the payment slot does not pay for a fourth.
    let payment = server.handle.run(move |s| s.players[&id].open_menu.as_ref().unwrap().craft_grid.clone()).unwrap();
    assert_eq!((payment[0].clone(), payment[2].clone()), (Slot::of(wheat, 4), Slot::EMPTY));
    wait_until("gossip about the trades", TIMEOUT, || merchant(&server, villager).is_some_and(|m| m.reputation(alice_uuid) > 0));

    // Closing gives the payment back and ends the trading.
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(window) });
    wait_until("the window to close", TIMEOUT, || merchant(&server, villager).is_some_and(|m| m.trading_player.is_none()));
    assert_eq!(count_of(&server, id, wheat), 4);

    // Enough experience for the next level: it comes a little after the trading ends, with
    // new trades from the farmer's level 2 trades.
    with_merchant(&server, villager, |m| m.xp = 9);
    server.handle.run(move |s| s.set_inventory_slot(id, 36, Slot::of(wheat, 20))).unwrap();
    alice.clear_backlog();
    alice.send(&interact(villager));
    let window = alice.find(TIMEOUT, |o: &play::OpenScreen| o.menu_type.0 == merchant_menu).expect("the trading window again").window_id.0;
    alice.send(&play::SelectTrade { item: VarInt(0) });
    alice.find(TIMEOUT, |c: &play::ContainerSetContent| c.window_id == window && c.slots[2] == Slot::of(emerald, 1)).expect("result");
    alice.send(&click(window, 2, 0, 0));
    wait_until("the trade", TIMEOUT, || merchant(&server, villager).is_some_and(|m| m.xp == 11 && m.level_up_pending));
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(window) });
    wait_until("level 2", TIMEOUT, || merchant(&server, villager).is_some_and(|m| m.level == 2));
    let m = merchant(&server, villager).unwrap();
    assert_eq!(m.offers.as_ref().unwrap().len(), 4, "two new trades");
    alice.find(TIMEOUT, |d: &play::SetEntityData| d.entity_id.0 == villager && d.metadata.0.windows(5).any(|w| w == [19, 18, 2, 5, 2])).expect("level 2 villager data");

    // A villager that traded keeps its profession when its workstation goes.
    server.handle.run(move |s| s.set_block_and_update(0, composter, blocks::AIR)).unwrap();
    wait_until("the job site to be lost", TIMEOUT, || merchant(&server, villager).is_some_and(|m| m.job_site.is_none()));
    assert_eq!(merchant(&server, villager).unwrap().profession, "minecraft:farmer");
    server.stop();
}

#[test]
fn unemployed_villagers_shake_their_heads_and_new_hires_quit() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let villager = server.handle.run(|s| s.spawn_mob(0, "minecraft:villager", Vec3::new(4.5, -60.0, 4.5), SpawnReason::Command).unwrap()).unwrap();
    // Nothing to trade: no window, just a head shake.
    stand_next_to(&server, id, villager);
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();
    alice.send(&interact(villager));
    wait_until("the head shake", TIMEOUT, || merchant(&server, villager).is_some_and(|m| m.unhappy > 0));
    alice.drain(Duration::from_millis(300));
    assert!(!alice.has::<play::OpenScreen>());

    // A lectern right next to it makes it a librarian; breaking it before anyone traded
    // makes it unemployed again.
    let lectern = server
        .handle
        .run(move |s| {
            let at = s.worlds[0].entities[&villager].pos;
            let pos = BlockPos::new(at.x.floor() as i32 + 1, at.y.floor() as i32, at.z.floor() as i32);
            s.set_block_and_update(0, pos, blocks::default_state("minecraft:lectern").unwrap());
            pos
        })
        .unwrap();
    wait_until("a librarian", Duration::from_secs(30), || merchant(&server, villager).is_some_and(|m| m.profession == "minecraft:librarian" && m.job_site == Some(lectern)));
    server.handle.run(move |s| s.set_block_and_update(0, lectern, blocks::AIR)).unwrap();
    wait_until("unemployed again", TIMEOUT, || merchant(&server, villager).is_some_and(|m| m.profession == "minecraft:none" && m.offers.is_none()));
    server.stop();
}

#[test]
fn wandering_traders_sell_and_leave() {
    let server = server();
    let mut alice = join(&server, "Alice");
    let id = alice.entity_id;
    let trader = server.handle.run(|s| s.spawn_mob(0, "minecraft:wandering_trader", Vec3::new(4.5, -60.0, 4.5), SpawnReason::Command).unwrap()).unwrap();
    // Its offers: two it buys, five common and two uncommon ones it sells.
    let offers = server.handle.run(move |s| s.merchant_offers(0, trader)).unwrap();
    assert_eq!(offers.len(), 9);
    stand_next_to(&server, id, trader);
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();
    alice.send(&interact(trader));
    let screen = alice.find(TIMEOUT, |o: &play::OpenScreen| o.title.to_plain() == "entity.minecraft.wandering_trader").expect("the trading window");
    assert_eq!(merchant(&server, trader).unwrap().trading_player, Some(id));
    // Walking away closes it.
    server.handle.run(move |s| s.teleport(id, Location { x: 40.5, y: -60.0, z: 40.5, yaw: 0.0, pitch: 0.0 })).unwrap();
    alice.find(TIMEOUT, |c: &play::ContainerClose| c.window_id.0 == screen.window_id.0).expect("the window to close");
    wait_until("the trader to be free", TIMEOUT, || merchant(&server, trader).is_some_and(|m| m.trading_player.is_none()));

    // Traders leave when their time is up.
    with_merchant(&server, trader, |m| m.despawn_delay = 5);
    wait_until("the trader to leave", TIMEOUT, || server.handle.run(move |s| !s.worlds[0].entities.contains_key(&trader)).unwrap());
    server.stop();
}
