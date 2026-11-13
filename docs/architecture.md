# Architecture

Paper-RS is a Minecraft server written in Rust plus a JVM that hosts Java plugins. This page
describes how the Rust side is organised and how work is spread over threads. The plugin
bridge has [its own page](plugin-bridge.md).

## Crates

```text
paper-nbt        NBT (file and network forms), SNBT, modified UTF-8
paper-data       version tables for 26.3, compiled in from data/26.3 by build.rs
paper-protocol   codec, framing, zlib compression, AES/CFB8, text components, packets
paper-bridge     the plugin bridge wire format (frames and dynamically typed values)
paper-server     the server itself, and the `paper` binary
```

`paper-server` is split into:

| Module | Responsibility |
| --- | --- |
| `net` | the listener and one task per connection: handshake, status, login (authentication, encryption, compression), configuration; then a reader and a writer per player |
| `server` | the main thread: the tick loop, players (`join`, `handlers`, `view`, `player`), chat, commands, player data, scoreboards (`scoreboard`), the name cache (`usercache`) |
| `world` | chunk storage, the chunk pipeline, generators, lighting, Anvil storage, block updates (fluids, redstone, block physics) and scheduled ticks |
| `plugins` | the plugin host process and the bridge: calls, events, the server API plugins use (`api`, and `inventory`, `entity`, `scoreboard`, `offline` for those areas), `/reload` |
| `config`, `access`, `logging` | `server.properties` and `config/paper-rs.toml`, ops/whitelist/bans, the console log |

## Threads

```text
            ┌────────────────────────┐   MainTask queue    ┌───────────────────────────┐
 clients ──►│ network I/O (tokio,    │ ──────────────────► │ main thread               │
            │ N threads): decode,    │                     │ 20 ticks/s; owns players, │
            │ decrypt, decompress    │ ◄────────────────── │ worlds, commands          │
            └────────────────────────┘   per-player queue  └───────┬───────────▲───────┘
                                                                   │ jobs      │ WorldEvent
                                                           ┌───────▼───────────┴───────┐
                                                           │ worker pool (rayon):      │
                                                           │ load/generate, light,     │
                                                           │ encode + compress chunks  │
                                                           └───────┬───────────────────┘
                                                                   │ chunk / player saves
                                                           ┌───────▼───────────────────┐
                                                           │ chunk I/O thread: writes  │
                                                           │ region files, level.dat,  │
                                                           │ player data, in order     │
                                                           └───────────────────────────┘
```

* **Network I/O** (`threads.io`, a multi-threaded tokio runtime). Each connection is a task.
  Before the play state the task handles everything itself. In play it splits into a reader,
  which decodes packets and queues them for the main thread, and a writer, which frames,
  compresses and encrypts what the main thread queued for that player. Game state never
  lives on these threads.
* **Main thread** (`Server::run`). Runs 20 ticks per second and between ticks handles
  `MainTask`s as they arrive: packets, joins, disconnects, finished worker jobs, console
  commands, plugin calls. All game state (players, worlds, the command registry) is owned
  here and needs no locks.
* **Workers** (`threads.workers`, a rayon pool). Pure jobs over snapshots: loading or
  generating a chunk, lighting a chunk from its 3x3 neighbourhood, encoding and compressing
  a chunk packet. Results come back to the main thread as `WorldEvent`s.
* **Chunk I/O** (one thread). Writes chunks, `level.dat` and player data in the order they
  were queued.

Sections and light arrays sit behind `Arc`s, so snapshotting a chunk for a worker copies
pointers; the main thread copies a section only if it changes it while a snapshot is alive
(`Arc::make_mut`).

## The chunk pipeline

```text
ticket added ──► load or generate (worker) ──► light (worker, needs 3x3 terrain) ──► ready
                                                         packet wanted ──► encode (worker)
```

Players hold tickets on the chunks in their view distance. `World::pump` starts jobs for
wanted chunks every tick, a bounded number per worker. A chunk is *ready* once it is lit;
then `World::chunk_packet` returns its cached, already framed packet or starts an encode
job. Each chunk has a revision; results computed from an older revision are discarded, and
light jobs compare a stamp of the 3x3 terrain they were started from.

Chunks are sent nearest first in batches that follow the client's reported throughput
(`ChunkBatchReceived`), like vanilla.

Block changes (`World::set_block`) update heightmaps and section counters immediately and
repair light incrementally on the main thread; the changed light sections are sent with the
block update.

## Storage

`world::storage` reads and writes vanilla's formats:

* `<world>/region/r.<x>.<z>.mca`: chunks as zlib-compressed NBT with paletted block states
  and biomes, light arrays, heightmaps and block entities. Oversized chunks go to `.mcc`
  files, as in vanilla.
* `<world>/level.dat`: seed, generator, spawn, time.
* `<world>/playerdata/<uuid>.dat`: position, game mode, health, food, inventory, equipment,
  the ender chest, and Bukkit's and Paper's play times (`bukkit.firstPlayed`, `Paper.LastLogin`, ...).
* `usercache.json` (in the server directory): names and UUIDs of players who joined, as vanilla keeps it.

Loading is part of the load-or-generate job on the workers. Every write goes through the
chunk I/O thread. Until a chunk or player file is written it stays in a pending table that
reads check first, so a chunk unloaded and requested again right away is never read stale.

Generated chunks are saved too, so explored terrain does not depend on the generator staying
the same. Changed chunks are saved when they are unloaded, on autosave
(`world.autosave-interval-ticks`, 6000 by default), on `/save-all` and at shutdown.
`session.lock` is locked while a server uses a world.

## Block updates

`World::set_block` only stores a block (and repairs light). Everything vanilla does around a
block change lives in `world::block_updates` and `world::ticks`, and game code reaches it
through the server:

* `Server::set_block_and_update(world, pos, state)` is the entry point for changing a block
  (`set_block_with_flags` takes vanilla's `Block.UPDATE_*` flags, `destroy_block` breaks a
  block with its effect and drops, `player_destroy_block` is a player breaking a block). The
  old block reacts to its removal, the new one to its placement, the six neighbours are told
  (west, east, down, up, north, south) and re-check their shape, and blocks schedule ticks.
  Updates go through a queue like vanilla's `CollectingNeighborUpdater`.
* Scheduled block and fluid ticks live in each chunk (`Chunk::ticks`), are saved as
  `block_ticks` / `fluid_ticks`, and run in `Server::tick_block_updates` once per tick (due
  time, priority, scheduling order; 65536 per kind per tick) for chunks players keep loaded,
  followed by block events (note blocks, pistons) and moving pistons.
* What clients must see (block and light changes, sounds, world and block events) and blocks
  that drop items are collected as `Effect`s in `World::updates` and published by
  `Server::publish_block_updates`.

Hooks for other systems: `Server::drop_block_items` receives every block that block physics
destroy with drops (washed away, lost support, broken by a piston); `block_updates::redstone::
container_signal` is what comparators read from containers; `Server::use_block` and
`Server::placement_state` are the right-click and placement rules.

## Plugins

Java plugins run in the plugin host JVM. The server starts it before it accepts players,
calls it to fire events and run plugin commands, and drives the plugin scheduler once per
tick. Calls nest across the two processes so plugin code always sees one main thread; see
[the plugin bridge](plugin-bridge.md).

## Version data

`data/26.3` holds the tables the server needs for 26.3: packet ids, registries, blocks and
their states, items, tags, dimension types. They were derived from the 26.3 server jar (read
as data; none of its code is run) and two public datasets; `data/26.3/README.md` lists the
sources and the checks.
