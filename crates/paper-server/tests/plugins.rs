//! The plugin bridge end to end, against a fake plugin host written in Rust.
//!
//! The fake host speaks the bridge protocol the way the Java host does: it answers the
//! server's calls (loading plugins, firing events, running commands, ticking) and calls the
//! server API back from inside them, so these tests cover the main-thread baton, async
//! events, commands, the status ping and the host going away, without needing Java.

mod common;

use common::*;
use paper_bridge::{bmap, read_frame, write_frame, Frame, Value, FLAG_SYNC, PROTOCOL_VERSION};
use paper_protocol::codec::{BlockPos, VarInt};
use paper_protocol::packets::play::{self, NodeKind};
use paper_server::plugins::Launcher;
use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(300);

/// What the fake host saw, for assertions.
#[derive(Default)]
struct Seen {
    ticks: AtomicU64,
    /// Methods called on the host, in order (ticks excluded).
    calls: Mutex<Vec<String>>,
    /// Player names by UUID, from `players.state` notifications.
    names: Mutex<HashMap<String, String>>,
    welcome: Mutex<Option<Value>>,
    /// Every WELCOME, in order (a second one after `/reload`).
    welcomes: Mutex<Vec<Value>>,
    /// Arguments of inventory events.
    inventory_events: Mutex<Vec<(String, Value)>>,
    /// Results of API calls made from `/api ...` commands.
    results: Mutex<HashMap<String, Value>>,
    /// Open the custom menu again when it is closed (once).
    reopen: std::sync::atomic::AtomicBool,
}

struct FakeHost {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
    seen: Arc<Seen>,
}

fn json(text: &str) -> Value {
    Value::Str(serde_json::json!({ "text": text }).to_string())
}

impl FakeHost {
    fn connect(port: u16, token: &str, seen: Arc<Seen>) -> FakeHost {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("fake host connects");
        stream.set_nodelay(true).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        write_frame(&mut writer, &Frame::Hello(bmap! { "token" => token, "protocol" => PROTOCOL_VERSION })).unwrap();
        writer.flush().unwrap();
        match read_frame(&mut reader).unwrap() {
            Some(Frame::Welcome(v)) => {
                seen.welcomes.lock().unwrap().push(v.clone());
                *seen.welcome.lock().unwrap() = Some(v);
            }
            other => panic!("expected WELCOME, got {other:?}"),
        }
        FakeHost { reader, writer, next_id: 1, seen }
    }

    fn send(&mut self, frame: &Frame) {
        write_frame(&mut self.writer, frame).unwrap();
        self.writer.flush().unwrap();
    }

    /// Calls the server API from the host's "main thread", serving nested calls meanwhile.
    fn call(&mut self, method: &str, args: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&Frame::Call { id, flags: FLAG_SYNC, method: method.into(), args });
        loop {
            match read_frame(&mut self.reader).unwrap() {
                Some(Frame::Return { id: rid, ok, value }) if rid == id => {
                    assert!(ok, "{method} failed: {value:?}");
                    return value;
                }
                Some(frame) => self.dispatch(frame),
                None => panic!("server closed the bridge during {method}"),
            }
        }
    }

    fn dispatch(&mut self, frame: Frame) {
        match frame {
            Frame::Call { id, method, args, .. } => {
                if method != "host.tick" {
                    self.seen.calls.lock().unwrap().push(method.clone());
                }
                let value = self.handle(&method, &args);
                self.send(&Frame::Return { id, ok: true, value });
                if method == "host.shutdown" || (method == "command.execute" && args.str("command") == Some("crash")) {
                    let _ = self.writer.shutdown(std::net::Shutdown::Both);
                }
            }
            Frame::Notify { method, args } if method == "players.state" => {
                let mut names = self.seen.names.lock().unwrap();
                for p in args.list("players").unwrap_or(&[]) {
                    names.insert(p.str("uuid").unwrap().to_owned(), p.str("name").unwrap().to_owned());
                }
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }

    fn plugin_list(enabled: bool) -> Value {
        Value::List(vec![
            bmap! { "name" => "FakePlugin", "version" => "1.0", "enabled" => enabled, "kind" => "paper" },
            bmap! { "name" => "OldPlugin", "version" => "2.0", "enabled" => enabled, "kind" => "bukkit" },
        ])
    }

    fn handle(&mut self, method: &str, args: &Value) -> Value {
        match method {
            "host.load" => bmap! {
                "plugins" => Self::plugin_list(false),
                "commands" => Value::List(vec!["hello".into(), "fakeplugin:hello".into(), "crash".into(), "api".into()]),
                "events" => Value::List(vec![
                    "PlayerCommandPreprocessEvent".into(), "BlockBreakEvent".into(), "AsyncChatEvent".into(), "PlayerMoveEvent".into(), "ServerCommandEvent".into(),
                    "InventoryOpenEvent".into(), "InventoryClickEvent".into(), "InventoryCloseEvent".into(), "InventoryDragEvent".into(),
                ]),
            },
            "host.post_world" => bmap! { "plugins" => Self::plugin_list(true) },
            "host.tick" => {
                self.seen.ticks.fetch_add(1, Ordering::Relaxed);
                Value::Null
            }
            "event.server_list_ping" => bmap! { "motd" => json("Fake MOTD"), "max" => 100 },
            "event.pre_login" => {
                if args.str("name") == Some("Mallory") {
                    bmap! { "result" => "KICK_BANNED", "message" => json("No Mallory allowed") }
                } else {
                    bmap! { "result" => "ALLOWED" }
                }
            }
            "event.join" => {
                let player = args.get("player");
                let uuid = player.str("uuid").unwrap().to_owned();
                let name = player.str("name").unwrap().to_owned();
                // The join event carries the player's full state; later changes arrive as
                // players.state notifications.
                self.seen.names.lock().unwrap().insert(uuid.clone(), name.clone());
                // A nested call into the server from inside the event.
                self.call("player.send_message", bmap! { "uuid" => uuid, "message" => json("Welcome from Java") });
                bmap! { "message" => json(&format!("{name} joined (via plugin)")) }
            }
            "event.quit" => bmap! { "message" => json("bye") },
            "command.visible" => bmap! { "labels" => Value::List(vec!["hello".into(), "fakeplugin:hello".into(), "crash".into(), "api".into()]) },
            "event.inventory_open" | "event.inventory_close" => {
                self.seen.inventory_events.lock().unwrap().push((method.to_owned(), args.clone()));
                // A "you cannot close this" menu: opened again from its close event.
                let top = args.get("inventory").get("top").clone();
                if method == "event.inventory_close" && top.str("kind") == Some("custom") && self.seen.reopen.swap(false, Ordering::Relaxed) {
                    let uuid = args.str("uuid").unwrap().to_owned();
                    let reopened = self.call("player.open_inventory", bmap! { "uuid" => uuid, "inventory" => top });
                    assert_eq!(reopened.bool("opened"), Some(true));
                }
                Value::Null
            }
            "event.inventory_click" => {
                self.seen.inventory_events.lock().unwrap().push((method.to_owned(), args.clone()));
                // Clicks in the fake plugin's menu are refused, as GUI plugins do.
                let custom = args.get("inventory").get("top").str("kind") == Some("custom");
                bmap! { "cancelled" => custom }
            }
            "command.execute" => {
                let sender = args.str("sender").unwrap().to_owned();
                let command = args.str("command").unwrap().to_owned();
                if let Some(area) = command.strip_prefix("api ") {
                    self.api(&sender, area);
                }
                if command.starts_with("hello") || command.starts_with("fakeplugin:hello") {
                    // Ask the server for the player's state, then answer, then run a vanilla command
                    // through the server.
                    let state = self.call("player.state", bmap! { "uuid" => sender.clone() });
                    let name = state.str("name").unwrap().to_owned();
                    self.call("player.send_message", bmap! { "uuid" => sender.clone(), "message" => json(&format!("Hello, {name}!")) });
                    let ran = self.call("server.dispatch_command", bmap! { "sender" => sender, "command" => "list" });
                    assert_eq!(ran, Value::Bool(true));
                }
                bmap! { "handled" => true }
            }
            "command.suggest" => bmap! { "start" => 7, "length" => 1, "matches" => Value::List(vec![bmap! { "text" => "world" }]) },
            "event.command_preprocess" => match args.str("message") {
                Some("/secret") => bmap! { "cancelled" => true },
                Some("/hi") => bmap! { "message" => "/hello" },
                Some(other) => bmap! { "message" => other },
                None => Value::Null,
            },
            "event.server_command" => bmap! { "command" => args.str("command").unwrap_or("") },
            "event.block_break" => bmap! { "cancelled" => true },
            "event.move" => {
                if args.get("to").f64("y").unwrap_or(0.0) > 100.0 {
                    bmap! { "cancelled" => true }
                } else {
                    Value::Null
                }
            }
            "event.chat" => {
                let uuid = args.str("uuid").unwrap().to_owned();
                let message = args.str("message").unwrap().to_owned();
                let name = self.seen.names.lock().unwrap().get(&uuid).cloned().unwrap_or_default();
                let line = json(&format!("[fake] {name}: {message}"));
                bmap! { "deliveries" => Value::List(vec![bmap! { "viewer" => uuid, "message" => line.clone() }, bmap! { "viewer" => "console", "message" => line }]) }
            }
            "host.shutdown" => Value::Null,
            other => panic!("the fake host does not handle {other}"),
        }
    }

    /// Calls the plugin API the way the Java host does for one area, recording the results.
    fn api(&mut self, sender: &str, area: &str) {
        let record = |key: &str, value: Value, seen: &Seen| {
            seen.results.lock().unwrap().insert(key.to_owned(), value);
        };
        match area {
            "menu" => {
                let created = self.call("inventory.create", bmap! { "type" => "CHEST", "size" => 9, "title" => json("Fake Menu") });
                let inventory = bmap! { "kind" => "custom", "id" => created.i64("id").unwrap() };
                let diamond = bmap! { "item" => "minecraft:diamond", "count" => 2, "name" => json("Shiny") };
                self.call("inventory.set", bmap! { "inventory" => inventory.clone(), "slots" => Value::List(vec![Value::List(vec![4.into(), diamond])]) });
                let contents = self.call("inventory.contents", bmap! { "inventory" => inventory.clone() });
                record("menu.contents", contents, &self.seen);
                let opened = self.call("player.open_inventory", bmap! { "uuid" => sender, "inventory" => inventory });
                record("menu.opened", opened, &self.seen);
                let state = self.call("player.state", bmap! { "uuid" => sender });
                record("menu.state", state, &self.seen);
            }
            "player" => {
                let player = bmap! { "kind" => "player", "uuid" => sender };
                // Bukkit slot 0 is the first hotbar slot; 39 the helmet.
                let slots = Value::List(vec![
                    Value::List(vec![0.into(), bmap! { "item" => "minecraft:stone", "count" => 5 }]),
                    Value::List(vec![39.into(), bmap! { "item" => "minecraft:iron_helmet", "count" => 1 }]),
                ]);
                self.call("inventory.set", bmap! { "inventory" => player.clone(), "slots" => slots });
                let contents = self.call("inventory.contents", bmap! { "inventory" => player });
                record("player.contents", contents, &self.seen);
                self.call("inventory.set", bmap! { "inventory" => bmap! { "kind" => "ender", "uuid" => sender }, "slots" => Value::List(vec![Value::List(vec![3.into(), bmap! { "item" => "minecraft:emerald", "count" => 7 }])]) });
            }
            "board" => {
                self.call("scoreboard.register_objective", bmap! { "board" => 0, "name" => "points", "criteria" => "dummy", "display_name" => json("Points") });
                self.call("scoreboard.set_display", bmap! { "board" => 0, "slot" => 1, "objective" => "points" });
                self.call("scoreboard.set_score", bmap! { "board" => 0, "objective" => "points", "entry" => "Alice", "value" => 12 });
                self.call("scoreboard.register_team", bmap! { "board" => 0, "name" => "blue" });
                self.call("scoreboard.update_team", bmap! { "board" => 0, "name" => "blue", "prefix" => json("[B] "), "color" => 9 });
                self.call("scoreboard.add_entries", bmap! { "board" => 0, "team" => "blue", "entries" => Value::List(vec!["Alice".into()]) });
                let score = self.call("scoreboard.score", bmap! { "board" => 0, "objective" => "points", "entry" => "Alice" });
                record("board.score", score, &self.seen);
                let created = self.call("scoreboard.new", Value::Null);
                record("board.new", created, &self.seen);
            }
            "mob" => {
                let spawned = self.call("world.spawn_entity", bmap! { "type" => "minecraft:cow", "x" => 12.5, "y" => -60.0, "z" => 8.5, "reason" => "CUSTOM" });
                let id = spawned.i64("id").unwrap();
                let named = self.call("entity.set", bmap! { "id" => id, "custom_name" => json("Daisy"), "max_health" => 30.0, "health" => 25.0 });
                record("mob.named", named, &self.seen);
                let dropped = self.call("world.drop_item", bmap! { "x" => 12.5, "y" => -59.0, "z" => 8.5, "item" => bmap! { "item" => "minecraft:apple", "count" => 3 } });
                record("mob.dropped", dropped, &self.seen);
                let around = self.call("world.entities", bmap! { "min" => bmap! { "x" => 10.0, "y" => -62.0, "z" => 6.0 }, "max" => bmap! { "x" => 15.0, "y" => -55.0, "z" => 11.0 } });
                record("mob.around", around, &self.seen);
            }
            "offline" => {
                let alice = self.call("offline.get", bmap! { "name" => "Alice" });
                record("offline.alice", alice, &self.seen);
                let nobody = self.call("offline.get", bmap! { "name" => "Nobody", "cached_only" => true });
                record("offline.nobody", nobody, &self.seen);
            }
            other => panic!("unknown api area {other}"),
        }
    }

    fn run(mut self) {
        loop {
            match read_frame(&mut self.reader) {
                Ok(Some(frame)) => self.dispatch(frame),
                Ok(None) | Err(_) => return,
            }
        }
    }
}

fn fake_launcher(seen: Arc<Seen>) -> Launcher {
    // Called again for every new host (`/reload`).
    Box::new(move |_config, port, token| {
        let token = token.to_owned();
        let seen = seen.clone();
        std::thread::spawn(move || FakeHost::connect(port, &token, seen).run());
        Ok(None)
    })
}

fn literals(commands: &play::Commands) -> Vec<String> {
    let root = &commands.nodes[commands.root_index as usize];
    root.children
        .iter()
        .filter_map(|i| match &commands.nodes[*i as usize].kind {
            NodeKind::Literal(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn plugins_drive_the_server_through_the_bridge() {
    let seen = Arc::new(Seen::default());
    let server = start_server_with_host(|_| {}, fake_launcher(seen.clone()));
    let welcome = seen.welcome.lock().unwrap().clone().expect("host was welcomed");
    assert_eq!(welcome.str("minecraft_version"), Some(paper_data::VERSION_NAME));
    let worlds: Vec<(String, String)> = welcome.list("worlds").unwrap().iter().map(|w| (w.str("name").unwrap().to_owned(), w.str("environment").unwrap().to_owned())).collect();
    assert_eq!(worlds.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>(), ["NORMAL", "NETHER", "THE_END"], "{worlds:?}");
    assert_eq!(seen.calls.lock().unwrap()[..2], ["host.load".to_owned(), "host.post_world".to_owned()]);

    // The plugin list reached the server.
    let plugins = server.handle.run(|s| s.plugin_names()).unwrap();
    assert_eq!(plugins, vec![("FakePlugin".to_owned(), true), ("OldPlugin".to_owned(), true)]);

    // The plugin scheduler is driven every tick.
    std::thread::sleep(Duration::from_millis(300));
    assert!(seen.ticks.load(Ordering::Relaxed) >= 2, "host.tick runs every tick");

    // PaperServerListPingEvent changes the server list entry.
    let status = Client::status(server.port());
    assert_eq!(status["description"]["text"], "Fake MOTD");
    assert_eq!(status["players"]["max"], 100);

    // AsyncPlayerPreLoginEvent can refuse a player.
    let (_, reply) = Client::begin_login(server.port(), paper_data::PROTOCOL_VERSION, "Mallory");
    let reply = reply.expect("login refusal");
    let refusal: paper_protocol::packets::login::LoginDisconnect = decode(&reply);
    assert_eq!(refusal.reason.0.to_plain(), "No Mallory allowed");

    // PlayerJoinEvent: the listener messages the player from inside the event (a nested call
    // into the server) and replaces the join message. The command tree, sent after the event,
    // includes the plugin commands the player may use.
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    let welcome: play::SystemChat = alice.expect();
    assert_eq!(welcome.content.to_plain(), "Welcome from Java");
    let joined: play::SystemChat = alice.expect();
    assert_eq!(joined.content.to_plain(), "Alice joined (via plugin)");
    let tree: play::Commands = alice.expect();
    let labels = literals(&tree);
    assert!(labels.contains(&"hello".to_owned()) && labels.contains(&"fakeplugin:hello".to_owned()), "{labels:?}");
    assert!(labels.contains(&"list".to_owned()), "built-in commands stay");
    alice.drain(QUIET);
    alice.clear_backlog();

    // A plugin command: the host asks for the player's state, answers, and runs a vanilla
    // command through the server, all inside the one command call.
    alice.command("hello");
    let reply: play::SystemChat = alice.expect();
    assert_eq!(reply.content.to_plain(), "Hello, Alice!");
    let list: play::SystemChat = alice.expect();
    assert_eq!(list.content.to_plain(), "There are 1 of a max of 20 players online: Alice");

    // PlayerCommandPreprocessEvent can rewrite and cancel commands.
    alice.command("hi");
    let reply: play::SystemChat = alice.expect();
    assert_eq!(reply.content.to_plain(), "Hello, Alice!");
    alice.drain(QUIET);
    alice.clear_backlog();
    alice.command("secret");
    alice.drain(QUIET);
    assert!(!alice.has::<play::SystemChat>(), "a cancelled command does nothing, not even 'unknown command'");

    // Tab completion of plugin commands comes from the host.
    alice.send(&play::CommandSuggestion { transaction_id: VarInt(3), text: "/hello w".into() });
    let suggestions: play::CommandSuggestions = alice.expect();
    assert_eq!((suggestions.transaction_id, suggestions.start, suggestions.length), (3, 7, 1));
    assert_eq!(suggestions.matches, vec![("world".to_owned(), None)]);

    // /plugins lists the plugins by kind.
    alice.clear_backlog();
    alice.command("plugins");
    let header: play::SystemChat = alice.expect();
    assert!(header.content.to_plain().ends_with("Server Plugins (2):"), "{}", header.content.to_plain());
    let mut lines = Vec::new();
    for _ in 0..4 {
        lines.push(alice.expect::<play::SystemChat>().content.to_plain());
    }
    assert_eq!(lines, vec!["Paper Plugins:", " - FakePlugin", "Bukkit Plugins:", " - OldPlugin"]);

    // AsyncChatEvent: the host renders the message per viewer; it arrives asynchronously.
    alice.chat("hi there");
    let line: play::SystemChat = alice.expect();
    assert_eq!(line.content.to_plain(), "[fake] Alice: hi there");

    // BlockBreakEvent cancelled: the block stays and the client is corrected.
    alice.drain(QUIET);
    alice.clear_backlog();
    let below = BlockPos::new(8, -61, 8);
    let before = server.handle.run(move |s| s.worlds[0].block_at(8, -61, 8)).unwrap();
    alice.send(&play::PlayerAction { status: VarInt(play::player_action::START_DESTROY_BLOCK), pos: below, face: 1, sequence: VarInt(9) });
    // (Animals eating grass elsewhere send block updates too.)
    let update: play::BlockUpdate = alice.find(TIMEOUT, |u: &play::BlockUpdate| u.pos == below).expect("the block is resent");
    assert_eq!((update.pos, Some(update.state.0 as u32)), (below, before));
    assert_eq!(server.handle.run(move |s| s.worlds[0].block_at(8, -61, 8)).unwrap(), before);

    // PlayerMoveEvent cancelled: the player is put back where the last event left them.
    alice.clear_backlog();
    alice.move_to(8.5, 150.0, 8.5);
    let back: play::PlayerPosition = alice.expect();
    assert!(back.y < 100.0, "moved back to y={}", back.y);

    // The server can kick through the API; PlayerQuitEvent replaces the quit message.
    let mut bob = Client::join(server.port(), "Bob");
    bob.expect::<play::PlayerPosition>();
    bob.drain(QUIET);
    bob.clear_backlog();
    server.handle.console_command("kick Alice");
    let quit: play::SystemChat = bob.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain() == "bye").expect("custom quit message");
    assert_eq!(quit.content.to_plain(), "bye");

    let calls = seen.calls.lock().unwrap().clone();
    for expected in ["event.join", "command.execute", "command.suggest", "event.command_preprocess", "event.chat", "event.block_break", "event.move", "event.quit", "event.server_command"] {
        assert!(calls.iter().any(|c| c == expected), "{expected} was not fired: {calls:?}");
    }

    // Stopping the server disables the plugins first.
    server.stop();
    assert_eq!(seen.calls.lock().unwrap().last().map(String::as_str), Some("host.shutdown"));
}

/// The plugin API calls of the new areas, made by the fake host the way the Java host makes
/// them: inventories (the player's, the ender chest, a custom menu and its click and close
/// events), scoreboards, entities, offline players, and `/reload` handing the players to a
/// new host.
#[test]
fn plugin_api_inventories_scoreboards_entities_offline_players_and_reload() {
    use paper_protocol::types::{HashedSlot, Slot};
    let seen = Arc::new(Seen::default());
    let server = start_server_with_host(|_| {}, fake_launcher(seen.clone()));
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.find(TIMEOUT, |c: &play::LevelChunkWithLight| c.x == 0 && c.z == 0).expect("spawn chunk");
    alice.drain(QUIET);
    alice.clear_backlog();
    let item = |name: &str| paper_data::items::id(name).unwrap() as i32;
    let result = |key: &str| seen.results.lock().unwrap().get(key).cloned().unwrap_or_else(|| panic!("no result {key}"));

    // The player inventory in Bukkit's slot order, and the ender chest.
    alice.command("api player");
    alice.drain(QUIET);
    let contents = result("player.contents");
    let slots = contents.list("items").unwrap();
    assert_eq!(slots.len(), 41);
    assert_eq!(slots[0].str("item"), Some("minecraft:stone"));
    assert_eq!(slots[39].str("item"), Some("minecraft:iron_helmet"));
    let (hotbar, helmet, ender) = server
        .handle
        .run(|s| {
            let p = s.player_by_name("Alice").unwrap();
            (p.inventory[36].clone(), p.inventory[5].clone(), p.ender_items[3].clone())
        })
        .unwrap();
    assert_eq!(hotbar, Slot::of(item("minecraft:stone"), 5), "Bukkit slot 0 is the first hotbar slot");
    assert_eq!(helmet, Slot::of(item("minecraft:iron_helmet"), 1), "Bukkit slot 39 is the helmet");
    assert_eq!(ender, Slot::of(item("minecraft:emerald"), 7));

    // A custom menu: created, filled and opened by the plugin; clicks go to the host, which
    // cancels them; closing it fires the close event.
    alice.clear_backlog();
    alice.command("api menu");
    let open: play::OpenScreen = alice.find(TIMEOUT, |_: &play::OpenScreen| true).expect("menu opened");
    assert_eq!(open.title.to_plain(), "Fake Menu");
    assert_eq!(open.menu_type.0, paper_data::registries::id("minecraft:menu", "minecraft:generic_9x1").unwrap() as i32);
    alice.drain(QUIET);
    let menu_items = result("menu.contents");
    assert_eq!(menu_items.list("items").unwrap()[4].str("item"), Some("minecraft:diamond"));
    assert_eq!(menu_items.list("items").unwrap()[4].get("name").as_str().map(|j| j.contains("Shiny")), Some(true));
    assert_eq!(result("menu.opened").bool("opened"), Some(true));
    let window = result("menu.state").get("open_inventory").clone();
    assert_eq!((window.str("type"), window.i64("size")), (Some("CHEST"), Some(9)));
    alice.send(&play::ContainerClick { window_id: open.window_id.0, state_id: 0, slot: 4, button: 0, mode: 0, changed_slots: Vec::new(), carried: HashedSlot(None) });
    alice.drain(QUIET);
    let (carried, kept) = server
        .handle
        .run(|s| {
            let p = s.player_by_name("Alice").unwrap();
            let kept = match p.open_menu.as_ref().map(|m| m.kind.clone()) {
                Some(paper_server::server::containers::MenuKind::Custom { id }) => s.plugin_data.inventories[&id].items[4].0.as_ref().map(|st| (st.item, st.count)),
                _ => None,
            };
            (p.carried.clone(), kept)
        })
        .unwrap();
    assert!(carried.is_empty(), "the host cancelled the click");
    assert_eq!(kept, Some((item("minecraft:diamond"), 2)));
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(open.window_id.0) });
    alice.drain(QUIET);
    {
        let events = seen.inventory_events.lock().unwrap();
        let kinds: Vec<&str> = events.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(kinds, vec!["event.inventory_open", "event.inventory_click", "event.inventory_close"]);
        let click = &events[1].1;
        assert_eq!((click.i64("slot"), click.i64("button"), click.i64("mode")), (Some(4), Some(0), Some(0)));
        assert_eq!(click.get("inventory").get("top").str("kind"), Some("custom"));
        assert_eq!(events[2].1.str("reason"), Some("PLAYER"));
    }

    // A plugin that opens the menu again from its close event keeps it open (no recursion,
    // and the new window is not closed with the old one).
    alice.clear_backlog();
    alice.command("api menu");
    let open: play::OpenScreen = alice.find(TIMEOUT, |_: &play::OpenScreen| true).expect("menu opened again");
    alice.drain(QUIET);
    seen.reopen.store(true, Ordering::Relaxed);
    alice.clear_backlog();
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(open.window_id.0) });
    let reopened: play::OpenScreen = alice.find(TIMEOUT, |_: &play::OpenScreen| true).expect("reopened from the close event");
    assert_ne!(reopened.window_id.0, open.window_id.0);
    alice.drain(QUIET);
    let open_window = server.handle.run(|s| s.player_by_name("Alice").unwrap().open_menu.as_ref().map(|m| m.window_id)).unwrap();
    assert_eq!(open_window, Some(reopened.window_id.0), "the reopened window stays open");
    alice.send(&play::ServerboundContainerClose { window_id: VarInt(reopened.window_id.0) });
    alice.drain(QUIET);

    // Taking a crafting result is a CraftItemEvent (when the game data has recipes); a drag is
    // one InventoryDragEvent when it ends.
    seen.inventory_events.lock().unwrap().clear();
    let has_recipes = server
        .handle
        .run(|s| {
            let id = s.player_by_name("Alice").unwrap().entity_id;
            s.set_inventory_slot(id, 1, Slot::of(paper_data::items::id("minecraft:oak_log").unwrap() as i32, 1));
            s.recipes.is_some()
        })
        .unwrap();
    let click = |slot: i16, button: i8, mode: i32| play::ContainerClick { window_id: 0, state_id: 0, slot, button, mode, changed_slots: Vec::new(), carried: HashedSlot(None) };
    if has_recipes {
        alice.send(&click(0, 0, 0));
        alice.drain(QUIET);
        let events = seen.inventory_events.lock().unwrap().clone();
        let craft = events.iter().find(|(m, a)| m == "event.inventory_click" && a.i64("slot") == Some(0)).map(|(_, a)| a.clone()).expect("craft click");
        assert_eq!(craft.get("craft").str("item"), Some("minecraft:oak_planks"), "{craft:?}");
        assert_eq!(craft.get("inventory").str("type"), Some("CRAFTING"));
        let carried = server.handle.run(|s| s.player_by_name("Alice").unwrap().carried.clone()).unwrap();
        assert_eq!(carried, Slot::of(item("minecraft:oak_planks"), 4), "the uncancelled craft took the result");
        seen.inventory_events.lock().unwrap().clear();
        for (slot, button) in [(-999, 0), (9, 1), (10, 1), (-999, 2)] {
            alice.send(&click(slot, button, 5));
        }
        alice.drain(QUIET);
        let events = seen.inventory_events.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "one event for the whole drag: {events:?}");
        let drag = &events[0].1;
        assert_eq!(drag.i64("mode"), Some(5));
        assert_eq!(drag.list("drag_slots").map(|l| l.iter().filter_map(Value::as_i64).collect::<Vec<_>>()), Some(vec![9, 10]));
        assert_eq!(drag.get("cursor").i64("count"), Some(4));
        let spread = server.handle.run(|s| { let p = s.player_by_name("Alice").unwrap(); (p.inventory[9].clone(), p.inventory[10].clone()) }).unwrap();
        assert_eq!(spread, (Slot::of(item("minecraft:oak_planks"), 2), Slot::of(item("minecraft:oak_planks"), 2)));
    }

    // Scoreboards: the packets follow the plugin's calls.
    alice.clear_backlog();
    alice.command("api board");
    let objective: play::SetObjective = alice.find(TIMEOUT, |o: &play::SetObjective| o.name == "points").expect("objective");
    assert_eq!((objective.mode, objective.display_name.to_plain()), (0, "Points".to_owned()));
    let display: play::SetDisplayObjective = alice.find(TIMEOUT, |_: &play::SetDisplayObjective| true).expect("display slot");
    assert_eq!((display.slot.0, display.objective.as_str()), (1, "points"));
    let score: play::SetScore = alice.find(TIMEOUT, |s: &play::SetScore| s.objective == "points").expect("score");
    assert_eq!((score.owner.as_str(), score.value), ("Alice", 12));
    let team: play::SetPlayerTeam = alice.find(TIMEOUT, |t: &play::SetPlayerTeam| t.name == "blue" && t.method == 2).expect("team update");
    assert_eq!(team.parameters.as_ref().map(|p| (p.prefix.to_plain(), p.color)), Some(("[B] ".to_owned(), Some(9))));
    let joined: play::SetPlayerTeam = alice.find(TIMEOUT, |t: &play::SetPlayerTeam| t.name == "blue" && t.method == 3).expect("team entries");
    assert_eq!(joined.members, vec!["Alice".to_owned()]);
    alice.drain(QUIET);
    assert_eq!(result("board.score").i64("value"), Some(12));
    assert!(result("board.new").i64("board").unwrap() > 0);

    // Entities: spawned, named and listed.
    alice.command("api mob");
    alice.drain(QUIET);
    let named = result("mob.named");
    assert_eq!(named.str("type"), Some("minecraft:cow"));
    assert_eq!((named.f64("health"), named.f64("max_health")), (Some(25.0), Some(30.0)));
    assert!(named.str("custom_name").is_some_and(|n| n.contains("Daisy")));
    assert_eq!(result("mob.dropped").get("item").str("item"), Some("minecraft:apple"));
    let around = result("mob.around");
    let types: Vec<&str> = around.as_list().unwrap_or(&[]).iter().filter_map(|e| e.str("type")).collect();
    assert!(types.contains(&"minecraft:cow") && types.contains(&"minecraft:item"), "{types:?}");

    // Offline players: Alice is known; an unknown name is not cached.
    alice.command("api offline");
    alice.drain(QUIET);
    let info = result("offline.alice");
    assert_eq!(info.str("name"), Some("Alice"));
    assert_eq!(info.bool("online"), Some(true));
    assert!(info.i64("first_played").unwrap() > 0);
    assert!(result("offline.nobody").is_null());

    // /reload: the host is stopped (plugins disabled) and started again with the players.
    alice.clear_backlog();
    server.handle.console_command("reload confirm");
    alice.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain() == "Reload complete.").expect("reload finished");
    let welcomes = seen.welcomes.lock().unwrap().clone();
    assert_eq!(welcomes.len(), 2, "a second host was started");
    assert_eq!(welcomes[1].bool("reload"), Some(true));
    let players = welcomes[1].list("players").unwrap();
    assert_eq!(players.iter().filter_map(|p| p.str("name")).collect::<Vec<_>>(), vec!["Alice"], "players are handed over");
    let calls = seen.calls.lock().unwrap().clone();
    let reload_at = calls.iter().position(|c| c == "host.shutdown").expect("the old host was shut down");
    assert_eq!(calls[reload_at + 1..].iter().filter(|c| *c == "event.join").count(), 0, "no join events after /reload");
    assert!(calls[reload_at + 1..].iter().any(|c| c == "host.load") && calls[reload_at + 1..].iter().any(|c| c == "host.post_world"));
    let board_kept = server.handle.run(|s| s.scoreboards.get(0).is_some_and(|b| b.objectives.contains_key("points"))).unwrap();
    assert!(board_kept, "the main scoreboard is the server's");
    // Plugin commands work with the new host.
    alice.clear_backlog();
    alice.command("hello");
    alice.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain() == "Hello, Alice!").expect("plugin command after reload");
    server.stop();
}

#[test]
fn server_continues_when_the_host_dies_if_configured() {
    let seen = Arc::new(Seen::default());
    let server = start_server_with_host(|c| c.paper.plugins.on_host_crash = paper_server::config::OnHostCrash::Continue, fake_launcher(seen.clone()));
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    alice.clear_backlog();

    // The fake host hangs up after this command.
    alice.command("crash");
    let tree: play::Commands = alice.expect();
    assert!(!literals(&tree).contains(&"hello".to_owned()), "plugin commands are withdrawn");
    assert_eq!(server.handle.run(|s| s.plugins.is_none()), Some(true));

    // The server keeps working without plugins; plugin commands are now unknown.
    alice.clear_backlog();
    alice.command("hello");
    let reply: play::SystemChat = alice.expect();
    assert_eq!(reply.content.0["translate"], "command.unknown.command");
    alice.chat("still here");
    let line: play::SystemChat = alice.find(TIMEOUT, |m: &play::SystemChat| m.content.to_plain().contains("still here")).expect("vanilla chat");
    assert_eq!(line.content.0["translate"], "chat.type.text");
    server.stop();
}

#[test]
fn server_stops_when_the_host_dies_by_default() {
    let seen = Arc::new(Seen::default());
    let server = start_server_with_host(|_| {}, fake_launcher(seen));
    let mut alice = Client::join(server.port(), "Alice");
    alice.expect::<play::PlayerPosition>();
    alice.drain(QUIET);
    alice.command("crash");
    assert!(alice.wait_closed(TIMEOUT), "the server shuts down and disconnects players");
    server.wait();
}
