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

    // PlayerLoginEvent: a plugin refuses a player, and lets in one the server banned.
    let (_, reply) = Client::begin_login(server.port(), paper_data::PROTOCOL_VERSION, "Loki");
    let refusal: paper_protocol::packets::login::LoginDisconnect = decode(&reply.expect("login refusal"));
    assert_eq!(refusal.reason.0.to_plain(), "No Loki today");
    server.handle.console_command("ban Wally testing");
    server.handle.run(|s| assert!(s.shared.access.read().banned_players.iter().any(|b| b.name == "Wally"))).unwrap();
    let mut wally = Client::join(server.port(), "Wally");
    wally.expect::<play::PlayerPosition>();
    assert_eq!(next_text(&mut wally), "Welcome, Wally, from a Bukkit plugin");
    drop(wally);
    server.handle.console_command("pardon Wally");
    let deadline = std::time::Instant::now() + TIMEOUT;
    while server.handle.run(|s| s.players.len()).unwrap() > 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
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

    // Tab completion from a TabCompleter (with one more from TabCompleteEvent) and from a
    // BasicCommand (which, as on Paper, is not filtered by what was typed).
    alice.send(&play::CommandSuggestion { transaction_id: VarInt(1), text: "/greet ".into() });
    let suggestions: play::CommandSuggestions = alice.expect();
    assert_eq!((suggestions.start, suggestions.length), (7, 0));
    assert_eq!(suggestions.matches.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), vec!["world", "everyone", "galaxy"]);
    // AsyncTabCompleteEvent answers before the command, off the main thread.
    alice.send(&play::CommandSuggestion { transaction_id: VarInt(3), text: "/greet @".into() });
    let suggestions: play::CommandSuggestions = alice.expect();
    assert_eq!((suggestions.start, suggestions.length), (7, 1));
    assert_eq!(suggestions.matches.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), vec!["alpha", "beta"]);
    assert_eq!(suggestions.matches[0].1.as_ref().map(|t| t.to_plain()), Some("async".to_owned()));
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
    // (Animals eating grass elsewhere send block updates too.)
    let update: play::BlockUpdate = alice.find(TIMEOUT, |u: &play::BlockUpdate| u.pos == grass).expect("the block is resent");
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

/// Waits for a chat line starting with `prefix`, and returns it.
fn line_starting(client: &mut Client, prefix: &str) -> String {
    let mut seen = Vec::new();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while std::time::Instant::now() < deadline {
        if let Some(m) = client.try_expect::<play::SystemChat>(Duration::from_millis(100)) {
            let text = m.content.to_plain();
            if text.starts_with(prefix) {
                return text;
            }
            seen.push(text);
        }
    }
    panic!("no line starting with {prefix:?} among {seen:?}");
}

/// Sets blocks in the overworld, with block updates or just storing them.
fn put(server: &paper_server::RunningServer, blocks: &[(BlockPos, &'static str)], update: bool) {
    let blocks = blocks.to_vec();
    server
        .handle
        .run(move |s| {
            for (pos, state) in blocks {
                let state = paper_data::blocks::parse_state(state).unwrap();
                if update {
                    s.set_block_and_update(0, pos, state);
                } else {
                    s.worlds[0].set_block(pos.x, pos.y, pos.z, state);
                }
            }
            s.publish_block_updates(0);
        })
        .unwrap();
}

fn block_state(server: &paper_server::RunningServer, pos: BlockPos) -> String {
    server.handle.run(move |s| s.worlds[0].block_at(pos.x, pos.y, pos.z).map(paper_data::blocks::state_to_string).unwrap_or_default()).unwrap()
}

fn random_tick_speed(server: &paper_server::RunningServer, speed: i32) {
    server.handle.run(move |s| s.worlds[0].rules.set("random_tick_speed", paper_data::game_rules::Value::Int(speed))).unwrap();
}

/// Runs `n` ticks of the overworld's block updates at once (fire takes its time).
fn run_block_ticks(server: &paper_server::RunningServer, n: usize) {
    server
        .handle
        .run(move |s| {
            for _ in 0..n {
                s.worlds[0].tick_time();
                s.tick_block_updates();
            }
        })
        .unwrap();
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

    // Special item metas: the server holds what the plugin wrote.
    alice.command("apitest metas");
    expect_line(&mut alice, "metas: ok");
    let (head, potion, book) = server
        .handle
        .run(|s| {
            let p = s.player_by_name("Alice").unwrap();
            (p.inventory[9].clone(), p.inventory[10].clone(), p.inventory[13].clone())
        })
        .unwrap();
    let component = |slot: &Slot, name: &str| slot.0.as_ref().and_then(|s| s.added.iter().find(|(t, _)| *t == component_id(name)).map(|(_, d)| d.clone()));
    let profile = component(&head, "minecraft:profile").expect("the head has a profile");
    assert!(profile.windows(5).any(|w| w == b"Alice"), "the profile names Alice: {profile:?}");
    assert_eq!(paper_server::server::effects::potion_of(potion.0.as_ref().unwrap()).as_deref(), Some("minecraft:strong_healing"));
    let content = component(&book, "minecraft:written_book_content").expect("the book has content");
    assert!(content.starts_with(b"\x05Guide"), "title first: {content:?}");
    server
        .handle
        .run(|s| {
            let id = s.player_by_name("Alice").unwrap().entity_id;
            for slot in 9..=16 {
                s.set_inventory_slot(id, slot, Slot::EMPTY);
            }
        })
        .unwrap();

    // Attributes of players and mobs.
    alice.command("apitest attributes");
    expect_line(&mut alice, "attributes: ok");

    // Resource packs: offered, answered, and the answers reach the plugin.
    alice.clear_backlog();
    alice.command("apitest packs");
    expect_line(&mut alice, "packs: sent");
    alice.find(TIMEOUT, |p: &play::ResourcePackPop| p.uuid.is_none()).expect("the old packs go");
    let push: play::ResourcePackPush = alice.find(TIMEOUT, |_: &play::ResourcePackPush| true).expect("the pack is offered");
    assert_eq!((push.url.as_str(), push.hash.len(), push.forced), ("https://example.com/pack.zip", 40, true));
    assert_eq!(push.prompt.as_ref().map(|p| p.to_plain()).as_deref(), Some("Please accept"));
    alice.send(&play::ResourcePackResponse { uuid: push.uuid, result: VarInt(3) });
    alice.send(&play::ResourcePackResponse { uuid: push.uuid, result: VarInt(0) });
    alice.drain(Duration::from_millis(300));
    alice.command("apitest packstatus");
    expect_line(&mut alice, "packstatus: SUCCESSFULLY_LOADED ACCEPTED,SUCCESSFULLY_LOADED");

    // A book opens on the client.
    alice.command("apitest book");
    let open: play::OpenBook = alice.find(TIMEOUT, |_: &play::OpenBook| true).expect("the book opens");
    assert_eq!(open.hand.0, 0);
    expect_line(&mut alice, "book: opened");

    // --- villagers and merchants -------------------------------------------------------------
    // The plugin sets a villager up and opens its trading window for Alice; she picks the
    // trade, pays and takes it, and the plugin sees TradeSelectEvent and PlayerTradeEvent.
    let merchant_menu = paper_data::registries::id("minecraft:menu", "minecraft:merchant").unwrap() as i32;
    let trade_click = |window: i32| play::ContainerClick { window_id: window, state_id: 0, slot: 2, button: 0, mode: 0, changed_slots: Vec::new(), carried: HashedSlot(None) };
    // (Alice's inventory holds stacks with components, which this client cannot decode in
    // window contents: the server's copy of the trading slots is checked instead.)
    let on_offer = |server: &paper_server::RunningServer| server.handle.run(|s| s.player_by_name("Alice").unwrap().open_menu.as_ref().map(|m| m.craft_grid[2].clone())).unwrap();
    let wait_for_offer = |server: &paper_server::RunningServer, wanted: Slot| {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while on_offer(server) != Some(wanted.clone()) {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {wanted:?} on offer");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let (emerald, bread, gold, diamond) = (item("minecraft:emerald"), item("minecraft:bread"), item("minecraft:gold_ingot"), item("minecraft:diamond"));
    server.handle.run(move |s| {
        let id = s.player_by_name("Alice").unwrap().entity_id;
        s.set_inventory_slot(id, 37, Slot::of(emerald, 10));
    }).unwrap();
    alice.clear_backlog();
    alice.command("apitest villagers");
    let screen: play::OpenScreen = alice.find(TIMEOUT, |o: &play::OpenScreen| o.menu_type.0 == merchant_menu).expect("the trading window");
    expect_line(&mut alice, "villagers: ok");
    let window = screen.window_id.0;
    let offers: play::MerchantOffers = alice.find(TIMEOUT, |o: &play::MerchantOffers| o.window_id == window).expect("the offers");
    assert_eq!((offers.offers.len(), offers.level, offers.offers[0].result.clone()), (1, 3, Slot::of(bread, 3)));
    alice.send(&play::SelectTrade { item: VarInt(0) });
    expect_line(&mut alice, "selected trade 0 of 1");
    wait_for_offer(&server, Slot::of(bread, 3));
    alice.send(&trade_click(window));
    expect_line(&mut alice, "trade with villager: bread x3 (exp true)");
    let (carried, uses) = server
        .handle
        .run(|s| {
            let p = s.player_by_name("Alice").unwrap();
            let uses = s.worlds[0].entities.values().find_map(|e| e.mob()?.merchant.as_ref()?.offers.as_ref()?.first().map(|o| o.uses));
            (p.carried.clone(), uses)
        })
        .unwrap();
    assert_eq!((carried, uses), (Slot::of(bread, 3), Some(1)));
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(window) });
    alice.drain(Duration::from_millis(200));

    // A merchant the plugin made: no levels, PlayerPurchaseEvent.
    server.handle.run(move |s| {
        let id = s.player_by_name("Alice").unwrap().entity_id;
        s.set_inventory_slot(id, 38, Slot::of(gold, 8));
        // The villager goes, so the next test's villager gets the lectern.
        for e in s.worlds[0].entities.values_mut().filter(|e| e.info.name == "minecraft:villager") {
            e.removed = true;
        }
    }).unwrap();
    alice.clear_backlog();
    alice.command("apitest merchant");
    let screen: play::OpenScreen = alice.find(TIMEOUT, |o: &play::OpenScreen| o.menu_type.0 == merchant_menu).expect("the shop window");
    assert_eq!(screen.title.to_plain(), "Shop");
    expect_line(&mut alice, "merchant: ok");
    let window = screen.window_id.0;
    let offers: play::MerchantOffers = alice.find(TIMEOUT, |o: &play::MerchantOffers| o.window_id == window).expect("the shop's offers");
    assert_eq!((offers.offers.len(), offers.show_progress), (1, false));
    alice.send(&play::SelectTrade { item: VarInt(0) });
    expect_line(&mut alice, "selected trade 0 of 1");
    wait_for_offer(&server, Slot::of(diamond, 1));
    alice.send(&trade_click(window));
    expect_line(&mut alice, "purchase: diamond x1 (exp false)");
    let carried = server.handle.run(|s| s.player_by_name("Alice").unwrap().carried.clone()).unwrap();
    assert_eq!(carried, Slot::of(diamond, 1));
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(window) });
    alice.drain(Duration::from_millis(200));

    // What the trades left in the inventory goes, for the tests below.
    server
        .handle
        .run(move |s| {
            let id = s.player_by_name("Alice").unwrap().entity_id;
            for slot in 9..45 {
                let traded = s.players[&id].inventory[slot].0.as_ref().is_some_and(|i| [emerald, bread, gold, diamond].contains(&i.item));
                if traded {
                    s.set_inventory_slot(id, slot, Slot::EMPTY);
                }
            }
        })
        .unwrap();

    // A villager next to a lectern takes the job: VillagerCareerChangeEvent.
    alice.command("apitest career");
    expect_line(&mut alice, "career: placed");
    expect_line(&mut alice, "career librarian EMPLOYED for Hired");

    // Passengers, pets and horses.
    alice.command("apitest riding");
    expect_line(&mut alice, "riding: added true vehicle pig passengers 1");
    expect_line(&mut alice, "riding: ejected true passengers 0 inside false");
    expect_line(&mut alice, "pets: tamed true owner Alice sitting true");
    expect_line(&mut alice, "horse: domestication 42/100 jump 9");

    // Persistent data is kept by the server with the player and the entity.
    alice.command("apitest pdc");
    expect_line(&mut alice, "pdc: 42 oink");
    let (player_pdc, pig_pdc) = server
        .handle
        .run(|s| {
            let player = s.players.values().find(|p| p.name() == "Alice").and_then(|p| p.pdc.clone());
            let pig = s.worlds[0].entities.values().find(|e| e.custom_name.as_ref().is_some_and(|n| n.to_plain() == "Kept")).and_then(|e| e.pdc.clone());
            (player, pig)
        })
        .unwrap();
    let score = |c: &Option<paper_nbt::Compound>| c.as_ref().and_then(|c| c.iter().find(|(k, _)| k.ends_with(":score")).map(|(_, v)| v.clone()));
    assert_eq!(score(&player_pdc), Some(paper_nbt::Tag::Int(42)));
    assert_eq!(score(&pig_pdc), Some(paper_nbt::Tag::String("oink".into())));
    server
        .handle
        .run(|s| {
            for e in s.worlds[0].entities.values_mut().filter(|e| e.info.name == "minecraft:villager") {
                e.removed = true;
            }
        })
        .unwrap();

    // A method Paper-RS does not implement fails with a named UnsupportedOperationException.
    alice.command("apitest unsupported");
    expect_line(&mut alice, "unsupported: World#locateNearestRaid is not supported by Paper-RS yet");
    // An item with an owner is only for that player: Alice picks up her feather, not the
    // stick meant for someone else.
    alice.command("apitest owner");
    expect_line(&mut alice, "owner: true true true true");
    let (feather, stick) = (paper_data::items::id("minecraft:feather").unwrap() as i32, paper_data::items::id("minecraft:stick").unwrap() as i32);
    let has = |server: &paper_server::RunningServer, item: i32| server.handle.run(move |s| s.player_by_name("Alice").unwrap().inventory.iter().any(|i| i.0.as_ref().is_some_and(|i| i.item == item))).unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !has(&server, feather) {
        assert!(std::time::Instant::now() < deadline, "Alice picks up her feather");
        alice.drain(Duration::from_millis(50));
    }
    alice.drain(Duration::from_millis(500));
    assert!(!has(&server, stick), "the stick is someone else's");
    let owned = server.handle.run(move |s| s.worlds[0].entities.values().any(|e| matches!(&e.kind, paper_server::world::entity::EntityKind::Item { stack, owner: Some(_), .. } if stack.0.as_ref().is_some_and(|i| i.item == stick)))).unwrap();
    assert!(owned, "the stick stays, still owned");
    // Attribute modifiers on an item's meta, worn by the server's attribute maps too.
    // Commodore rewrites the plugin's Material.values() and enum switches to leave out the
    // legacy materials; legacy constants it names still convert, from the legacy id table.
    alice.command("apitest legacy");
    expect_line(&mut alice, "legacy: ok");
    // Player state through the API: speeds reach the client's abilities, the player's own
    // time and weather replace the world's for that client.
    alice.clear_backlog();
    alice.command("apitest playerstate");
    alice.find(TIMEOUT, |a: &play::PlayerAbilities| (a.fov_modifier - 0.2).abs() < 1e-6 && (a.flying_speed - 0.15).abs() < 1e-6).expect("both speeds in the abilities");
    let time: play::SetTime = alice.find(TIMEOUT, |t: &play::SetTime| t.clocks.first().is_some_and(|c| c.time % 24000 == 6000 && c.rate == 0.0)).expect("the player's fixed time");
    assert_eq!(time.clocks.len(), 1);
    alice.find(TIMEOUT, |g: &play::GameEvent| g.event == play::game_event::START_RAINING).expect("the player's own rain");
    expect_line(&mut alice, "playerstate: ok");
    // A world's border and weather: border changes reach the world's players.
    alice.clear_backlog();
    alice.command("apitest worldstate");
    alice.find(TIMEOUT, |c: &play::SetBorderCenter| c.center_x == 100.0 && c.center_z == 50.0).expect("the new border center");
    alice.find(TIMEOUT, |b: &play::SetBorderSize| b.size == 200.0).expect("the new border size");
    alice.find(TIMEOUT, |l: &play::SetBorderLerpSize| l.old_size == 200.0 && l.new_size == 100.0 && l.lerp_time.0 == 400).expect("the border moving");
    expect_line(&mut alice, "worldstate: ok");
    // Signs: plugins read and write both sides (Sign, SignSide); what a player writes goes
    // through SignChangeEvent, where the plugin shouts "shout ...".
    alice.command("apitest signs");
    expect_line(&mut alice, "signs: ok");
    let id = alice.entity_id;
    let oak_sign = paper_data::items::id("minecraft:oak_sign").unwrap() as i32;
    server.handle.run(move |s| s.set_inventory_slot(id, 36, paper_protocol::types::Slot::of(oak_sign, 1))).unwrap();
    let sign = BlockPos::new(8, -60, 10);
    alice.clear_backlog();
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: BlockPos::new(8, -61, 10), face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(50) });
    alice.find(TIMEOUT, |e: &play::OpenSignEditor| e.pos == sign).expect("the sign editor");
    alice.send(&play::SignUpdate { pos: sign, lines: ["shout hello".into(), "quiet".into(), String::new(), String::new()], front: true });
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let front = server.handle.run(move |s| s.block_entity_data(0, sign).and_then(|d| d.compound("front_text").cloned())).unwrap();
        let text = format!("{front:?}");
        if text.contains("SHOUT HELLO") {
            assert!(text.contains("quiet"), "{text}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the plugin's line never arrived: {text}");
        alice.drain(Duration::from_millis(50));
    }
    // Game rules (one changed on its way by the plugin's WorldGameRuleChangeEvent listener),
    // the world's plugin data and metadata, stone broken naturally into cobblestone, and block
    // entities: a chest's name and plugin data, a banner's patterns, a spawner's mob.
    let at = server.handle.run(|s| s.player_by_name("Alice").unwrap().loc).unwrap();
    let block = |dx: f64, dz: f64| BlockPos::new((at.x + dx).floor() as i32, at.y.floor() as i32, (at.z + dz).floor() as i32);
    alice.command("apitest worldapi");
    let pling = paper_data::registries::id("minecraft:sound_event", "minecraft:block.note_block.pling").unwrap() as i32;
    alice.find(TIMEOUT, |s: &play::Sound| matches!(s.sound, play::SoundRef::Id(id) if id == pling) && s.pitch == 2.0).expect("the world's sound");
    expect_line(&mut alice, "worldapi: ok");
    let rules = server.handle.run(|s| (s.worlds[0].rules.bool("keep_inventory"), s.worlds[0].rules.int("max_entity_cramming"), s.worlds[0].rules.int("random_tick_speed"))).unwrap();
    assert_eq!(rules, (true, 14, 5));
    let data = server.handle.run(|s| s.worlds[0].plugin_data.clone()).unwrap().expect("the world's plugin data");
    assert!(format!("{data:?}").contains("visits"), "{data:?}");
    let stone = block(3.0, -3.0);
    let cobblestone = paper_data::items::id("minecraft:cobblestone").unwrap() as i32;
    let dropped = server
        .handle
        .run(move |s| {
            s.worlds[0].entities.values().any(|e| {
                matches!(&e.kind, paper_server::world::entity::EntityKind::Item { stack, .. } if stack.0.as_ref().is_some_and(|i| i.item == cobblestone))
                    && (e.pos.x - stone.x as f64 - 0.5).abs() < 1.5
                    && (e.pos.z - stone.z as f64 - 0.5).abs() < 1.5
            })
        })
        .unwrap();
    assert!(dropped, "the stone's cobblestone");
    let (chest, banner) = (block(3.0, -5.0), block(5.0, -5.0));
    let chest_data = server.handle.run(move |s| s.block_entity_data(0, chest).cloned()).unwrap().expect("the chest");
    let name = chest_data.get("CustomName").map(paper_protocol::text::Component::from_nbt).map(|c| c.to_plain());
    assert_eq!(name.as_deref(), Some("Loot"));
    assert!(chest_data.compound("PublicBukkitValues").is_some_and(|v| format!("{v:?}").contains("owned")), "{chest_data:?}");
    let patterns = server.handle.run(move |s| s.block_entity_data(0, banner).and_then(|d| d.get("patterns").cloned())).unwrap();
    assert!(format!("{patterns:?}").contains("minecraft:stripe_top"), "{patterns:?}");
    // Plugins' recipes are crafted by the server: the heart only around the star named "Core"
    // the plugin gave Alice, vanilla's stick recipe is gone, rotten flesh smelts into leather.
    alice.command("apitest recipes");
    expect_line(&mut alice, "recipes: ok");
    let (heart, plain_star, stick, leather) = server
        .handle
        .run(move |s| {
            use paper_protocol::types::Slot;
            let item = |name: &str| paper_data::items::id(name).unwrap() as i32;
            let core = s.players[&id].inventory[36].clone();
            let diamond = Slot::of(item("minecraft:diamond"), 1);
            let grid = |center: &Slot| vec![diamond.clone(), diamond.clone(), diamond.clone(), diamond.clone(), center.clone(), diamond.clone(), diamond.clone(), diamond.clone(), diamond.clone()];
            let recipes = s.recipes.as_ref().unwrap();
            let heart = recipes.craft(&grid(&core), 3).map(|(key, result)| (key.to_owned(), result.item));
            let plain_star = recipes.craft(&grid(&Slot::of(item("minecraft:nether_star"), 1)), 3).map(|(key, _)| key.to_owned());
            let planks = Slot::of(item("minecraft:oak_planks"), 1);
            let stick = recipes.craft(&[planks.clone(), Slot::EMPTY, planks, Slot::EMPTY], 2).map(|(key, _)| key.to_owned());
            let leather = recipes.cook("minecraft:smelting", &Slot::of(item("minecraft:rotten_flesh"), 1)).map(|r| r.result.item);
            (heart, plain_star, stick, leather)
        })
        .unwrap();
    assert_eq!(heart, Some(("bukkittestplugin:heart".to_owned(), paper_data::items::id("minecraft:red_dye").unwrap() as i32)));
    assert_eq!(plain_star, None, "a star without the name");
    assert_eq!(stick, None, "the stick recipe was removed");
    assert_eq!(leather, paper_data::items::id("minecraft:leather").map(|i| i as i32));
    // Data component values built with Paper's builders reach the item: the sword the plugin
    // gave Alice has its model data.
    alice.command("apitest components");
    expect_line(&mut alice, "components: ok");
    let model_data = paper_data::registries::id("minecraft:data_component_type", "minecraft:custom_model_data").unwrap() as i32;
    let sword = server.handle.run(move |s| s.players[&id].inventory[37].clone()).unwrap();
    assert!(sword.0.as_ref().is_some_and(|s| s.added.iter().any(|(ty, bytes)| *ty == model_data && bytes.windows(5).any(|w| w == b"blade"))), "{sword:?}");
    // Particles reach the players (the world's to all near, the player's to Alice), so do the
    // level effect of a broken stone block and the flash of lightning; the explosion leaves a
    // crater.
    let at = server.handle.run(|s| s.player_by_name("Alice").unwrap().loc).unwrap();
    alice.clear_backlog();
    alice.command("apitest worldeffects");
    let particle = |name: &str| paper_data::registries::id("minecraft:particle_type", name).unwrap() as i32;
    let (heart, flame) = (particle("minecraft:heart"), particle("minecraft:flame"));
    let is = |p: &play::LevelParticles, id: i32| p.particle.first().is_some_and(|b| *b as i32 == id);
    alice.find(TIMEOUT, |p: &play::LevelParticles| is(p, heart) && p.count == 3 && p.offset == (0.5, 0.5, 0.5)).expect("hearts");
    alice.find(TIMEOUT, |p: &play::LevelParticles| is(p, flame) && p.count == 5).expect("flames for Alice");
    let stone = paper_data::blocks::default_state("minecraft:stone").unwrap() as i32;
    alice.find(TIMEOUT, |e: &play::LevelEvent| e.event == 2001 && e.data == stone).expect("the stone's break effect");
    let bolt = paper_data::entity_types::by_name("minecraft:lightning_bolt").unwrap().id;
    alice.find(TIMEOUT, |a: &play::AddEntity| a.entity_type.0 == bolt).expect("the lightning");
    expect_line(&mut alice, "worldeffects: true true");
    let crater = BlockPos::new((at.x - 12.0).floor() as i32, at.y.floor() as i32 - 1, (at.z - 12.0).floor() as i32);
    let hole = server.handle.run(move |s| s.worlds[0].block_at(crater.x, crater.y, crater.z).map(paper_data::blocks::is_air)).unwrap();
    assert_eq!(hole, Some(true), "the explosion's crater");
    // --- entity flags and tags, age and breeding, sheep ----------------------------------------
    alice.clear_backlog();
    alice.command("apitest entityflags");
    let ids: play::SystemChat = alice.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain().starts_with("entityflags ids: ")).expect("the entities");
    let ids: Vec<i32> = ids.content.to_plain()["entityflags ids: ".len()..].split(' ').map(|i| i.parse().unwrap()).collect();
    let (cow, sheep) = (ids[0], ids[1]);
    expect_line(&mut alice, "entityflags: ok");
    // Alice sees the cow glow and keep quiet.
    alice.find(TIMEOUT, |m: &play::SetEntityData| m.entity_id.0 == cow && m.metadata.0.windows(3).any(|w| w == [0, 0, 0x40]) && m.metadata.0.windows(3).any(|w| w == [4, 8, 1])).expect("a glowing, silent cow");
    let (flags, age, sheared, color, saved) = server
        .handle
        .run(move |s| {
            let e = &s.worlds[0].entities[&cow];
            let m = e.mob().unwrap();
            let saved = paper_server::world::entity_nbt::entity_from_nbt(cow, &paper_server::world::entity_nbt::entity_to_nbt(e).unwrap()).unwrap().flags;
            let sheep = s.worlds[0].entities[&sheep].mob().unwrap();
            (e.flags.clone(), m.age, sheep.sheared, sheep.variant, saved)
        })
        .unwrap();
    assert!(flags.glowing && flags.silent && flags.invulnerable && flags.no_ai && flags.age_locked, "{flags:?}");
    assert_eq!(flags.tags, vec!["paper-rs".to_owned()]);
    assert_eq!(flags.spawn_reason.as_deref(), Some("CUSTOM"));
    assert_eq!(age, 0, "ready to breed");
    let warm = server.handle.run(move |s| s.worlds[0].entities[&cow].mob().unwrap().variant).unwrap();
    assert_eq!(paper_server::server::variants::key_of("minecraft:cow_variant", warm), Some("minecraft:warm"));
    assert!(sheared && color == 14, "a sheared red sheep: {sheared} {color}");
    assert!(saved.glowing && saved.silent && saved.invulnerable && saved.no_ai && saved.age_locked && saved.tags == flags.tags && saved.spawn_reason == flags.spawn_reason, "saved {saved:?}");
    // Only what bypasses invulnerability hurts it.
    let hurt = server
        .handle
        .run(move |s| {
            use paper_server::server::survival::Damage;
            (s.hurt_entity(0, cow, Damage::new("minecraft:generic"), 2.0), s.hurt_entity(0, cow, Damage::new("minecraft:generic_kill"), 2.0))
        })
        .unwrap();
    assert_eq!(hurt, (false, true));
    // Projectiles: a snowball Alice threw, a fireball the plugin aimed up and gave her.
    alice.command("apitest projectiles");
    let ids: play::SystemChat = alice.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain().starts_with("projectiles ids: ")).expect("the projectiles");
    let ids: Vec<i32> = ids.content.to_plain()["projectiles ids: ".len()..].split(' ').map(|i| i.parse().unwrap()).collect();
    expect_line(&mut alice, "projectiles: ok");
    let alice_id = alice.entity_id;
    let fireball = ids[1];
    let shots = server
        .handle
        .run(move |s| {
            let owner = |id: i32| s.worlds[0].entities.get(&id).and_then(|e| match e.kind {
                paper_server::world::entity::EntityKind::Thrown { owner, .. } => Some((owner, e.vel.y)),
                _ => None,
            });
            owner(fireball)
        })
        .unwrap();
    let (owner, vy) = shots.expect("the fireball flies");
    assert!(owner == Some(alice_id) && vy > 0.0, "{owner:?} {vy}");

    // --- clicks and explosions plugins stop (the test plugin's guard) ------------------------
    let (feet, chest, poppy) = server
        .handle
        .run(move |s| {
            let p = s.players.get_mut(&alice_id).unwrap();
            p.loc.pitch = -90.0;
            let feet = BlockPos::new(p.loc.x.floor() as i32, p.loc.y.floor() as i32, p.loc.z.floor() as i32);
            let (chest, poppy) = (feet.offset(-2, 0, 0), feet.offset(0, 0, -2));
            s.worlds[0].set_block(chest.x, chest.y, chest.z, paper_data::blocks::default_state("minecraft:chest").unwrap());
            s.worlds[0].set_block(poppy.x, poppy.y, poppy.z, paper_data::blocks::default_state("minecraft:poppy").unwrap());
            (feet, chest, poppy)
        })
        .unwrap();
    alice.command("apitest guard");
    expect_line(&mut alice, "guard: true");
    alice.drain(QUIET);
    alice.clear_backlog();
    // Opening the chest, picking the poppy, a swing at the sky, throwing a snowball, a right
    // click on the cow: all stopped.
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: chest, face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(90) });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: poppy, face: 1, sequence: VarInt(91) });
    alice.send(&play::Punch {});
    // (Some ticks later, or it would count as the item's use after the click on the chest.)
    alice.drain(Duration::from_millis(250));
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(92), yaw: 0.0, pitch: -90.0 });
    alice.send(&play::Interact { entity_id: VarInt(cow), hand: VarInt(0), target: paper_protocol::types::LpVec3(0.0, 0.5, 0.0), sneaking: false });
    alice.command("apitest interactions");
    expect_line(&mut alice, "interactions: RIGHT_CLICK_BLOCK CHEST UP HAND SNOWBALL, LEFT_CLICK_BLOCK POPPY UP HAND SNOWBALL, LEFT_CLICK_AIR - SELF HAND SNOWBALL, RIGHT_CLICK_AIR - SELF HAND SNOWBALL, ENTITY COW HAND");
    let (shut, poppy_left, snowballs) = server
        .handle
        .run(move |s| {
            let shut = s.players[&alice_id].open_menu.is_none();
            let poppy_left = s.worlds[0].block_at(poppy.x, poppy.y, poppy.z).map(|b| paper_data::blocks::block_of_state(b).name);
            let snowballs = s.worlds[0].entities.values().filter(|e| e.info.name == "minecraft:snowball" && matches!(e.kind, EntityKind::Thrown { owner: Some(o), .. } if o == alice_id)).count();
            (shut, poppy_left, snowballs)
        })
        .unwrap();
    assert!(shut, "the chest stays shut");
    assert_eq!(poppy_left, Some("minecraft:poppy"));
    assert_eq!(snowballs, 0, "no snowball thrown");
    // A blast the cow sets off breaks nothing.
    let blast = feet.offset(8, -1, 8);
    server.handle.run(move |s| s.explode_with(0, paper_server::world::entity::Vec3::new(blast.x as f64 + 0.5, blast.y as f64 + 0.5, blast.z as f64 + 0.5), 2.0, true, false, Some(cow))).unwrap();
    alice.command("apitest interactions");
    expect_line(&mut alice, "interactions: EXPLODE COW true");
    let ground = server.handle.run(move |s| s.worlds[0].block_at(blast.x, blast.y, blast.z).map(paper_data::blocks::is_air)).unwrap();
    assert_eq!(ground, Some(false), "the ground is spared");
    alice.command("apitest guard");
    expect_line(&mut alice, "guard: false");
    server.handle.run(move |s| s.players.get_mut(&alice_id).unwrap().loc.pitch = 0.0).unwrap();

    // --- player state and world events plugins stop (the test plugin's watch) -----------------
    alice.command("apitest watch");
    expect_line(&mut alice, "watch: true");
    let held_before = server.handle.run(move |s| s.players[&alice_id].held_slot).unwrap();
    let hands_before = server.handle.run(move |s| (s.players[&alice_id].inventory[36 + s.players[&alice_id].held_slot as usize].clone(), s.players[&alice_id].inventory[45].clone())).unwrap();
    alice.send(&play::PlayerInput { flags: play::input_flags::SNEAK });
    alice.send(&play::SetCarriedItem { slot: (held_before as i16 + 3) % 9 });
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::SWAP_ITEM_WITH_OFFHAND), pos: BlockPos::new(0, 0, 0), face: 0, sequence: VarInt(93) });
    // Emptying the water bucket at the ground below her.
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(94), yaw: 0.0, pitch: 90.0 });
    server.handle.console_command("weather rain");
    // (In survival, so that the zombie looks for her.)
    let zombie = server
        .handle
        .run(move |s| {
            s.set_game_mode(alice_id, paper_server::config::GameMode::Survival);
            let p = s.players.get_mut(&alice_id).unwrap();
            (p.food, p.saturation, p.exhaustion) = (20, 0.0, 4.5);
            let at = paper_server::world::entity::Vec3::new(p.loc.x + 3.0, p.loc.y, p.loc.z);
            let zombie = s.spawn_mob(0, "minecraft:zombie", at, paper_server::server::mobs::SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&zombie).unwrap().flags.tags.push("watched".into());
            zombie
        })
        .unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        alice.command("apitest watched");
        let text = alice.expect::<play::SystemChat>().content.to_plain();
        if text.contains("TARGET ZOMBIE") {
            assert_eq!(text, format!("watched: BUCKET WATER_BUCKET UP, FOOD 19, HELD {held_before} {}, SNEAK true, SWAP WATER_BUCKET, TARGET ZOMBIE PLAYER CLOSEST_PLAYER, WEATHER true COMMAND", (held_before + 3) % 9));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the zombie never looked for Alice: {text}");
        alice.drain(Duration::from_millis(200));
    }
    let after = server
        .handle
        .run(move |s| {
            let p = &s.players[&alice_id];
            let below = BlockPos::new(p.loc.x.floor() as i32, p.loc.y.floor() as i32, p.loc.z.floor() as i32);
            let water = s.worlds[0].block_at(below.x, below.y, below.z).is_some_and(|b| paper_data::blocks::block_of_state(b).name == "minecraft:water");
            let target = s.worlds[0].entities.get(&zombie).and_then(|e| e.mob()).and_then(|m| m.brain.target);
            (p.sneaking, p.held_slot, (p.inventory[36 + p.held_slot as usize].clone(), p.inventory[45].clone()), water, s.worlds[0].weather.raining, p.food, target)
        })
        .unwrap();
    assert_eq!(after, (false, held_before, hands_before, false, false, 20, None), "nothing changed");
    let home = server
        .handle
        .run(move |s| {
            s.worlds[0].entities.get_mut(&zombie).unwrap().removed = true;
            s.set_game_mode(alice_id, paper_server::config::GameMode::Creative);
            s.players[&alice_id].loc
        })
        .unwrap();
    // A trip through a nether portal the plugin stops, and a teleport it moves up.
    let feet = BlockPos::new(home.x.floor() as i32, home.y.floor() as i32, home.z.floor() as i32);
    server.handle.run(move |s| s.worlds[0].set_block(feet.x, feet.y, feet.z, paper_data::blocks::default_state("minecraft:nether_portal").unwrap())).unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        alice.command("apitest watched");
        if alice.expect::<play::SystemChat>().content.to_plain().contains("PORTAL") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no portal trip");
        alice.drain(Duration::from_millis(200));
    }
    let in_overworld = server.handle.run(move |s| {
        s.worlds[0].set_block(feet.x, feet.y, feet.z, paper_data::blocks::AIR);
        s.players[&alice_id].world == 0 && s.players[&alice_id].travel.is_none()
    });
    assert!(in_overworld.unwrap(), "the trip was stopped");
    server.handle.console_command("tp Alice 100 -60 100");
    alice.command("apitest watched");
    expect_line(&mut alice, &format!("watched: BUCKET WATER_BUCKET UP, FOOD 19, HELD {held_before} {}, PORTAL NETHER_PORTAL NETHER, SNEAK true, SWAP WATER_BUCKET, TARGET ZOMBIE PLAYER CLOSEST_PLAYER, TELEPORT COMMAND, WEATHER true COMMAND", (held_before + 3) % 9));
    let at = server.handle.run(move |s| s.players[&alice_id].loc).unwrap();
    assert_eq!((at.x, at.y, at.z), (100.5, -57.0, 100.5), "moved up by the plugin");
    server.handle.run(move |s| s.teleport(alice_id, home)).unwrap();
    alice.command("apitest watch");
    expect_line(&mut alice, "watch: false");

    // --- pistons and fire plugins stop (the test plugin's "blocks") ---------------------------
    // (Without random ticks, unless a step needs them: grass spreading would show too.)
    let normal_speed = server.handle.run(|s| s.worlds[0].rules.int("random_tick_speed")).unwrap();
    random_tick_speed(&server, 0);
    // A sticky piston facing east with stone in front, powered by a redstone block behind it.
    let piston = feet.offset(10, 3, -10);
    let (front, two, behind) = (piston.offset(1, 0, 0), piston.offset(2, 0, 0), piston.offset(-1, 0, 0));
    put(&server, &[(piston, "minecraft:sticky_piston[facing=east,extended=false]"), (front, "minecraft:stone")], false);
    alice.command("apitest blocks cancel");
    expect_line(&mut alice, "blocks: cancel");
    put(&server, &[(behind, "minecraft:redstone_block")], true);
    alice.drain(Duration::from_millis(500));
    assert_eq!(block_state(&server, front), "minecraft:stone", "the push was stopped");
    assert!(block_state(&server, piston).contains("extended=false"));
    alice.command("apitest blockevents");
    expect_line(&mut alice, "blockevents: EXTEND EAST [STONE]");
    // Allowed, it pushes.
    put(&server, &[(behind, "minecraft:air")], true);
    alice.command("apitest blocks record");
    expect_line(&mut alice, "blocks: record");
    put(&server, &[(behind, "minecraft:redstone_block")], true);
    alice.drain(Duration::from_millis(500));
    assert_eq!(block_state(&server, two), "minecraft:stone", "pushed");
    assert!(block_state(&server, piston).contains("extended=true"));
    alice.command("apitest blockevents");
    expect_line(&mut alice, "blockevents: EXTEND EAST [STONE]");
    // A cancelled pull: the piston still retracts and the stone stays, as in Paper.
    alice.command("apitest blocks cancel");
    expect_line(&mut alice, "blocks: cancel");
    put(&server, &[(behind, "minecraft:air")], true);
    alice.drain(Duration::from_millis(500));
    assert!(block_state(&server, piston).contains("extended=false"), "{}", block_state(&server, piston));
    assert_eq!(block_state(&server, front), "minecraft:air");
    assert_eq!(block_state(&server, two), "minecraft:stone", "not pulled");
    alice.command("apitest blockevents");
    expect_line(&mut alice, "blockevents: RETRACT WEST [STONE]");
    // A piston that pulls nothing stays out when its retraction is cancelled.
    let plain = feet.offset(10, 3, -14);
    put(&server, &[(plain, "minecraft:piston[facing=east,extended=false]")], false);
    alice.command("apitest blocks record");
    expect_line(&mut alice, "blocks: record");
    put(&server, &[(plain.offset(-1, 0, 0), "minecraft:redstone_block")], true);
    alice.drain(Duration::from_millis(500));
    assert!(block_state(&server, plain).contains("extended=true"));
    alice.command("apitest blocks cancel");
    expect_line(&mut alice, "blocks: cancel");
    put(&server, &[(plain.offset(-1, 0, 0), "minecraft:air")], true);
    alice.drain(Duration::from_millis(500));
    assert!(block_state(&server, plain).contains("extended=true"), "stays out");
    assert!(block_state(&server, plain.offset(1, 0, 0)).starts_with("minecraft:piston_head"));
    alice.command("apitest blockevents");
    expect_line(&mut alice, "blockevents: RETRACT EAST []");

    // Flint and steel on the ground: the fire is stopped.
    alice.command("apitest blocks cancel");
    expect_line(&mut alice, "blocks: cancel");
    let held = server.handle.run(move |s| s.players[&alice_id].held_slot as usize).unwrap();
    let in_hand = server
        .handle
        .run(move |s| {
            let p = s.players.get_mut(&alice_id).unwrap();
            let in_hand = std::mem::replace(&mut p.inventory[36 + held], Slot::of(paper_data::items::id("minecraft:flint_and_steel").unwrap() as i32, 1));
            s.resync_inventory(alice_id);
            in_hand
        })
        .unwrap();
    let ground = feet.offset(1, -1, 0);
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: ground, face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(93) });
    alice.drain(QUIET);
    assert_eq!(block_state(&server, ground.offset(0, 1, 0)), "minecraft:air", "no fire");
    alice.command("apitest blockevents");
    expect_line(&mut alice, "blockevents: IGNITE FLINT_AND_STEEL player");
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = in_hand;
            s.resync_inventory(alice_id);
        })
        .unwrap();

    // Fire on a floor of planks: plugins stop it burning and spreading, then let it.
    let floor = feet.offset(-12, 4, 12);
    let area = move |dy: std::ops::RangeInclusive<i32>, r: i32| (-r..=r).flat_map(move |dx| dy.clone().flat_map(move |y| (-r..=r).map(move |dz| floor.offset(dx, y, dz))));
    let planks: Vec<_> = area(0..=0, 3).map(|p| (p, "minecraft:oak_planks")).collect();
    put(&server, &planks, false);
    alice.command("apitest blocks cancel");
    expect_line(&mut alice, "blocks: cancel");
    let fires: Vec<_> = area(1..=1, 1).map(|p| (p, "minecraft:fire")).collect();
    put(&server, &fires, true);
    let lit: Vec<BlockPos> = fires.iter().map(|(p, _)| *p).collect();
    let fire_count = move |s: &mut paper_server::server::Server| {
        let (mut planks, mut spread) = (0, 0);
        for p in area(-2..=6, 4) {
            let name = s.worlds[0].block_at(p.x, p.y, p.z).map_or("", |b| paper_data::blocks::block_of_state(b).name);
            planks += (name == "minecraft:oak_planks") as usize;
            spread += (name == "minecraft:fire" && !lit.contains(&p)) as usize;
        }
        (planks, spread)
    };
    let fire_count2 = fire_count.clone();
    let mut seen = String::new();
    for _ in 0..10 {
        run_block_ticks(&server, 1000);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        if seen.contains("BURN OAK_PLANKS by FIRE") {
            break;
        }
    }
    // (The spread's BlockIgniteEvent was cancelled, so its BlockSpreadEvent never fired.)
    assert!(seen.contains("BURN OAK_PLANKS by FIRE") && !seen.contains("SPREAD FIRE ->"), "{seen}");
    // Lava in a stone basin under the floor, ticking fast: it would set the floor alight.
    let lava = floor.offset(0, -2, 0);
    let basin: Vec<_> = [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1), (0, -1, 0)].into_iter().map(|(x, y, z)| (lava.offset(x, y, z), "minecraft:stone")).chain([(lava, "minecraft:lava")]).collect();
    put(&server, &basin, false);
    random_tick_speed(&server, 512);
    for _ in 0..20 {
        run_block_ticks(&server, 50);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        if seen.contains("IGNITE LAVA LAVA") {
            break;
        }
    }
    random_tick_speed(&server, 0);
    assert!(seen.contains("IGNITE LAVA LAVA"), "{seen}");
    assert_eq!(server.handle.run(fire_count).unwrap(), (49, 0), "nothing burned or caught fire");
    alice.command("apitest blocks record");
    expect_line(&mut alice, "blocks: record");
    for _ in 0..30 {
        run_block_ticks(&server, 1000);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        if ["BURN OAK_PLANKS by FIRE", "IGNITE SPREAD FIRE", "SPREAD FIRE -> FIRE"].iter().all(|e| seen.contains(e)) {
            break;
        }
    }
    assert!(["BURN OAK_PLANKS by FIRE", "IGNITE SPREAD FIRE", "SPREAD FIRE -> FIRE"].iter().all(|e| seen.contains(e)), "{seen}");
    let (planks_left, _) = server.handle.run(fire_count2).unwrap();
    assert!(planks_left < 49, "planks burned");
    alice.command("apitest blocks off");
    expect_line(&mut alice, "blocks: off");
    let cleared: Vec<_> =
        area(-2..=6, 4).map(|p| (p, "minecraft:air")).chain([piston, front, two, behind, plain, plain.offset(1, 0, 0), lava.offset(0, -1, 0)].map(|p| (p, "minecraft:air"))).collect();
    put(&server, &cleared, false);

    // Leaves decaying and lava hardening beside water (in stone, so nothing flows): stopped,
    // then let happen.
    let leaves = feet.offset(-6, 5, -6);
    let (water, hot) = (feet.offset(-6, 5, -3), feet.offset(-5, 5, -3));
    let mut basin: Vec<(BlockPos, &str)> = [water, hot].iter().flat_map(|p| [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1), (0, -1, 0)].map(|(x, y, z)| (p.offset(x, y, z), "minecraft:stone"))).collect();
    basin.extend([(water, "minecraft:water"), (leaves, "minecraft:oak_leaves[distance=7,persistent=false]")]);
    put(&server, &basin, false);
    random_tick_speed(&server, 512);
    let name_at = |p: BlockPos| block_state(&server, p).split('[').next().unwrap_or_default().to_owned();
    // (Lava kept from hardening is then washed over by the water, as the fluid rules allow.)
    for (mode, (leaves_left, hardened)) in [("cancel", ("minecraft:oak_leaves", false)), ("record", ("minecraft:air", true))] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        put(&server, &[(hot, "minecraft:air")], false);
        put(&server, &[(hot, "minecraft:lava")], true);
        for _ in 0..20 {
            run_block_ticks(&server, 50);
            alice.command("apitest blockevents");
            seen = line_starting(&mut alice, "blockevents: ");
            if seen.contains("DECAY OAK_LEAVES") || mode == "record" && block_state(&server, leaves) == "minecraft:air" {
                break;
            }
        }
        assert!(seen.contains("FORM LAVA -> OBSIDIAN") && (seen.contains("DECAY OAK_LEAVES") || mode == "record"), "{mode}: {seen}");
        assert_eq!((name_at(leaves).as_str(), name_at(hot) == "minecraft:obsidian"), (leaves_left, hardened), "{mode}");
    }
    let cleared: Vec<_> = basin.iter().map(|(p, _)| (*p, "minecraft:air")).chain([(hot, "minecraft:air")]).collect();
    put(&server, &cleared, false);

    // Water flowing down a well and sand falling into a cup: stopped, then let happen.
    let (spring, sand) = (feet.offset(-10, 6, -10), feet.offset(-13, 6, -10));
    let sides = [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)];
    let mut stone: Vec<(BlockPos, &str)> = [spring, spring.offset(0, -1, 0), sand.offset(0, -1, 0)].iter().flat_map(|p| sides.map(|(x, y, z)| (p.offset(x, y, z), "minecraft:stone"))).collect();
    stone.extend([(spring.offset(0, -2, 0), "minecraft:stone"), (sand.offset(0, -2, 0), "minecraft:stone")]);
    put(&server, &stone, false);
    for (mode, happened) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        put(&server, &[spring, spring.offset(0, -1, 0), sand, sand.offset(0, -1, 0)].map(|p| (p, "minecraft:air")), false);
        put(&server, &[(spring, "minecraft:water"), (sand, "minecraft:sand")], true);
        alice.drain(Duration::from_millis(1000));
        let outcome = (name_at(spring.offset(0, -1, 0)) == "minecraft:water", name_at(sand.offset(0, -1, 0)) == "minecraft:sand", name_at(sand) == "minecraft:sand");
        assert_eq!(outcome, (happened, happened, !happened), "{mode}");
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        let expected: &[&str] = if happened { &["FLOW WATER DOWN", "CHANGE FALLING_BLOCK SAND -> AIR", "CHANGE FALLING_BLOCK AIR -> SAND"] } else { &["FLOW WATER DOWN", "CHANGE FALLING_BLOCK SAND -> AIR"] };
        assert!(expected.iter().all(|e| seen.contains(e)), "{mode}: {seen}");
    }
    let cleared: Vec<_> = stone.iter().map(|(p, _)| *p).chain([spring, spring.offset(0, -1, 0), sand, sand.offset(0, -1, 0)]).map(|p| (p, "minecraft:air")).collect();
    put(&server, &cleared, false);

    // A survival player starting to break stone, being healed, set alight and kicked:
    // stopped, then let happen (all but the kick).
    let stone_at = feet.offset(2, 0, 0);
    put(&server, &[(stone_at, "minecraft:stone")], false);
    server.handle.run(move |s| s.set_game_mode(alice_id, paper_server::config::GameMode::Survival)).unwrap();
    for (mode, happened) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        server.handle.run(move |s| s.players.get_mut(&alice_id).unwrap().health = 10.0).unwrap();
        alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: stone_at, face: 1, sequence: VarInt(95) });
        alice.drain(QUIET);
        // (Healed by how much: a mob about may have hurt her meanwhile.)
        let (digging, healed, burning) = server
            .handle
            .run(move |s| {
                let before = s.players[&alice_id].health;
                s.heal_entity(0, alice_id, 4.0, "MAGIC");
                s.players.get_mut(&alice_id).unwrap().fire_ticks = 0;
                s.set_on_fire(0, alice_id, 100, paper_server::plugins::events::Combuster::Nothing);
                let p = &s.players[&alice_id];
                (p.digging.is_some(), p.health - before, p.fire_ticks > 0)
            })
            .unwrap();
        assert_eq!((digging, healed, burning), (happened, if happened { 4.0 } else { 0.0 }, happened), "{mode}");
        let mut expected = vec!["DAMAGE STONE false", "REGAIN 4.0 MAGIC", "COMBUST 5.0"];
        if mode == "cancel" {
            server.handle.console_command("kick Alice Bye");
            expected.push("KICK KICKED");
        }
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(expected.iter().all(|e| seen.contains(e)), "{mode}: {seen}");
        assert!(server.handle.run(move |s| s.players.contains_key(&alice_id)).unwrap(), "{mode}: still here");
        alice.send(&play::PlayerAction { status: VarInt(play::player_action::ABORT_DESTROY_BLOCK), pos: stone_at, face: 1, sequence: VarInt(96) });
        server.handle.run(move |s| s.players.get_mut(&alice_id).unwrap().fire_ticks = 0).unwrap();
    }
    server
        .handle
        .run(move |s| {
            s.set_game_mode(alice_id, paper_server::config::GameMode::Creative);
            s.players.get_mut(&alice_id).unwrap().health = 20.0;
            s.worlds[0].set_block(stone_at.x, stone_at.y, stone_at.z, paper_data::blocks::AIR);
        })
        .unwrap();

    // A snowball thrown down at the ground, and a dispenser shooting out a stick: stopped,
    // then let happen.
    let (above, dispenser) = (feet.offset(-3, 4, 3), feet.offset(-5, 0, 3));
    let stick = Slot::of(paper_data::items::id("minecraft:stick").unwrap() as i32, 1);
    for (mode, happened) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let snowball = server
            .handle
            .run(move |s| {
                let ball = Slot::of(paper_data::items::id("minecraft:snowball").unwrap() as i32, 1);
                let at = paper_server::world::entity::Vec3::new(above.x as f64 + 0.5, above.y as f64, above.z as f64 + 0.5);
                s.spawn_thrown(0, "minecraft:snowball", at, paper_server::world::entity::Vec3::new(0.0, -1.0, 0.0), 1.0, 0.0, 0.03, ball, None)
            })
            .unwrap();
        assert_eq!(snowball.is_some(), happened, "{mode}: thrown");
        let stick = stick.clone();
        server
            .handle
            .run(move |s| {
                s.worlds[0].set_block(dispenser.x, dispenser.y, dispenser.z, paper_data::blocks::parse_state("minecraft:dispenser[facing=up]").unwrap());
                s.set_container_items(0, dispenser, &[stick]);
                s.set_block_and_update(0, dispenser.offset(1, 0, 0), paper_data::blocks::default_state("minecraft:redstone_block").unwrap());
            })
            .unwrap();
        alice.drain(Duration::from_millis(600));
        let (gone, kept) = server
            .handle
            .run(move |s| (snowball.is_none_or(|b| !s.worlds[0].entities.contains_key(&b)), !s.container_items(0, dispenser)[0].is_empty()))
            .unwrap();
        assert_eq!((gone, kept), (true, !happened), "{mode}: the snowball hit, the stick went");
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        let expected: &[&str] = if happened { &["LAUNCH SNOWBALL", "HIT SNOWBALL - GRASS_BLOCK", "DISPENSE STICK"] } else { &["LAUNCH SNOWBALL", "DISPENSE STICK"] };
        assert!(expected.iter().all(|e| seen.contains(e)), "{mode}: {seen}");
        put(&server, &[(dispenser, "minecraft:air"), (dispenser.offset(1, 0, 0), "minecraft:air")], false);
    }
    random_tick_speed(&server, normal_speed);
    alice.command("apitest blocks off");
    expect_line(&mut alice, "blocks: off");

    // PlayerChangedWorldEvent on the way to the nether and back.
    alice.command("apitest worldhop");
    expect_line(&mut alice, &format!("worldhop: {0} -> {0}_nether, {0}_nether -> {0}", server.handle.run(|s| s.worlds[0].name.clone()).unwrap()));
    // ChunkLoadEvent (new chunks too) and, once chunks nobody needs go, ChunkUnloadEvent.
    server.handle.run(|s| s.unload_unused_chunks()).unwrap();
    alice.command("apitest chunks");
    expect_line(&mut alice, "chunks: true true true");

    // Crafting tables and anvils without a block, as Essentials' /craft and /anvil open them.
    alice.command("apitest stations");
    expect_line(&mut alice, "stations: true WORKBENCH ANVIL");

    // Reading many blocks: a section at a time.
    alice.command("apitest scan");
    expect_line(&mut alice, "scan: 16384 true true true true");

    // The host's main thread waiting for one of its async threads, which calls the server.
    alice.command("apitest asyncwait");
    expect_line(&mut alice, "asyncwait: ok");

    alice.command("apitest attrmods");
    expect_line(&mut alice, "attrmods: true bonus 10.0 mainhand 1 0");
    // Statistics: read and written by plugins, online and offline; the plugin cancels the
    // jump PlayerStatisticIncrementEvent reports, so the count stays.
    alice.command("apitest stats");
    expect_line(&mut alice, "stats: ok");
    let at = server.handle.run(|s| s.player_by_name("Alice").unwrap().loc).unwrap();
    alice.send(&play::MovePlayerPos { x: at.x, y: at.y, z: at.z, flags: play::move_flags::ON_GROUND });
    alice.send(&play::ClientTickEnd {});
    alice.send(&play::MovePlayerPos { x: at.x, y: at.y + 0.42, z: at.z, flags: 0 });
    alice.send(&play::ClientTickEnd {});
    alice.send(&play::MovePlayerPos { x: at.x, y: at.y, z: at.z, flags: play::move_flags::ON_GROUND });
    alice.send(&play::ClientTickEnd {});
    alice.command("apitest statevents");
    expect_line(&mut alice, "statevents: JUMP 7->8 now 7");

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

    // A chest boat a plugin spawns, with what the plugin put in its chest.
    alice.command("apitest chestboat");
    expect_line(&mut alice, "chestboat: ok");
    let diamonds = server
        .handle
        .run(|s| s.worlds[0].entities.values().filter_map(|e| e.chest()).any(|c| c.items[4].0.as_ref().is_some_and(|i| i.count == 3)))
        .unwrap();
    assert!(diamonds, "the chest boat holds the diamonds");

    // A world a plugin makes (`Bukkit.createWorld`): flat as asked, with Alice sent there,
    // and saved in its own folder.
    alice.command("apitest worlds");
    expect_line(&mut alice, "worlds: ok");
    let (index, generator) = server
        .handle
        .run(|s| {
            let index = s.worlds.iter().position(|w| w.name == "plugin_flat").unwrap();
            (index, s.worlds[index].generator.name())
        })
        .unwrap();
    assert_eq!(generator, "flat");
    let deadline = std::time::Instant::now() + TIMEOUT;
    while server.handle.run(|s| s.player_by_name("Alice").unwrap().world).unwrap() != index {
        assert!(std::time::Instant::now() < deadline, "Alice goes to the new world");
        alice.drain(Duration::from_millis(50));
    }

    server.stop();
    assert!(dir.join("plugin_flat").join("level.dat").exists(), "the new world is saved in its folder");
    let _ = std::fs::remove_dir_all(&dir);
}
