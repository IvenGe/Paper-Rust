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
> [Status](#status)); some vanilla features and parts of the plugin API are still missing.

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
* **Worlds**: vanilla world generation for the overworld, the Nether and the End, ported
  from 26.3 and driven by the vanilla jar's worldgen data (`crates/paper-worldgen`): biomes,
  terrain with aquifers and ore veins, surface rules, caves and canyons, and the features
  stage (ores, trees, plants, lakes, dungeons, geodes, icebergs, nether fungi and basalt,
  end spikes and islands, sculk patches, ...), and structures (villages, strongholds,
  mineshafts, nether fortresses, bastions, end cities, ancient cities, trial chambers,
  temples, shipwrecks, ruined portals, ocean monuments, woodland mansions, ...;
  `generate-structures`) with their loot chests and mobs, saved with the chunks in
  vanilla's format so structures continue the same after a restart, and their own natural
  spawns (guardians at monuments, witches at swamp huts, pillagers at outposts, the
  mobs of fortresses), with vanilla's spawn search for new worlds; also the flat preset.
  `/locate structure` and thrown eyes of ender find structures. Sky and block light,
  chunk streaming with the client's flow control, block breaking and placing (with
  orientation for stairs, logs, slabs, doors, beds, ...), world time.
* **Block updates**: neighbour and shape updates (wire, fence, pane and stair connections,
  blocks that lose their support break, doors and beds break together), scheduled block and
  fluid ticks (saved with the chunk), water and lava (flowing, falling, infinite water,
  waterlogged blocks, obsidian, cobblestone, stone and basalt), redstone (wire, torches,
  levers, buttons, repeaters, comparators, observers, lamps, note blocks, redstone blocks,
  doors, trapdoors and fence gates, pistons and sticky pistons, tripwire hooks and string),
  and using levers, buttons, doors, repeaters and comparators by hand. Pistons push the
  entities in their way (slime blocks launch them, honey blocks carry what stands on them),
  and moving blocks can be stood on. String between two hooks sets them off while anything
  is in it, unless cut with shears. Dispensers shoot arrows, thrown items and fire charges,
  place and pick up water, lava and powder snow (fish come out of their buckets), light
  fires and TNT, use bone meal, shear sheep and full beehives, fill glass bottles with
  water or honey, make mud with water bottles, put armour, heads, elytra, saddles, horse
  and wolf armour and llama carpets on players and mobs in front, and hatch spawn eggs;
  droppers move items into containers. Crafters craft on each pulse (into the container in
  front, or out of the front), hoppers fill them evenly, players switch their slots off,
  and comparators count their used and switched-off slots. Players fill glass bottles at water and full
  beehives, take honeycomb with shears, and make mud with water bottles.
* **Random ticks and farming**: vanilla's random ticks (`random_tick_speed` per section near
  players): crops grow by vanilla's farmland rules, farmland gets wet near water or dries
  back to dirt, grass and mycelium spread or die in the dark, sugar cane, cactus (and its
  flowers), pumpkin and melon stems, sweet berries, nether wart and cocoa grow, leaves away
  from logs decay, ice and snow melt near light, lava sets things on fire. Saplings grow
  into vanilla's trees (2x2 ones from four saplings, bee nests near flowers), and bone meal
  grows them, azaleas, huge mushrooms and huge fungi; flat worlds take the tree features
  from the vanilla jar too. Hoes till, shovels make paths, axes strip logs and wood, bone
  meal grows crops; seeds need farmland and jumping on it tramples it.
* **Entities**: dropped items (pickup, merging, despawning), experience orbs, arrows, falling
  blocks and primed TNT, and mobs: animals wander, look at players and panic when hurt;
  zombies, spiders and other monsters chase and hit players; skeletons shoot arrows;
  creepers explode; undead burn in daylight. Mobs use A* pathfinding and vanilla's walking
  physics, spawn naturally from the biomes' spawner lists (animals also when a chunk is
  generated), despawn far from players, and drop loot (vanilla loot tables) and experience.
  Mob spawners (dungeons) spawn their mob near players; end crystals explode when hit.
  Animals follow players holding their food and breed when fed (babies grow up, faster when
  fed); cows and goats give milk, mooshrooms stew; sheep are sheared and dyed and eat grass
  to grow their wool back; chickens lay eggs; name tags name mobs.
  Snowballs, eggs (one in eight hatches a chick), ender pearls (with their cooldown, fall
  damage and the odd endermite), bottles o' enchanting and splash and lingering potions
  (effects weaker further from the splash, water putting out fires and hurting endermen
  and blazes, clouds that shrink as they are used) are thrown by players and dispensers.
  Spawn eggs spawn their mob on the clicked block (named by an anvil name), a baby when
  used on a grown mob of their kind, or set the mob of a spawner, and dispensers hatch them
  too.
  Endermen turn on players who look them in the eyes (not through a carved pumpkin),
  teleport away from water, rain, daylight and arrows (and towards a far target), and pick
  up and put down blocks.
* **Riding and pets**: saddles go on pigs, striders, horses and camels; players get on and
  steer (their client moves the mount; carrots and warped fungi on sticks steer and boost
  pigs and striders) and get off with the sneak key. Horses have random health, speed,
  jump and coat, buck their rider off until their temper (raised by feeding) lets them be
  tamed, wear horse armour and eat to heal and grow. Wolves (bones), cats (fish) and
  parrots (seeds) are tamed, sit when told, follow their owner (teleporting when far
  behind), heal from their food, and wolves wear dyed collars and armour and join their
  owner's fights; pets never turn on their owner. Owners, saddles and pets are saved.
* **Villagers and trading**: villagers take the type of the biome they are born in, walk to a
  free workstation and take its profession (and lose it again if it is broken before anyone
  traded with them), and trade: the trading window shows vanilla's trades, read from the
  26.3 trade data in the server jar (`trade_set`, `villager_trade` and their tags, with
  enchanted, dyed and potion goods and type-specific trades); picking a trade fills the
  payment from the inventory, prices follow demand, gossip (trading raises a player's
  reputation, hurting a villager lowers it, killing one in front of others more) and Hero
  of the Village; trading gives experience orbs and villager experience, and villagers level
  up (with new trades) after the trading ends. In the day villagers work at their
  workstation and restock there up to twice a day. Wandering traders offer nine trades (two
  they buy, seven they sell), turn up near players with vanilla's daily chance
  (`spawn_wandering_traders`, saved in `level.dat`) and leave after two days. Villagers,
  their trades, experience, gossip and workstation are saved in vanilla's format.
* **Survival**: damage from falls, the void, lava, fire, drowning, suffocation, cacti and
  magma, with armour, protection enchantments, difficulty scaling and invulnerability frames;
  hunger, saturation and natural regeneration; eating; death (dropped inventory and
  experience, death messages) and respawning; attacking with the attack cooldown, critical
  hits, sweeping, knockback and weapon enchantments; tool and weapon wear; bows; attributes
  (max health, armour, attack damage and speed, movement speed, knockback resistance, fall
  damage, reach, ...) with the modifiers of equipment, effects and sprinting, sent to the
  clients and saved with players and mobs; explosions
  that break blocks and hurt and push entities. Blocks take vanilla's mining time (tool
  speeds, efficiency, haste, mining fatigue, water and air) and drop their loot. Status
  effects (`/effect`, golden apples, potions, poisonous food, milk, chorus fruit, poisonous
  mob bites): regeneration, poison, wither, hunger, saturation, instant health and damage,
  absorption, resistance, fire resistance, water breathing, strength, weakness, jump boost,
  slow falling, invisibility, and the attribute effects (speed, slowness, ...) for clients.
  Beds set the respawn point and let players sleep through the night (with
  `players_sleeping_percentage`; not with monsters nearby, and getting hurt wakes them); they
  explode in the Nether and the End.
* **Containers**: chests (also double), trapped chests, barrels, ender chests, crafting tables
  and the 2x2 inventory grid with the vanilla recipes, furnaces, smokers and blast furnaces
  that smelt over time, hoppers, droppers and dispensers; vanilla's click rules
  (shift-clicks, drags, number keys). Containers with a loot table (dungeon chests) are
  filled when first opened, reached by a hopper or broken, with vanilla's loot modifiers
  (counts, random enchantments, enchanting with levels, damage, potions). Enchanting tables
  offer vanilla's three enchantments from the bookshelves around them for lapis and levels;
  anvils repair with materials, combine items and enchanted books, rename, add the prior
  work penalty and wear out. Brewing stands brew vanilla's potions (and splash and
  lingering ones) with blaze powder, also fed by hoppers.
* **Redstone with items and entities**: hoppers move items between containers (furnace
  sides included), pick up dropped items and lock when powered; droppers and dispensers
  fire on a rising signal (also from above), droppers into containers in front of them,
  dispensers shooting arrows, pouring and scooping buckets, priming TNT and lighting fire;
  pressure plates (wooden, stone, weighted) are pressed by players, mobs and items;
  comparators read container contents; redstone primes TNT.
* **Dimensions**: the Nether and the End as separate worlds in CraftBukkit's layout
  (`world_nether/DIM-1`, `world_the_end/DIM1`; `allow-nether` in `server.properties`,
  `allow-end` in `config/paper-rs.toml`). Nether portals are lit with flint and steel or fire
  charges, take players across after vanilla's delay with coordinates scaled by eight, and
  lead to an existing portal nearby or a new one built like vanilla's; frames that break put
  the portal out. End portal frames filled with eyes of ender open the End portal, players
  arrive on the obsidian platform, and the exit portal shows the credits and brings them
  home. The gateways on the outer islands lead back to the main island. Fire burns, spreads and lights portals; buckets scoop up and pour water and lava
  (water boils away in the Nether).
* **The dragon fight**: a new End's island has its exit portal closed and the ender dragon
  circling the pillars, with its boss bar (music and fog) for players within 192 blocks. The
  dragon follows vanilla's phases: it flies a ring of 24 nodes round the pillars (the inner
  rings once the crystals are gone), strafes players with fireballs whose breath lingers,
  charges, and lands on the portal to breathe fire before taking off again. Crystals on the
  pillars heal it, and breaking the one healing it hurts it; only players and explosions
  hurt it, its head takes full damage and the rest a quarter, and arrows bounce off while it
  sits. Its wings knock players back and its head bites; it breaks through blocks that are
  not dragon-immune. Killed, it flies to the portal and rises for ten seconds while 12000
  experience pours out (500 after the first time); then the exit portal opens, the dragon
  egg appears the first time, and the next of twenty gateways opens around the island,
  leading out to the islands beyond (found along its direction from the terrain noise, with
  a little island made if there is no room) where a gateway leads back. The fight is saved in
  the End's `level.dat` as vanilla's `DragonFight`.
* **Weather**: vanilla's rain and thunder cycle (`advance_weather`) with `/weather`, saved in
  `level.dat`: clients see rain and thunder fade in and out, storms darken the sky (enough to
  sleep in a thunderstorm), rain puts out fires, keeps farmland wet and protects undead from
  the sun, lightning strikes in thunderstorms (fire, damage), still water freezes and snow
  piles up in cold biomes, and sleeping through the night clears the weather.
* **Game rules**: all 26.3 rules with `/gamerule`, saved in `data/minecraft/game_rules.dat`.
* **Saving**: chunks in region files, entities (mobs, items, arrows, ...) in
  `entities/` region files, `level.dat`, game rules and player data (inventory, ender chest,
  experience, ...) in vanilla's layout, autosave, `/save-all`, saving on shutdown.
* **Players**: movement and seeing each other, chat, game modes, creative inventory,
  operators, whitelist and bans, the player list.
* **Commands**: `help list me msg say gamemode defaultgamemode tp give clear setblock time
  weather difficulty gamerule summon kill effect seed locate setworldspawn tps mspt kick ban pardon ban-ip
  pardon-ip banlist op deop whitelist save-all stop version plugins reload`.
* **Java plugins**: loading and enabling, `onLoad`/`onEnable`/`onDisable`, Paper
  bootstrappers and the `COMMANDS` lifecycle event, plugin commands (`plugin.yml` commands with
  permissions, aliases and tab completion; Brigadier commands and `BasicCommand`), the
  Bukkit, async and global-region schedulers, configuration files, permissions, and these
  events: `AsyncPlayerPreLoginEvent`, `PaperServerListPingEvent`, `PlayerJoinEvent`,
  `PlayerQuitEvent`, `AsyncChatEvent` and `AsyncPlayerChatEvent`,
  `PlayerCommandPreprocessEvent`, `ServerCommandEvent`, `PlayerCommandSendEvent`,
  `BlockBreakEvent`, `BlockPlaceEvent`, `PlayerMoveEvent`, `PlayerTeleportEvent` (for
  teleports made by plugins), `PlayerGameModeChangeEvent`, `ServerLoadEvent`,
  `InventoryOpenEvent`, `InventoryClickEvent`, `InventoryDragEvent`, `InventoryCloseEvent`,
  `CraftItemEvent`, `PlayerAttemptPickupItemEvent`, `EntityPickupItemEvent`,
  `PlayerDropItemEvent`, `EntityDamageEvent` (and its `ByEntity`/`ByBlock` forms),
  `EntityDeathEvent`, `PlayerDeathEvent`, `PlayerRespawnEvent`, `CreatureSpawnEvent`. Players,
  worlds and blocks can be read and changed through the API (messages, titles, sounds,
  teleporting, health, flight, blocks, time, spawn, ...), and so can:
  * item stacks and metas (names, lore, enchantments, damage, unbreakable, custom model data,
    item flags, the persistent data container; other components are kept intact), the
    special metas of heads (`SkullMeta`), potions and tipped arrows (`PotionMeta`, and
    `PotionType` data), suspicious stew, fireworks and firework stars, banners, written and
    writable books, block data (`BlockDataMeta`), maps (id and colour), ominous bottles, and
    inventories: the player inventory, ender chests, chests, barrels and furnaces (as block
    states), custom inventories from `Bukkit.createInventory` opened with
    `Player.openInventory`, open windows and the cursor;
  * entities: listing, spawning and dropping items, and the basics of entities, living
    entities, mobs, items, experience orbs, arrows and primed TNT (location, teleporting,
    velocity, names, health, damage, equipment, targets, ...);
  * offline players, from the player data files and the name cache (`usercache.json`);
  * scoreboards: the main scoreboard and new ones, objectives, display slots, scores
    (`health`, `food`, `air`, `armor`, `level` and `xp` follow the players), teams and
    `Player.setScoreboard`;
  * potion effects of players and mobs (`addPotionEffect`, `getActivePotionEffects`, ...);
  * attributes of players and mobs (`getAttribute`, base values, modifiers,
    `registerAttribute`, `getMaxHealth`/`setMaxHealth`);
  * resource packs (Bukkit's `setResourcePack`/`addResourcePack` and Adventure's
    `sendResourcePacks`, removing them, `getResourcePackStatus` and
    `PlayerResourcePackStatusEvent`) and `openBook`;
  * boss bars, Adventure's (`showBossBar`) and Bukkit's (`Server.createBossBar`, keyed ones
    included);
  * block data: `Bukkit.createBlockData`, `Material`/`BlockType.createBlockData`,
    `Block`/`World.getBlockData` and `setBlockData` (with or without physics), every block's
    data interface (`Stairs`, `Ageable`, `Directional`, `Door`, `Wall`, ...), block states as
    snapshots that `update()` writes back, and `Player.sendBlockChange`. In
    `BlockPlaceEvent` the placed block reads as the new block and the replaced state as the old
    one, and a plugin's change to the placed block stays;
  * villagers, wandering traders and merchants: professions, types, levels, experience,
    restocks, reputations, trades as `MerchantRecipe`s (`getRecipes`, `setRecipes`, ...),
    `increaseLevel`, `addTrades`, `shakeHead`, `restock`, the despawn delay of wandering
    traders, merchants of plugins (`Bukkit.createMerchant`) and trading windows
    (`HumanEntity.openMerchant`, `MerchantInventory`, `MerchantView`), with
    `PlayerTradeEvent`, `PlayerPurchaseEvent`, `TradeSelectEvent`,
    `VillagerAcquireTradeEvent`, `VillagerCareerChangeEvent` and
    `VillagerReplenishTradeEvent`;
  * `/reload`, which restarts the plugin host (see [the plugin bridge](docs/plugin-bridge.md#reload)).

Not yet:

* World generation: sculk sensors, shriekers and catalysts from world generation have no
  block entities on the server.
* The dragon fight: the dragon cannot be summoned again with four end crystals on the exit
  portal, its breath cannot be bottled, and its fireballs cannot be hit back.
* Gameplay: boats and minecarts cannot be ridden, there is no horse inventory window, donkeys,
  mules and llamas carry no chests, parrots do not ride on shoulders, cats do not sleep on
  beds or bring gifts, and wild wolves do not hunt. Villagers do not breed, sleep, gather at bells, farm or call iron golems, there are no
  zombie villagers to cure, and explorer maps are not offered (this server has no maps yet,
  so cartographers offer their other trades). Poplar saplings do not grow (how they pick their tree is not in the
  data). Dispensers do not place shulker boxes, build golems or withers, or launch
  fireworks, wind charges, boats or minecarts. Spawn eggs are not
  used on water (for fish and squid), and buckets do not catch fish.
* Plugin API: creating worlds, registry and tag lifecycle events,
  statistics, item attribute modifiers (`ItemMeta#getAttributeModifiers`), dialogs, signed chat messages, entity AI and passengers, the data component API
  for components without plain values, the remaining special item metas (bundles,
  crossbows, compasses, armour trims, block states of items, map views, spawn eggs, buckets of
  fish, ...), inventories of other block types and anvil and enchanting views, villager
  inventories, sleeping and zombifying villagers,
  stat-based scoreboard criteria, saving the main scoreboard (`data/scoreboard.dat`), the
  `/scoreboard` and `/team` commands, and vanilla-format item serialization
  (`ItemStack.serialize` uses Paper-RS's own form). Block data knows block properties only:
  its questions about block behaviour (sound groups, `isSupported`, `rotate`/`mirror`,
  ...) are not answered yet. The persistent data containers of
  entities other than players live only in the plugin host and are not saved with the entity.
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
| `crates/paper-worldgen` | vanilla world generation (terrain, biomes, carvers, features, structures, spawn search) |
| `crates/paper-server` | the server: networking, tick loop, players, worlds, storage, plugin bridge |
| `plugin-host/` | the plugin host (Java), plus two small test plugins |
| `data/26.3` | the version data tables, see its README |
| `tools/datagen` | the tools that extract those tables from the vanilla jar without running it |
| `docs/` | [architecture](docs/architecture.md) and the [plugin bridge](docs/plugin-bridge.md) |

The server is Rust only. The one Java part is the plugin host: Bukkit and Paper plugins are
Java bytecode written against Java interfaces, so they run in a JVM next to the server. The
host builds against the published Paper API and compiles copies of Paper's plugin-loading
classes (`plugin-host/host/src/lifted`, refreshed from a Paper checkout by
`plugin-host/host/tools/lift.py`). Upstream Paper's own Java sources are no longer in this
repository.

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
