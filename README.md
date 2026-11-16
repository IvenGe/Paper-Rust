Paper-RS
========

A rewrite of the [Paper](https://github.com/PaperMC/Paper) Minecraft server in Rust, for
Minecraft Java Edition 26.3 (protocol 777), that still runs unmodified Java plugins: both
classic `plugin.yml` (Bukkit/Spigot) plugins and `paper-plugin.yml` plugins with
bootstrappers and Brigadier commands.

The game server is Rust. Java plugins run in a separate JVM, the *plugin host*, which talks
to the server over a local socket ("separate mode"). Without plugins no JVM is started.
Plugins can also be written in Rust: they load into the server process from the same
`plugins` folder and use the same API (see [Rust plugins](docs/rust-plugins.md)).

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

### Rust plugins

A Rust plugin is a `cdylib` crate that depends on `crates/paper-plugin`, implements its
`Plugin` trait and exports itself with `export_plugin!`. Copy the built library
(`libmy_plugin.so`, `my_plugin.dll` or `libmy_plugin.dylib`) into `my-server/plugins/`.
Rust plugins need no JVM: the plugin host starts only if there are jars. An example:

```bash
cargo build --release -p paper-plugin --example hello_plugin
cp target/release/examples/libhello_plugin.so my-server/plugins/
```

[Rust plugins](docs/rust-plugins.md) covers events, commands, the API, threads and the
plugin interface.

Status
------

What works:

* **Networking**: server list ping, online (Mojang authentication, encryption) and offline
  login, compression, the configuration phase, keep-alive.
* **Older clients**: 26.2, 26.1, 1.8.x, 1.7.x, 1.6.2-1.6.4, 1.5.x and 1.4.6/1.4.7 clients
  join too. Their packets are
  translated on the network threads (`crates/paper-compat`, with ViaVersion, ViaBackwards and
  ViaRewind's mapping data and ViaLegacy's knowledge of the protocols from before Netty): ids
  of blocks, items, entities, sounds and particles, registries they lack, packets that
  changed. 1.8 clients get the world in legacy block and item ids (between y 0 and 255, the
  height they know), entities in their old kinds, chat as JSON text, and the translator does
  for them what 1.8 never did (the configuration phase, teleport and chunk confirmations).
  1.7 clients go through the 1.8 translator and one more step (no compression, the blocks,
  items and mobs 1.8 added shown as ones 1.7 has). So do clients from before Netty (1.4.6
  to 1.6.4), with a step of their own: their own login (a handshake, then an encryption
  exchange even offline), packets without lengths, UTF-16 text, and blocks, items and mobs
  that came later shown as ones they have (a hay bale as yellow wool on 1.5, a rabbit as a
  chicken). The server list shows these clients the server as
  compatible, old-style pings included. Not yet: villager trades and boss bars on 1.8 and
  older, windows of blocks they do not have (smithing tables, looms, hoppers on 1.4.6, ...),
  and clients from before Netty on an online-mode server (the session servers they signed
  in with are gone).
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
* **World borders**: each world has one, saved in its `level.dat`, sent to players, moving
  to a new size over time, with damage outside it; `/worldborder` with vanilla's messages
  and limits.
* **Block updates**: neighbour and shape updates (wire, fence, pane and stair connections,
  blocks that lose their support break, doors and beds break together), scheduled block and
  fluid ticks (saved with the chunk), water and lava (flowing, falling, infinite water,
  waterlogged blocks, obsidian, cobblestone, stone and basalt), redstone (wire, torches,
  levers, buttons, repeaters, comparators, observers, lamps, note blocks, redstone blocks,
  doors, trapdoors and fence gates, pistons and sticky pistons, tripwire hooks and string),
  and using levers, buttons, doors, repeaters and comparators by hand. Pistons push the
  entities in their way (slime blocks launch them, honey blocks carry what stands on them),
  and moving blocks can be stood on. String between two hooks sets them off while anything
  is in it, unless cut with shears. Sand, gravel, concrete powder, anvils and dragon eggs
  fall when nothing holds them (anvils hurt what they land on and wear down), and concrete
  powder hardens in water. Rails join, bend and climb towards the rails next to
  them (a signal switches a junction), pop off without a block below, and powered and
  activator rails pass power along up to eight rails; detector rails are pressed by
  minecarts. Dispensers shoot arrows, thrown items and fire charges,
  place and pick up water, lava and powder snow (fish come out of their buckets), light
  fires and TNT, use bone meal, shear sheep and full beehives, fill glass bottles with
  water or honey, make mud with water bottles, put armour, heads, elytra, saddles, horse
  and wolf armour and llama carpets on players and mobs in front, hatch spawn eggs and put
  boats on water;
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
  meal grows crops; seeds need farmland and jumping on it tramples it (bigger mobs too,
  with mob griefing).
* **Entities**: dropped items (pickup, merging, despawning), experience orbs, arrows, falling
  blocks and primed TNT, and mobs: animals wander, look at players and panic when hurt;
  zombies, spiders and other monsters chase and hit players; skeletons shoot arrows;
  creepers explode; undead burn in daylight; snow golems throw snowballs at monsters, leave
  snow where they walk and melt in the Nether, hot biomes, water and rain. Mobs use A* pathfinding and vanilla's walking
  physics, spawn naturally from the biomes' spawn lists (26.3 keeps them in the biomes'
  `natural_mob_spawns` attributes), each type by its own vanilla rules: animals on their own
  ground (rabbits on sand and snow, goats on stone, mooshrooms on mycelium, ...) and also when
  a chunk is generated, under the trees too; bats in dark caves, fish and squid near the
  surface of water, glow squid deep down, axolotls in lush caves, slimes in swamps and slime
  chunks, the nether's own mobs. Each spawns as the variant of its place, by the spawn
  conditions of the vanilla data pack: cold cows, pigs and chickens in snowy biomes and
  warm ones in deserts, savannas and jungles, wolves and frogs of their biomes, cats of any
  coat (the all-black one only under a full moon), white rabbits and snow foxes in snowy places and gold
  rabbits in deserts, llamas, parrots and axolotls of any colour; the young take a
  parent's (horses now and then neither parent's coat or markings, rarely a blue axolotl).
  Variants are shown to players and saved under vanilla's keys. They despawn far from
  players, and drop loot (vanilla loot tables) and experience.
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
* **Signs**: the editor opens when a sign is placed and when it is clicked (on the side the
  player faces); the text is kept and shown to everyone, dyes colour it, glow ink sacs make
  it glow (ink sacs undo that), and honeycomb waxes the sign against editing.
* **Crossbows**: held, a crossbow charges for 25 ticks (less with Quick Charge, with
  vanilla's sounds); released, it loads a rocket or arrow from either hand, else an arrow
  from the inventory (three with Multishot, one used up) and shows as loaded. The next use
  fires: arrows always critical, rockets flying straight, Multishot's three spread 10 degrees
  apart (only the used-up arrow can be picked up); the crossbow wears a point per arrow and
  three per rocket.
* **Shields**: a raised shield blocks after a quarter second, taking hits from within 90
  degrees of where the player looks whole (not damage that bypasses shields), with its
  sound and wear from 3 damage up; an axe blow disables it for five seconds. Others see it
  raised.
* **Elytra and fireworks**: a player in the air glides when wearing an elytra (anything with
  the `glider` component that its next wear would not break), shown to others; the elytra
  wears a point a second, and gliding stops on landing, riding or levitating. A firework
  rocket used while gliding is attached to the player, whose client boosts them; used on a
  block it flies up from there. Rockets burst at the end of their flight (early on what
  they hit), and those with explosions hurt the living things they can see within five
  blocks.
* **Fishing**: a fishing rod casts a bobber that hooks what it hits or bobs in water, where
  after vanilla's waits (shorter in rain and with Lure, longer under a roof) a fish is
  lured, swims up with its wake and bites. Reeling in then catches from the
  `gameplay/fishing` loot table (Luck of the Sea and luck turn junk into treasure, treasure
  only in open water, bamboo only in jungles) with 1-6 experience, pulls a hooked mob or
  item, or brings the bobber back; the rod wears as in vanilla.
* **Item frames and paintings**: item frames and glow item frames hang on any side of a
  block, paintings on walls, taking a random one of the largest variants that fit. Frames
  take an item, turn it an eighth at each click and give it back when hit; a hit, an arrow or
  an explosion breaks them, and so does losing the block behind (checked every 100 ticks, as
  on Paper), dropping them (not for creative players). They are saved in vanilla's format.
* **Riding and pets**: boats and rafts are put on water (by players and dispensers),
  boarded (two to a boat, one beside a chest), steered, float, and break into their item;
  chest boats' chests open with a sneaking click or the rider's inventory key, spill when
  the boat breaks, and are reached by hoppers. Saddles go on pigs, striders, horses and
  camels; players get on and steer (their client moves the mount; carrots and warped fungi on
  sticks steer and boost pigs and striders) and get off with the sneak key. Horses have
  random health, speed, jump and coat, buck their rider off until their temper (raised by
  feeding) lets them be tamed, wear horse armour and eat to heal and grow. Tamed ones open their inventory when
  their rider asks or a player sneaks and clicks them: saddle, armour (carpets for llamas)
  and the chests that donkeys, mules and llamas carry (llamas' by their strength), all saved
  with them and dropped when they die. Wolves (bones), cats (fish) and
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
  speeds, efficiency, haste, mining fatigue, water and air) and drop their loot; ores,
  spawners and sculk give experience when mined with the right tool, not with silk touch. Status
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
  lingering ones) with blaze powder, also fed by hoppers. Stonecutters cut by vanilla's
  stonecutting recipes (clients are sent the list to pick from); smithing tables upgrade
  to netherite (keeping enchantments and names) and put armour trims on in the material
  of the addition; grindstones take enchantments other than curses off and repair two
  items into one, giving experience back; looms add banner patterns (with or without a
  pattern item) in the dye's colour, up to six layers. Plugins see `PrepareResultEvent`,
  `PrepareSmithingEvent`, `PrepareGrindstoneEvent` and the stonecutter and loom select
  events, and open these windows (`openStonecutter`, `openSmithingTable`, ...).
* **Redstone with items and entities**: hoppers move items between containers (furnace
  sides and chest boats included), pick up dropped items and lock when powered; droppers and dispensers
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
  `entities/` region files, `level.dat`, game rules, player data (inventory, ender chest,
  experience, ...) and statistics (`stats/<uuid>.json`) in vanilla's layout, autosave,
  `/save-all`, saving on shutdown.
* **Players**: movement and seeing each other, chat, game modes, creative inventory,
  operators, whitelist and bans, the player list.
* **Statistics**: counted as players walk, swim, climb, fly, fall, ride, jump, mine, place,
  use, craft, pick up and drop items, fight, die, sleep, open containers, talk and trade with
  villagers, breed animals and enchant; shown on the statistics screen, and followed by
  scoreboard objectives with statistic criteria (`minecraft.mined:minecraft.stone`, ...).
* **Commands**: `help list me msg say gamemode defaultgamemode tp give clear setblock time
  weather difficulty gamerule summon kill effect seed locate setworldspawn scoreboard team trigger tps mspt
  kick ban pardon ban-ip pardon-ip banlist op deop whitelist save-all stop version plugins
  reload`. The main scoreboard (objectives, scores, display slots and teams) is saved with the
  main world as vanilla's `data/minecraft/scoreboard.dat`.
* **Rust plugins** (`crates/paper-plugin`): libraries loaded from the plugins folder or
  linked into the server, which hear events after Java plugins (and may change their
  outcome), add commands with aliases, operator levels and completions, run every tick and
  call the whole plugin API of the bridge; see [Rust plugins](docs/rust-plugins.md).
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
  `EntityDeathEvent`, `PlayerDeathEvent`, `PlayerRespawnEvent`, `CreatureSpawnEvent`,
  `PlayerStatisticIncrementEvent`, `SignChangeEvent`, `PlayerInteractEvent` (clicks on
  blocks and in the air, stepping on pressure plates and trampling farmland; a denied
  block or item is not used, as in CraftBukkit), `PlayerInteractAtEntityEvent` (which
  `PlayerInteractEntityEvent` listeners hear too: as in Paper, it is fired for every
  right-click on an entity or player),
  `EntityExplodeEvent` and `BlockExplodeEvent` (only the blocks plugins leave are
  broken), `PlayerToggleSneakEvent`, `PlayerToggleSprintEvent`, `PlayerToggleFlightEvent`,
  `PlayerItemHeldEvent`, `PlayerSwapHandItemsEvent`, `PlayerItemConsumeEvent`,
  `FoodLevelChangeEvent`, `PlayerBucketEmptyEvent` and `PlayerBucketFillEvent`,
  `WeatherChangeEvent` and `ThunderChangeEvent` (natural, `/weather`, plugins, sleeping)
  and `EntityTargetLivingEntityEvent` (a mob finding a player), `PlayerTeleportEvent` for
  the teleports the game makes too (`/tp`, ender pearls, chorus fruit, end gateways,
  getting out of bed, dismounting; plugins may stop them or send the player elsewhere, to
  another world as well) and `PlayerPortalEvent` (a trip through a nether or end portal
  stopped or sent to another world and place, as Multiverse-NetherPortals does),
  `BlockPistonExtendEvent` and `BlockPistonRetractEvent` (with the blocks a piston moves;
  as in Paper, a cancelled pull still lets a sticky piston retract), `BlockBurnEvent`,
  `BlockIgniteEvent` with `BlockSpreadEvent` for fire spreading, and `BlockIgniteEvent` for
  fires players light with flint and steel or fire charges (campfires and candles too) and
  fires lightning, lava, small fireballs, dispensers and explosions start,
  `PlayerLoginEvent` (with the server's own verdict on bans, the whitelist and a full server
  for plugins to change; the player then joins as the same object, as in Paper),
  `PlayerChangedWorldEvent`, `ChunkLoadEvent` and `ChunkUnloadEvent` (a chunk's changes are
  saved even if a listener asks not to), `AsyncTabCompleteEvent` (off the main thread)
  and `TabCompleteEvent` for command completions, `BlockGrowEvent` (crops, stems and their
  fruit, sugar cane, cactus, sweet berries, nether wart, cocoa), `BlockFadeEvent` (ice and
  snow melting; grass, mycelium and farmland turning to dirt; fire going out),
  `BlockFormEvent` (lava hardening beside water, ice and snow forming in cold weather),
  `BlockSpreadEvent` for grass and mycelium, `LeavesDecayEvent` and `MoistureChangeEvent`
  (a listener may change the new state where Paper lets it), `BlockFromToEvent` for water
  and lava flowing (as in Paper, a stopped flow down stops the spreading to the sides too,
  and the fluid tries again when something next to it changes), and `EntityChangeBlockEvent`
  (endermen taking and placing blocks, blocks starting to fall and landing, sheep eating
  grass, players trampling farmland), `BlockDamageEvent` (a survival player starting to
  break a block; plugins may stop it or make it break at once), `PlayerKickEvent` for every
  kick (`/kick`, bans, timeouts, duplicate logins, plugins), `EntityRegainHealthEvent`
  (natural regeneration, regeneration and instant health) and `EntityCombustEvent` with its
  `ByEntity` and `ByBlock` forms (fire aspect, burning zombies, lightning, small fireballs,
  and players catching fire in lava or fire), `ProjectileLaunchEvent` (thrown items, arrows
  and fireballs; a stopped throw keeps the item in hand), `ProjectileHitEvent` (a stopped hit
  lets the projectile fly on, as in Paper), `BlockDispenseEvent` (plugins may stop a
  dispenser or dropper, or send another item), `StructureGrowEvent` (trees and huge
  mushrooms growing on their own or from bone meal, with all the blocks they change, which
  plugins may edit), `BlockFertilizeEvent` (bone meal from players and dispensers; a
  stopped one keeps the bone meal, as in Paper), `PortalCreateEvent` (fire lighting a frame,
  which stays lit if stopped; the portal a trip builds at the other end, whose trip ends there
  if stopped; the End platform), `PotionSplashEvent` with Paper's `WaterBottleSplashEvent`
  (fired for every thrown potion; water puts out fires where it breaks, too),
  `LingeringPotionSplashEvent`, `AreaEffectCloudApplyEvent`, `PlayerItemDamageEvent` and
  `PlayerItemBreakEvent` (players' tools, weapons and armour wearing), `PlayerShearEntityEvent`
  and `BlockShearEntityEvent` (sheep; plugins may change what drops), `PlayerBedEnterEvent`
  (with Paper's enter action; plugins may put a player to bed all the same) and
  `PlayerBedLeaveEvent`, `PlayerChangedMainHandEvent`, `PlayerLocaleChangeEvent`,
  `PlayerPickupArrowEvent`, `PlayerEggThrowEvent` and Paper's `ThrownEggHatchEvent`,
  `ExpBottleEvent`, `ItemDespawnEvent` (a kept item starts its five minutes again),
  `ItemMergeEvent`, `TNTPrimeEvent` (redstone, fire, explosions, players and dispensers setting
  TNT off), `ExplosionPrimeEvent` (primed TNT, creepers, ghast fireballs and end crystals,
  whose blasts plugins may change), `LightningStrikeEvent`, `CreeperPowerEvent` (lightning
  charges creepers now, which blow up twice as hard, and plugins may set the charge),
  `PigZapEvent` and Paper's `EntityZapEvent` with `EntityTransformEvent` (lightning turns pigs
  into zombified piglins and villagers into witches now), and `EntityCombustEvent` for mobs
  burning in daylight and lava. TNT and fireballs are the entities of their
  `EntityExplodeEvent`, as in Paper. Vehicles: `VehicleEnterEvent` and `VehicleExitEvent`
  (boats, horses, pigs, striders and the other Bukkit vehicles, before `EntityMountEvent` and
  `EntityDismountEvent`), and for boats `VehicleDamageEvent`, `VehicleDestroyEvent` (a boat
  kept whole stays at its most damaged), `VehicleCreateEvent`, `EntityPlaceEvent` and
  `VehicleMoveEvent`. Also `EntityPotionEffectEvent` (effects from food, milk, potions,
  splashes, clouds, mobs, trades, commands, plugins, death and running out),
  `EntityShootBowEvent` (players and skeletons, before the arrow's `ProjectileLaunchEvent`),
  `ItemSpawnEvent`, `SpawnerSpawnEvent`, Paper's `PlayerOpenSignEvent` and Bukkit's
  `PlayerSignOpenEvent`, `UnknownCommandEvent`, `PlayerClientLoadedWorldEvent`, and Paper's
  `BlockPreDispenseEvent` and `BlockFailedDispenseEvent`. And `InventoryMoveItemEvent`
  (hoppers pushing and pulling, droppers filling containers), `InventoryPickupItemEvent`,
  `EntityInteractEvent` (mobs trampling farmland, which they now do, and entities on pressure
  plates and string), `ChunkPopulateEvent` (plugins' block populators run on new chunks
  first), Paper's `PreCreatureSpawnEvent` (natural spawning and spawners), plain
  `EntitySpawnEvent` (primed TNT, falling blocks, experience orbs, clouds, eyes of ender) and
  `BlockMultiPlaceEvent` (doors, beds and tall plants). Then Bukkit's `EntityKnockbackEvent`
  and Paper's after it (hits, sweeps, hurts and explosions; `...ByEntityEvent` when something
  pushed), and `EntityBlockFormEvent` for snow golems' snow (a snow golem's melting is a
  `FIRE_TICK` cause, not Paper's `MELTING`). Paper's world border events
  (`WorldBorderBoundsChangeEvent`, `WorldBorderCenterChangeEvent` and
  `WorldBorderBoundsChangeFinishEvent`) come from `/worldborder` and plugins' changes alike, as
  do `HangingPlaceEvent`, `HangingBreakEvent` (`...ByEntityEvent` for hits and explosions),
  Paper's `EntityBreakEvent` after it, Paper's `PlayerItemFrameChangeEvent` for item frames,
  `PlayerFishEvent` in all its states (a plugin may change the catch and its experience),
  `EntityToggleGlideEvent`, Paper's `PlayerElytraBoostEvent` (which may keep the rocket in
  hand), `FireworkExplodeEvent` (a cancelled one keeps flying, as on Paper), Paper's
  `EntityLoadCrossbowEvent` (crossbow shots are `EntityShootBowEvent`s) and Paper's
  `PlayerShieldDisableEvent` (a fully blocked hit fires no damage event, where Paper fires
  one with a blocking modifier). Players,
  worlds and blocks can be read and changed through the API (messages, titles, sounds,
  teleporting, health, flight, blocks, time, spawn, ...), and so can:
  * item stacks and metas (names, lore, enchantments, damage, unbreakable, custom model data,
    item flags, the persistent data container, attribute modifiers; other components are kept
    intact), the
    special metas of heads (`SkullMeta`), potions and tipped arrows (`PotionMeta`, and
    `PotionType` data), suspicious stew, fireworks and firework stars, banners, written and
    writable books, block data (`BlockDataMeta`), maps (id and colour), ominous bottles, lodestone compasses, buckets of tropical
    fish and axolotls, and
    inventories: the player inventory, ender chests, chests, barrels and furnaces (as block
    states), custom inventories from `Bukkit.createInventory` opened with
    `Player.openInventory`, open windows and the cursor;
  * entities: listing, spawning and dropping items, and the basics of entities, living
    entities, mobs, items, experience orbs, arrows and primed TNT (location, teleporting,
    velocity, names, health, damage, equipment, targets, ...), the owners (who alone may pick
    them up) and throwers of items, passengers and vehicles
    (`addPassenger`, `eject`, `leaveVehicle`, ...), tamed animals (`Tameable`, `Sittable`) and
    horses (domestication, jump strength), the flags of any entity (glowing, silent,
    invisible, invulnerable, visual fire, freezing, portal cooldown, scoreboard tags) and of
    mobs (`setAI`, `setAware`, left-handed), why a mob was spawned, ages, age locks and love
    mode (`Ageable`, `Breedable`, `Animals`), sheep (wool colour, `shear`), variants (of
    cats, chickens, cows, frogs, pigs, wolves, parrots, axolotls, mooshrooms, rabbits, foxes,
    llamas, horses' coats and markings) and collars, projectiles
    launched by players and mobs (`launchProjectile`: arrows, fireballs, snowballs, eggs,
    ender pearls, bottles o' enchanting, potions) or spawned and aimed by plugins
    (`setShooter`, `Fireball.setDirection`), and the
    persistent data containers of players and entities, saved with them as Bukkit's
    `BukkitValues`;
  * the server's settings as plugins read them from `Bukkit.spigot()` (`getConfig`,
    `getBukkitConfig`, `getPaperConfig`; proxy forwarding reads as off, as Paper-RS has
    none);
  * offline players, from the player data files and the name cache (`usercache.json`);
  * statistics of online and offline players (`getStatistic`, `setStatistic`,
    `incrementStatistic`, `decrementStatistic`, with CraftBukkit's argument checks);
  * scoreboards: the main scoreboard and new ones, objectives, display slots, scores
    (`health`, `food`, `air`, `armor`, `level` and `xp` follow the players), teams and
    `Player.setScoreboard`;
  * potion effects of players and mobs (`addPotionEffect`, `getActivePotionEffects`, ...),
    and what effect types are (their category, whether instant, Bukkit's old names);
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
  * worlds made by plugins (`Bukkit.createWorld`): normal or flat; overworld, Nether or End;
    opened again from their folders; unloaded again (`Bukkit.unloadWorld`, with
    `WorldUnloadEvent`); and players teleported from one world to another;
  * chunks' persistent data containers (saved with the chunks, under `ChunkBukkitValues` as in
    Paper) and chunk snapshots (`Chunk#getChunkSnapshot`);
  * player state: walking and flying speeds, the time and weather one player sees
    (`setPlayerTime`, `setPlayerWeather`), being invulnerable, glowing, silent, invisible or
    without gravity, custom names, scoreboard tags, air, fire, freezing and fall distance,
    experience (`setLevel`, `setExp`, `giveExpLevels`, ...), the compass target, the respawn
    point, the health scale, item cooldowns, ignoring sleep, velocity, hiding players and
    entities from a player (`hidePlayer`, `hideEntity`), Bukkit's title, sound and tab-list
    methods, what a player looks at (`getTargetBlock`, `getLineOfSight`), client options, and
    packets for one client (`sendExperienceChange`, `sendBlockDamage`, `sendEquipmentChange`,
    `sendHurtAnimation`, the demo and credits screens, `transfer`);
  * firework rockets (`Firework`: item and meta, which sets the flight, ticks flown and to
    fly, what it is attached to, shot at an angle, detonating), `isGliding`/`setGliding` and
    `HumanEntity#isBlocking`;
  * fishing bobbers (`FishHook`: waits, lure times and angles, Lure, rain and sky, open
    water, what it hooked and pulling it, its state);
  * item frames and paintings (`ItemFrame`: item, rotation, fixed, visible, drop chance;
    `Painting#setArt`, which like `Hanging#setFacingDirection` only takes where it then
    fits unless forced; `Art`'s sizes; spawned against a neighbouring block as on Paper);
  * signs as block states (`Sign`, `SignSide`: both sides' lines, colour and glow, waxing),
    and `SignChangeEvent` for what players write;
  * world borders (`World.getWorldBorder`: size, moving to a size over time, center, damage,
    warnings), and borders of one player's own (`Server.createWorldBorder`,
    `Player.setWorldBorder`: only that player's client sees it, moving with the ticks while
    they have it, as on Paper; Paper-RS keeps showing it after a respawn or a change of
    world, where Paper shows the world's again) and weather (`setStorm`, `setThundering` and their durations, with
    `WeatherChangeEvent` and `ThunderChangeEvent`);
  * chunks: `getChunkAt` loads or generates the chunk on the spot as on Paper, loaded and
    generated chunks, force-loaded chunks, plugin chunk tickets, a chunk's entities and block
    entities, slime chunks;
  * game rules (`getGameRuleValue`, `setGameRule`, by name too, the Bukkit constants of rules
    vanilla replaced, `WorldGameRuleChangeEvent`), a world's own plugin data (saved as Paper
    saves it, `data/paper/persistent_data_container.dat`), metadata on worlds, blocks and
    block states, `World#save`, the world folder, plugin messages to a world's players;
  * blocks: `breakNaturally` (drops, experience and the break effect, as with the given
    tool), `getDrops`, collision, how pistons move them, legacy data values; block entities as
    block states written back by `update()`: custom names (`Nameable`), plugin data
    (`TileState`), banner patterns, a spawner's mob and timings;
  * recipes: plugins add shaped, shapeless, transmute and cooking recipes (`addRecipe`), with
    item types or exact items as ingredients (`ExactChoice`: the same item with the same
    components), which the server crafts and smelts before vanilla's; recipes are listed
    (`recipeIterator`, `getRecipe`, `getRecipesFor`, vanilla's too), removed, cleared and
    reset; `CraftItemEvent#getRecipe` is the recipe crafted (a plugin's own object). Not yet:
    stonecutting and smithing recipes, `PrepareItemCraftEvent`, the recipe book;
  * Paper's data component builders for custom model data and lore
    (`CustomModelData.customModelData()`, `ItemLore.lore()`, with `ItemStack#setData`); a world's
    dimension (`getCoordinateScale`, `hasCeiling`, `isUltraWarm`, `isBedWorks`, ...), the
    difficulty (shared by all worlds), sounds at a place (`World#playSound`), spawn limits and
    flags, entity counts;
  * particles (`World#spawnParticle` for the players near, `Player#spawnParticle` for one,
    with their options: dust colours, block and item particles, trails, vibrations, spells...),
    level effects (`World#playEffect`), explosions (`createExplosion`, breaking blocks and
    starting fires if asked) and lightning (`strikeLightning`, `strikeLightningEffect`);
  * Folia's entity scheduler (`Entity.getScheduler`) and region checks, `matchPlayer`, player
    profiles, server-wide plugin messages and the server's settings;
  * WorldEdit: it only works through an adapter for the server version, and the ones it ships
    use Minecraft's internal classes, so the plugin host gives it its own
    (`plugin-host/host/src/main/java/io/papermc/paperrs/host/compat`), defined in WorldEdit's
    class loader when WorldEdit is enabled. Selections, `//set`, `//replace`, `//copy`,
    `//paste`, shapes and `//undo` work, block entities included; regeneration, trees and
    features from WorldEdit are not supported;
  * Paper's start-up order: STARTUP plugins are enabled before the worlds load, then each
    world loads with `WorldInitEvent` and `WorldLoadEvent`, then the POSTWORLD plugins.
    Libraries bundled in plugins recognise the server as Paper (PaperLib), and reading or
    changing a block of an unloaded chunk loads it first, as on Paper. A plugin whose
    lifecycle handler fails (often one reaching for Minecraft's own classes) is logged, and
    the other plugins still start;
  * what Paper's server offers plugins besides the API: Log4j with log4j-core (plugins add
    console filters and appenders; it all ends up in the server's log), the SQLite and MySQL
    JDBC drivers, Commons Lang 2, json-simple; and the field permission plugins swap their own
    permissions into (`CraftHumanEntity.perm`, for LuckPerms). In a test with 32 popular
    plugins loaded together, 25 enabled (EssentialsX, LuckPerms, Vault, WorldEdit, WorldGuard,
    Multiverse, GriefPrevention, BlueMap, DiscordSRV, TAB, PlaceholderAPI, Plan, DeluxeMenus,
    LifeStealZ, Chunky, ...); the others use Minecraft's internal classes (dynmap, squaremap,
    InvSee++, DecentHolograms, Simple Voice Chat, LagFixer) or refuse the version
    (CoreProtect 24.1). With a player clicking, WorldEdit's wand selects and `//set` fills
    the selection, and GriefPrevention's golden shovel marks the corners of claims;
  * `/reload`, which restarts the plugin host (see [the plugin bridge](docs/plugin-bridge.md#reload));
  * Paper's class rewriting as plugins load (Commodore): fields and enum constants renamed
    since a plugin's api-version, enums that became registries, and `Material.values()`
    without the legacy materials; and the legacy materials themselves (`LEGACY_` constants,
    `UnsafeValues.fromLegacy`/`toLegacy`), converted with a table of the pre-1.13 ids.

Not yet:

* World generation: sculk sensors, shriekers and catalysts from world generation have no
  block entities on the server.
* The dragon fight: the dragon cannot be summoned again with four end crystals on the exit
  portal.
* Gameplay: minecarts cannot be ridden, parrots do not ride on shoulders, cats do not sleep on
  beds or bring gifts, and wild wolves do not hunt. Villagers do not breed, sleep, gather at bells, farm or call iron golems, there are no
  zombie villagers to cure, and explorer maps are not offered (this server has no maps yet,
  so cartographers offer their other trades). Poplar saplings do not grow (how they pick their tree is not in the
  data). Withers and copper golems cannot be built. Dispensers do not place shulker boxes, build withers, or launch
  fireworks, wind charges or minecarts.
* Statistics of what the server does not do yet (raids, note blocks, bells,
  cauldrons, cakes, flower pots, ...) stay at zero, and items count as
  used or broken only when placed, eaten or drunk, thrown, used as spawn eggs, buckets or
  bottles, or worn out as tools.
* Plugin API: plugins' own chunk generators and biome providers (block populators get the
  classic `populate(World, Random, Chunk)` call only), amplified and large-biome
  worlds, registry and tag lifecycle events,
  dialogs, signed chat messages, entity AI goals, the data component API
  for components without plain values, the remaining special item metas (bundles,
  crossbows, armour trims, block states of items, map views, spawn eggs, ...), inventories of other block types and anvil and enchanting views, villager
  inventories, sleeping and zombifying villagers, and vanilla-format item serialization
  (`ItemStack.serialize` uses Paper-RS's own form). Block data knows block properties only:
  its questions about block behaviour (sound groups, `isSupported`, `rotate`/`mirror`,
  ...) are not answered yet.
  Chunk snapshots answer the biomes' own temperatures, without vanilla's adjustment for
  height.
  Events not fired yet include mobs catching fire in fire blocks, `VehicleUpdateEvent`,
  vehicle collisions, and minecarts' own events (minecarts do nothing yet).
  Methods that are not implemented throw an `UnsupportedOperationException` that names
  them. Plugins without an api-version (written for 1.12 or older) are loaded without
  Paper's legacy conversion.

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
| `crates/paper-plugin` | writing plugins in Rust: the plugin trait, events, commands, the server API |
| `crates/paper-compat` | translating packets for clients of older versions |
| `plugin-host/` | the plugin host (Java), plus two small test plugins |
| `data/26.3` | the version data tables, see its README |
| `data/compat` | what older versions differ in (packet orders, id mappings), see its README |
| `tools/datagen` | the tools that extract those tables from the vanilla jar without running it |
| `docs/` | [architecture](docs/architecture.md), the [plugin bridge](docs/plugin-bridge.md) and [Rust plugins](docs/rust-plugins.md) |

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
skips itself. `crates/paper-server/tests/third_party_plugins.rs` runs EssentialsX and
WorldEdit the same way when `PAPER_RS_TEST_PLUGINS` names a folder with their jars (they are
not part of this repository).

License
-------

Like Paper, this project is licensed under the GPLv3; see [LICENSE.md](LICENSE.md).
