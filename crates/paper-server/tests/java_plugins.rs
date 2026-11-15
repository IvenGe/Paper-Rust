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

/// Waits `d` (while mobs move about, draining until quiet can take ten times as long), then
/// reads what came meanwhile.
fn pause(client: &mut Client, d: Duration) {
    std::thread::sleep(d);
    client.drain(Duration::from_millis(10));
}

/// A splash or lingering potion (`kind`) of `potion`.
fn potion_item(kind: &str, potion: &str) -> paper_protocol::types::Slot {
    let id = paper_data::registries::id("minecraft:potion", potion).unwrap() as i32;
    let mut stack = paper_protocol::types::ItemStack::new(paper_data::items::id(kind).unwrap() as i32, 1);
    let mut data = vec![1u8];
    paper_protocol::codec::Encode::encode(&VarInt(id), &mut data);
    data.extend_from_slice(&[0, 0, 0]);
    let ty = paper_data::registries::id("minecraft:data_component_type", "minecraft:potion_contents").unwrap() as i32;
    stack.added.push((ty, data));
    paper_protocol::types::Slot(Some(stack))
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
    expect_line(&mut alice, "interactions: RIGHT_CLICK_BLOCK CHEST UP HAND SNOWBALL, LEFT_CLICK_BLOCK POPPY UP HAND SNOWBALL, LEFT_CLICK_AIR - SELF HAND SNOWBALL, RIGHT_CLICK_AIR - SELF HAND SNOWBALL, ENTITY COW HAND AT 0.0,0.5,0.0");
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

    // Fire on a floor of planks: plugins stop it burning and spreading, then let it. (In
    // clear weather: rain puts fires out.)
    server.handle.console_command("weather clear 1000000");
    server.handle.run(|_| ()).unwrap();
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
    // (Fires burn their planks away and go out within a few hundred ticks, which may be before
    // they spread: fresh ones now and then.)
    for round in 0..30 {
        if round % 3 == 0 {
            put(&server, &planks, false);
            put(&server, &fires, true);
        }
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
    random_tick_speed(&server, 0);
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

    // Bone meal on an oak sapling: the tree (and so the bone meal) stopped, then let grow.
    let sapling = feet.offset(2, 0, -3);
    let held = server.handle.run(move |s| s.players[&alice_id].held_slot as usize).unwrap();
    let in_hand = server
        .handle
        .run(move |s| {
            let p = s.players.get_mut(&alice_id).unwrap();
            let in_hand = std::mem::replace(&mut p.inventory[36 + held], Slot::of(paper_data::items::id("minecraft:bone_meal").unwrap() as i32, 64));
            s.resync_inventory(alice_id);
            in_hand
        })
        .unwrap();
    for (mode, grows) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        put(&server, &[(sapling, "minecraft:oak_sapling[stage=1]")], false);
        // (It grows 45% of the time.)
        for attempt in 0..30 {
            alice.send(&play::UseItemOn { hand: VarInt(0), pos: sapling, face: VarInt(1), cursor_x: 0.5, cursor_y: 0.5, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(100 + attempt) });
            alice.drain(Duration::from_millis(150));
            alice.command("apitest blockevents");
            seen = line_starting(&mut alice, "blockevents: ");
            if seen.contains("STRUCTURE") {
                break;
            }
        }
        assert!(seen.contains("STRUCTURE true player") && seen.contains("FERTILIZE OAK_SAPLING player"), "{mode}: {seen}");
        assert_eq!(name_at(sapling), if grows { "minecraft:oak_log" } else { "minecraft:oak_sapling" }, "{mode}");
        let tree: Vec<_> = (-3..=3).flat_map(|x| (0..=9).flat_map(move |y| (-3..=3).map(move |z| (sapling.offset(x, y, z), "minecraft:air")))).collect();
        put(&server, &tree, false);
    }
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = in_hand;
            s.resync_inventory(alice_id);
        })
        .unwrap();

    // Fire Alice lights in an obsidian frame: the portal stopped (the fire stays), then let
    // be.
    let base = feet.offset(-2, 0, 6);
    let frame: Vec<BlockPos> = (0..4).flat_map(|x| (0..5).map(move |y| base.offset(x, y, 0))).filter(|p| p.x == base.x || p.x == base.x + 3 || p.y == base.y || p.y == base.y + 4).collect();
    let inside: Vec<BlockPos> = (1..3).flat_map(|x| (1..4).map(move |y| base.offset(x, y, 0))).collect();
    for (mode, lit) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let mut blocks: Vec<(BlockPos, &'static str)> = frame.iter().map(|p| (*p, "minecraft:obsidian")).collect();
        blocks.extend(inside.iter().map(|p| (*p, "minecraft:air")));
        put(&server, &blocks, false);
        let fire_at = inside[0];
        server
            .handle
            .run(move |s| {
                let fire = paper_data::blocks::default_state("minecraft:fire").unwrap();
                s.set_block_by(0, fire_at, fire, paper_server::world::block_updates::flags::ALL, Some(alice_id));
            })
            .unwrap();
        alice.drain(QUIET);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("PORTAL FIRE player 6"), "{mode}: {seen}");
        assert_eq!(inside.iter().all(|p| name_at(*p) == "minecraft:nether_portal"), lit, "{mode}: portal");
        if !lit {
            assert_eq!(name_at(fire_at), "minecraft:fire", "the fire stays");
        }
        let gone: Vec<_> = frame.iter().chain(inside.iter()).map(|p| (*p, "minecraft:air")).collect();
        put(&server, &gone, false);
    }

    // A splash potion of swiftness Alice throws onto a cow (WaterBottleSplashEvent comes
    // first, for every potion): stopped, then let reach it.
    let speed = paper_data::effects::by_name("minecraft:speed").unwrap().id;
    for (mode, reaches) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let at = feet.offset(0, 0, 14);
        let cow = server
            .handle
            .run(move |s| {
                let at = paper_server::world::entity::Vec3::new(at.x as f64 + 0.5, at.y as f64, at.z as f64 + 0.5);
                // (Alone: mobs spawn on their own here too.)
                for e in s.worlds[0].entities.values_mut().filter(|e| e.mob().is_some() && e.pos.distance_squared(at) < 100.0) {
                    e.removed = true;
                }
                let cow = s.spawn_mob(0, "minecraft:cow", at, paper_server::server::mobs::SpawnReason::Command).unwrap();
                s.worlds[0].entities.get_mut(&cow).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
                s.spawn_thrown(0, "minecraft:splash_potion", paper_server::world::entity::Vec3::new(at.x, at.y + 2.5, at.z), paper_server::world::entity::Vec3::new(0.0, -1.0, 0.0), 0.5, 0.0, 0.05, potion_item("minecraft:splash_potion", "minecraft:swiftness"), Some(alice_id));
                cow
            })
            .unwrap();
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            alice.command("apitest blockevents");
            seen = line_starting(&mut alice, "blockevents: ");
            if seen.contains("SPLASH Potion") || std::time::Instant::now() > deadline {
                break;
            }
            alice.drain(Duration::from_millis(100));
        }
        assert!(seen.contains("SPLASH PotionSplashEvent [COW] [speed] player") && seen.contains("SPLASH WaterBottleSplashEvent [] [speed] player"), "{mode}: {seen}");
        let fast = server.handle.run(move |s| s.worlds[0].entities[&cow].mob().unwrap().living.effects.iter().any(|(e, _, _)| *e == speed)).unwrap();
        assert_eq!(fast, reaches, "{mode}: swiftness");
        server.handle.run(move |s| s.worlds[0].entities.get_mut(&cow).unwrap().removed = true).unwrap();
    }

    // A lingering potion of regeneration beside a cow: the cloud stopped, then let stay and
    // give the cow its effect (AreaEffectCloudApplyEvent).
    let regeneration = paper_data::effects::by_name("minecraft:regeneration").unwrap().id;
    for (mode, stays) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let at = feet.offset(0, 0, 20);
        let cow = server
            .handle
            .run(move |s| {
                let at = paper_server::world::entity::Vec3::new(at.x as f64 + 0.5, at.y as f64, at.z as f64 + 0.5);
                // (Alone: mobs spawn on their own here too.)
                for e in s.worlds[0].entities.values_mut().filter(|e| e.mob().is_some() && e.pos.distance_squared(at) < 100.0) {
                    e.removed = true;
                }
                let cow = s.spawn_mob(0, "minecraft:cow", at, paper_server::server::mobs::SpawnReason::Command).unwrap();
                s.worlds[0].entities.get_mut(&cow).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
                s.spawn_thrown(0, "minecraft:lingering_potion", paper_server::world::entity::Vec3::new(at.x + 1.5, at.y + 2.5, at.z), paper_server::world::entity::Vec3::new(0.0, -1.0, 0.0), 0.5, 0.0, 0.05, potion_item("minecraft:lingering_potion", "minecraft:regeneration"), Some(alice_id));
                cow
            })
            .unwrap();
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            alice.command("apitest blockevents");
            seen = line_starting(&mut alice, "blockevents: ");
            let done = if stays { seen.contains("CLOUD") } else { seen.contains("LINGER") };
            if done || std::time::Instant::now() > deadline {
                break;
            }
            alice.drain(Duration::from_millis(100));
        }
        assert!(seen.contains("LINGER REGENERATION 3.0"), "{mode}: {seen}");
        let clouds = server.handle.run(|s| s.worlds[0].entities.values().filter(|e| e.info.name == "minecraft:area_effect_cloud" && !e.removed).count()).unwrap();
        assert_eq!(clouds, usize::from(stays), "{mode}: clouds");
        if stays {
            assert!(seen.contains("CLOUD [COW] player"), "{mode}: {seen}");
            let healing = server.handle.run(move |s| s.worlds[0].entities[&cow].mob().unwrap().living.effects.iter().any(|(e, _, _)| *e == regeneration)).unwrap();
            assert!(healing, "the cow gets regeneration");
        }
        server
            .handle
            .run(move |s| {
                for e in s.worlds[0].entities.values_mut().filter(|e| e.id == cow || e.info.name == "minecraft:area_effect_cloud") {
                    e.removed = true;
                }
            })
            .unwrap();
    }

    // A trip through a nether portal whose new portal at the other end plugins stop: the trip
    // ends there.
    alice.command("apitest blocks cancel");
    expect_line(&mut alice, "blocks: cancel");
    server.handle.run(move |s| s.worlds[0].set_block(feet.x, feet.y, feet.z, paper_data::blocks::default_state("minecraft:nether_portal").unwrap())).unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        if seen.contains("PORTAL NETHER_PAIR") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no portal built: {seen}");
        alice.drain(Duration::from_millis(200));
    }
    let stayed = server.handle.run(move |s| {
        s.worlds[0].set_block(feet.x, feet.y, feet.z, paper_data::blocks::AIR);
        s.players[&alice_id].world == 0
    });
    assert!(stayed.unwrap(), "the trip ended");
    assert!(seen.contains("PORTAL NETHER_PAIR player 6"), "{seen}");

    // Shearing a sheep (in survival, where shears wear): stopped, shears and all; then let be,
    // with the diamond the plugin drops instead of the wool, the shears wearing a point.
    let held = server.handle.run(move |s| s.players[&alice_id].held_slot as usize).unwrap();
    server.handle.run(move |s| s.set_game_mode(alice_id, paper_server::config::GameMode::Survival)).unwrap();
    for (mode, sheared) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let at = feet.offset(3, 0, 3);
        // (On stone: grass would let it grow its wool back.)
        put(&server, &[(at.offset(0, -1, 0), "minecraft:stone")], false);
        let sheep = server
            .handle
            .run(move |s| {
                let at = paper_server::world::entity::Vec3::new(at.x as f64 + 0.5, at.y as f64, at.z as f64 + 0.5);
                for e in s.worlds[0].entities.values_mut().filter(|e| e.pos.distance_squared(at) < 36.0 && (e.mob().is_some() || matches!(e.kind, EntityKind::Item { .. }))) {
                    e.removed = true;
                }
                let sheep = s.spawn_mob(0, "minecraft:sheep", at, paper_server::server::mobs::SpawnReason::Command).unwrap();
                s.worlds[0].entities.get_mut(&sheep).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
                s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::of(paper_data::items::id("minecraft:shears").unwrap() as i32, 1);
                s.resync_inventory(alice_id);
                sheep
            })
            .unwrap();
        pause(&mut alice, Duration::from_millis(150));
        alice.send(&play::Interact { entity_id: VarInt(sheep), hand: VarInt(0), target: paper_protocol::types::LpVec3(0.0, 0.5, 0.0), sneaking: false });
        pause(&mut alice, QUIET);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("SHEAR SHEEP SHEARS HAND "), "{mode}: {seen}");
        assert_eq!(seen.contains("WEAR SHEARS 1 1"), sheared, "{mode}: {seen}");
        let (shorn, damage, diamonds, wool) = server
            .handle
            .run(move |s| {
                let shorn = s.worlds[0].entities[&sheep].mob().unwrap().sheared;
                let damage = s.players[&alice_id].inventory[36 + held].0.as_ref().map_or(-1, paper_server::server::survival::item_damage);
                let near = |e: &&paper_server::world::entity::Entity, item: &str| !e.removed && e.pos.distance_squared(s.worlds[0].entities[&sheep].pos) < 16.0 && matches!(&e.kind, EntityKind::Item { stack: Slot(Some(st)), .. } if paper_data::items::get(st.item as u32).is_some_and(|i| i.name == item));
                let diamonds = s.worlds[0].entities.values().filter(|e| near(e, "minecraft:diamond")).count();
                let wool = s.worlds[0].entities.values().filter(|e| near(e, "minecraft:white_wool")).count();
                (shorn, damage, diamonds, wool)
            })
            .unwrap();
        assert_eq!((shorn, damage, diamonds, wool), if sheared { (true, 1, 1, 0) } else { (false, 0, 0, 0) }, "{mode}");
        server.handle.run(move |s| s.worlds[0].entities.get_mut(&sheep).unwrap().removed = true).unwrap();
    }
    server
        .handle
        .run(move |s| {
            s.set_game_mode(alice_id, paper_server::config::GameMode::Creative);
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::EMPTY;
            s.resync_inventory(alice_id);
        })
        .unwrap();

    // A bed by day: stopped; then the plugin puts Alice in it all the same, and daylight gets
    // her out again (PlayerBedLeaveEvent).
    server.handle.console_command("time set 1000");
    let (foot, head) = (feet.offset(1, 0, 2), feet.offset(1, 0, 3));
    for (mode, slept) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        put(&server, &[(foot, "minecraft:red_bed[facing=south,part=foot]"), (head, "minecraft:red_bed[facing=south,part=head]")], false);
        server.handle.run(move |s| s.teleport(alice_id, home)).unwrap();
        pause(&mut alice, Duration::from_millis(150));
        alice.send(&play::UseItemOn { hand: VarInt(0), pos: head, face: VarInt(1), cursor_x: 0.5, cursor_y: 0.5, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(140) });
        pause(&mut alice, QUIET);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("BED NOT_POSSIBLE_NOW false true"), "{mode}: {seen}");
        assert_eq!(seen.contains("WAKE true"), slept, "{mode}: {seen}");
        let sleeping = server.handle.run(move |s| s.players[&alice_id].sleeping.is_some()).unwrap();
        assert!(!sleeping, "{mode}: awake");
        put(&server, &[(foot, "minecraft:air"), (head, "minecraft:air")], false);
    }
    server.handle.run(move |s| s.teleport(alice_id, home)).unwrap();

    // A new main hand and language from the client.
    alice.command("apitest blocks record");
    expect_line(&mut alice, "blocks: record");
    alice.send(&play::ClientInformation { settings: paper_protocol::packets::common::ClientSettings { locale: "fr_fr".into(), main_hand: 0, ..Default::default() } });
    pause(&mut alice, QUIET);
    alice.command("apitest blockevents");
    seen = line_starting(&mut alice, "blockevents: ");
    assert!(seen.contains("HAND LEFT") && seen.contains("LOCALE fr_fr"), "{seen}");
    alice.command("apitest blocks off");
    expect_line(&mut alice, "blocks: off");
    alice.send(&play::ClientInformation { settings: paper_protocol::packets::common::ClientSettings::default() });

    // An arrow in the ground at Alice's feet: left there, then picked up.
    for (mode, taken) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let arrow = server
            .handle
            .run(move |s| {
                let at = s.players[&alice_id].loc;
                let id = s.spawn_arrow(0, paper_server::world::entity::Vec3::new(at.x, at.y + 0.1, at.z), 0.0, 90.0, 0.1, 0.0, None, 2.0, false, true);
                let e = s.worlds[0].entities.get_mut(&id).unwrap();
                if let EntityKind::Arrow { in_ground, .. } = &mut e.kind {
                    *in_ground = true;
                }
                id
            })
            .unwrap();
        pause(&mut alice, QUIET);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("ARROW ARROW ARROW"), "{mode}: {seen}");
        let left = server.handle.run(move |s| s.worlds[0].entities.get(&arrow).is_some_and(|e| !e.removed)).unwrap();
        assert_eq!(left, !taken, "{mode}: the arrow");
        server.handle.run(move |s| if let Some(e) = s.worlds[0].entities.get_mut(&arrow) { e.removed = true }).unwrap();
    }

    // Alice's egg: the plugin makes two chicks hatch ("cancelling"); her bottle o' enchanting
    // gives no experience then, and some when recording.
    let ground = feet.offset(-12, 0, -2);
    for (mode, chicks) in [("cancel", true), ("record", false)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let before = server.handle.run(move |s| s.worlds[0].entities.values().filter(|e| e.info.name == "minecraft:chicken" && !e.removed).count()).unwrap();
        server
            .handle
            .run(move |s| {
                let at = paper_server::world::entity::Vec3::new(ground.x as f64 + 0.5, ground.y as f64 + 2.0, ground.z as f64 + 0.5);
                for e in s.worlds[0].entities.values_mut().filter(|e| e.info.name == "minecraft:experience_orb") {
                    e.removed = true;
                }
                s.spawn_thrown(0, "minecraft:egg", at, paper_server::world::entity::Vec3::new(0.0, -1.0, 0.0), 0.5, 0.0, 0.03, Slot::of(paper_data::items::id("minecraft:egg").unwrap() as i32, 1), Some(alice_id));
                s.spawn_thrown(0, "minecraft:experience_bottle", paper_server::world::entity::Vec3::new(at.x + 3.0, at.y, at.z), paper_server::world::entity::Vec3::new(0.0, -1.0, 0.0), 0.5, 0.0, 0.07, Slot::of(paper_data::items::id("minecraft:experience_bottle").unwrap() as i32, 1), Some(alice_id));
            })
            .unwrap();
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            alice.command("apitest blockevents");
            seen = line_starting(&mut alice, "blockevents: ");
            if seen.contains("HATCH") && seen.contains("XP") || std::time::Instant::now() > deadline {
                break;
            }
            alice.drain(Duration::from_millis(100));
        }
        assert!(seen.contains("EGG CHICKEN") && seen.contains("XP true"), "{mode}: {seen}");
        let (hatched, orbs) = server
            .handle
            .run(move |s| {
                let w = &s.worlds[0];
                (w.entities.values().filter(|e| e.info.name == "minecraft:chicken" && !e.removed).count(), w.entities.values().filter(|e| e.info.name == "minecraft:experience_orb" && !e.removed).count())
            })
            .unwrap();
        if chicks {
            assert!(seen.contains("HATCH 2") && hatched == before + 2, "{mode}: {seen}, {before} then {hatched} chickens");
            assert_eq!(orbs, 0, "{mode}: no experience");
        } else {
            assert!(orbs > 0, "{mode}: experience");
        }
        // (The chicks would keep the connection busy.)
        server
            .handle
            .run(move |s| {
                for e in s.worlds[0].entities.values_mut().filter(|e| matches!(e.info.name.as_str(), "minecraft:chicken" | "minecraft:experience_orb")) {
                    e.removed = true;
                }
            })
            .unwrap();
    }

    // Two emeralds on the ground: kept apart, then let merge; then old enough to go: kept
    // (and young again), then let go.
    let pile = feet.offset(6, 0, -2);
    let spawn_emerald = move |s: &mut paper_server::server::Server, dx: f64| {
        let at = paper_server::world::entity::Vec3::new(pile.x as f64 + 0.5 + dx, pile.y as f64, pile.z as f64 + 0.5);
        s.spawn_item(0, at, Slot::of(paper_data::items::id("minecraft:emerald").unwrap() as i32, 1), paper_server::world::entity::Vec3::new(0.0, 0.0, 0.0), 1000).unwrap()
    };
    let emeralds = move |s: &paper_server::server::Server| -> Vec<(i32, i32, i32)> {
        s.worlds[0].entities.values().filter(|e| !e.removed).filter_map(|e| match &e.kind {
            EntityKind::Item { stack: Slot(Some(st)), .. } if paper_data::items::get(st.item as u32).is_some_and(|i| i.name == "minecraft:emerald") => Some((e.id, st.count, e.age)),
            _ => None,
        }).collect()
    };
    for (mode, merged) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        server
            .handle
            .run(move |s| {
                for e in s.worlds[0].entities.values_mut().filter(|e| matches!(e.kind, EntityKind::Item { .. })) {
                    e.removed = true;
                }
                spawn_emerald(s, 0.0);
                spawn_emerald(s, 0.2);
            })
            .unwrap();
        pause(&mut alice, QUIET);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("MERGE 1 1"), "{mode}: {seen}");
        let piles: Vec<i32> = server.handle.run(move |s| emeralds(s).into_iter().map(|(_, n, _)| n).collect()).unwrap();
        assert_eq!(piles.len(), if merged { 1 } else { 2 }, "{mode}: {piles:?}");
    }
    for (mode, kept) in [("cancel", true), ("record", false)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let id = server
            .handle
            .run(move |s| {
                let (id, _, _) = emeralds(s)[0];
                s.worlds[0].entities.get_mut(&id).unwrap().age = 5990;
                id
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        alice.drain(Duration::from_millis(50));
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("DESPAWN 2"), "{mode}: {seen}");
        // (Kept, it is young again: well under the 5990 ticks it was given.)
        let age = server.handle.run(move |s| s.worlds[0].entities.get(&id).filter(|e| !e.removed).map(|e| e.age)).unwrap();
        assert_eq!(age.is_some_and(|a| a < 3000), kept, "{mode}: {age:?}");
    }
    server
        .handle
        .run(move |s| {
            for e in s.worlds[0].entities.values_mut().filter(|e| matches!(e.kind, EntityKind::Item { .. })) {
                e.removed = true;
            }
        })
        .unwrap();

    // TNT a redstone block powers, and TNT Alice lights: left be. Then the redstone sets it
    // off, and its blast is stopped (ExplosionPrimeEvent).
    let (tnt, power) = (feet.offset(-2, 0, 4), feet.offset(-3, 0, 4));
    alice.command("apitest blocks cancel");
    expect_line(&mut alice, "blocks: cancel");
    put(&server, &[(tnt, "minecraft:tnt")], false);
    put(&server, &[(power, "minecraft:redstone_block")], true);
    pause(&mut alice, QUIET);
    let held = server.handle.run(move |s| s.players[&alice_id].held_slot as usize).unwrap();
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::of(paper_data::items::id("minecraft:flint_and_steel").unwrap() as i32, 1);
            s.resync_inventory(alice_id);
            s.teleport(alice_id, home);
        })
        .unwrap();
    pause(&mut alice, Duration::from_millis(150));
    alice.send(&play::UseItemOn { hand: VarInt(0), pos: tnt, face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(150) });
    pause(&mut alice, QUIET);
    alice.command("apitest blockevents");
    seen = line_starting(&mut alice, "blockevents: ");
    assert!(seen.contains("TNT REDSTONE - -") && seen.contains("TNT PLAYER PLAYER -"), "{seen}");
    assert_eq!(name_at(tnt), "minecraft:tnt", "left be");
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::EMPTY;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    alice.command("apitest blocks record");
    expect_line(&mut alice, "blocks: record");
    put(&server, &[(power, "minecraft:air")], true);
    put(&server, &[(power, "minecraft:redstone_block")], true);
    pause(&mut alice, QUIET);
    assert_eq!(name_at(tnt), "minecraft:air", "set off");
    alice.command("apitest blocks cancel");
    expect_line(&mut alice, "blocks: cancel");
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        if seen.contains("BOOM") || std::time::Instant::now() > deadline {
            break;
        }
        pause(&mut alice, Duration::from_millis(250));
    }
    assert!(seen.contains("BOOM TNT 4.0 false"), "{seen}");
    pause(&mut alice, QUIET);
    assert_eq!(name_at(power), "minecraft:redstone_block", "no blast");
    put(&server, &[(power, "minecraft:air")], false);

    // A bolt a plugin strikes beside a creeper, a pig and a villager: stopped; then let strike,
    // which charges the creeper, and turns the pig into a zombified piglin and the villager into a
    // witch.
    let spot = feet.offset(16, 0, 0);
    let near_spot = move |s: &paper_server::server::Server, name: &str| -> Vec<i32> {
        let at = paper_server::world::entity::Vec3::new(spot.x as f64 + 0.5, spot.y as f64, spot.z as f64 + 0.5);
        s.worlds[0].entities.values().filter(|e| !e.removed && e.info.name == name && e.pos.distance_squared(at) < 25.0).map(|e| e.id).collect()
    };
    for (mode, struck) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let creeper = server
            .handle
            .run(move |s| {
                let at = paper_server::world::entity::Vec3::new(spot.x as f64 + 0.5, spot.y as f64, spot.z as f64 + 0.5);
                for e in s.worlds[0].entities.values_mut().filter(|e| e.mob().is_some() && e.pos.distance_squared(at) < 64.0) {
                    e.removed = true;
                }
                let mut spawn = |name: &str, dx: f64, dz: f64| {
                    let id = s.spawn_mob(0, name, paper_server::world::entity::Vec3::new(at.x + dx, at.y, at.z + dz), paper_server::server::mobs::SpawnReason::Command).unwrap();
                    s.worlds[0].entities.get_mut(&id).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
                    id
                };
                let creeper = spawn("minecraft:creeper", 1.0, 0.0);
                spawn("minecraft:pig", -1.0, 0.0);
                spawn("minecraft:villager", 0.0, 1.0);
                s.strike_lightning(0, spot, "CUSTOM");
                creeper
            })
            .unwrap();
        pause(&mut alice, QUIET);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("LIGHTNING CUSTOM false"), "{mode}: {seen}");
        let (powered, pigs, piglins, villagers, witches) = server
            .handle
            .run(move |s| {
                let powered = s.worlds[0].entities.get(&creeper).and_then(|e| e.mob()).is_some_and(|m| m.powered);
                (powered, near_spot(s, "minecraft:pig").len(), near_spot(s, "minecraft:zombified_piglin").len(), near_spot(s, "minecraft:villager").len(), near_spot(s, "minecraft:witch").len())
            })
            .unwrap();
        if struck {
            assert!(seen.contains("POWER LIGHTNING true") && seen.contains("PIGZAP ZOMBIFIED_PIGLIN") && seen.contains("ZAP VILLAGER WITCH") && seen.contains("TRANSFORM VILLAGER WITCH LIGHTNING"), "{mode}: {seen}");
            assert_eq!((powered, pigs, piglins, villagers, witches), (true, 0, 1, 0, 1), "{mode}");
        } else {
            assert_eq!((powered, pigs, piglins, villagers, witches), (false, 1, 0, 1, 0), "{mode}");
        }
    }
    server
        .handle
        .run(move |s| {
            for name in ["minecraft:creeper", "minecraft:pig", "minecraft:zombified_piglin", "minecraft:villager", "minecraft:witch", "minecraft:lightning_bolt"] {
                for id in near_spot(s, name) {
                    s.worlds[0].entities.get_mut(&id).unwrap().removed = true;
                }
            }
        })
        .unwrap();

    // A zombie in the midday sun: kept from catching fire; then let burn (EntityCombustEvent).
    server.handle.console_command("time set 6000");
    // (Rain would put it out: the weather cycle may start some at any time.)
    server.handle.console_command("weather clear");
    for (mode, burns) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let zombie = server
            .handle
            .run(move |s| {
                let at = paper_server::world::entity::Vec3::new(spot.x as f64 + 0.5, spot.y as f64, spot.z as f64 + 4.5);
                let id = s.spawn_mob(0, "minecraft:zombie", at, paper_server::server::mobs::SpawnReason::Command).unwrap();
                let e = s.worlds[0].entities.get_mut(&id).unwrap();
                e.flags.tags.push("burn".into());
                let m = e.mob_mut().unwrap();
                m.brain.idle_ticks = 1_000_000;
                m.equipment[5] = Slot::EMPTY;
                id
            })
            .unwrap();
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            alice.command("apitest blockevents");
            seen = line_starting(&mut alice, "blockevents: ");
            if seen.contains("COMBUST") || std::time::Instant::now() > deadline {
                break;
            }
            pause(&mut alice, Duration::from_millis(200));
        }
        assert!(seen.contains("COMBUST 8.0"), "{mode}: {seen}");
        let (fire, state) = server
            .handle
            .run(move |s| match s.worlds[0].entities.get(&zombie) {
                Some(e) => (e.mob().map_or(0, |m| m.living.fire_ticks), format!("removed {} at {:?}, health {:?}", e.removed, e.pos, e.mob().map(|m| m.living.health))),
                None => (0, "gone".to_owned()),
            })
            .unwrap();
        assert_eq!(fire > 0, burns, "{mode}: fire {fire}, {state}, {seen}");
        server.handle.run(move |s| s.worlds[0].entities.get_mut(&zombie).unwrap().removed = true).unwrap();
    }

    // A boat Alice puts down (EntityPlaceEvent, VehicleCreateEvent): taken back, then let be.
    // She gets in and out (VehicleEnterEvent, VehicleExitEvent), steers it along
    // (VehicleMoveEvent), and breaks it (VehicleDamageEvent, VehicleDestroyEvent): each
    // stopped first.
    let held = server.handle.run(move |s| s.players[&alice_id].held_slot as usize).unwrap();
    server
        .handle
        .run(move |s| {
            s.teleport(alice_id, paper_server::server::player::Location { yaw: 0.0, pitch: 50.0, ..home });
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::of(paper_data::items::id("minecraft:oak_boat").unwrap() as i32, 1);
            s.resync_inventory(alice_id);
        })
        .unwrap();
    let boats = move |s: &paper_server::server::Server| -> Vec<i32> {
        let at = paper_server::world::entity::Vec3::new(home.x, home.y, home.z);
        s.worlds[0].entities.values().filter(|e| !e.removed && e.info.name == "minecraft:oak_boat" && e.pos.distance_squared(at) < 36.0).map(|e| e.id).collect()
    };
    pause(&mut alice, Duration::from_millis(150));
    for (mode, placed) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(160), yaw: 0.0, pitch: 50.0 });
        pause(&mut alice, QUIET);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("PLACE OAK_BOAT UP HAND true") && seen.contains("VCREATE OAK_BOAT") == placed, "{mode}: {seen}");
        assert_eq!(server.handle.run(move |s| boats(s).len()).unwrap(), usize::from(placed), "{mode}: boats");
    }
    let boat = server.handle.run(move |s| boats(s)[0]).unwrap();
    for (mode, rides) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        alice.send(&play::Interact { entity_id: VarInt(boat), hand: VarInt(0), target: paper_protocol::types::LpVec3(0.0, 0.3, 0.0), sneaking: false });
        pause(&mut alice, QUIET);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("ENTER OAK_BOAT PLAYER"), "{mode}: {seen}");
        assert_eq!(server.handle.run(move |s| s.players[&alice_id].vehicle).unwrap() == Some(boat), rides, "{mode}: in the boat");
    }
    let at = server.handle.run(move |s| s.worlds[0].entities[&boat].pos).unwrap();
    alice.send(&play::MoveVehicle { x: at.x, y: at.y, z: at.z + 0.5, yaw: 0.0, pitch: 0.0, on_ground: true });
    pause(&mut alice, QUIET);
    alice.command("apitest blockevents");
    seen = line_starting(&mut alice, "blockevents: ");
    assert!(seen.contains("VMOVE OAK_BOAT"), "{seen}");
    for (mode, out) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        alice.send(&play::PlayerInput { flags: play::input_flags::SNEAK });
        pause(&mut alice, QUIET);
        alice.send(&play::PlayerInput { flags: 0 });
        pause(&mut alice, Duration::from_millis(150));
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("EXIT OAK_BOAT PLAYER"), "{mode}: {seen}");
        assert_eq!(server.handle.run(move |s| s.players[&alice_id].vehicle.is_none()).unwrap(), out, "{mode}: out of the boat");
    }
    for (mode, broken) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        alice.send(&play::Attack { entity_id: VarInt(boat) });
        pause(&mut alice, QUIET);
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("VDAMAGE OAK_BOAT PLAYER") && seen.contains("VDESTROY OAK_BOAT PLAYER") == broken, "{mode}: {seen}");
        assert_eq!(server.handle.run(move |s| s.worlds[0].entities.get(&boat).is_none_or(|e| e.removed)).unwrap(), broken, "{mode}: broken");
    }
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::EMPTY;
            s.resync_inventory(alice_id);
            s.teleport(alice_id, home);
        })
        .unwrap();

    // Effects by command (EntityPotionEffectEvent): haste given (refused first), made
    // stronger, not made weaker, and taken away.
    let op = server
        .handle
        .run(move |s| {
            let p = s.players.get_mut(&alice_id).unwrap();
            p.effects.retain(|e| e.name() != "minecraft:haste");
            std::mem::replace(&mut p.op_level, 4)
        })
        .unwrap();
    let haste = move |s: &mut paper_server::server::Server| s.players[&alice_id].effects.iter().find(|e| e.name() == "minecraft:haste").map(|e| e.amplifier);
    for (mode, given) in [("cancel", None), ("record", Some(0))] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        alice.command("effect give Alice minecraft:haste 60 0");
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("EFFECT haste ADDED - 0 true"), "{mode}: {seen}");
        assert_eq!(server.handle.run(haste).unwrap(), given, "{mode}: haste");
    }
    alice.command("effect give Alice minecraft:haste 60 2");
    alice.command("effect give Alice minecraft:haste 60 1");
    alice.command("apitest blockevents");
    seen = line_starting(&mut alice, "blockevents: ");
    assert!(seen.contains("EFFECT haste CHANGED 0 2 true") && seen.contains("EFFECT haste CHANGED 2 1 false"), "{seen}");
    assert_eq!(server.handle.run(haste).unwrap(), Some(2), "a weaker effect does not replace a stronger one");
    alice.command("effect clear Alice minecraft:haste");
    alice.command("apitest blockevents");
    seen = line_starting(&mut alice, "blockevents: ");
    assert!(seen.contains("EFFECT haste REMOVED 2 - false"), "{seen}");
    assert_eq!(server.handle.run(haste).unwrap(), None);
    server.handle.run(move |s| s.players.get_mut(&alice_id).unwrap().op_level = op).unwrap();

    // A bow drawn and let go (EntityShootBowEvent): the shot refused, then let fly. In
    // creative, nothing is used up.
    let bow = Slot::of(paper_data::items::id("minecraft:bow").unwrap() as i32, 1);
    server
        .handle
        .run(move |s| {
            s.teleport(alice_id, paper_server::server::player::Location { yaw: 0.0, pitch: -30.0, ..home });
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = bow;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    let arrows = move |s: &mut paper_server::server::Server| -> Vec<i32> {
        s.worlds[0].entities.values().filter(|e| !e.removed && matches!(e.kind, EntityKind::Arrow { owner: Some(o), .. } if o == alice_id)).map(|e| e.id).collect()
    };
    for (mode, shot) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        server
            .handle
            .run(move |s| {
                for id in arrows(s) {
                    s.worlds[0].entities.get_mut(&id).unwrap().removed = true;
                }
            })
            .unwrap();
        pause(&mut alice, Duration::from_millis(100));
        alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(170), yaw: 0.0, pitch: -30.0 });
        pause(&mut alice, Duration::from_millis(400));
        alice.send(&play::PlayerAction { status: VarInt(play::player_action::RELEASE_USE_ITEM), pos: BlockPos::new(0, 0, 0), face: 0, sequence: VarInt(0) });
        pause(&mut alice, Duration::from_millis(100));
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("BOW BOW ARROW ARROW HAND false"), "{mode}: {seen}");
        assert_eq!(server.handle.run(move |s| arrows(s).len()).unwrap(), usize::from(shot), "{mode}: arrows");
    }

    // An amethyst shard coming into the world (ItemSpawnEvent): refused, then let be.
    let shard = paper_data::items::id("minecraft:amethyst_shard").unwrap() as i32;
    for (mode, stays) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let spawned = server
            .handle
            .run(move |s| {
                let at = paper_server::world::entity::Vec3::new(home.x + 2.0, home.y + 0.5, home.z - 3.0);
                let id = s.spawn_item(0, at, Slot::of(shard, 3), paper_server::world::entity::Vec3::new(0.0, 0.0, 0.0), 1000);
                (id, id.is_some_and(|id| s.worlds[0].entities.contains_key(&id)))
            })
            .unwrap();
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("ITEMSPAWN 3"), "{mode}: {seen}");
        assert_eq!((spawned.0.is_some(), spawned.1), (stays, stays), "{mode}: the shard");
        if let Some(id) = spawned.0 {
            server.handle.run(move |s| s.worlds[0].entities.get_mut(&id).unwrap().removed = true).unwrap();
        }
    }

    // A spawner's pigs (SpawnerSpawnEvent): refused, then let come.
    let spawner = feet.offset(-6, 0, -6);
    let pigs = move |s: &mut paper_server::server::Server| {
        let center = (spawner.x as f64 + 0.5, spawner.z as f64 + 0.5);
        s.worlds[0].entities.values().filter(|e| !e.removed && e.info.name == "minecraft:pig" && (e.pos.x - center.0).abs() <= 5.0 && (e.pos.z - center.1).abs() <= 5.0).count()
    };
    for (mode, come) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        server
            .handle
            .run(move |s| {
                for e in s.worlds[0].entities.values_mut().filter(|e| e.info.name == "minecraft:pig") {
                    e.removed = true;
                }
                s.set_block_and_update(0, spawner, paper_data::blocks::default_state("minecraft:spawner").unwrap());
                let chunk = paper_server::world::chunk::ChunkPos::of_block(spawner.x, spawner.z);
                let pig = paper_nbt::Compound::new().with("id", paper_nbt::Tag::String("minecraft:pig".into()));
                let nbt = paper_nbt::Compound::new()
                    .with("id", paper_nbt::Tag::String("minecraft:mob_spawner".into()))
                    .with("SpawnData", paper_nbt::Tag::Compound(paper_nbt::Compound::new().with("entity", paper_nbt::Tag::Compound(pig))))
                    .with("Delay", paper_nbt::Tag::Short(0));
                s.worlds[0].chunks.get_mut(&chunk).unwrap().block_entities.insert((spawner.y, (spawner.z & 15) as u8, (spawner.x & 15) as u8), nbt);
            })
            .unwrap();
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            alice.command("apitest blockevents");
            seen = line_starting(&mut alice, "blockevents: ");
            if seen.contains("SPAWNER PIG PIG") || std::time::Instant::now() > deadline {
                break;
            }
            pause(&mut alice, Duration::from_millis(100));
        }
        assert!(seen.contains("SPAWNER PIG PIG") && seen.contains("PRESPAWN PIG SPAWNER"), "{mode}: {seen}");
        assert_eq!(server.handle.run(pigs).unwrap() > 0, come, "{mode}: pigs");
    }
    put(&server, &[(spawner, "minecraft:air")], false);
    server
        .handle
        .run(move |s| {
            for e in s.worlds[0].entities.values_mut().filter(|e| e.info.name == "minecraft:pig") {
                e.removed = true;
            }
        })
        .unwrap();

    // A sign Alice puts down and then clicks (Paper's PlayerOpenSignEvent, then Bukkit's
    // PlayerSignOpenEvent): its editor kept shut, then let open.
    let (ground, sign_at) = (feet.offset(1, -1, -2), feet.offset(1, 0, -2));
    let oak_sign = Slot::of(paper_data::items::id("minecraft:oak_sign").unwrap() as i32, 1);
    server
        .handle
        .run(move |s| {
            // Facing north, towards the sign, which then faces her.
            s.teleport(alice_id, paper_server::server::player::Location { yaw: 180.0, pitch: 30.0, ..home });
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = oak_sign;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    for (mode, opens) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        pause(&mut alice, Duration::from_millis(100));
        alice.clear_backlog();
        let (clicked, cause) = if opens { (sign_at, "INTERACT") } else { (ground, "PLACE") };
        if opens {
            // (With an empty hand: a click, not another sign.)
            server
                .handle
                .run(move |s| {
                    s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::EMPTY;
                    s.resync_inventory(alice_id);
                })
                .unwrap();
        }
        alice.send(&play::UseItemOn { hand: VarInt(0), pos: clicked, face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(180) });
        let editor = alice.find(Duration::from_millis(500), |e: &play::OpenSignEditor| e.pos == sign_at);
        assert_eq!(editor.is_some(), opens, "{mode}: the editor");
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains(&format!("OPENSIGN {cause} FRONT OAK_SIGN")) && seen.contains(&format!("SIGNOPEN {cause} FRONT")) == opens, "{mode}: {seen}");
    }
    put(&server, &[(sign_at, "minecraft:air")], false);

    // A command nobody has (UnknownCommandEvent): the plugin words the reply, or keeps quiet.
    alice.clear_backlog();
    alice.command("nosuch thing");
    let reply = line_starting(&mut alice, "No /nosuch");
    assert_eq!(reply, "No /nosuch thing for Alice");
    alice.command("nosuch quiet");
    alice.command("apitest blockevents");
    loop {
        let text = alice.expect::<play::SystemChat>().content.to_plain();
        if text.starts_with("blockevents: ") {
            break;
        }
        assert!(!text.contains("nosuch") && !text.contains("Unknown"), "{text}");
    }

    // A dropper about to drop an amethyst shard (BlockPreDispenseEvent): stopped, then let
    // go; and an empty one firing (BlockFailedDispenseEvent): kept quiet, then let click.
    let dropper = feet.offset(-5, 0, 3);
    let facing_up = paper_data::blocks::parse_state("minecraft:dropper[facing=up]").unwrap();
    let power = paper_data::blocks::default_state("minecraft:redstone_block").unwrap();
    for (mode, dropped) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        server
            .handle
            .run(move |s| {
                s.worlds[0].set_block(dropper.x, dropper.y, dropper.z, facing_up);
                s.set_container_items(0, dropper, &[Slot::of(shard, 1)]);
                s.set_block_and_update(0, dropper.offset(1, 0, 0), power);
            })
            .unwrap();
        pause(&mut alice, Duration::from_millis(500));
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("PREDISPENSE AMETHYST_SHARD 0") && seen.contains("ITEMSPAWN 1") == dropped, "{mode}: {seen}");
        let emptied = server.handle.run(move |s| s.container_items(0, dropper)[0].is_empty()).unwrap();
        assert_eq!(emptied, dropped, "{mode}: the shard went");
        server.handle.run(move |s| s.set_container_items(0, dropper, &[Slot::EMPTY])).unwrap();
        put(&server, &[(dropper, "minecraft:air"), (dropper.offset(1, 0, 0), "minecraft:air")], false);
    }
    for (mode, clicks) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        alice.clear_backlog();
        server
            .handle
            .run(move |s| {
                s.worlds[0].set_block(dropper.x, dropper.y, dropper.z, facing_up);
                s.set_block_and_update(0, dropper.offset(1, 0, 0), power);
            })
            .unwrap();
        let click = alice.find(Duration::from_millis(800), |e: &play::LevelEvent| e.event == 1001 && e.pos == dropper);
        assert_eq!(click.is_some(), clicks, "{mode}: the click");
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("FAILDISPENSE DROPPER"), "{mode}: {seen}");
        put(&server, &[(dropper, "minecraft:air"), (dropper.offset(1, 0, 0), "minecraft:air")], false);
    }
    server
        .handle
        .run(move |s| {
            for e in s.worlds[0].entities.values_mut().filter(|e| matches!(e.kind, EntityKind::Item { .. })) {
                e.removed = true;
            }
        })
        .unwrap();

    // Nether quartz through a hopper (InventoryMoveItemEvent): pulled out of a barrel above
    // and pushed into a chest beside it, stopped and then let go. Then quartz lying on the
    // hopper (InventoryPickupItemEvent), and a dropper filling the chest.
    let quartz = paper_data::items::id("minecraft:quartz").unwrap() as i32;
    let hopper = feet.offset(4, 0, 4);
    let (barrel, chest, dropper) = (hopper.offset(0, 1, 0), hopper.offset(1, 0, 0), hopper.offset(1, 0, 1));
    let count = move |s: &mut paper_server::server::Server, at: BlockPos| s.container_items(0, at).iter().filter_map(|i| i.0.as_ref()).map(|i| i.count).sum::<i32>();
    let poll = |alice: &mut Client, wanted: &str| -> String {
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            alice.command("apitest blockevents");
            let seen = line_starting(alice, "blockevents: ");
            if seen.contains(wanted) || std::time::Instant::now() > deadline {
                return seen;
            }
            pause(alice, Duration::from_millis(100));
        }
    };
    put(&server, &[(chest, "minecraft:chest[facing=north]"), (hopper, "minecraft:hopper[facing=east]")], true);
    for (mode, moves) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        put(&server, &[(barrel, "minecraft:barrel[facing=up]")], true);
        server
            .handle
            .run(move |s| {
                s.set_container_items(0, chest, &[]);
                s.set_container_items(0, hopper, &[]);
                s.set_container_items(0, barrel, &[Slot::of(quartz, 1)]);
            })
            .unwrap();
        let wanted = if moves { "MOVE HOPPER CHEST pushed" } else { "MOVE BARREL HOPPER pulled" };
        seen = poll(&mut alice, wanted);
        assert!(seen.contains("MOVE BARREL HOPPER pulled") && seen.contains(wanted), "{mode}: {seen}");
        let at = server.handle.run(move |s| (count(s, barrel), count(s, chest))).unwrap();
        assert_eq!(at, if moves { (0, 1) } else { (1, 0) }, "{mode}: quartz in the barrel and the chest");
        server.handle.run(move |s| s.set_container_items(0, barrel, &[])).unwrap();
        put(&server, &[(barrel, "minecraft:air")], true);
    }
    for (mode, taken) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let lying = server
            .handle
            .run(move |s| {
                s.set_container_items(0, hopper, &[]);
                s.set_container_items(0, chest, &[]);
                let at = paper_server::world::entity::Vec3::new(hopper.x as f64 + 0.5, hopper.y as f64 + 1.0, hopper.z as f64 + 0.5);
                s.spawn_item(0, at, Slot::of(quartz, 2), paper_server::world::entity::Vec3::new(0.0, 0.0, 0.0), 0)
            })
            .unwrap()
            .expect("quartz on the hopper");
        seen = poll(&mut alice, "PICKUP HOPPER 2");
        assert!(seen.contains("PICKUP HOPPER 2"), "{mode}: {seen}");
        pause(&mut alice, Duration::from_millis(100));
        let gone = server.handle.run(move |s| s.worlds[0].entities.get(&lying).is_none_or(|e| e.removed)).unwrap();
        assert_eq!(gone, taken, "{mode}: the quartz taken in");
        server.handle.run(move |s| s.worlds[0].entities.get_mut(&lying).map(|e| e.removed = true)).unwrap();
    }
    for (mode, fills) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        put(&server, &[(dropper, "minecraft:dropper[facing=north]")], false);
        server
            .handle
            .run(move |s| {
                s.set_container_items(0, hopper, &[]);
                s.set_container_items(0, chest, &[]);
                s.set_container_items(0, dropper, &[Slot::of(quartz, 1)]);
                s.set_block_and_update(0, dropper.offset(0, 0, 1), paper_data::blocks::default_state("minecraft:redstone_block").unwrap());
            })
            .unwrap();
        seen = poll(&mut alice, "MOVE DROPPER CHEST pushed");
        assert!(seen.contains("MOVE DROPPER CHEST pushed"), "{mode}: {seen}");
        let at = server.handle.run(move |s| (count(s, dropper), count(s, chest) + count(s, hopper))).unwrap();
        assert_eq!(at, if fills { (0, 1) } else { (1, 0) }, "{mode}: quartz in the dropper and the chest");
        server.handle.run(move |s| s.set_container_items(0, dropper, &[])).unwrap();
        put(&server, &[(dropper, "minecraft:air"), (dropper.offset(0, 0, 1), "minecraft:air")], true);
    }
    server
        .handle
        .run(move |s| {
            s.set_container_items(0, hopper, &[]);
            s.set_container_items(0, chest, &[]);
        })
        .unwrap();
    put(&server, &[(hopper, "minecraft:air"), (chest, "minecraft:air")], true);

    // A cow on a stone pressure plate, and one falling onto farmland, which tramples it
    // (EntityInteractEvent): stopped, then let be.
    let (plate, soil) = (feet.offset(-4, 0, -4), feet.offset(-4, -1, -1));
    let cow_at = move |s: &mut paper_server::server::Server, at: paper_server::world::entity::Vec3, ai: bool| -> i32 {
        let cow = s.spawn_mob(0, "minecraft:cow", at, paper_server::server::mobs::SpawnReason::Command).unwrap();
        let e = s.worlds[0].entities.get_mut(&cow).unwrap();
        e.flags.no_ai = !ai;
        e.mob_mut().unwrap().brain.idle_ticks = 1_000_000;
        cow
    };
    for (mode, pressed) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        put(&server, &[(plate, "minecraft:stone_pressure_plate")], true);
        let cow = server.handle.run(move |s| cow_at(s, paper_server::world::entity::Vec3::new(plate.x as f64 + 0.5, plate.y as f64, plate.z as f64 + 0.5), false)).unwrap();
        seen = poll(&mut alice, "EINTERACT COW STONE_PRESSURE_PLATE");
        assert!(seen.contains("EINTERACT COW STONE_PRESSURE_PLATE"), "{mode}: {seen}");
        pause(&mut alice, Duration::from_millis(100));
        assert_eq!(block_state(&server, plate).contains("powered=true"), pressed, "{mode}: the plate");
        server.handle.run(move |s| s.worlds[0].entities.get_mut(&cow).unwrap().removed = true).unwrap();
        put(&server, &[(plate, "minecraft:air")], true);
    }
    for (mode, trampled) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        put(&server, &[(soil, "minecraft:farmland")], false);
        let cow = server.handle.run(move |s| cow_at(s, paper_server::world::entity::Vec3::new(soil.x as f64 + 0.5, soil.y as f64 + 3.0, soil.z as f64 + 0.5), true)).unwrap();
        seen = poll(&mut alice, "EINTERACT COW FARMLAND");
        assert!(seen.contains("EINTERACT COW FARMLAND"), "{mode}: {seen}");
        pause(&mut alice, Duration::from_millis(100));
        assert_eq!(block_state(&server, soil).starts_with("minecraft:dirt"), trampled, "{mode}: {}", block_state(&server, soil));
        server.handle.run(move |s| s.worlds[0].entities.get_mut(&cow).unwrap().removed = true).unwrap();
        put(&server, &[(soil, "minecraft:grass_block")], false);
    }

    // Experience orbs coming into the world (plain EntitySpawnEvent): refused, then let be.
    for (mode, stay) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let orbs = server
            .handle
            .run(move |s| {
                s.award_experience(0, paper_server::world::entity::Vec3::new(home.x + 3.0, home.y + 1.0, home.z - 3.0), 7);
                let orbs: Vec<i32> = s.worlds[0].entities.values().filter(|e| !e.removed && matches!(e.kind, EntityKind::ExperienceOrb { .. })).map(|e| e.id).collect();
                for id in &orbs {
                    s.worlds[0].entities.get_mut(id).unwrap().removed = true;
                }
                orbs.len()
            })
            .unwrap();
        alice.command("apitest blockevents");
        seen = line_starting(&mut alice, "blockevents: ");
        assert!(seen.contains("ESPAWN EXPERIENCE_ORB"), "{mode}: {seen}");
        assert_eq!(orbs > 0, stay, "{mode}: {orbs} orbs");
    }

    // An oak door Alice puts down in two halves (BlockMultiPlaceEvent): refused, then let be.
    let (step, door) = (feet.offset(2, -1, 2), feet.offset(2, 0, 2));
    let oak_door = Slot::of(paper_data::items::id("minecraft:oak_door").unwrap() as i32, 1);
    server
        .handle
        .run(move |s| {
            s.teleport(alice_id, home);
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = oak_door;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    for (mode, placed) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        pause(&mut alice, Duration::from_millis(100));
        alice.send(&play::UseItemOn { hand: VarInt(0), pos: step, face: VarInt(1), cursor_x: 0.5, cursor_y: 1.0, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(190) });
        seen = poll(&mut alice, "MULTIPLACE OAK_DOOR");
        assert!(seen.contains("MULTIPLACE OAK_DOOR [AIR, AIR]"), "{mode}: {seen}");
        pause(&mut alice, Duration::from_millis(100));
        let halves = (block_state(&server, door), block_state(&server, door.offset(0, 1, 0)));
        assert_eq!(halves.0.contains("half=lower") && halves.1.contains("half=upper"), placed, "{mode}: {halves:?}");
        put(&server, &[(door.offset(0, 1, 0), "minecraft:air"), (door, "minecraft:air")], false);
    }
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::EMPTY;
            s.resync_inventory(alice_id);
        })
        .unwrap();

    // An item frame Alice hangs on a wall (HangingPlaceEvent): refused, then let be. Then an
    // item goes in and is knocked out (Paper's PlayerItemFrameChangeEvent), and the frame
    // breaks (HangingBreakByEntityEvent). Then frames and paintings through the API.
    let (wall, front) = (feet.offset(2, 0, -2), feet.offset(1, 0, -2));
    put(&server, &[(wall, "minecraft:stone")], false);
    let frame_item = Slot::of(paper_data::items::id("minecraft:item_frame").unwrap() as i32, 1);
    server
        .handle
        .run(move |s| {
            s.teleport(alice_id, home);
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = frame_item;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    let frame_at = |server: &paper_server::RunningServer| {
        server.handle.run(move |s| s.worlds[0].entities.values().find(|e| !e.removed && matches!(&e.kind, EntityKind::Hanging(h) if h.block == front)).map(|e| e.id)).unwrap()
    };
    for (mode, placed) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        pause(&mut alice, Duration::from_millis(100));
        alice.send(&play::UseItemOn { hand: VarInt(0), pos: wall, face: VarInt(4), cursor_x: 0.0, cursor_y: 0.5, cursor_z: 0.5, inside_block: false, world_border_hit: false, sequence: VarInt(191) });
        seen = poll(&mut alice, "HANGPLACE ITEM_FRAME");
        assert!(seen.contains("HANGPLACE ITEM_FRAME WEST ITEM_FRAME"), "{mode}: {seen}");
        pause(&mut alice, Duration::from_millis(100));
        assert_eq!(frame_at(&server).is_some(), placed, "{mode}: {seen}");
    }
    let frame = frame_at(&server).unwrap();
    let emerald = Slot::of(paper_data::items::id("minecraft:emerald").unwrap() as i32, 1);
    server.handle.run(move |s| s.set_inventory_slot(alice_id, 36 + held, emerald)).unwrap();
    alice.send(&play::Interact { entity_id: VarInt(frame), hand: VarInt(0), target: paper_protocol::types::LpVec3(0.0, 0.0, 0.0), sneaking: false });
    seen = poll(&mut alice, "FRAME PLACE EMERALD");
    assert!(seen.contains("FRAME PLACE EMERALD"), "{seen}");
    alice.send(&play::Attack { entity_id: VarInt(frame) });
    seen = poll(&mut alice, "FRAME REMOVE EMERALD");
    assert!(seen.contains("FRAME REMOVE EMERALD"), "{seen}");
    pause(&mut alice, Duration::from_millis(600));
    alice.send(&play::Attack { entity_id: VarInt(frame) });
    seen = poll(&mut alice, "HANGBREAK ITEM_FRAME ENTITY by PLAYER");
    assert!(seen.contains("HANGBREAK ITEM_FRAME ENTITY by PLAYER"), "{seen}");
    assert!(frame_at(&server).is_none(), "{seen}");
    alice.command("apitest blocks off");
    expect_line(&mut alice, "blocks: off");
    put(&server, &[(wall, "minecraft:air")], false);
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::EMPTY;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    alice.command("apitest hanging");
    expect_line(&mut alice, "hanging: ok");

    // Fishing (PlayerFishEvent): a cast refused, then let be. The plugin shortens the hook's
    // waits, so a fish is soon lured and bites; the catch it turns into a diamond worth 42
    // experience.
    let pond: Vec<(BlockPos, &'static str)> = (-4..=4).flat_map(|dx| (3..=12).flat_map(move |dz| (-3..=-1).map(move |dy| (feet.offset(dx, dy, dz), "minecraft:water")))).collect();
    put(&server, &pond, false);
    let rod = Slot::of(paper_data::items::id("minecraft:fishing_rod").unwrap() as i32, 1);
    server
        .handle
        .run(move |s| {
            s.teleport(alice_id, home);
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = rod;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    let bobber = |server: &paper_server::RunningServer| server.handle.run(|s| s.worlds[0].entities.values().any(|e| !e.removed && matches!(e.kind, EntityKind::Fishing(_)))).unwrap();
    let cast = |alice: &mut Client, sequence: i32| alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(sequence), yaw: 0.0, pitch: 20.0 });
    alice.command("apitest blocks cancel");
    expect_line(&mut alice, "blocks: cancel");
    cast(&mut alice, 192);
    seen = poll(&mut alice, "FISH FISHING");
    assert!(seen.contains("FISH FISHING UNHOOKED 100-600 true") && !bobber(&server), "{seen}");
    alice.command("apitest blocks record");
    expect_line(&mut alice, "blocks: record");
    cast(&mut alice, 193);
    seen = poll(&mut alice, "FISH BITE");
    assert!(seen.contains("FISH LURED") && seen.contains("FISH BITE") && bobber(&server), "{seen}");
    let xp_before = server.handle.run(move |s| s.players[&alice_id].xp_total).unwrap();
    cast(&mut alice, 194);
    seen = poll(&mut alice, "FISH CAUGHT_FISH");
    assert!(seen.contains("FISH CAUGHT_FISH"), "{seen}");
    let diamond = paper_data::items::id("minecraft:diamond").unwrap() as i32;
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let (caught, xp, orbs) = server
            .handle
            .run(move |s| {
                let lying = s.worlds[0].entities.values().any(|e| !e.removed && matches!(&e.kind, EntityKind::Item { stack, .. } if stack.0.as_ref().is_some_and(|i| i.item == diamond)));
                let held = s.players[&alice_id].inventory.iter().any(|i| i.0.as_ref().is_some_and(|i| i.item == diamond));
                let orbs: i32 = s.worlds[0].entities.values().filter(|e| !e.removed).filter_map(|e| match e.kind { EntityKind::ExperienceOrb { value } => Some(value), _ => None }).sum();
                (lying || held, s.players[&alice_id].xp_total, orbs)
            })
            .unwrap();
        if caught && (xp - xp_before + orbs >= 42) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "caught {caught}, experience {xp_before} -> {xp} with {orbs} in orbs");
        pause(&mut alice, Duration::from_millis(100));
    }
    alice.command("apitest blocks off");
    expect_line(&mut alice, "blocks: off");
    let drained: Vec<(BlockPos, &'static str)> = pond.iter().map(|(pos, _)| (*pos, "minecraft:air")).collect();
    put(&server, &drained, false);
    server
        .handle
        .run(move |s| {
            s.players.get_mut(&alice_id).unwrap().inventory[36 + held] = Slot::EMPTY;
            s.resync_inventory(alice_id);
        })
        .unwrap();

    // Gliding and boosting (EntityToggleGlideEvent, Paper's PlayerElytraBoostEvent, which the
    // plugin has keep the rocket in hand, and FireworkExplodeEvent); then rockets through the API.
    let (elytra, rockets) = (Slot::of(paper_data::items::id("minecraft:elytra").unwrap() as i32, 1), Slot::of(paper_data::items::id("minecraft:firework_rocket").unwrap() as i32, 3));
    server
        .handle
        .run(move |s| {
            s.set_game_mode(alice_id, paper_server::config::GameMode::Survival);
            let p = s.players.get_mut(&alice_id).unwrap();
            p.inventory[6] = elytra;
            p.inventory[36 + held] = rockets;
            p.on_ground = false;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    alice.command("apitest blocks record");
    expect_line(&mut alice, "blocks: record");
    alice.send(&play::PlayerCommand { entity_id: VarInt(alice_id), action: VarInt(play::player_command::START_ELYTRA_FLYING), jump_boost: VarInt(0) });
    seen = poll(&mut alice, "GLIDE true");
    assert!(seen.contains("GLIDE true"), "{seen}");
    alice.send(&play::UseItem { hand: VarInt(0), sequence: VarInt(195), yaw: 0.0, pitch: 0.0 });
    seen = poll(&mut alice, "FIREWORK exploded");
    assert!(seen.contains("BOOST FIREWORK_ROCKET true") && seen.contains("FIREWORK exploded"), "{seen}");
    let kept = server.handle.run(move |s| s.players[&alice_id].inventory[36 + held].0.as_ref().map(|i| i.count)).unwrap();
    assert_eq!(kept, Some(3), "the plugin kept the rocket in hand");
    server.handle.run(move |s| s.players.get_mut(&alice_id).unwrap().on_ground = true).unwrap();
    seen = poll(&mut alice, "GLIDE false");
    assert!(seen.contains("GLIDE false"), "{seen}");
    alice.command("apitest blocks off");
    expect_line(&mut alice, "blocks: off");
    server
        .handle
        .run(move |s| {
            s.set_game_mode(alice_id, paper_server::config::GameMode::Creative);
            let p = s.players.get_mut(&alice_id).unwrap();
            p.inventory[6] = Slot::EMPTY;
            p.inventory[36 + held] = Slot::EMPTY;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    alice.command("apitest fireworks");
    expect_line(&mut alice, "fireworks: ok");

    // A shield (Paper's PlayerShieldDisableEvent): an axe-wielding zombie's blow disables
    // Alice's raised shield, for the three seconds the plugin says.
    let shield = Slot::of(paper_data::items::id("minecraft:shield").unwrap() as i32, 1);
    server
        .handle
        .run(move |s| {
            s.set_game_mode(alice_id, paper_server::config::GameMode::Survival);
            let p = s.players.get_mut(&alice_id).unwrap();
            p.inventory[45] = shield;
            p.loc.yaw = 0.0;
            s.resync_inventory(alice_id);
        })
        .unwrap();
    alice.send(&play::UseItem { hand: VarInt(1), sequence: VarInt(196), yaw: 0.0, pitch: 0.0 });
    pause(&mut alice, Duration::from_millis(400));
    alice.command("apitest blocks record");
    expect_line(&mut alice, "blocks: record");
    alice.clear_backlog();
    let axe = Slot::of(paper_data::items::id("minecraft:iron_axe").unwrap() as i32, 1);
    server
        .handle
        .run(move |s| {
            let at = s.players[&alice_id].loc;
            let front = paper_server::world::entity::Vec3::new(at.x, at.y, at.z + 2.0);
            let zombie = s.spawn_mob(0, "minecraft:zombie", front, paper_server::server::mobs::SpawnReason::Command).unwrap();
            if let Some(m) = s.worlds[0].entities.get_mut(&zombie).and_then(|e| e.mob_mut()) {
                m.equipment[0] = axe;
            }
            let blow = paper_server::server::survival::Damage { kind: "minecraft:mob_attack", attacker: Some(zombie), direct: Some(zombie), position: Some(front) };
            let landed = s.hurt_player(alice_id, blow, 4.0);
            s.worlds[0].entities.get_mut(&zombie).unwrap().removed = true;
            assert!(!landed, "the blow is blocked");
        })
        .unwrap();
    let cooldown: play::Cooldown = alice.find(TIMEOUT, |c: &play::Cooldown| c.group == "minecraft:shield").expect("the shield cools down");
    assert_eq!(cooldown.duration.0, 60, "as the plugin said");
    seen = poll(&mut alice, "SHIELD ZOMBIE");
    assert!(seen.contains("SHIELD ZOMBIE 100 true"), "{seen}");
    alice.command("apitest blocks off");
    expect_line(&mut alice, "blocks: off");
    server
        .handle
        .run(move |s| {
            s.set_game_mode(alice_id, paper_server::config::GameMode::Creative);
            s.players.get_mut(&alice_id).unwrap().inventory[45] = Slot::EMPTY;
            s.resync_inventory(alice_id);
        })
        .unwrap();

    // A plugin's block populator and ChunkPopulateEvent: chunks newly made far away get both.
    alice.command("apitest populator");
    expect_line(&mut alice, "populator: added");
    server.handle.run(move |s| s.teleport(alice_id, paper_server::server::player::Location { x: home.x + 3000.0, ..home })).unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    let marked = loop {
        alice.command("apitest populated");
        let line = line_starting(&mut alice, "populated: ");
        let parts: Vec<&str> = line["populated: ".len()..].split(' ').collect();
        if parts.len() == 4 && parts[0] == "true" && parts[1] == "true" {
            break (parts[2].parse::<i32>().unwrap(), parts[3].parse::<i32>().unwrap());
        }
        assert!(std::time::Instant::now() < deadline, "{line}");
        pause(&mut alice, Duration::from_millis(200));
    };
    assert_eq!(block_state(&server, BlockPos::new(marked.0 * 16, 100, marked.1 * 16)), "minecraft:gold_block", "the populator's mark");
    server.handle.run(move |s| s.teleport(alice_id, home)).unwrap();
    pause(&mut alice, Duration::from_millis(300));

    // Knockback (Bukkit's EntityKnockbackEvent, then Paper's): a cow Alice hits, and a push
    // the server gives it, stopped and then let be.
    let cow = server
        .handle
        .run(move |s| {
            let at = paper_server::world::entity::Vec3::new(home.x + 2.0, home.y, home.z);
            let cow = s.spawn_mob(0, "minecraft:cow", at, paper_server::server::mobs::SpawnReason::Command).unwrap();
            s.worlds[0].entities.get_mut(&cow).unwrap().mob_mut().unwrap().brain.idle_ticks = 1_000_000;
            cow
        })
        .unwrap();
    alice.command("apitest blocks record");
    expect_line(&mut alice, "blocks: record");
    pause(&mut alice, Duration::from_millis(100));
    alice.send(&play::Attack { entity_id: VarInt(cow) });
    seen = poll(&mut alice, "KNOCK EntityKnockbackByEntityEvent");
    assert!(seen.contains("KNOCK EntityKnockbackByEntityEvent ENTITY_ATTACK") && seen.contains("BKNOCK EntityKnockbackByEntityEvent ENTITY_ATTACK PLAYER"), "{seen}");
    for (mode, pushed) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let vel = server
            .handle
            .run(move |s| {
                s.worlds[0].entities.get_mut(&cow).unwrap().vel = paper_server::world::entity::Vec3::new(0.0, 0.0, 0.0);
                s.knockback_entity(0, cow, 0.4, 1.0, 0.0, Some(alice_id), "ENTITY_ATTACK");
                s.worlds[0].entities[&cow].vel
            })
            .unwrap();
        assert_eq!(vel.x < -0.3, pushed, "{mode}: {vel:?}");
    }
    server.handle.run(move |s| s.worlds[0].entities.get_mut(&cow).unwrap().removed = true).unwrap();

    // A snow golem's snow (EntityBlockFormEvent): stopped, then let be.
    let spot = feet.offset(-6, 0, 8);
    for (mode, snows) in [("cancel", false), ("record", true)] {
        alice.command(&format!("apitest blocks {mode}"));
        expect_line(&mut alice, &format!("blocks: {mode}"));
        let golem = server
            .handle
            .run(move |s| {
                let at = paper_server::world::entity::Vec3::new(spot.x as f64 + 0.5, spot.y as f64, spot.z as f64 + 0.5);
                let golem = s.spawn_mob(0, "minecraft:snow_golem", at, paper_server::server::mobs::SpawnReason::Command).unwrap();
                s.worlds[0].entities.get_mut(&golem).unwrap().flags.no_ai = true;
                golem
            })
            .unwrap();
        seen = poll(&mut alice, "SNOWFORM SNOW");
        assert!(seen.contains("SNOWFORM SNOW"), "{mode}: {seen}");
        server.handle.run(move |s| s.worlds[0].entities.get_mut(&golem).unwrap().removed = true).unwrap();
        pause(&mut alice, Duration::from_millis(100));
        assert_eq!(block_state(&server, spot).starts_with("minecraft:snow"), snows, "{mode}: {}", block_state(&server, spot));
        put(&server, &[(spot, "minecraft:air")], true);
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
    // A value a plugin keeps with a chunk (`Chunk#getPersistentDataContainer`), and a snapshot
    // of the chunk (`getChunkSnapshot`).
    alice.command("apitest chunkdata");
    expect_line(&mut alice, "chunkdata: ok");
    let kept = server
        .handle
        .run(move |s| {
            let p = &s.players[&alice_id];
            let pos = paper_server::world::chunk::ChunkPos::of_block(p.loc.x.floor() as i32, p.loc.z.floor() as i32);
            s.worlds[p.world].chunks.get(&pos).and_then(|c| c.plugin_data.as_ref()).map(|d| format!("{d:?}")).unwrap_or_default()
        })
        .unwrap();
    assert!(kept.contains("kept with the chunk"), "{kept}");

    // A border only Alice sees (`Player#setWorldBorder`), moving with the ticks on her copy;
    // and Paper's border events, for the API and `/worldborder` alike.
    alice.command("apitest borders");
    expect_line(&mut alice, "borders: ok");
    let (own, world_size) = server.handle.run(move |s| (s.players[&alice_id].own_border.clone(), s.worlds[0].border.lerp_target)).unwrap();
    let own = own.expect("Alice's own border");
    assert_eq!((own.center_x, own.center_z, own.lerp_target, own.warning_blocks), (100.0, 200.0, 80.0, 3), "{own:?}");
    assert_eq!(world_size, 1100.0);
    pause(&mut alice, Duration::from_millis(1500));
    server.handle.console_command("worldborder set 1150");
    pause(&mut alice, Duration::from_millis(200));
    alice.command("apitest borderevents");
    expect_line(&mut alice, "borderevents: ok");
    let (own, world_size) = server.handle.run(move |s| (s.players[&alice_id].own_border.clone(), s.worlds[0].border.size)).unwrap();
    assert!(own.is_none() && world_size == paper_server::world::MAX_BORDER_SIZE, "{own:?} {world_size}");

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
    carol.send(&play::PlayerLoaded {});
    let objective: play::SetObjective = carol.find(TIMEOUT, |o: &play::SetObjective| o.name == "kills").expect("main board for newcomers");
    assert_eq!(objective.mode, 0);
    let team: play::SetPlayerTeam = carol.find(TIMEOUT, |t: &play::SetPlayerTeam| t.name == "red").expect("teams for newcomers");
    assert_eq!((team.method, team.members.clone()), (0, vec!["Alice".to_owned()]));
    assert_eq!(team.parameters.as_ref().and_then(|p| p.color), Some(12), "red");
    assert_eq!(team.parameters.as_ref().map(|p| p.prefix.to_plain()), Some("[R] ".to_owned()));
    // PlayerClientLoadedWorldEvent: Carol's client said it had loaded the world; Alice's never
    // did, so the server stopped waiting for it.
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        alice.command("apitest loaded");
        let loaded = line_starting(&mut alice, "loaded: ");
        if loaded.contains("Carol false") {
            assert!(loaded.contains("Alice true"), "{loaded}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{loaded}");
        pause(&mut alice, Duration::from_millis(100));
    }

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
    alice.expect::<play::Respawn>();
    // The respawn point is spread around the world spawn, onto the ground there; the plugin
    // moves it five blocks up from that.
    let respawned: play::PlayerPosition = alice.expect();
    let (x, z) = (respawned.x.floor() as i32, respawned.z.floor() as i32);
    let ground = server.handle.run(move |s| s.worlds[0].top_y(x, z)).unwrap().unwrap_or(spawn_y);
    assert!(respawned.y >= ground as f64 + 4.9, "PlayerRespawnEvent moved the respawn point up: {} over ground at {ground}", respawned.y);

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
    // Unloading it (`Bukkit.unloadWorld`, `WorldUnloadEvent`): its place stays, empty and
    // nameless; loading it again opens it anew from its folder, flat as it was made.
    alice.command("apitest unloadworld");
    expect_line(&mut alice, "unloadworld: ok");
    let (old, again) = server
        .handle
        .run(move |s| {
            let old = &s.worlds[index];
            let again = s.worlds.iter().position(|w| w.name == "plugin_flat").map(|i| (i, s.worlds[i].generator.name()));
            ((old.unloaded, old.name.clone(), old.chunks.len(), old.storage.is_some()), again)
        })
        .unwrap();
    assert_eq!(old, (true, "unloaded:plugin_flat".to_owned(), 0, false));
    assert!(again.is_some_and(|(i, generator)| i != index && generator == "flat"), "{again:?}");

    server.stop();
    assert!(dir.join("plugin_flat").join("level.dat").exists(), "the new world is saved in its folder");
    let _ = std::fs::remove_dir_all(&dir);
}
