# The plugin bridge

Java plugins run in a separate JVM, the *plugin host* (`plugin-host/`), which talks to the
Rust server over a local TCP socket. This page describes how the two processes cooperate
and the messages they exchange.

## Starting the host

Before it accepts players, the server:

1. listens on a random port on `127.0.0.1` and makes a random 128-bit token;
2. runs `java [jvm-args] -jar paper-rs-plugin-host.jar --connect 127.0.0.1:<port> --root <dir>
   --plugins <dir>` with the token in the environment variable `PAPER_RS_BRIDGE_TOKEN`;
3. expects a `HELLO` carrying the token and the bridge protocol version, and answers with
   `WELCOME`, which describes the server (name, versions, properties, worlds);
4. calls `host.load`, then `host.post_world`.

The host's standard output becomes the server's log: the host writes one `LEVEL|message`
line per log line, and anything else (a plugin printing to `System.out`) is logged as is.

If the host dies, the server stops (the default, since protection plugins would be gone
silently) or, with `on-host-crash = "continue"`, keeps running without plugins.

## One main thread across two processes

Plugins expect Paper's single main thread. Here each process has a main thread, and only one
of them runs game logic at a time:

* When the server's main thread needs plugins (an event, a plugin command, the per-tick
  scheduler run) it sends a `CALL` with the `SYNC` flag and waits.
* The host runs that call on its main thread. When plugin code uses the API, the host sends
  its own `SYNC` call and waits; the server's main thread, still inside its wait, runs it
  and answers.

So calls nest like ordinary function calls across the two processes, and each side's wait
loop serves the other side's nested calls. Plugin code always sees current state and runs
on what `Bukkit.isPrimaryThread()` reports as the main thread.

Calls without the `SYNC` flag are asynchronous. The server uses them for events Paper fires
off the main thread (chat, the server list ping, pre-login), which the host runs on worker
threads. The host uses them for API calls from its other threads (async tasks, async event
handlers); the server queues those and runs them on its main thread between ticks.

The host keeps a copy of each player's state (position, game mode, health, permissions
level, ...). Before every sync call and every tick the server sends the states that changed
(`players.state`), so plugin reads do not need a round trip.

## Wire format

```text
frame   := u32 length (big endian, counts the bytes after it) | u8 kind | body
HELLO   (1, host -> server) := value
WELCOME (2, server -> host) := value
CALL    (3, either way)     := uvarint call_id | u8 flags | string method | value args
RETURN  (4, either way)     := uvarint call_id | u8 status (0 ok, 1 error) | value
NOTIFY  (5, either way)     := string method | value args

flags   := bit 0: SYNC

value   := 0                          null
         | 1 | 2                      false | true
         | 3 zigzag-varint            64-bit integer
         | 4 f64 (big endian)         double
         | 5 uvarint len, utf-8       string
         | 6 uvarint len, bytes       byte string
         | 7 uvarint n, value * n     list
         | 8 uvarint n, (string value) * n   map with string keys
string  := uvarint len, utf-8
```

Values are dynamically typed, so new calls and events need no schema change. The Rust
implementation is `crates/paper-bridge`; the Java one is
`plugin-host/host/src/main/java/io/papermc/paperrs/host/bridge/Codec.java`. Both are tested
against the same golden byte vector. Text components travel as their JSON, players as UUID
strings, worlds by name.

## Calls from the server to the host

| Method | Arguments | Answer |
| --- | --- | --- |
| `host.load` | | `plugins`, `commands`, `events` (see below). Scans `plugins/`, runs bootstrappers and the bootstrap `COMMANDS` event, creates the server object, constructs plugins (`onLoad`), enables `STARTUP` plugins |
| `host.post_world` | | the same. Enables `POSTWORLD` plugins, fires `COMMANDS` for plugins and `ServerLoadEvent` |
| `host.tick` | `tick` | runs the Bukkit scheduler's main-thread tasks |
| `host.shutdown` | | disables all plugins, then the host exits |
| `event.join` | `player` (state), `inventory`, `message` | `message` (`PlayerJoinEvent`) |
| `event.quit` | `uuid`, `reason`, `message` | `message` (`PlayerQuitEvent`) |
| `event.command_preprocess` | `uuid`, `message` (with `/`) | `cancelled`, `message` |
| `event.server_command` | `command` | `cancelled`, `command` |
| `event.block_break` | `uuid`, `world`, `x`, `y`, `z`, `state` | `cancelled` |
| `event.block_place` | the same, `replaced`, `against` `[x, y, z]`, `offhand` | `cancelled` |
| `event.move` | `uuid`, `from`, `to` (`x y z yaw pitch`) | `cancelled`, or a changed `to` |
| `event.game_mode_change` | `uuid`, `mode` | `cancelled` |
| `event.chat` (async) | `uuid`, `message`, `default` | `cancelled`, `deliveries`: `[{viewer, message}]` rendered per viewer (`viewer` is a UUID or `console`) |
| `event.server_list_ping` (async) | `address`, `motd`, `online`, `max`, `protocol`, `version` | the same fields as changed, `hide_players`, `sample`, `cancelled` |
| `event.pre_login` (async) | `name`, `uuid`, `address` | `result` (`ALLOWED`, `KICK_BANNED`, ...), `message` |
| `command.execute` | `sender` (UUID or `console`), `command` | `handled` |
| `command.suggest` | `sender`, `text` (with `/`) | `start`, `length`, `matches`: `[{text, tooltip}]` |
| `command.visible` | `uuid` | `labels`: the plugin commands this player may see |

Some events are only fired when a plugin listens to them (the host reports which in
`events`): command preprocess, server command, block break and place, move, game mode
change and chat. Join, quit, the ping and pre-login are always fired.

`plugins` is a list of `{name, version, enabled, kind}` with `kind` `paper` or `bukkit`.
`commands` lists every label of every plugin command (with and without the plugin's
namespace); the server routes those to the host and puts them in each player's command tree
when `command.visible` allows. Plugin commands take precedence over built-in commands of the
same name, which stay reachable as `minecraft:<name>`.

## Notifications

| Direction | Method | Arguments |
| --- | --- | --- |
| server → host | `players.state` | `players`: the states that changed |
| host → server | `host.update` | any of `plugins`, `commands`, `events` that changed after a call |

## Calls from the host to the server

| Method | Arguments |
| --- | --- |
| `player.state` | `uuid` |
| `player.send_message` | `uuid`, `message`, `overlay` |
| `player.send_title` / `player.clear_title` | `uuid`, `title`, `subtitle`, `fade_in`, `stay`, `fade_out` / `reset` |
| `player.send_tab_list` | `uuid`, `header`, `footer` |
| `player.kick` | `uuid`, `reason` |
| `player.teleport` | `uuid`, `world`, `x`, `y`, `z`, `yaw`, `pitch` (answers the new state) |
| `player.set_game_mode` | `uuid`, `mode` |
| `player.set_op` | `uuid`, `op` |
| `player.set_display_name` / `player.set_list_name` | `uuid`, `name` |
| `player.set_health` | `uuid`, any of `health`, `food`, `saturation` |
| `player.set_flight` | `uuid`, any of `allow_flight`, `flying` |
| `player.perform_command` / `player.chat` | `uuid`, `command` / `message` |
| `player.send_plugin_message` | `uuid`, `channel`, `data` |
| `player.update_commands` | `uuid` |
| `player.play_sound` | `uuid`, `sound`, `category`, `x`, `y`, `z`, `volume`, `pitch` |
| `player.inventory` / `player.set_slot` / `player.set_held_slot` | `uuid` / `slot`, `item` / `slot` |
| `world.info` | `world` |
| `world.get_block` / `world.set_block` | `world`, `x`, `y`, `z` / `state` |
| `world.highest_block_y` | `world`, `x`, `z` |
| `world.set_time` | `world`, `time`, `daylight_cycle` |
| `world.set_spawn` | `world`, `x`, `y`, `z`, `yaw` |
| `world.is_chunk_loaded` | `world`, `x`, `z` |
| `server.dispatch_command` | `sender`, `command` (built-in commands; answers whether one ran) |
| `server.broadcast` | `message` |
| `server.status` | (answers `tps`, `mspt`, `tick`, `online`) |
| `server.whitelist` | (answers `enabled`, `players`, `ops`) |
| `server.shutdown` | |

## The host

The host reuses Paper's own plugin loading code (scanning, class loaders, `plugin.yml` and
`paper-plugin.yml` parsing, bootstrappers, the plugin manager, the lifecycle event system,
the scheduler), copied from `paper-server` by `plugin-host/host/tools/lift.py`. The classes
Paper implements on top of the Minecraft server are written for the bridge instead:
`CraftServer`, players, worlds and blocks, registries built from the server's data tables,
the command map and a Brigadier registrar for Paper plugins, and the events above. API
methods the host does not implement yet throw an `UnsupportedOperationException` naming the
method (`plugin-host/host/tools/StubGen.java` generates those defaults from the API).
