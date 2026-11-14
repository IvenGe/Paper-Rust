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
   `WELCOME`, which describes the server (name, versions, properties, worlds, the vanilla
   enchantment definitions as `enchantments`, and, after `/reload`, `reload: true` and the
   online players as `players`);
4. calls `host.load`, then `host.post_world`.

The host's standard output becomes the server's log: the host writes one `LEVEL|message`
line per log line, and anything else (a plugin printing to `System.out`) is logged as is.

If the host dies, the server stops (the default, since protection plugins would be gone
silently) or, with `on-host-crash = "continue"`, keeps running without plugins.

## `/reload`

Paper reloads plugins inside its JVM. Paper-RS restarts the plugin host instead:
`/reload confirm` (or `Bukkit.reload()`, which runs it) sends `host.shutdown` (plugins see
`onDisable`), stops the process and starts a new one with the same launcher. Its WELCOME
carries `reload: true` and the online players, so the host knows them before any plugin is
constructed: plugins see the players from `onLoad`/`onEnable` on, no `PlayerJoinEvent` is
fired, and `ServerLoadEvent` reports `LoadType.RELOAD`. Nothing of the old JVM survives
(static fields, scheduled tasks, metadata, objects plugins kept). What the server owns stays:
the main scoreboard, worlds, players and their inventories. Windows showing plugin
inventories are closed, plugin inventories and scoreboards made with `getNewScoreboard()` are
dropped, and players go back to the main scoreboard. The reload runs after the command that
asked for it has returned, so a plugin can trigger it from its own code. Notifications still
arriving from the old host are ignored.

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
strings, worlds by name, other entities by entity id.

### Item stacks

An item stack is `null` (empty) or a map: `item` (`minecraft:diamond_sword`), `count`, `raw`
(every added data component as `[component type id, network bytes]`), `removed` (ids of
default components the stack removes), and the text components the server can translate also
as JSON: `name` (custom name), `item_name`, `lore` (a list), and `book` for a written book's
content (`title`, `author`, `generation`, `pages` as JSON, `resolved`). When the host sends a
stack back, these replace the raw bytes of the same component unless they encode the same
text, so a stack that went through the host unchanged keeps its exact bytes and still stacks
with its twins. The host decodes the other components it understands from the bytes
(enchantments, damage, unbreakable, custom model data, tooltip display, custom data and the
persistent data container in it, profiles, potion contents, fireworks, banner patterns, block
states, writable book pages, ...; see `MetaCodecs`) and passes unknown ones through untouched.

### Inventories

An inventory is named by a descriptor map with a `kind`: `player` (`uuid`; Bukkit's slot
order: 0-8 hotbar, 9-35 main, 36-39 boots to helmet, 40 off hand), `ender` (`uuid`), `crafting`
(`uuid`: the result and grid of the player's crafting table window, or of their 2x2 grid),
`block` (`world`, `x`, `y`, `z`, and `positions` for both halves of a double chest) or `custom`
(`id`, an inventory made with `inventory.create`). The contents always live on the server.

A window is described by `type` (Bukkit `InventoryType`), `size`, `title`, `top` (the
descriptor of its top inventory), `window_id`, and for blocks also `world`, `x`, `y`, `z`.

## Calls from the server to the host

| Method | Arguments | Answer |
| --- | --- | --- |
| `host.load` | | `plugins`, `commands`, `events` (see below). Scans `plugins/`, runs bootstrappers and the bootstrap `COMMANDS` event, creates the server object, constructs plugins (`onLoad`), enables `STARTUP` plugins |
| `host.post_world` | | the same. Enables `POSTWORLD` plugins, fires `COMMANDS` for plugins and `ServerLoadEvent` |
| `host.tick` | `tick` | runs the Bukkit scheduler's main-thread tasks |
| `host.shutdown` | | disables all plugins, then the host exits |
| `event.join` | `player` (state), `inventory`, `message` | `message` (`PlayerJoinEvent`) |
| `event.quit` | `uuid`, `reason`, `message` | `message` (`PlayerQuitEvent`) |
| `event.inventory_open` | `uuid`, `inventory` (window) | `cancelled` (`InventoryOpenEvent`) |
| `event.inventory_click` | `uuid`, `inventory` (window), `slot`, `button`, `mode`, `cursor`, `held_slot`; `craft` (the result) for a crafting result; `drag_slots`, `drag_kind` when a drag ends | `cancelled` (`InventoryClickEvent`, `CraftItemEvent`, `InventoryDragEvent`); the click then works on the slots as listeners left them |
| `event.inventory_close` | `uuid`, `inventory` (window), `reason` (`PLAYER`, `OPEN_NEW`, `PLUGIN`, `DISCONNECT`, ...) | (`InventoryCloseEvent`; a window a listener opens from it stays open, and the event does not fire again for that) |
| `event.pickup_item` | `uuid`, `item_entity`, `item`, `remaining` | `cancelled` (`PlayerAttemptPickupItemEvent`, `PlayerPickupItemEvent`, `EntityPickupItemEvent`) |
| `event.drop_item` | `uuid`, `item_entity` (already spawned), `item` | `cancelled` (`PlayerDropItemEvent`; the item goes back to the player) |
| `event.entity_damage` | `target`, `cause` (damage type), `amount`, `attacker`, `direct` (the projectile), `source_position`; also `attacker_player` / `attacker_entity` | `cancelled`, `amount` (`EntityDamageEvent`, `EntityDamageByEntityEvent`, `EntityDamageByBlockEvent`) |
| `event.entity_death` | `entity`, `type`, `value` (state), `cause`, `killer`, `drops`, `xp` | `cancelled`, `drops`, `xp` (`EntityDeathEvent`) |
| `event.player_death` | `uuid`, `message`, `cause`, `killer`, `drops`, `xp`, `keep_inventory`, `keep_level` | `cancelled`, `message` (null: none), `drops`, `xp`, `keep_inventory`, `keep_level` (`PlayerDeathEvent`) |
| `event.respawn` | `uuid`, `world`, `x`, `y`, `z`, `yaw`, `pitch` | the location (`PlayerRespawnEvent`) |
| `event.creature_spawn` | `entity`, `type`, `value` (state), `world`, `x`, `y`, `z`, `reason` | `cancelled` (`CreatureSpawnEvent`; the mob is removed again) |
| `event.trade` | `uuid`, `merchant` (`{entity, value}` or `{merchant}`), `recipe`, `reward_exp`, `increase_uses` | `cancelled`, `recipe`, `reward_exp`, `increase_uses` (`PlayerTradeEvent` for villagers and wandering traders, `PlayerPurchaseEvent` for plugin merchants; a changed recipe stays on the offer) |
| `event.trade_select` | `uuid`, `index` | `cancelled` (`TradeSelectEvent`) |
| `event.villager_acquire_trade` / `event.villager_replenish_trade` | `entity`, `value` (state), `recipe` | `cancelled`, `recipe` (`VillagerAcquireTradeEvent`, `VillagerReplenishTradeEvent`; one per trade) |
| `event.villager_career_change` | `entity`, `value`, `profession`, `reason` (`EMPLOYED`, `LOSING_JOB`) | `cancelled`, `profession` (`VillagerCareerChangeEvent`) |
| `event.mount` / `event.dismount` | `entity` (the rider), `mount` / `dismounted` (the vehicle) | `cancelled` (`EntityMountEvent`, `EntityDismountEvent`) |
| `event.tame` | `entity` (the animal), `owner` (the player) | `cancelled` (`EntityTameEvent`) |
| `event.statistic_increment` | `uuid`, `type` (stat type), `key`, `old`, `new` | `cancelled` (`PlayerStatisticIncrementEvent`; as in CraftBukkit, not fired for statistics counted every tick or by the centimetre) |
| `event.command_preprocess` | `uuid`, `message` (with `/`) | `cancelled`, `message` |
| `event.server_command` | `command` | `cancelled`, `command` |
| `event.block_break` | `uuid`, `world`, `x`, `y`, `z`, `state` | `cancelled` |
| `event.resource_pack_status` | `uuid`, `id` (the pack), `status` (0 loaded ... 7 discarded) | nothing |
| `event.block_place` | the same, `replaced`, `against` `[x, y, z]`, `offhand` | `cancelled`; the server places the block after the event, unless a listener has set that block itself |
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
change, chat, and the inventory, item, damage, death, respawn and spawn events. Join, quit,
the ping and pre-login are always fired. Entities in events are `{player: uuid}` or
`{entity: id, value: <state>}` (see `entity.state`).

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
| `player.block_change` | `uuid`, `x`, `y`, `z`, `state` (a block only this player sees) |
| `player.resource_pack_push` / `player.resource_pack_pop` | `uuid`, `id` (the pack), `url`, `hash`, `forced`, `prompt` (JSON) / `uuid`, and `id` or nothing for all |
| `player.open_book` | `uuid`, `item` (a written book) |
| `player.statistic` / `player.set_statistic` | `uuid` (online or offline), `type` (stat type, `minecraft:custom` by default), `key` / and `value` |
| `player.stop_sound` | `uuid`, and optionally `sound` and `category` (none: all sounds) |
| `player.inventory` / `player.set_slot` / `player.set_held_slot` | `uuid` / `slot`, `item` (window slot numbers) / `slot` |
| `player.open_inventory` | `uuid`, `inventory` (descriptor), `title` (answers `opened`, `window`) |
| `player.close_inventory` | `uuid`, `reason` |
| `player.cursor` / `player.set_cursor` | `uuid` / `item` |
| `player.set_scoreboard` | `uuid`, `board` |
| `player.give_exp` | `uuid`, `amount` |
| `inventory.contents` | `inventory` (answers `items`) |
| `inventory.set` | `inventory`, `slots` (`[[index, item], ...]`) or `items` (all) |
| `inventory.create` | `type`, `size`, `title` (answers `id`) |
| `inventory.release` | `id` (the host dropped its object; the server forgets it once nobody views it) |
| `inventory.viewers` | `inventory` (answers player UUIDs) |
| `inventory.of_block` | `world`, `x`, `y`, `z` (answers `type`, `size`, `inventory`, or null) |
| `world.entities` | `world`, `types`, `min`/`max` (`{x, y, z}`) (answers entity states) |
| `world.spawn_entity` | `world`, `type`, `x`, `y`, `z`, `yaw`, `pitch`, `reason` (answers the state) |
| `world.drop_item` | `world`, `x`, `y`, `z`, `item`, `natural` (answers the state) |
| `entity.state` | `id` (answers `id`, `uuid`, `type`, `world`, position, rotation, velocity, `custom_name`, `living`, health, `equipment`, `target`, `item`, `fuse`, `passengers`, `vehicle`, `tamed`, `owner`, `sitting`, `temper`, `jump_strength`, ... or null once gone) |
| `entity.set` | `id` and any of `custom_name`, `clear_custom_name`, `custom_name_visible`, `gravity`, `vx`/`vy`/`vz`, `health`, `max_health`, `fire_ticks`, `air`, `no_damage_ticks`, `persistent`, `baby`, `equipment` (`[[slot 0-5, item]]`), `target`, `clear_target`, `item`, `pickup_delay`, `damage`, `pickup`, `critical`, `experience`, `fuse`, `tamed`, `owner` (a UUID, empty for none), `sitting`, `temper`, `jump_strength` (answers the state) |
| `entity.add_passenger` / `entity.remove_passenger` / `entity.eject` | `id`, `passenger` (`{player}`/`{entity}`) / `id`, `passenger` / `id` (answers whether anything changed) |
| `entity.leave_vehicle` | `rider` (`{player}`/`{entity}`) (answers whether it was riding) |
| `entity.pdc` / `entity.set_pdc` | `target` (`{player}`/`{entity}`) / `target`, `data` (network NBT bytes, empty to clear): a persistent data container, saved with the player or entity as `BukkitValues` |
| `entity.teleport` / `entity.remove` | `id`, `world`, `x`, `y`, `z`, `yaw`, `pitch` / `id` |
| `entity.damage` | `id` (also of a player), `amount`, `source` (`{player}`/`{entity}`) |
| `entity.add_effect` | `id` (also of a player), `effect`, `duration` (-1 infinite), `amplifier`, `ambient`, `particles`, `icon` (answers whether it took) |
| `entity.remove_effect` / `entity.clear_effects` / `entity.effects` | `id`, `effect` / `id` (answers how many) / `id` (answers `[{effect, duration, amplifier, ambient, particles, icon}]`) |
| `bossbar.update` | `id` (a UUID the host picks), and any of `title`, `progress`, `color` (`pink`...`white`), `overlay` (`progress`, `notched_6`...), `flags` (1 darken, 2 music, 4 fog); creates the bar |
| `bossbar.show` / `bossbar.hide` / `bossbar.remove` | `id`, `player` / `id`, `player` / `id` |
| `attribute.get` | `id` (entity id of a player or mob), `attribute`; `{base, value, default, modifiers: [{id, amount, operation}]}` or null if the entity does not have it |
| `attribute.set_base` / `attribute.add_modifier` / `attribute.remove_modifier` / `attribute.register` | `id`, `attribute` / and `base` / `modifier` (id), `amount`, `operation` (0 add, 1 multiply base, 2 multiply total), `persistent` / `modifier` / nothing |
| `attribute.default` | `attribute`; its default value |
| `merchant.recipes` / `merchant.set_recipes` / `merchant.set_recipe` | `entity` (a villager or wandering trader) or `merchant` (a plugin's), and `recipes` / `index`, `recipe`; a recipe is `{result, ingredients, uses, max_uses, reward_exp, villager_xp, price_multiplier, demand, special_price, ignore_discounts}` |
| `merchant.create` / `merchant.release` | `title` (JSON; none says "Villager"); answers `id` / `merchant` (forgotten once nobody trades with it) |
| `merchant.trader` / `merchant.open` / `merchant.window` | the merchant / the merchant, `uuid`, `force` (answers `opened`) / `uuid`: the open trading window's `merchant`, `selected` and `recipe` |
| `villager.increase_level` / `villager.add_trades` | `entity`, `amount` (answers `changed`) |
| `villager.shake_head` / `villager.restock` / `villager.update_demand` / `villager.reset_offers` | `entity` |
| `villager.reputations` / `villager.set_reputation` / `villager.clear_reputations` | `entity` / and `uuid`, `values` (`{major_negative: n, ...}`) / |
| `scoreboard.new` / `scoreboard.release` / `scoreboard.retain` | / `board` / `board` |
| `scoreboard.objectives` / `scoreboard.objective` | `board` / `board`, `name` |
| `scoreboard.register_objective` / `unregister_objective` / `update_objective` | `board`, `name`, `criteria`, `display_name`, `render_type` |
| `scoreboard.set_display` | `board`, `slot` (0 list, 1 sidebar, 2 below name, 3-18 team sidebars), `objective` (none clears) |
| `scoreboard.score` / `set_score` / `reset_scores` | `board`, `objective`, `entry`, `value`, `display_name` |
| `scoreboard.entries` / `entry_scores` / `entry_team` | `board` / `board`, `entry` / `board`, `entry` |
| `scoreboard.teams` / `team` / `register_team` / `unregister_team` | `board`, `name` |
| `scoreboard.update_team` | `board`, `name` and any of `display_name`, `prefix`, `suffix`, `color` (0-15), `clear_color`, `friendly_fire`, `see_friendly_invisibles`, `name_tag_visibility`, `death_message_visibility`, `collision_rule` |
| `scoreboard.add_entries` / `remove_entries` | `board`, `team`, `entries` |
| `offline.get` | `uuid`, or `name` (and `cached_only`) (answers `uuid`, `name`, `online`, `played_before`, `first_played`, `last_played`, `last_login`, `last_seen`, `op`, `banned`, `whitelisted`, `world`, `position`) |
| `offline.list` | (answers the same for every known player) |
| `offline.set_op` / `set_whitelisted` / `ban` / `pardon` | `uuid`, `name`, `op` / `value` / `reason`, `source` / |
| `world.info` | `world` |
| `world.get_block` / `world.set_block` | `world`, `x`, `y`, `z` / `state` (`minecraft:name[property=value,...]`), `physics` (default true) |
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
the scheduler), copied from a checkout of Paper's `paper-server` by
`plugin-host/host/tools/lift.py`. The classes
Paper implements on top of the Minecraft server are written for the bridge instead:
`CraftServer`, players, worlds and blocks, registries built from the server's data tables,
the command map and a Brigadier registrar for Paper plugins, and the events above. API
methods the host does not implement yet throw an `UnsupportedOperationException` naming the
method (`plugin-host/host/tools/StubGen.java` generates those defaults from the API).

Objects whose API interface depends on what they represent (entities: `Zombie`, `Item`,
`TNTPrimed`, ...; block states: `Chest`, `Furnace`, ...; item metas: `SkullMeta`,
`EnchantmentStorageMeta`, ...; inventories: `PlayerInventory`, `FurnaceInventory`, ...;
inventory views, equipment and scoreboards) are dynamic proxies (`HostProxies`): the proxy
implements the specific interface and forwards each call to an implementation class with a
public method of the same signature. Calls without one run the interface's default method or
throw the same named `UnsupportedOperationException`, so `instanceof` checks work and missing
features still fail clearly. `ProxySignatureTest` checks the implementation classes against
the API interfaces, since the compiler cannot.

Block data (`HostBlockData`) is a proxy too, of the interface Paper gives the block: the type
argument of its `BlockType.Typed` constant (`BlockType.Typed<Stairs> OAK_STAIRS`), the same
mapping `CraftBlockData` is generated from. Its getters and setters read and write the block's
properties by name (`getHoneyLevel` is `honey_level`, `getHeight(BlockFace)` is the property
named after the face), and `BlockDataTest` calls every one of them on every block.
