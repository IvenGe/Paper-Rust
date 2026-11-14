//! Real Java plugins end to end: the Rust server starts the actual plugin host JVM, which
//! loads the two test plugins in `plugin-host/test-plugins` (a `plugin.yml` plugin and a
//! `paper-plugin.yml` plugin with a bootstrapper), and a protocol client checks what players
//! see.
//!
//! Needs Java 25 and the built jars (`mvn package` in `plugin-host/`). Without them the test
//! prints why and passes, so `cargo test` works on machines without Java. Point
//! `PAPER_RS_TEST_JAVA` at a Java 25 executable if it is not found automatically.

mod common;

use common::*;
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play::{self, NodeKind};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(300);

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// A Java 25+ executable, if one can be found.
fn find_java() -> Option<String> {
    let mut candidates = Vec::new();
    if let Ok(java) = std::env::var("PAPER_RS_TEST_JAVA") {
        candidates.push(java);
    }
    if let Ok(home) = std::env::var("JAVA_HOME") {
        candidates.push(format!("{home}/bin/java"));
    }
    candidates.push("/opt/jdk25/bin/java".into());
    candidates.push("java".into());
    candidates.into_iter().find(|java| {
        let Ok(out) = Command::new(java).arg("-version").output() else { return false };
        let text = String::from_utf8_lossy(&out.stderr);
        // `openjdk version "25.0.1" ...`
        text.split('"').nth(1).and_then(|v| v.split('.').next()).and_then(|major| major.parse::<u32>().ok()).is_some_and(|major| major >= 25)
    })
}

struct Setup {
    java: String,
    host_dir: PathBuf,
    plugins: Vec<PathBuf>,
}

fn setup() -> Option<Setup> {
    let root = repo_root().join("plugin-host");
    let host_dir = root.join("host/target");
    let plugins = vec![root.join("test-plugins/bukkit-test-plugin/target/bukkit-test-plugin.jar"), root.join("test-plugins/paper-test-plugin/target/paper-test-plugin.jar")];
    let Some(java) = find_java() else {
        eprintln!("skipping: no Java 25 found (set PAPER_RS_TEST_JAVA)");
        return None;
    };
    if !host_dir.join("paper-rs-plugin-host.jar").is_file() || plugins.iter().any(|p| !p.is_file()) {
        eprintln!("skipping: build the plugin host and test plugins first (mvn package in plugin-host/)");
        return None;
    }
    Some(Setup { java, host_dir, plugins })
}

fn literals(commands: &play::Commands) -> Vec<String> {
    commands.nodes[commands.root_index as usize]
        .children
        .iter()
        .filter_map(|i| match &commands.nodes[*i as usize].kind {
            NodeKind::Literal(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn next_text(client: &mut Client) -> String {
    client.expect::<play::SystemChat>().content.to_plain()
}

#[test]
fn bukkit_and_paper_plugins_run_against_the_rust_server() {
    let Some(setup) = setup() else { return };
    let dir = std::env::temp_dir().join(format!("paper-rs-java-plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let plugins_dir = dir.join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();
    for jar in &setup.plugins {
        std::fs::copy(jar, plugins_dir.join(jar.file_name().unwrap())).unwrap();
    }

    let server = start_server_with_host(
        |c| {
            c.root = dir.clone();
            c.paper.plugins.java = setup.java.clone();
            c.paper.plugins.host_dir = setup.host_dir.display().to_string();
            c.paper.plugins.plugins_dir = plugins_dir.display().to_string();
            c.paper.plugins.jvm_args = vec!["-Xmx512M".into()];
            c.paper.plugins.startup_timeout_seconds = 120;
        },
        paper_server::plugins::java_launcher(),
    );
    let plugins = server.handle.run(|s| s.plugin_names()).unwrap();
    assert_eq!(plugins, vec![("BukkitTestPlugin".to_owned(), true), ("PaperTestPlugin".to_owned(), true)], "both plugins are enabled");

    // PlayerJoinEvent: a message to the player and a replaced join message. The command tree
    // then holds the plugin commands this player may use.
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    assert_eq!(next_text(&mut alice), "Welcome, Alice, from a Bukkit plugin");
    assert_eq!(next_text(&mut alice), "Alice joined (bukkit-test)");
    let tree: play::Commands = alice.expect();
    let labels = literals(&tree);
    for label in ["greet", "hi", "bukkittestplugin:greet", "dice", "papertestplugin:dice", "where"] {
        assert!(labels.contains(&label.to_owned()), "{label} missing from {labels:?}");
    }
    assert!(!labels.contains(&"secret".to_owned()), "operator-only command hidden: {labels:?}");
    alice.drain(QUIET);
    alice.clear_backlog();

    // A plugin.yml command with configuration, an alias, and a permission check.
    alice.command("greet");
    assert_eq!(next_text(&mut alice), "Hello, Alice!");
    alice.command("hi Bob");
    assert_eq!(next_text(&mut alice), "Hello, Bob!");
    alice.command("secret");
    assert!(next_text(&mut alice).starts_with("I'm sorry, but you do not have permission"));

    // Brigadier commands: one registered by a bootstrapper, one BasicCommand from onEnable.
    alice.command("dice 6");
    assert_eq!(next_text(&mut alice), "You rolled a d6");
    alice.command("dice");
    assert_eq!(next_text(&mut alice), "Usage: /dice <sides>");
    alice.command("where");
    assert!(next_text(&mut alice).starts_with("You are in "));

    // Tab completion from a TabCompleter and from a BasicCommand (which, as on Paper, is not
    // filtered by what was typed).
    alice.send(&play::CommandSuggestion { transaction_id: VarInt(1), text: "/greet ".into() });
    let suggestions: play::CommandSuggestions = alice.expect();
    assert_eq!((suggestions.start, suggestions.length), (7, 0));
    assert_eq!(suggestions.matches.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), vec!["world", "everyone"]);
    alice.send(&play::CommandSuggestion { transaction_id: VarInt(2), text: "/where t".into() });
    let suggestions: play::CommandSuggestions = alice.expect();
    assert_eq!((suggestions.start, suggestions.length), (7, 1));
    assert_eq!(suggestions.matches.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), vec!["here", "there"]);

    // Becoming an operator re-evaluates permissions and the command tree.
    alice.clear_backlog();
    server.handle.console_command("op Alice");
    let tree: play::Commands = alice.find(TIMEOUT, |t: &play::Commands| literals(t).contains(&"secret".to_owned())).expect("tree with /secret");
    assert!(literals(&tree).contains(&"secret".to_owned()));
    alice.drain(QUIET);
    alice.clear_backlog();
    alice.command("secret");
    assert_eq!(next_text(&mut alice), "The secret is 42");

    // AsyncChatEvent with a custom renderer.
    alice.chat("hello there");
    assert_eq!(next_text(&mut alice), "[paper] Alice: hello there");

    // BlockBreakEvent: the plugin protects grass and tells the player.
    alice.drain(QUIET);
    alice.clear_backlog();
    let grass = BlockPos::new(8, -61, 8);
    let before = server.handle.run(move |s| s.worlds[0].block_at(8, -61, 8)).unwrap();
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: grass, face: 1, sequence: VarInt(3) });
    let protected: play::SystemChat = alice.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain() == "This block is protected").expect("protection message");
    assert_eq!(protected.content.to_plain(), "This block is protected");
    let update: play::BlockUpdate = alice.expect();
    assert_eq!((update.pos, Some(update.state.0 as u32)), (grass, before));
    assert_eq!(server.handle.run(move |s| s.worlds[0].block_at(8, -61, 8)).unwrap(), before, "the block is still there");

    // /plugins lists them by kind.
    alice.clear_backlog();
    alice.command("plugins");
    alice.drain(QUIET);
    let mut lines = Vec::new();
    while let Some(m) = alice.try_expect::<play::SystemChat>(Duration::from_millis(10)) {
        lines.push(m.content.to_plain());
    }
    assert!(lines.iter().any(|l| l.contains("PaperTestPlugin")) && lines.iter().any(|l| l.contains("BukkitTestPlugin")), "{lines:?}");

    // PlayerQuitEvent replaces the quit message other players see.
    let mut bob = Client::join(server.port(), "Bob");
    bob.expect::<play::PlayerPosition>();
    bob.drain(QUIET);
    bob.clear_backlog();
    server.handle.console_command("kick Alice");
    bob.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain() == "Alice left (bukkit-test)").expect("custom quit message");

    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Waits for a chat line, failing with what arrived instead.
fn expect_line(client: &mut Client, wanted: &str) {
    let mut seen = Vec::new();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while std::time::Instant::now() < deadline {
        if let Some(m) = client.try_expect::<play::SystemChat>(Duration::from_millis(100)) {
            let text = m.content.to_plain();
            if text == wanted {
                return;
            }
            seen.push(text);
        }
    }
    panic!("no {wanted:?} among {seen:?}");
}

fn component_id(name: &str) -> i32 {
    paper_data::registries::id("minecraft:data_component_type", name).unwrap() as i32
}

/// The plugin API areas end to end with the test plugin's `/apitest`: item stacks and metas,
/// inventories and their events, entities and their events, offline players, scoreboards,
/// and `/reload`.
#[test]
fn plugin_api_items_inventories_entities_scoreboards_and_reload() {
    use paper_protocol::types::{HashedSlot, Slot};
    use paper_server::world::entity::EntityKind;

    let Some(setup) = setup() else { return };
    let dir = std::env::temp_dir().join(format!("paper-rs-java-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let plugins_dir = dir.join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();
    for jar in &setup.plugins {
        std::fs::copy(jar, plugins_dir.join(jar.file_name().unwrap())).unwrap();
    }
    let server = start_server_with_host(
        |c| {
            c.root = dir.clone();
            c.paper.plugins.java = setup.java.clone();
            c.paper.plugins.host_dir = setup.host_dir.display().to_string();
            c.paper.plugins.plugins_dir = plugins_dir.display().to_string();
            c.paper.plugins.jvm_args = vec!["-Xmx512M".into()];
            c.paper.plugins.startup_timeout_seconds = 120;
        },
        paper_server::plugins::java_launcher(),
    );
    let item = |name: &str| paper_data::items::id(name).unwrap() as i32;

    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    alice.drain(QUIET);
    alice.clear_backlog();

    // --- item stacks and the player inventory ---------------------------------------------
    alice.command("apitest items");
    expect_line(&mut alice, "items: ok");
    let (sword, ender) = server
        .handle
        .run(|s| {
            let p = s.player_by_name("Alice").unwrap();
            (p.inventory[36].clone(), p.ender_items[4].clone())
        })
        .unwrap();
    let stack = sword.0.clone().expect("the sword is in the first hotbar slot");
    assert_eq!(stack.item, item("minecraft:diamond_sword"));
    let types: Vec<i32> = stack.added.iter().map(|(t, _)| *t).collect();
    for component in ["minecraft:custom_name", "minecraft:lore", "minecraft:unbreakable", "minecraft:custom_model_data", "minecraft:custom_data", "minecraft:tooltip_display", "minecraft:enchantments", "minecraft:damage"] {
        assert!(types.contains(&component_id(component)), "{component} missing from {types:?}");
    }
    assert_eq!(paper_server::server::loot::enchantment_level(Some(&sword), "minecraft:sharpness"), 5, "the server reads the plugin's enchantment");
    assert_eq!(paper_server::server::survival::item_damage(&stack), 10, "and its damage");
    assert_eq!(ender, Slot::of(item("minecraft:emerald"), 2), "the ender chest");

    // A method Paper-RS does not implement fails with a named UnsupportedOperationException.
    alice.command("apitest unsupported");
    expect_line(&mut alice, "unsupported: ItemMeta#getAttributeModifiers is not supported by Paper-RS yet");

    // --- a custom inventory: open, click (cancelled by the plugin), closed by the plugin ----
    alice.clear_backlog();
    alice.command("apitest gui");
    let open: play::OpenScreen = alice.find(TIMEOUT, |_: &play::OpenScreen| true).expect("the menu opens");
    assert_eq!(open.title.to_plain(), "Test Menu");
    assert_eq!(open.menu_type.0, paper_data::registries::id("minecraft:menu", "minecraft:generic_9x3").unwrap() as i32);
    let window = open.window_id.0;
    // (The window content holds stacks with components, which this test client cannot
    // decode; the server's copy is checked instead.)
    let button = server
        .handle
        .run(|s| match s.player_by_name("Alice").unwrap().open_menu.as_ref().map(|m| m.kind.clone()) {
            Some(paper_server::server::containers::MenuKind::Custom { id }) => s.plugin_data.inventories.get(&id).map(|i| (i.items.len(), i.items[13].0.as_ref().map(|s| s.item))),
            _ => None,
        })
        .unwrap()
        .expect("a custom menu is open");
    assert_eq!(button, (27, Some(item("minecraft:emerald"))));
    expect_line(&mut alice, "opened Test Menu");
    expect_line(&mut alice, "gui: open true");
    alice.send(&play::ContainerClick { window_id: window, state_id: 0, slot: 13, button: 0, mode: 0, changed_slots: Vec::new(), carried: HashedSlot(None) });
    expect_line(&mut alice, "clicked emerald with LEFT");
    expect_line(&mut alice, "menu closed (PLUGIN)");
    let carried = server.handle.run(|s| s.player_by_name("Alice").unwrap().carried.clone()).unwrap();
    assert!(carried.is_empty(), "the cancelled click took nothing");
    let open_menu = server.handle.run(|s| s.player_by_name("Alice").unwrap().open_menu.is_some()).unwrap();
    assert!(!open_menu, "the plugin closed the window");

    // --- a chest's block state and inventory -----------------------------------------------
    let chest = BlockPos::new(10, -60, 8);
    server
        .handle
        .run(move |s| {
            let state = paper_data::blocks::default_state("minecraft:chest").unwrap();
            let change = s.worlds[0].set_block(chest.x, chest.y, chest.z, state).unwrap();
            s.publish_block_change(0, &change);
        })
        .unwrap();
    alice.command("apitest chest 10 -60 8");
    expect_line(&mut alice, "chest: ok");
    let held = server.handle.run(move |s| s.container_items(0, chest)[0].clone()).unwrap();
    assert_eq!(held, Slot::of(item("minecraft:diamond"), 3));

    // --- crafting (CraftItemEvent) and dragging (InventoryDragEvent) in the own inventory -----
    let has_recipes = server
        .handle
        .run(|s| {
            let id = s.player_by_name("Alice").unwrap().entity_id;
            s.set_inventory_slot(id, 1, Slot::of(paper_data::items::id("minecraft:oak_log").unwrap() as i32, 1));
            s.recipes.is_some()
        })
        .unwrap();
    if has_recipes {
        let click = |slot: i16, button: i8, mode: i32| play::ContainerClick { window_id: 0, state_id: 0, slot, button, mode, changed_slots: Vec::new(), carried: HashedSlot(None) };
        alice.send(&click(0, 0, 0));
        expect_line(&mut alice, "crafted oak_planks x4 in CRAFTING (LEFT, PICKUP_ALL)");
        for (slot, button) in [(-999, 0), (9, 1), (10, 1), (-999, 2)] {
            alice.send(&click(slot, button, 5));
        }
        expect_line(&mut alice, "dragged over 2 slots (EVEN)");
        let spread = server.handle.run(|s| s.player_by_name("Alice").unwrap().inventory[9].clone()).unwrap();
        assert_eq!(spread, Slot::of(item("minecraft:oak_planks"), 2));
    }

    // --- entities: spawning, state, damage and their events --------------------------------
    alice.command("apitest entities");
    expect_line(&mut alice, "spawned zombie");
    expect_line(&mut alice, "hit Bob the Zombie for 4 (ENTITY_ATTACK)");
    expect_line(&mut alice, "entities: ok");
    let zombie = server
        .handle
        .run(|s| {
            s.worlds[0]
                .entities
                .values()
                .find(|e| e.custom_name.as_ref().is_some_and(|n| n.to_plain() == "Bob the Zombie"))
                .map(|e| (e.id, e.mob().unwrap().living.health, e.mob().unwrap().equipment[5].clone()))
        })
        .unwrap()
        .expect("the zombie exists");
    assert!(zombie.1 < 10.0, "damaged: {}", zombie.1);
    assert_eq!(zombie.2.0.as_ref().map(|s| s.item), Some(item("minecraft:golden_helmet")));
    // EntityDeathEvent replaces the drops with one diamond.
    let zombie_id = zombie.0;
    let diamonds = server
        .handle
        .run(move |s| {
            s.hurt_entity(0, zombie_id, paper_server::server::survival::Damage::new("minecraft:generic_kill"), 1000.0);
            s.worlds[0].entities.values().filter(|e| matches!(&e.kind, EntityKind::Item { stack: Slot(Some(st)), .. } if st.item == paper_data::items::id("minecraft:diamond").unwrap() as i32)).count()
        })
        .unwrap();
    assert_eq!(diamonds, 1, "the plugin's drop");

    // --- dropping (cancelled for dirt) and picking up ----------------------------------------
    let dirt_slot = server.handle.run(move |s| s.player_by_name("Alice").unwrap().inventory.iter().position(|i| i.0.as_ref().is_some_and(|st| st.item == paper_data::items::id("minecraft:dirt").unwrap() as i32))).unwrap().expect("dirt in the inventory");
    assert!((36..45).contains(&dirt_slot), "on the hotbar: {dirt_slot}");
    alice.send(&play::SetCarriedItem { slot: (dirt_slot - 36) as i16 });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::DROP_ITEM), pos: BlockPos::new(0, 0, 0), face: 0, sequence: VarInt(0) });
    expect_line(&mut alice, "dirt stays with you");
    let dirt = server.handle.run(move |s| s.player_by_name("Alice").unwrap().inventory[dirt_slot].clone()).unwrap();
    assert_eq!(dirt, Slot::of(item("minecraft:dirt"), 3), "the cancelled drop went back");
    server
        .handle
        .run(|s| {
            let p = s.player_by_name("Alice").unwrap();
            let at = paper_server::world::entity::Vec3::new(p.loc.x, p.loc.y, p.loc.z);
            s.spawn_item(0, at, Slot::of(paper_data::items::id("minecraft:apple").unwrap() as i32, 1), paper_server::world::entity::Vec3::ZERO, 0);
        })
        .unwrap();
    expect_line(&mut alice, "picked up apple");

    // --- scoreboards: objectives, scores, teams and per-player boards -------------------------
    alice.clear_backlog();
    alice.command("apitest scoreboard");
    let objective: play::SetObjective = alice.find(TIMEOUT, |o: &play::SetObjective| o.name == "kills" && o.mode == 0).expect("objective created");
    assert_eq!(objective.display_name.to_plain(), "Kills");
    let display: play::SetDisplayObjective = alice.find(TIMEOUT, |_: &play::SetDisplayObjective| true).expect("shown in the sidebar");
    assert_eq!((display.slot.0, display.objective.as_str()), (1, "kills"));
    let score: play::SetScore = alice.find(TIMEOUT, |s: &play::SetScore| s.objective == "kills").expect("score");
    assert_eq!((score.owner.as_str(), score.value), ("Alice", 3));
    let team: play::SetPlayerTeam = alice.find(TIMEOUT, |t: &play::SetPlayerTeam| t.name == "red" && t.method == 3).expect("Alice joins red");
    assert_eq!(team.members, vec!["Alice".to_owned()]);
    let personal: play::SetScore = alice.find(TIMEOUT, |s: &play::SetScore| s.objective == "personal").expect("the personal board is shown");
    assert_eq!((personal.owner.as_str(), personal.value), ("coins", 42));
    expect_line(&mut alice, "scoreboard: ok");
    // A player who joins later sees the main board, team colours included.
    let mut carol = Client::join(server.port(), "Carol");
    let objective: play::SetObjective = carol.find(TIMEOUT, |o: &play::SetObjective| o.name == "kills").expect("main board for newcomers");
    assert_eq!(objective.mode, 0);
    let team: play::SetPlayerTeam = carol.find(TIMEOUT, |t: &play::SetPlayerTeam| t.name == "red").expect("teams for newcomers");
    assert_eq!((team.method, team.members.clone()), (0, vec!["Alice".to_owned()]));
    assert_eq!(team.parameters.as_ref().and_then(|p| p.color), Some(12), "red");
    assert_eq!(team.parameters.as_ref().map(|p| p.prefix.to_plain()), Some("[R] ".to_owned()));

    // --- potion effects and boss bars -----------------------------------------------------------
    alice.clear_backlog();
    alice.command("apitest effects");
    expect_line(&mut alice, "effects: ok");
    let glowing = server.handle.run(|s| s.player_by_name("Alice").unwrap().effects.iter().any(|e| e.name() == "minecraft:glowing")).unwrap();
    assert!(glowing, "the plugin's effect is on the player");
    alice.command("apitest bossbar");
    let raid: play::BossEvent = alice.find(TIMEOUT, |b: &play::BossEvent| matches!(&b.action, play::BossEventAction::Add { title, .. } if title.to_plain() == "Raid")).expect("the raid bar");
    assert!(matches!(raid.action, play::BossEventAction::Add { color: 2, division: 2, .. }), "{raid:?}");
    let progress: play::BossEvent = alice.find(TIMEOUT, |b: &play::BossEvent| b.uuid == raid.uuid && matches!(b.action, play::BossEventAction::UpdateHealth(_))).expect("progress update");
    assert_eq!(progress.action, play::BossEventAction::UpdateHealth(0.25));
    alice.find(TIMEOUT, |b: &play::BossEvent| matches!(&b.action, play::BossEventAction::Add { title, color: 1, .. } if title.to_plain() == "Event")).expect("the Bukkit bar");
    expect_line(&mut alice, "bossbar: ok");

    // --- block data --------------------------------------------------------------------------------
    alice.clear_backlog();
    let (id, feet) = server
        .handle
        .run(|s| {
            let p = s.player_by_name("Alice").unwrap();
            (p.entity_id, BlockPos::new(p.loc.x.floor() as i32, p.loc.y.floor() as i32, p.loc.z.floor() as i32))
        })
        .unwrap();
    let state_at = |pos: BlockPos| server.handle.run(move |s| s.worlds[0].block_at(pos.x, pos.y, pos.z)).unwrap().unwrap();
    alice.command("apitest blockdata");
    expect_line(&mut alice, "blockdata: ok");
    let (base, fence) = (feet.offset(3, 0, -3), feet.offset(3, 0, -4));
    assert_eq!(paper_data::blocks::block_of_state(state_at(base)).name, "minecraft:gold_block", "the forced snapshot update");
    assert_eq!(paper_data::blocks::block_of_state(state_at(fence)).name, "minecraft:oak_fence");
    let fake: play::BlockUpdate = alice.find(TIMEOUT, |u: &play::BlockUpdate| u.pos == base.offset(0, 1, 0)).expect("the block only Alice sees");
    assert_eq!(Some(fake.state.0 as u32), paper_data::blocks::default_state("minecraft:diamond_block"));
    assert_eq!(state_at(base.offset(0, 1, 0)), paper_data::blocks::AIR, "the world is unchanged");

    // BlockPlaceEvent sees the placed block, and the plugin's change to it stays.
    alice.clear_backlog();
    let stairs = paper_data::items::id("minecraft:oak_stairs").unwrap() as i32;
    server.handle.run(move |s| s.set_inventory_slot(id, 36 + s.players[&id].held_slot as usize, paper_protocol::types::Slot::of(stairs, 4))).unwrap();
    let below = feet.offset(-2, -1, 2);
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: below, face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(40) });
    expect_line(&mut alice, "place: OAK_STAIRS over AIR with OAK_STAIRS");
    let placed = state_at(below.offset(0, 1, 0));
    assert_eq!(paper_data::blocks::block_of_state(placed).name, "minecraft:oak_stairs");
    assert_eq!(paper_data::blocks::property(placed, "half"), Some("top"), "the plugin turned it over");

    // --- offline players ------------------------------------------------------------------------
    server.handle.console_command("kick Carol");
    carol.wait_closed(TIMEOUT);
    alice.clear_backlog();
    alice.command("apitest offline Carol");
    expect_line(&mut alice, "offline Carol played=true online=false first=true last=true known=2");
    alice.command("apitest offline Nobody");
    expect_line(&mut alice, "offline Nobody played=false online=false first=false last=false known=2");

    // --- death and respawn events ----------------------------------------------------------------
    alice.clear_backlog();
    let spawn_y = server.handle.run(|s| s.worlds[0].spawn.y).unwrap();
    server
        .handle
        .run(|s| {
            let id = s.player_by_name("Alice").unwrap().entity_id;
            s.hurt_player(id, paper_server::server::survival::Damage::new("minecraft:generic_kill"), 1000.0);
        })
        .unwrap();
    expect_line(&mut alice, "Alice was defeated (api test)");
    let kept = server.handle.run(|s| s.player_by_name("Alice").unwrap().inventory[36].clone()).unwrap();
    assert_eq!(kept.0.as_ref().map(|s| s.item), Some(item("minecraft:diamond_sword")), "PlayerDeathEvent kept the inventory");
    alice.send(&play::ClientCommand { action: VarInt(0) });
    let respawned: play::PlayerPosition = alice.find(TIMEOUT, |p: &play::PlayerPosition| p.y > spawn_y as f64 + 3.0).expect("PlayerRespawnEvent moved the respawn point up");
    assert!(respawned.y >= spawn_y as f64 + 4.9, "{}", respawned.y);

    // --- /reload: a new plugin host with the players already online ----------------------------
    alice.drain(QUIET);
    alice.clear_backlog();
    server.handle.console_command("reload confirm");
    expect_line(&mut alice, "BukkitTestPlugin enabled with 1 player(s) online");
    expect_line(&mut alice, "server loaded: RELOAD");
    expect_line(&mut alice, "Reload complete.");
    let (plugins, kills) = server.handle.run(|s| (s.plugin_names(), s.scoreboards.get(0).is_some_and(|b| b.objectives.contains_key("kills")))).unwrap();
    assert_eq!(plugins, vec![("BukkitTestPlugin".to_owned(), true), ("PaperTestPlugin".to_owned(), true)]);
    assert!(kills, "the main scoreboard outlives the plugin host");
    alice.drain(QUIET);
    alice.clear_backlog();
    alice.command("apitest items");
    expect_line(&mut alice, "items: ok");

    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
