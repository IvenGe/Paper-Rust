//! `/scoreboard` and `/team` on the main scoreboard, and the main scoreboard saved with the
//! main world (`data/minecraft/scoreboard.dat`).

mod common;

use common::*;
use std::path::PathBuf;

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("paper-rs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn scoreboard_and_team_commands_change_the_saved_main_board() {
    let root = temp_root("scoreboard");
    let server = start_server(|c| c.root = root.clone());
    for command in [
        "scoreboard objectives add kills dummy",
        "scoreboard objectives add hp health",
        "scoreboard objectives setdisplay sidebar kills",
        "scoreboard objectives modify kills displayname {\"text\":\"Kills\"}",
        "scoreboard players set Alice kills 5",
        "scoreboard players add Alice kills 2",
        "scoreboard players set #total kills 10",
        "scoreboard players operation Alice kills += #total kills",
        "scoreboard players set Alice hp 3",
        "scoreboard players set Bob kills 1",
        "scoreboard players reset Bob",
        "team add red",
        "team modify red color red",
        "team modify red friendlyFire false",
        "team modify red collisionRule never",
        "team join red Alice",
        "team add blue",
        "team remove blue",
    ] {
        server.handle.console_command(command);
    }
    let state = |server: &paper_server::RunningServer| {
        server
            .handle
            .run(|s| {
                let b = s.scoreboards.get(0).unwrap();
                let score = |holder: &str, objective: &str| b.scores.get(holder).and_then(|s| s.get(objective)).map(|s| s.value);
                let red = b.teams.get("red").map(|t| (t.color, t.friendly_fire, t.collision_rule, t.entries.contains("Alice")));
                (score("Alice", "kills"), score("#total", "kills"), score("Alice", "hp"), score("Bob", "kills"), b.display.get(&1).cloned(), b.objectives["kills"].display_name.to_plain(), red, b.teams.contains_key("blue"))
            })
            .unwrap()
    };
    let expected = (Some(17), Some(10), None, None, Some("kills".to_owned()), "Kills".to_owned(), Some((Some(12), false, 1, true)), false);
    assert_eq!(state(&server), expected, "health scores are read-only, Bob was reset");
    server.stop();

    // After a restart the main board is back.
    let server = start_server(|c| c.root = root.clone());
    assert_eq!(state(&server), expected);
    server.stop();
}
