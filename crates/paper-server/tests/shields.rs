//! Shields: a raised shield takes hits from the front whole and wears, not those from
//! behind; an axe disables it for five seconds, during which it cannot be raised.

mod common;

use common::*;
use paper_data::items;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play;
use paper_protocol::types::Slot;
use paper_server::config::GameMode;
use paper_server::server::survival::Damage;
use paper_server::world::entity::Vec3;
use std::time::Duration;

fn item(name: &str) -> i32 {
    items::id(name).unwrap() as i32
}

#[test]
fn a_raised_shield_blocks_from_the_front_until_an_axe_disables_it() {
    let server = start_server(|c| c.properties.gamemode = GameMode::Survival);
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    let mut bob = Client::join(server.port(), "Bob");
    bob.expect::<play::PlayerPosition>();
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();
    let (id, bob_id) = (alice.entity_id, bob.entity_id);
    server
        .handle
        .run(move |s| {
            s.set_inventory_slot(id, 45, Slot::of(item("minecraft:shield"), 1));
            let p = s.players.get_mut(&id).unwrap();
            p.loc.yaw = 0.0;
        })
        .unwrap();
    // Raised with the off hand; it blocks after a quarter second.
    alice.send(&play::UseItem { hand: VarInt(1), sequence: VarInt(1), yaw: 0.0, pitch: 0.0 });
    std::thread::sleep(Duration::from_millis(500));
    let blocking = server.handle.run(move |s| s.blocking_slot(id)).unwrap();
    assert_eq!(blocking, Some(45), "blocking with the off hand");

    // Bob hits from the south, where she looks: nothing gets through, the shield wears.
    let hit = move |s: &mut paper_server::server::Server, at: Vec3| {
        let damage = Damage { kind: "minecraft:player_attack", attacker: Some(bob_id), direct: Some(bob_id), position: Some(at) };
        let landed = s.hurt_player(id, damage, 5.0);
        (landed, s.players[&id].health)
    };
    let front = Vec3::new(8.5, -60.0, 12.0);
    let (landed, health) = server.handle.run(move |s| hit(s, front)).unwrap();
    assert!(!landed && health == 20.0, "blocked: landed {landed}, health {health}");
    let worn = server.handle.run(move |s| paper_server::server::survival::item_damage(s.players[&id].inventory[45].0.as_ref().unwrap())).unwrap();
    assert_eq!(worn, 6, "a point and one per point blocked");

    // From behind it lands.
    let (landed, health) = server.handle.run(move |s| hit(s, Vec3::new(8.5, -60.0, 5.0))).unwrap();
    assert!(landed && health < 20.0, "from behind: landed {landed}, health {health}");

    // An axe disables it for five seconds, and it is lowered.
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&id).unwrap().invulnerable_time = 0;
            s.set_inventory_slot(bob_id, 36, Slot::of(item("minecraft:iron_axe"), 1));
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    alice.clear_backlog();
    let (landed, _) = server.handle.run(move |s| hit(s, front)).unwrap();
    assert!(!landed, "the axe blow is still blocked");
    let cooldown: play::Cooldown = alice.find(TIMEOUT, |c: &play::Cooldown| c.group == "minecraft:shield").expect("the shield cools down");
    assert_eq!(cooldown.duration.0, 100);
    let using = server.handle.run(move |s| s.players[&id].using_item).unwrap();
    assert!(using.is_none(), "the shield is lowered");
    alice.send(&play::UseItem { hand: VarInt(1), sequence: VarInt(2), yaw: 0.0, pitch: 0.0 });
    std::thread::sleep(Duration::from_millis(300));
    let using = server.handle.run(move |s| s.players[&id].using_item).unwrap();
    assert!(using.is_none(), "it cannot be raised while cooling down");
    server.stop();
}
