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
| `server` | the main thread: the tick loop, players (`join`, `handlers`, `view`, `player`), chat, commands, player data |
| `world` | chunk storage, the chunk pipeline, generators, lighting, Anvil storage, block updates (fluids, redstone, block physics) and scheduled ticks |
| `plugins` | the plugin host process and the bridge: calls, events, the server API plugins use |
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
* `<world>/entities/r.<x>.<z>.mca`: the mobs, dropped items, arrows, ... of each chunk
  (`Entities` with `id`, `Pos`, `Motion`, `Rotation`, `UUID`, `Health`, `equipment`, ...),
  loaded with their chunk on the worker and brought back to life on the main thread; saved
  on autosave and when their chunk unloads, which also drops them from memory.
* `<world>/level.dat`: seed, generator, spawn, time; `<world>/data/minecraft/game_rules.dat`:
  the game rules (older `level.dat` rules are imported).
* `<world>/playerdata/<uuid>.dat`: position, dimension, game mode, health, food, effects,
  experience, inventory, equipment and ender chest.
* The Nether and the End are separate worlds in CraftBukkit's layout: `<world>_nether` with
  its regions in `DIM-1/`, `<world>_the_end` with `DIM1/`, each with its own `level.dat`.
  Player data only lives in the main world.

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

## Entities

Entities other than players live in `World::entities`, indexed by chunk
(`World::entity_chunks`) for area queries. `world::entity` has the shared parts: boxes,
movement with collisions and step-up (vanilla's axis order), and the kinds (`Item`, `Mob`,
`Arrow`, `FallingBlock`, `ExperienceOrb`, `Tnt`). Each tick `Server::tick_entities` runs the
entities within simulation distance of a player: items fall, float and merge, players pick
them up; arrows fly and stick; mobs run `server::mobs` (environment damage, the behaviour
picked from the entity type, A* pathfinding over walkable blocks, vanilla's walking physics);
then removed entities are dropped and `track_entities` sends spawns, moves, metadata and
equipment to the players in tracking range.

`server::mobs` also does natural spawning (biome spawner lists from the vanilla data, the
mob caps, vanilla's light rules), animals for newly generated chunks, and despawning.

## Survival

`server::survival` is the player side of vanilla's `LivingEntity`, `Player` and `FoodData`:
the damage pipeline (`Server::hurt_player`: invulnerability, game rules, difficulty scaling
from the damage type, armour, protection enchantments, resistance, absorption), environmental
damage each tick, fall damage from movement packets, hunger and natural regeneration,
eating, experience, death and respawn. `server::combat` is fighting (`Player.attack` with the
cooldown, criticals, sweeps, knockback and enchantments; damage to mobs and their deaths with
loot tables and experience; bows and arrows; explosions). `server::effects` keeps status
effects on players and mobs; `server::mining` checks how long blocks take to break.
Damage types, recipes and loot tables are read from the vanilla server jar at startup
(`vanilla.rs`).

## Containers

Container contents live in the chunk's block entities (`Items`, as vanilla saves them). A
window is a `server::containers::OpenMenu`; clicks run through `server::menu`, a port of
vanilla's `AbstractContainerMenu.doClick` and the menus' `quickMoveStack`, on a copy of the
slots that is written back and sent to every viewer. Crafting uses `server::recipes`
(shaped, shapeless and transmute recipes from the vanilla data); furnaces, hoppers, droppers
and dispensers tick in `server::containers` and `server::machines`. When a block entity is
removed, `World::set_block` hands it to the server, which drops a container's contents.

## Dimensions

`server::portals` moves players between worlds: Nether portals (frames and lighting in
`world::portal` and `world::block_updates::fire`, the wait in the portal, coordinates scaled
by the dimension types, the exit portal found nearby or built like vanilla's
`PortalForcer`), End portals (frames filled with eyes, the obsidian platform, the exit
portal and the credits). A trip first loads the destination chunks with tickets; then
`Server::change_dimension` releases the old world, sends the Respawn packet and everything
the client needs again, and starts the view in the new world.

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
