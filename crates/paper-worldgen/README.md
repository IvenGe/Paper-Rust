# paper-worldgen

Vanilla Minecraft 26.3 world generation for paper-rs: the overworld, the nether and the
end, block for block as vanilla generates them.

## Data

The generator is built at runtime from the worldgen data of the vanilla server jar
(`data/minecraft/worldgen/**`, `data/minecraft/dimension_type/**`, block tags), read
through the `DataSource` trait. The server implements it over the jar it downloads at
startup; the tests implement it over a local jar with the `zip` crate (see
`tests/common/mod.rs`, jar path from `PAPER_RS_VANILLA_JAR`). No Mojang JSON is
committed here.

## Provenance

The algorithms are ported from [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin)
(GPL-3.0, commit e9dc12e9), a Rust server that reproduces vanilla 26.3 world generation,
and adapted to build everything from the jar's data at runtime instead of Pumpkin's
build-time code generation. Each file derived from Pumpkin names its source files in its
header.

The reference outputs in `tests/fixtures/` are vanilla 26.3 dumps from Pumpkin's
`assets/tests/` (converted to gzip-compressed little-endian `u16` arrays or gzipped JSON).
