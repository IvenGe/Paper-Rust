//! World borders: `/worldborder` changes the border of the source's world for the players in
//! it, with vanilla's messages and limits; a player with a border of their own (a plugin's
//! virtual border) keeps seeing that one.

mod common;

use common::*;
use paper_protocol::packets::play;
use std::time::Duration;

fn join(server: &paper_server::RunningServer, name: &str) -> Client {
    let mut client = Client::join(server.port(), name);
    client.expect::<play::PlayerPosition>();
    client.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    client.drain(Duration::from_millis(300));
    client.clear_backlog();
    client
}

/// The next translated reply: its key and arguments as text.
fn reply(client: &mut Client) -> (String, Vec<String>) {
    let chat: play::SystemChat = client.find(TIMEOUT, |c: &play::SystemChat| c.content.0.get("translate").is_some()).expect("a reply");
    let json = &chat.content.0;
    let args = json["with"]
        .as_array()
        .map(|args| args.iter().map(|a| a.as_str().map_or_else(|| a["text"].as_str().unwrap_or_default().to_owned(), str::to_owned)).collect())
        .unwrap_or_default();
    (json["translate"].as_str().unwrap_or_default().to_owned(), args)
}

fn run(client: &mut Client, command: &str) -> (String, Vec<String>) {
    client.command(command);
    reply(client)
}

fn key(key: &str, args: &[&str]) -> (String, Vec<String>) {
    (key.to_owned(), args.iter().map(|a| (*a).to_owned()).collect())
}

#[test]
fn worldborder_command_moves_the_border_for_players_who_see_it() {
    let server = start_server(|_| {});
    let mut alice = join(&server, "Alice");
    server.handle.console_command("op Alice");
    alice.drain(Duration::from_millis(300));
    alice.clear_backlog();

    assert_eq!(run(&mut alice, "worldborder get"), key("commands.worldborder.get", &["59999968"]));
    assert_eq!(run(&mut alice, "worldborder set 100"), key("commands.worldborder.set.immediate", &["100.0"]));
    let size: play::SetBorderSize = alice.find(TIMEOUT, |_: &play::SetBorderSize| true).expect("the new size");
    assert_eq!(size.size, 100.0);
    assert_eq!(run(&mut alice, "worldborder set 100"), key("commands.worldborder.set.failed.nochange", &[]));
    assert_eq!(run(&mut alice, "worldborder set 0.5"), key("commands.worldborder.set.failed.small", &[]));
    assert_eq!(run(&mut alice, "worldborder add 59999900"), key("commands.worldborder.set.failed.big", &["5.9999968E7"]));

    // Growing over ten seconds: clients move it themselves, the server ticks its own.
    assert_eq!(run(&mut alice, "worldborder add 50 10"), key("commands.worldborder.set.grow", &["150.0", "10"]));
    let lerp: play::SetBorderLerpSize = alice.find(TIMEOUT, |_: &play::SetBorderLerpSize| true).expect("the move");
    assert_eq!((lerp.old_size, lerp.new_size, lerp.lerp_time.0), (100.0, 150.0, 200));
    pause(&mut alice, Duration::from_millis(500));
    let (size, left) = server.handle.run(|s| (s.worlds[0].border.size, s.worlds[0].border.lerp_left)).unwrap();
    assert!(size > 100.0 && size < 150.0 && left > 0 && left < 200, "size {size}, {left} ticks left");
    assert_eq!(run(&mut alice, "worldborder set 80 5"), key("commands.worldborder.set.shrink", &["80.0", "5"]));

    // Whole numbers are block centers, as vanilla's `Vec2Argument`.
    assert_eq!(run(&mut alice, "worldborder center 10 -20"), key("commands.worldborder.center.success", &["10.50", "-19.50"]));
    let center: play::SetBorderCenter = alice.find(TIMEOUT, |_: &play::SetBorderCenter| true).expect("the new center");
    assert_eq!((center.center_x, center.center_z), (10.5, -19.5));
    assert_eq!(run(&mut alice, "worldborder center 10 -20"), key("commands.worldborder.center.failed", &[]));
    assert_eq!(run(&mut alice, "worldborder center 30000000 0"), key("commands.worldborder.set.failed.far", &["2.9999984E7"]));

    assert_eq!(run(&mut alice, "worldborder damage amount 1"), key("commands.worldborder.damage.amount.success", &["1.00"]));
    assert_eq!(run(&mut alice, "worldborder damage amount 1"), key("commands.worldborder.damage.amount.failed", &[]));
    assert_eq!(run(&mut alice, "worldborder damage buffer 2.5"), key("commands.worldborder.damage.buffer.success", &["2.50"]));
    assert_eq!(run(&mut alice, "worldborder warning distance 8"), key("commands.worldborder.warning.distance.success", &["8"]));
    let warning: play::SetBorderWarningDistance = alice.find(TIMEOUT, |_: &play::SetBorderWarningDistance| true).expect("the warning distance");
    assert_eq!(warning.warning_blocks.0, 8);
    assert_eq!(run(&mut alice, "worldborder warning time 30"), key("commands.worldborder.warning.time.success", &["30"]));
    assert_eq!(run(&mut alice, "worldborder warning time 30"), key("commands.worldborder.warning.time.failed", &[]));
    let border = server.handle.run(|s| s.worlds[0].border.clone()).unwrap();
    assert_eq!((border.damage_per_block, border.damage_safe_zone, border.warning_blocks, border.warning_time), (1.0, 2.5, 8, 30));

    // Bob has a border of his own: changes to the world's are not his to see, and a respawn
    // shows him his own again.
    let mut bob = join(&server, "Bob");
    alice.drain(Duration::from_millis(200));
    alice.clear_backlog();
    let bob_id = server.handle.run(|s| s.players.values().find(|p| p.name() == "Bob").unwrap().entity_id).unwrap();
    server
        .handle
        .run(move |s| {
            let mut own = paper_server::world::Border::default();
            own.size = 42.0;
            own.lerp_target = 42.0;
            s.players.get_mut(&bob_id).unwrap().own_border = Some(own);
        })
        .unwrap();
    assert_eq!(run(&mut alice, "worldborder set 300"), key("commands.worldborder.set.immediate", &["300.0"]));
    alice.find(TIMEOUT, |b: &play::SetBorderSize| b.size == 300.0).expect("Alice sees the world's border move");
    assert!(bob.find(Duration::from_secs(1), |_: &play::SetBorderSize| true).is_none(), "Bob keeps his own border");
    server.handle.console_command("kill Bob");
    bob.expect::<play::PlayerCombatKill>();
    bob.clear_backlog();
    bob.send(&play::ClientCommand { action: paper_protocol::codec::VarInt(0) });
    let shown: play::InitializeBorder = bob.find(TIMEOUT, |_: &play::InitializeBorder| true).expect("the border after respawning");
    assert_eq!(shown.new_size, 42.0, "Bob's own border");
    server.stop();
}

fn pause(client: &mut Client, d: Duration) {
    std::thread::sleep(d);
    client.drain(Duration::from_millis(10));
}
