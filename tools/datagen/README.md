# datagen — version data tables for paper-rs

Regenerates the tables in `data/<version>/` that paper-rs embeds: packet ids,
built-in registry ids, block-state ids, per-state block physics, items, the
synchronized registries, tags, dimension types and default feature flags.

* Standard-library Python only (3.8+). No JDK, no third-party packages.
* **Nothing from Mojang's jar is executed.** The jar is opened as a zip, class
  files are decoded by `classfile.py`, and the interesting static initialisers
  are evaluated *symbolically* by `symexec.py` (see "How the bytecode is
  analysed"). No class is ever loaded into a JVM and no library code is present.
* Every input is treated as untrusted data: run with `python3 -I` and pass paths
  as arguments.

## Regenerating

```sh
python3 -I tools/datagen/extract.py \
    --jar      /home/user/upstream/vanilla/jar/server.jar \
    --mcmeta   /home/user/upstream/mcmeta/summary \
    --pumpkin  /home/user/upstream/pumpkin/assets \
    --pumpkin-packets /home/user/upstream/refdata/pumpkin-packets.json \
    --wiki     /home/user/upstream/wiki \
    --burger   /home/user/upstream/burger/26.3.json \
    --minecraft-data /home/user/upstream/minecraft-data/package/minecraft-data/data/pc/26.1 \
    --double-check \
    --out      data/26.3

python3 -I tools/datagen/check.py data/26.3        # self-check of the outputs
```

About 12 s (25 s with `--double-check`). Re-runs are byte-identical.

Required: `--jar` (the bundler `server.jar` or the inner `server-<ver>.jar`),
`--mcmeta` (checkout of [misode/mcmeta](https://github.com/misode/mcmeta) at tag
`<ver>-summary`) and `--out`. Everything else is optional and only adds
cross-checks, except `--pumpkin`, which is also the data source for three
per-state properties (see `block_states.json` below).

Exit code: `0` all outputs written and every check passed; `1` outputs written
but at least one check disagreed (read the report); `2` something could not be
produced. The full check list is printed and saved to
`tools/datagen/reports/<version>-validation.json` (`--report` to change, `-` to skip).

Inputs used for the committed 26.3 tables:

| input | revision |
|---|---|
| `server.jar` 26.3 | sha1 `33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c` (matches Mojang's version manifest) |
| misode/mcmeta | tag `26.3-summary`, commit `d96c75fec200c4580dd75e033a76341521165461` |
| Pumpkin-MC/Pumpkin `assets/` | commit `e9dc12e9a137675002025be9efa5bf8a2be4bd1e` (2026-11-13) |

## Output files and where each value comes from

| file | content | source |
|---|---|---|
| `version.json` | id, protocol, data version, pack versions | `version.json` inside the jar |
| `packets.json` | per state / direction, array index = packet id | bytecode |
| `registries.json` | 95 built-in registries, array index = protocol id; plus `paper-rs:entity_data_serializer` | bytecode |
| `blocks.json` | blocks in registry order with state id ranges, properties, item, hardness, blast resistance | ids/item/hardness/resistance: bytecode; property names, value order, default state: mcmeta `blocks/data.json` |
| `block_states.json` | luminance, opacity, flags per state id | see below |
| `items.json` | items in registry order, max stack size, placed block | order + block: bytecode; stack size: mcmeta `item_components` |
| `synced_registries.json` | synchronized registries in vanilla's send order and their entries | order: bytecode; entries: json files in the jar |
| `tags.json` | tags vanilla sends, as numeric ids | tag files in the jar |
| `dimension_types.json` | raw `dimension_type/*.json` | jar |
| `feature_flags.json` | flags enabled by default | bytecode |

JSON is compact and deterministic. Floats are written as the shortest decimal
that round-trips to the game's `float` (e.g. `0.6`, not `0.6000000238418579`).

### Rules

The structural facts these rules rest on are re-checked against the bytecode on
every run (first section of the report), so a future version that changes them
is noticed.

* **Registry protocol ids** — `MappedRegistry.register` does
  `id = byId.size(); byId.add(holder); toId.put(value, id)`, so an entry's id is
  its registration index. The tool records every
  `WritableRegistry.register(key, value, RegistrationInfo.BUILT_IN)` reached
  from `net.minecraft.server.Bootstrap.bootStrap()` (which runs
  `BuiltInRegistries.createContents()`, i.e. each registry's bootstrap).
  The root registry (`minecraft:root`, the registry of registries) is not written.
* **Packet ids** — the tool hands a visitor to vanilla's own
  `ProtocolInfo.Details.listPackets` for the nine protocol templates that
  `net.minecraft.data.info.PacketReport.serializePackets` enumerates; the id is
  the index in the builder's packet list (this is what Mojang's `packets.json`
  report contains). `handshake/clientbound` does not exist and is `[]`.
  The bundle delimiter is play/clientbound id 0.
* **Block state ids** — `Blocks.<clinit>` adds, for each block in registry
  order, each of `getStateDefinition().getPossibleStates()` to
  `Block.BLOCK_STATE_REGISTRY`; `StateDefinition` sorts properties by name
  (`ImmutableSortedMap`) and enumerates the cartesian product with the first
  property varying slowest. Note that mcmeta lists `type` *before* `facing` for
  chests and pistons; the tool sorts by name itself.
* **Entity data serializer ids** — order of
  `EntityDataSerializers.registerSerializer` calls; names are the lower-cased
  static field names.
* **Block → item** (`blocks.json` `item`) is `Item.BY_BLOCK`, i.e. what
  `Block.asItem()` answers (so `wall_torch` → `torch`; `null` when the block is
  not in the map — vanilla's `asItem()` falls back to `minecraft:air` there).
  **Item → block** (`items.json` `block`) is the `block` field of items
  whose class is a `BlockItem` (so `redstone` → `redstone_wire`,
  `wheat_seeds` → `wheat`, `string` → `tripwire`; buckets, spawn eggs etc. are `null`).
* **Hardness / blast resistance** — `BlockBehaviour.Properties.destroyTime` and
  `explosionResistance` of the properties object each block was constructed with.
* **Synchronized registries** — `RegistryDataLoader.SYNCHRONIZED_REGISTRIES`, in
  list order (32 in 26.3, including `decorated_pot_pattern`, `block_transformer`,
  `worldgen/block_state_provider`, `test_environment`, `test_instance`).
  Entries are the json files below `data/<ns>/<registry path>/`
  (`Registries.elementsDirPath`), sorted by path then namespace — the order of
  vanilla's `MultiPackResourceManager.listResources` (a `TreeMap`); with only the
  `minecraft` namespace that is plain id order. The registry loader was not
  traced further: treat the entry order as paper-rs's own (the client takes ids
  from the order of the registry-data packet).
* **Tags** — `TagNetworkSerialization.serializeTagsToNetwork` sends tags for
  `RegistrySynchronization.networkSafeRegistries`: every registry of
  `RegistryLayer.STATIC` (= all built-in registries) plus the synchronized
  registries of the world layers, dropping registries whose tag payload is empty.
  In 26.3 that is 7 built-in (`block`, `item`, `fluid`, `entity_type`,
  `game_event`, `point_of_interest_type`, `potion`) and 8 synchronized registries.
  Tags of `villager_trade`, `worldgen/feature`, `worldgen/structure`,
  `worldgen/flat_level_generator_preset` and `worldgen/world_preset` exist in the
  data pack but are **not** sent. Tag files are in `data/<ns>/tags/<registry path>/`;
  entries are resolved in file order into a `LinkedHashSet`
  (`TagLoader.tryBuildTag`): `#tag` references expanded in place, first
  occurrence wins, optional entries skipped when missing. Ids are protocol ids
  for built-in registries and indexes into `synced_registries.json` `entries`
  for synchronized ones. Tag names are sorted; element order is vanilla's.
* **Feature flags** — the flags contained in `FeatureFlags.DEFAULT_FLAGS`
  (the enabled set of `WorldDataConfiguration.DEFAULT`); 26.3: `["minecraft:vanilla"]`.

### `block_states.json`

Three arrays indexed by state id.

| value | source | cross-check |
|---|---|---|
| `luminance` | **bytecode**: each block's `Properties.lightEmission` function evaluated for every state | equals Pumpkin's `state.lightEmission` for all 35 723 states |
| flag bit 0 air | **bytecode**: `Properties.isAir` | equals Pumpkin (`state.isAir`) for all states |
| flag bit 4 replaceable | **bytecode**: `Properties.replaceable` | equals Pumpkin (`canBeReplaced()`) for all states |
| flag bit 5 block entity | **bytecode**: block class implements `EntityBlock` | equals Pumpkin (a block entity type is valid for the state) for all states |
| `opacity` | **Pumpkin**: `state.getLightDampening()` | — |
| flag bit 1 any collision shape | **Pumpkin**: `getCollisionShape(EmptyBlockGetter, ZERO)` is not empty | blocks built with `noCollision()` have none |
| flag bit 2 full-cube collision | **Pumpkin**: `isCollisionShapeFullBlock` | set exactly when the collision shape is the single unit-cube box |
| flag bit 3 contains fluid | **Pumpkin**: `!getFluidState().isEmpty()` | equals `waterlogged == true` wherever the property exists; only on classes overriding `getFluidState` |

Opacity and collision shapes are the result of voxel-shape computations that
cannot be recovered without running the game, and the fluid bit needs
`getFluidState`; those three come from Pumpkin.

**Without `--pumpkin`** the other nine files are byte-identical and
`block_states.json` is still written, with these *defaults* (reported loudly):
`opacity` = 15 for any block that has collision, else 0; bit 1 =
`Properties.hasCollision`; bit 2 never set; bit 3 = `waterlogged == true` or the
block is `water`/`lava`. Compared with the Pumpkin-backed table that is wrong
for 28 690 opacity values, 548 collision bits, 3 450 full-cube bits and 32 fluid
bits (kelp, seagrass, bubble column, ...), so do not ship it.

#### Attribution and licence of the Pumpkin data

`opacity` and flag bits 1–3 are taken from
[Pumpkin-MC/Pumpkin](https://github.com/Pumpkin-MC/Pumpkin) `assets/blocks.json`.
That file is generated from a running Minecraft 26.3 by
[Pumpkin-MC/Extractor](https://github.com/Pumpkin-MC/Extractor) (`Blocks.kt`;
**MIT**, © 2024 Pumpkin MC).

Note for maintainers: the Pumpkin repository itself is **GPL-3.0**, not MIT.
Its `assets/NOTICE.md` says of `assets/blocks.json` (and `items.json`,
`entities.json`, ...) that they are "extracted or derived from Minecraft client
and server releases", "**not** licensed under Pumpkin's GPLv3 license and remain
the intellectual property of Mojang Studios". Decide for yourselves whether that
is acceptable for paper-rs; the tool works without Pumpkin (see above) and the
dependency is confined to three columns of one file.

Pumpkin's assets were confirmed to be for 26.3 before use: identical block
names/order (1 286), state id ranges, default states, property names and value
orders, 35 723 states, `packets.json` version 777.

### `block_behaviour.json`

Written by a separate script, `physics.py`, from Pumpkin's `assets/blocks.json`
(same source and licence note as above) for block updates, fluids and redstone:

```sh
python3 -I tools/datagen/physics.py --pumpkin /home/user/upstream/pumpkin/assets --data data/26.3
```

It refuses to write anything unless Pumpkin's block names, block order and
state id ranges equal `data/26.3/blocks.json`. Arrays indexed by state id:

| value | meaning |
|---|---|
| `support` | Pumpkin `side_flags`: bits 0-5 `isFaceSturdy(dir, FULL)` for down, up, north, south, west, east; bits 6/7 `CENTER` support of the down/up face |
| `flags` | bit 0 legacy `isSolid()`, bit 1 `isRedstoneConductor`, bit 2 `hasAnalogOutputSignal`, bits 3-4 push reaction (normal, destroy, block, push only) |
| `collision_faces` | bits 0-5: the collision boxes touching that face cover all of it (computed from Pumpkin's collision boxes; what `Shapes.mergedFaceOccludes` needs) |
| `instrument` | index into `instruments`, the values of `note_block[instrument]` |

### `block_physics.json`, `item_components.json`, `entities.json`, `entity_data.json`, `game_rules.json`

Written by `pumpkin_extra.py` from Pumpkin's `assets/` (same source and licence
note as above) for entities, items, loot and survival:

```sh
python3 -I tools/datagen/pumpkin_extra.py /home/user/upstream/pumpkin/assets data/26.3
```

It checks Pumpkin's item ids and block state ids against `items.json` and
`blocks.json`, and game rule names against the `minecraft:game_rule` registry,
and reports mismatches instead of writing them.

## How the bytecode is analysed

26.x jars are not obfuscated, but the registration code is no longer a flat list
of `register("name", ...)` calls: names come from `net.minecraft.references.*Ids`,
and whole families are registered through records and lambdas
(`ColorCollection`, `WeatheringCopperCollection`). A linear scan would have to
guess what those helpers do, so the tool evaluates the code instead:

* `classfile.py` — class-file parser and disassembler (`dis.py` is a small
  javap-like front end for it).
* `symexec.py` — a symbolic evaluator: walks the instructions with concrete
  values where they are known (strings, numbers, enum constants, records,
  lambdas, arrays) and `Unknown` everywhere else.
  * Code is evaluated in program order. Classes are initialised when the JVM
    would initialise them (first active use, superclass first).
  * Methods that can reach a registration through the static call graph (or are
    handed a lambda that can) are **hard**: if anything they need is `Unknown` —
    a branch condition, a registry key, an iteration order — the enclosing unit
    (a class initialiser or a registry bootstrap) is reported as *failed* and its
    registries are omitted. The evaluator never guesses.
  * Everything else is evaluated tolerantly: a failure only turns that result
    into `Unknown`.
* `intrinsics.py` — models of the library classes the code touches (strings,
  boxed numbers, `ArrayList`/`LinkedHashMap`/`EnumMap`/Guava immutables,
  `Optional`, sequential streams). Collections without a defined iteration order
  (`HashMap`, `HashSet`, `Map.of`, `Set.of`, ...) refuse to be iterated.
  `Identifier` and `ResourceKey` are modelled natively; the constants they need
  (`"minecraft"`, `':'`, the root registry name) are read from the bytecode.
* `registries.py`, `vanilla.py` — the sinks (registry registration, serializer
  registration) and the extraction of packets, flags, block and item details.

Assumptions built into the evaluation (each is the stock behaviour of a vanilla
server):

1. No JVM system property is set: `System.getProperty` is `null`. The code asks
   for `MC_DEBUG_ENABLED`, `MC_DEBUG_PRINT_PROPERTIES` and `max.bg.threads`
   (listed in the report). With `MC_DEBUG_*` set, vanilla changes e.g. game-rule
   defaults.
2. Assertions are disabled (`Class.desiredAssertionStatus()` is `false`).
3. The bootstrap completes without throwing: when a condition is `Unknown` and
   exactly one branch leads straight to `athrow`, the other branch is taken.
4. A loop or `if` whose condition is `Unknown` may be skipped in registering
   code only if nothing in it can register; this is reported. It happens once in
   26.3: the loop in `Fluids.<clinit>` that fills `Fluid.FLUID_STATE_REGISTRY`
   *after* the five fluids are registered.

Safety nets: every registry must have exactly the name set mcmeta lists; every
registry must be filled by exactly one class initialiser / bootstrap method
(otherwise the interleaving would depend on class-initialisation order — none is
in 26.3); `--double-check` repeats the analysis with a different strategy
(deferred class initialisation) and requires identical results.

## Validation performed for 26.3

All from one run of the command above; details in
`reports/26.3-validation.json`.

* **Registries vs mcmeta** (Mojang's generator output, alphabetical): 95 of 95
  built-in registries — identical set and count, no duplicates. None omitted.
* **Registry order vs Pumpkin** (dumps from a running game): identical order for
  all 22 registries Pumpkin covers — `block` (1 286), `item` (1 658),
  `entity_type` (161), `sound_event` (1 991), `particle_type` (128),
  `data_component_type` (122), `mob_effect`, `attribute`, `fluid`, `game_event`,
  `potion`, `custom_stat`, `stat_type`, `point_of_interest_type`, `menu`,
  `block_entity_type`, `chunk_status`, `map_decoration_type`,
  `position_source_type`, `game_rule`, `villager_profession`, `villager_type`.
  The other 73 registries have no complete independent *order* reference; they
  rest on the set check, the single-unit check, `--double-check` and the
  evaluator (`command_argument_type` is partly corroborated, see the wiki item
  below).
* **Block states vs Pumpkin**: 35 723 states; id range, default state, property
  names and value order of every block; item, hardness and blast resistance of
  every block — all identical.
* **Packets**: identical to `pumpkin-packets.json` and to Pumpkin's
  `assets/packets.json` (both declare 777) for all nine lists
  (1 / 2+2 / 5+6 / 10+21 / 69+144). Identical order to `packets.wiki`; **one
  disagreement**: the wiki spells four play/clientbound resource names with an
  underscore where the game (and Pumpkin) have a slash —
  `minecraft:debug/block_value`, `debug/chunk_value`, `debug/entity_value`,
  `debug/event` (ids 0x1A–0x1D). The bytecode constant is `"debug/block_value"`.
  This and the `Command_data.wiki` disagreement below are why the reference
  command exits with code 1.
* **Entity data serializers**: 44, identical to Pumpkin's `meta_data_type.json`
  and to the order of the wiki's metadata type table.
* **Id tables on the wiki pages**: entity types (`Entity_metadata.wiki`, 161) and
  menu types (`Inventory.wiki`, 25) identical. **Disagreement**:
  `Command_data.wiki` lists 57 command parsers, the game has 62
  (`command_argument_type`; mcmeta has the same 62 names). Ids 0–54 agree except
  that id 16 is `minecraft:team_color` (wiki: `color`); the wiki lacks
  `context_float_provider` (55), `context_int_provider` (56), `slot_source` (57),
  `feature` (59), `swing_animation` (60) and therefore has `dialog` = 55 and
  `uuid` = 56 where the game has 58 and 61. Pumpkin's protocol unit test
  `ids_follow_vanilla` (26.3) asserts the same ids 54–61 as the bytecode.
  The components table in `Slot_data.wiki` has no id column, lists 109 of the
  122 component types and is not in registry order, so it says nothing about ids.
* **Burger**: particle types identical. Its sound list is incomplete (1 897 of
  1 991, it misses e.g. `entity.cat.*`), so its ids are not comparable; what it
  has is in the same relative order. Its resolved tags equal mine for 750 of 773
  tags; the other 23 (`worldgen/biome`) are tags where Burger itself did not
  expand `#tag` references.
* **Synchronized registry entries** and **tag names** vs mcmeta: identical sets
  for all 32 registries / 15 tag groups.
* **Older minecraft-data (26.1)**: sanity only — 1 131 of 1 168 common blocks and
  1 487 of 1 506 common items keep their relative order; three slabs have a
  different blast resistance in 26.1 (Pumpkin's 26.3 dump agrees with the bytecode).
* `check.py`: 24 728 assertions on the written files.

## `--reports`: Mojang's official generator output (UNTESTED)

If you can run the data generator
(`java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports`),
pass `--reports <dir>/generated/reports`. `registries.json`, `blocks.json` and
`packets.json` from there then supply the registry, block-state and packet ids
(and are compared with the bytecode-derived ones); everything else is produced as
usual.

**This path has never seen real generator output** — the generator may not be
run where this tool was written. `selftest_reports.py` only proves that reports
*rebuilt from the tables in the documented format* are read back to byte-identical
output:

```sh
python3 -I tools/datagen/selftest_reports.py --data data/26.3 --work /tmp/datagen-selftest \
    --jar /home/user/upstream/vanilla/jar/server.jar --mcmeta /home/user/upstream/mcmeta/summary \
    --pumpkin /home/user/upstream/pumpkin/assets
```

## Files

| file | purpose |
|---|---|
| `extract.py` | command line entry point, writes the tables and the report |
| `check.py` | self-check of a generated directory |
| `selftest_reports.py` | round trip of the `--reports` mode with synthetic reports |
| `classfile.py`, `dis.py` | class-file parser / disassembler |
| `symexec.py`, `intrinsics.py` | symbolic evaluator and library models |
| `registries.py`, `vanilla.py` | what is extracted from the bytecode |
| `datapack.py` | registry entries, tags, dimension types from the jar's `data/` |
| `tables.py` | output table builders, Pumpkin reader |
| `crosscheck.py` | comparisons with mcmeta, Pumpkin, the wiki, Burger, minecraft-data |
| `reports.py` | reader for Mojang's generator reports (untested, see above) |
| `reports/<version>-validation.json` | check list of the last run |

## Moving to a new Minecraft version

Point `--jar`, `--mcmeta` and `--pumpkin` at the new version. The evaluator
fails closed: new library calls or code shapes it cannot evaluate show up as
failed units or omitted registries, not as wrong ids — extend `intrinsics.py`
and re-run until the report is clean.
