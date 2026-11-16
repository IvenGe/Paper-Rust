# Older client versions

What the server needs to translate its packets for clients of older versions
(`crates/paper-compat`): one file per version (26.2, 26.1, 1.21.11, 1.21.9, 1.21.7, 1.21.6,
1.21.5), made by `tools/datagen/compat.py`.

Each file holds:

| Field | What |
| --- | --- |
| `name`, `protocol` | the version and the protocol number its handshake carries |
| `pack_versions` | the versions such a client gives its vanilla data pack (`26.1`, `26.1.1`, `26.1.2`) |
| `packets` | its packet names in id order, by state and direction, spelled as 26.3 spells them |
| `sizes` | how many ids each registry below has in that version |
| `mappings` | for each id registry that differs (block states, blocks, items, entity types, sounds, particles, data component types, ...), the version's id of every 26.3 id (-1: none), as runs `[first 26.3 id, its old id, length]` |
| `registries` | for the data-driven registries the server sends during configuration: the 26.3 entries the version knows (`keep`, by index) and what stands in for the others (`fallback`), or `dropped` if it does not have the registry |

`legacy.json` (made by `tools/datagen/compat_legacy.py`) is for clients from before the
flattening. For each such version (now 1.8) it holds, base64-encoded as little-endian arrays,
the legacy block (`id << 4 | meta`) of every 26.3 block state, the legacy item (`id << 16 |
damage`) of every 26.3 item, and the index in `sound_names` of the legacy sound of every 26.3
sound event. Entities, biomes, enchantments, effects and particles are small tables in
`crates/paper-compat/src/legacy/tables.rs`. Clients older than 1.8 get 1.8's ids with what
came later replaced by blocks, items and mobs they have: `crates/paper-compat/src/legacy/v1_7/ids.rs`
for 1.7 (after ViaRewind's replacements), `crates/paper-compat/src/legacy/prenetty/ids.rs` for
1.4.6 to 1.6.4.

## Sources

* The id mappings are those of [ViaVersion](https://github.com/ViaVersion/ViaVersion) and
  [ViaBackwards](https://github.com/ViaVersion/ViaBackwards): their mapping files
  (`mappings-26.3to26.2.nbt`, ...), composed step by step from 26.3 down. Where a version lacks
  a block, item or entity, they map it to a similar one. ViaVersion and ViaBackwards are
  licensed under the GPL, version 3 or later, as Paper-RS is. The translators follow what their
  protocol classes describe of each version's changes, written anew in Rust.
* The legacy tables compose ViaBackwards' mappings from 26.3 down to 1.13, its flattening
  table (1.13 to 1.12), then the replacements ViaBackwards and
  [ViaRewind](https://github.com/ViaVersion/ViaRewind) (GPL-3.0 or later) give blocks and items
  each older version lacks; the 1.8 sound names are ViaVersion's.
* The protocols from before Netty (1.4.6 to 1.6.4: packet layouts, strings, items, entity data
  and the few sounds 1.6 renamed) follow [ViaLegacy](https://github.com/ViaVersion/ViaLegacy)
  (GPL-3.0 or later), written anew in Rust.
* The packet orders come from ViaVersion's packet enums, checked against
  [minecraft-data](https://github.com/PrismarineJS/minecraft-data) (MIT) in
  `crates/paper-server/tests/compat.rs`.
* Which entries of the data-driven registries (biomes, damage types, ...) and of the built-in
  registries ViaVersion does not map a version has comes from
  [misode/mcmeta](https://github.com/misode/mcmeta)'s `<version>-summary` tags. Built-in
  registries are mapped by name, assuming 26.3 only added entries.

## Regenerating

```bash
python3 tools/datagen/compat_legacy.py <ViaVersion checkout> <ViaBackwards checkout> \
    <ViaRewind checkout> .
```

```bash
python3 tools/datagen/compat.py <ViaVersion checkout> <ViaBackwards checkout> \
    <mcmeta versions dir> <minecraft-data>/data/pc/common/protocolVersions.json .
```

`<mcmeta versions dir>/<version>/` holds a version's summary (for a protocol shared by several
releases, the last: 1.21.10 for 1.21.9's, 1.21.8 for 1.21.7's), fetched from
`https://raw.githubusercontent.com/misode/mcmeta/<version>-summary/` (`version.json`,
`registries/data.min.json`, `blocks/data.min.json` and `data/<registry>/data.min.json` for each
data-driven registry the server sends).
