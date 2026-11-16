# Rust plugins

Besides Java plugins, Paper-RS loads plugins written in Rust. They run inside the server
process, on its main thread, with no JVM involved. The crate `paper-plugin` is all a plugin
needs. It provides the `Plugin` trait, events, commands and the server's API.

A Rust plugin reaches the same API as Java plugins: the calls and events of
[the plugin bridge](plugin-bridge.md). It does not see Java plugins' objects. It cannot call
another plugin's Java API (Vault, LuckPerms, ...), and Java plugins cannot call it.

## Writing one

A plugin is a library crate built as a `cdylib`:

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
paper-plugin = { git = "https://github.com/IvenGe/Paper-Rust" }   # or a path to a checkout
```

It implements `Plugin` and exports the type with `export_plugin!`:

```rust
use paper_plugin::{export_plugin, Event, Info, Plugin, Sender, Server};

#[derive(Default)]
struct Greeter;

impl Plugin for Greeter {
    fn info(&self) -> Info {
        Info::new("Greeter", "1.0").listen("event.join").command("hello", "Says hello")
    }

    fn event(&mut self, server: &mut Server, event: &mut Event) {
        if let Some(player) = event.player() {
            player.send_message(server, "Welcome, from a Rust plugin!");
        }
    }

    fn command(&mut self, server: &mut Server, sender: &Sender, _label: &str, _args: &[String]) -> bool {
        sender.send_message(server, "Hello!");
        true
    }
}

export_plugin!(Greeter);
```

Build it with `cargo build --release`. Copy `target/release/libmy_plugin.so` into the
server's `plugins` folder (`my_plugin.dll` on Windows, `libmy_plugin.dylib` on macOS). The
server loads every such library at start, sorted by file name. The Java plugin host still
starts only if there are jars.

`crates/paper-plugin/examples/hello_plugin.rs` is a complete example: it welcomes players,
keeps diamond blocks from being broken, and adds `/hello` (with an alias and completions)
and `/ticks`:

```bash
cargo build --release -p paper-plugin --example hello_plugin
```

### Linking plugins into the server

A server binary of one's own can link plugins in instead of loading libraries:

```rust
let config = paper_server::config::Config::load(root)?;
let server = paper_server::start_with_native_plugins(
    config,
    paper_server::plugins::java_launcher(),
    vec![paper_plugin::entry::<Greeter>(), paper_plugin::entry::<Other>()],
)?;
server.wait();
```

Linked plugins load before the libraries of the plugins folder. The integration test
`crates/paper-server/tests/native_plugins.rs` uses both ways.

## What a plugin is told

`Plugin::info` names the plugin and says what it wants. Its fields are:

* `name`, `version`, `description`, `authors`. Names are unique: a second plugin with a
  name already loaded is not loaded.
* `events`: the events the plugin is given (see below).
* `commands`: the plugin's commands, each with a name, description, usage, aliases and the
  operator level it needs (0 for everyone).
* `ticks`: whether `Plugin::tick` is called every tick.

Then, in order:

1. `enable`, once the worlds are loaded and Java plugins are enabled, before players can
   join. If it panics, the plugin stays disabled.
2. While the server runs:
   * `event` for the events the plugin listens to;
   * `command` and `complete` for its commands;
   * `tick` once per tick, just before Java plugins' scheduler runs, if asked for.
3. `disable` when the server stops. Plugins are disabled in reverse load order, and then
   their libraries are unloaded.

`/reload` restarts only the Java plugin host. Rust plugins stay as they are.

## Events

An event is named either by its bridge call or by the Bukkit event that call stands for:
`event.join` or `PlayerJoinEvent`, `event.block_break` or `BlockBreakEvent`, and so on.
[The plugin bridge](plugin-bridge.md#calls-from-the-server-to-the-host) lists the calls,
their arguments and the outcome each one reads back. The server fires an event only if a
plugin listens, as it does for Java plugins.

Java plugins see an event first. Rust plugins then see it in load order:

* `Event::get` reads the event's arguments, as the bridge sends them to Java plugins.
* `Event::result` holds the outcome so far. That is what Java plugins decided. If no Java
  plugin heard the event, it is the event's arguments, whose fields have the names the
  outcome uses.
* `Event::set`, `Event::cancel` and `Event::set_cancelled` change the outcome. The last Rust
  plugin has the final word.

`Event::player` is the player the event is about, when there is one: the `uuid` of the
arguments, or of their `player`.

## Commands

A plugin's commands are in the command tree that players' clients get, as `name` and
`plugin:name`, together with their aliases. A command is hidden from players whose operator
level is below the command's. `command` receives the command's name, whichever alias was
typed, and the arguments split at spaces. If it returns false, the sender is shown the
usage. `complete` gives suggestions for the last argument as it is being typed. The console
runs the commands too, as `Sender::Console`.

## The server's API

`Server::call(method, args)` makes any call of
[the bridge's API for plugins](plugin-bridge.md#calls-from-the-host-to-the-server) with its
arguments as a map (`bmap!`). It returns the answer as a `Value`, or the server's error. Two
calls are for Rust plugins only:

| Method | Arguments |
| --- | --- |
| `server.log` | `message` (logged as `[Plugin] message`) |
| `server.players` | none; answers the online players, each with `uuid` and `name` |

`Server`, `Player` and `Sender` have shortcuts for the most common calls: `log`, `broadcast`,
`dispatch_command`, `players`, `player`, `block`, `set_block`, `send_message`, `send_title`,
`kick`, `state`, `teleport`, `set_game_mode` and `perform_command`.

## Threads

Plugins run on the server's main thread, as Bukkit plugins do on Paper's main thread. They
see the game as it is and their changes take effect at once. While the server is calling a
plugin, the plugin may call the server. The `Server` it is handed is valid only for that
call: it cannot be kept, and it cannot be sent to another thread.

Slow work (files, the network, databases) belongs on threads of the plugin's own. Hand the
results back, for example through a channel that `tick` reads. A plugin is never called
again while it is still running. If a call it makes leads back to it, for example by
dispatching one of its own commands, that inner call is not delivered.

## Failures

The server catches a panic in a plugin. It logs the panic, and the event or command goes on
as if the plugin had not answered. A failing command tells the sender an internal error
occurred.

A plugin is native code in the server's process, though. Undefined behaviour, an abort or a
crash in it takes the whole server down, while a Java plugin's crash only takes down the
plugin host. Load only plugins you trust.

## The interface

A plugin and the server talk through a small C interface, so a plugin need not be built with
the server's compiler version:

* The library exports `paper_plugin_entry`, which returns an `Entry`. The `Entry` holds the
  interface version (`ABI_VERSION`, now 1) and four functions: `create`, `call`, `destroy`
  and `free`.
* `create` receives the server's `Host`, which provides `call` and `free`.
* Every call carries a method name and its arguments, encoded as the bridge encodes
  `Value`s (`paper-bridge`).
* The answer comes back in a buffer that the side that made it frees. Plugin and server may
  therefore use different allocators.
* A call returns 0 if it worked and 1 if it failed, with the message in the buffer. A
  plugin's call returns 2 if the plugin panicked.

The server calls the plugin with these methods:

| Method | Arguments | Answer |
| --- | --- | --- |
| `plugin.info` | none | the plugin's `Info` as a map |
| `plugin.enable` / `plugin.disable` | none | none |
| `plugin.event` | `name` (the call), `bukkit` (the Bukkit event, or empty), `args`, `result` (the outcome so far), `tick` | the outcome |
| `plugin.command` | `sender` (`console` or the player's uuid), `label` (the command's name), `args`, `tick` | `handled` |
| `plugin.complete` | as `plugin.command` | a list of suggestions |
| `plugin.tick` | `tick` | none |

The server refuses a library built for a different interface version and logs the reason.
