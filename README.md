Paper-RS
========

A rewrite of the [Paper](https://github.com/PaperMC/Paper) Minecraft server in Rust, for
Minecraft Java Edition 26.3 (protocol 777), that still runs unmodified Java plugins: both
classic `plugin.yml` (Bukkit/Spigot) plugins and `paper-plugin.yml` plugins with
bootstrappers and Brigadier commands.

The game server is Rust. Java plugins run in a separate JVM, the *plugin host*, which talks
to the server over a local socket ("separate mode"). Without plugins no JVM is started.

> This is an early port. The protocol, worlds, players, survival gameplay (mobs, damage,
> hunger, containers, crafting, redstone, fluids) and the plugin bridge work end to end (see
> [Status](#status)); vanilla world generation, the Nether and the End and parts of the
> plugin API are still missing.

Running
-------

You need Rust 1.89 or newer. For Java plugins you also need JDK 25 and Maven.

```bash
cargo build --release
```

```bash
./target/release/paper --root my-server
```

The first start creates `my-server/server.properties` (the vanilla settings) and
`my-server/config/paper-rs.toml` (threads, network, world and plugin settings). The world is
saved in vanilla's Anvil format under `my-server/<level-name>/`.

### Java plugins

Build the plugin host once:

```bash
cd plugin-host && mvn package
```

Then either copy `plugin-host/host/target/paper-rs-plugin-host.jar` into
`my-server/plugin-host/`, or point `host-dir` in `config/paper-rs.toml` at
`plugin-host/host/target`. Put plugin jars in `my-server/plugins/` as usual.

```toml
[plugins]
enabled = true
java = "java"            # a Java 25 runtime
host-dir = "plugin-host"
jvm-args = ["-Xmx1G"]
on-host-crash = "shutdown"   # or "continue" to keep running without plugins
```

Status
------

What works:

* **Networking**: server list ping, online (Mojang authentication, encryption) and offline
  login, compression, the configuration phase, keep-alive.
* **Worlds**: a flat generator and a built-in noise terrain generator, sky and block light,
  chunk streaming with the client's flow control, block breaking and placing (with
  orientation for stairs, logs, slabs, doors, beds, ...), world time.
* **Block updates**: neighbour and shape updates (wire, fence, pane and stair connections,
  blocks that lose their support break, doors and beds break together), scheduled block and
  fluid ticks (saved with the chunk), water and lava (flowing, falling, infinite water,
  waterlogged blocks, obsidian, cobblestone, stone and basalt), redstone (wire, torches,
  levers, buttons, repeaters, comparators, observers, lamps, note blocks, redstone blocks,
  doors, trapdoors and fence gates, pistons and sticky pistons), and using levers, buttons,
  doors, repeaters and comparators by hand.
* **Entities**: dropped items (pickup, merging, despawning), experience orbs, arrows, falling
  blocks and primed TNT, and mobs: animals wander, look at players and panic when hurt;
  zombies, spiders and other monsters chase and hit players; skeletons shoot arrows;
  creepers explode; undead burn in daylight. Mobs use A* pathfinding and vanilla's walking
  physics, spawn naturally from the biomes' spawner lists (animals also when a chunk is
  generated), despawn far from players, and drop loot (vanilla loot tables) and experience.
* **Survival**: damage from falls, the void, lava, fire, drowning, suffocation, cacti and
  magma, with armour, protection enchantments, difficulty scaling and invulnerability frames;
  hunger, saturation and natural regeneration; eating; death (dropped inventory and
  experience, death messages) and respawning; attacking with the attack cooldown, critical
  hits, sweeping, knockback and weapon enchantments; tool and weapon wear; bows; explosions
  that break blocks and hurt and push entities. Blocks drop their loot when broken.
* **Containers**: chests (also double), trapped chests, barrels, ender chests, crafting tables
  and the 2x2 inventory grid with the vanilla recipes, furnaces, smokers and blast furnaces
  that smelt over time; vanilla's click rules (shift-clicks, drags, number keys).
* **Game rules**: all 26.3 rules with `/gamerule`, saved in `data/minecraft/game_rules.dat`.
* **Saving**: chunks in region files, `level.dat` and player data in vanilla's layout,
  autosave, `/save-all`, saving on shutdown.
* **Players**: movement and seeing each other, chat, game modes, creative inventory,
  operators, whitelist and bans, the player list.
* **Commands**: `help list me msg say gamemode defaultgamemode tp give clear setblock time
  difficulty gamerule summon kill seed setworldspawn tps mspt kick ban pardon ban-ip
  pardon-ip banlist op deop whitelist save-all stop version plugins`.
* **Java plugins**: loading and enabling, `onLoad`/`onEnable`/`onDisable`, Paper
  bootstrappers and the `COMMANDS` lifecycle event, plugin commands (`plugin.yml` commands with
  permissions, aliases and tab completion; Brigadier commands and `BasicCommand`), the
  Bukkit, async and global-region schedulers, configuration files, permissions, and these
  events: `AsyncPlayerPreLoginEvent`, `PaperServerListPingEvent`, `PlayerJoinEvent`,
  `PlayerQuitEvent`, `AsyncChatEvent` and `AsyncPlayerChatEvent`,
  `PlayerCommandPreprocessEvent`, `ServerCommandEvent`, `PlayerCommandSendEvent`,
  `BlockBreakEvent`, `BlockPlaceEvent`, `PlayerMoveEvent`, `PlayerTeleportEvent` (for
  teleports made by plugins), `PlayerGameModeChangeEvent`, `ServerLoadEvent`. Players, worlds and blocks can be read and
  changed through the API (messages, titles, sounds, teleporting, health, flight, blocks,
  time, spawn, ...).

Not yet:

* Vanilla world generation, the Nether and the End. Mobs are not saved with the world yet,
  status effects, breeding, villagers and trading, beds, enchanting and anvils are missing.
  Pressure plates, tripwires, dispensers, droppers and hoppers do not react yet, and
  pistons do not push entities.
* Plugin API: item stacks and inventories, offline players, entities other than players,
  block data, scoreboards, boss bars, creating worlds, registry and tag lifecycle events,
  `/reload`.
  Methods that are not implemented throw an `UnsupportedOperationException` that names
  them.

Layout
------

| Path | What |
| --- | --- |
| `crates/paper-nbt` | NBT reading and writing, SNBT, modified UTF-8 |
| `crates/paper-protocol` | packet framing, compression, encryption, text components, packets |
| `crates/paper-data` | block, item, registry and tag tables for 26.3 (built from `data/26.3`) |
| `crates/paper-bridge` | the wire format shared with the plugin host |
| `crates/paper-server` | the server: networking, tick loop, players, worlds, storage, plugin bridge |
| `plugin-host/` | the plugin host (Java), plus two small test plugins |
| `data/26.3` | the version data tables, see its README |
| `docs/` | [architecture](docs/architecture.md) and the [plugin bridge](docs/plugin-bridge.md) |

The Java sources of upstream Paper (`paper-api`, `paper-server`, ...) are still in the tree as
the reference for the port; the plugin host builds against the published Paper API.

Testing
-------

```bash
cargo test
```

The tests include protocol clients that join a real server over TCP. The Java plugin test
(`crates/paper-server/tests/java_plugins.rs`) runs the real plugin host with the two test
plugins when Java 25 and the built jars are available (run `mvn package` in `plugin-host/`
first, and set `PAPER_RS_TEST_JAVA` if Java 25 is not found automatically); otherwise it
skips itself.

License
-------

Like Paper, this project is licensed under the GPLv3; see [LICENSE.md](LICENSE.md).
