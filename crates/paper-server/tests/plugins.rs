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
            Some(Frame::Welcome(v)) => *seen.welcome.lock().unwrap() = Some(v),
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
                "commands" => Value::List(vec!["hello".into(), "fakeplugin:hello".into(), "crash".into()]),
                "events" => Value::List(vec!["PlayerCommandPreprocessEvent".into(), "BlockBreakEvent".into(), "AsyncChatEvent".into(), "PlayerMoveEvent".into(), "ServerCommandEvent".into()]),
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
            "command.visible" => bmap! { "labels" => Value::List(vec!["hello".into(), "fakeplugin:hello".into(), "crash".into()]) },
            "command.execute" => {
                let sender = args.str("sender").unwrap().to_owned();
                let command = args.str("command").unwrap().to_owned();
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
    Box::new(move |_config, port, token| {
        let token = token.to_owned();
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
    let update: play::BlockUpdate = alice.expect();
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
