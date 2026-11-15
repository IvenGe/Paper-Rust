package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.bridge.Values;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import net.kyori.adventure.key.Key;
import org.bukkit.ChunkSnapshot;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.block.Biome;
import org.bukkit.block.data.BlockData;

/**
 * {@code Chunk#getChunkSnapshot}: the chunk as it was when asked, all in one answer from the
 * server (each section's blocks, light and biomes, and the highest block of each column), so
 * that it can be read from any thread afterwards. As in {@code CraftChunkSnapshot}, the
 * heights, biomes and temperatures are there only if asked for. Biome temperatures are the
 * biomes' own, without vanilla's adjustment for height.
 */
final class HostChunkSnapshot implements ChunkSnapshot {
    /** Each vanilla biome's {@code temperature} (26.3 data). */
    private static final Map<String, Double> TEMPERATURES = new HashMap<>();

    static {
        final Object[] table = {
        "badlands", 2.0, "bamboo_jungle", 0.95, "basalt_deltas", 2.0, "beach", 0.8,
        "birch_forest", 0.6, "cherry_grove", 0.5, "cold_ocean", 0.5, "crimson_forest", 2.0,
        "dappled_forest", 0.6, "dark_forest", 0.7, "deep_cold_ocean", 0.5, "deep_dark", 0.8,
        "deep_frozen_ocean", 0.5, "deep_lukewarm_ocean", 0.5, "deep_ocean", 0.5, "desert", 2.0,
        "dripstone_caves", 0.8, "end_barrens", 0.5, "end_highlands", 0.5, "end_midlands", 0.5,
        "eroded_badlands", 2.0, "flower_forest", 0.7, "forest", 0.7, "frozen_ocean", 0.0,
        "frozen_peaks", -0.7, "frozen_river", 0.0, "grove", -0.2, "ice_spikes", 0.0,
        "jagged_peaks", -0.7, "jungle", 0.95, "lukewarm_ocean", 0.5, "lush_caves", 0.5,
        "mangrove_swamp", 0.8, "meadow", 0.5, "mushroom_fields", 0.9, "nether_wastes", 2.0,
        "ocean", 0.5, "old_growth_birch_forest", 0.6, "old_growth_pine_taiga", 0.3, "old_growth_spruce_taiga", 0.25,
        "pale_garden", 0.7, "plains", 0.8, "river", 0.5, "savanna", 2.0,
        "savanna_plateau", 2.0, "small_end_islands", 0.5, "snowy_beach", 0.05, "snowy_plains", 0.0,
        "snowy_slopes", -0.3, "snowy_taiga", -0.5, "soul_sand_valley", 2.0, "sparse_jungle", 0.95,
        "stony_peaks", 1.0, "stony_shore", 0.2, "sulfur_caves", 0.8, "sunflower_plains", 0.8,
        "swamp", 0.8, "taiga", 0.25, "the_end", 0.5, "the_void", 0.5,
        "warm_ocean", 0.5, "warped_forest", 2.0, "windswept_forest", 0.2, "windswept_gravelly_hills", 0.2,
        "windswept_hills", 0.2, "windswept_savanna", 2.0, "wooded_badlands", 2.0

        };
        for (int i = 0; i < table.length; i += 2) {
            TEMPERATURES.put("minecraft:" + table[i], (Double) table[i + 1]);
        }
    }

    private record Section(String[] palette, byte[] indices, int sky, byte[] skyNibbles, int block, byte[] blockNibbles, String[] biomes) {
        String state(final int x, final int y, final int z) {
            if (this.indices == null) {
                return this.palette[0];
            }
            final int i = (y << 8 | z << 4 | x) * 2;
            return this.palette[(this.indices[i] & 0xFF) | (this.indices[i + 1] & 0xFF) << 8];
        }

        static int light(final int uniform, final byte[] nibbles, final int x, final int y, final int z) {
            if (nibbles == null) {
                return uniform;
            }
            final int i = y << 8 | z << 4 | x;
            return (nibbles[i >> 1] >> ((i & 1) << 2)) & 0x0F;
        }
    }

    private final HostWorld world;
    private final int x;
    private final int z;
    private final int minY;
    private final Section[] sections;
    private final int[] heights;
    private final long time;
    private final boolean withHeights;
    private final boolean withBiomes;
    private final boolean withTemperatures;
    private final Map<String, BlockData> data = new HashMap<>();

    HostChunkSnapshot(final HostWorld world, final int x, final int z, final Map<String, Object> read, final boolean heights, final boolean biomes, final boolean temperatures) {
        this.world = world;
        this.x = x;
        this.z = z;
        this.minY = Values.i32(read, "min_y", world.getMinHeight());
        final List<Object> list = Values.list(read.get("sections"));
        this.sections = new Section[list.size()];
        for (int i = 0; i < this.sections.length; i++) {
            final Map<String, Object> s = Values.map(list.get(i));
            final List<Object> palette = Values.list(s.get("palette"));
            final String[] states = new String[palette.size()];
            for (int j = 0; j < states.length; j++) {
                states[j] = String.valueOf(palette.get(j));
            }
            final List<Object> names = Values.list(s.get("biomes"));
            final String[] biomeNames = new String[64];
            for (int j = 0; j < 64; j++) {
                biomeNames[j] = j < names.size() ? String.valueOf(names.get(j)) : "minecraft:plains";
            }
            this.sections[i] = new Section(states, s.get("indices") instanceof byte[] b ? b : null,
                s.get("sky") instanceof Number n ? n.intValue() : 0, s.get("sky") instanceof byte[] b ? b : null,
                s.get("block") instanceof Number n ? n.intValue() : 0, s.get("block") instanceof byte[] b ? b : null, biomeNames);
        }
        final List<Object> h = Values.list(read.get("heights"));
        this.heights = new int[256];
        for (int i = 0; i < 256 && i < h.size(); i++) {
            this.heights[i] = ((Number) h.get(i)).intValue();
        }
        this.time = Values.i64(read, "time");
        this.withHeights = heights;
        this.withBiomes = biomes;
        this.withTemperatures = temperatures;
    }

    private Section section(final int x, final int y, final int z) {
        if (x < 0 || x > 15 || z < 0 || z > 15) {
            throw new IllegalArgumentException("x and z must be 0-15, not " + x + ", " + z);
        }
        final int index = (y - this.minY) >> 4;
        if (index < 0 || index >= this.sections.length) {
            throw new IllegalArgumentException("y " + y + " is outside the world");
        }
        return this.sections[index];
    }

    @Override
    public int getX() {
        return this.x;
    }

    @Override
    public int getZ() {
        return this.z;
    }

    @Override
    public String getWorldName() {
        return this.world.getName();
    }

    @Override
    public Key getWorldKey() {
        return this.world.getKey();
    }

    @Override
    public BlockData getBlockData(final int x, final int y, final int z) {
        final String state = this.section(x, y, z).state(x, (y - this.minY) & 15, z);
        return this.data.computeIfAbsent(state, HostBlockData::ofState).clone();
    }

    @Override
    public Material getBlockType(final int x, final int y, final int z) {
        return this.getBlockData(x, y, z).getMaterial();
    }

    @Override
    @SuppressWarnings("deprecation")
    public int getData(final int x, final int y, final int z) {
        return 0;
    }

    @Override
    public int getBlockSkyLight(final int x, final int y, final int z) {
        final Section s = this.section(x, y, z);
        return Section.light(s.sky(), s.skyNibbles(), x, (y - this.minY) & 15, z);
    }

    @Override
    public int getBlockEmittedLight(final int x, final int y, final int z) {
        final Section s = this.section(x, y, z);
        return Section.light(s.block(), s.blockNibbles(), x, (y - this.minY) & 15, z);
    }

    @Override
    public int getHighestBlockYAt(final int x, final int z) {
        if (!this.withHeights) {
            throw new IllegalStateException("ChunkSnapshot created without height map. Please call getSnapshot with includeMaxblocky=true");
        }
        this.section(x, this.minY, z);
        return this.heights[z << 4 | x];
    }

    private String biomeKey(final int x, final int y, final int z) {
        if (!this.withBiomes && !this.withTemperatures) {
            throw new IllegalStateException("ChunkSnapshot created without biome. Please call getSnapshot with includeBiome=true");
        }
        final Section s = this.section(x, y, z);
        return s.biomes()[((((y - this.minY) & 15) >> 2) << 4) | ((z >> 2) << 2) | (x >> 2)];
    }

    @Override
    @SuppressWarnings("deprecation")
    public Biome getBiome(final int x, final int z) {
        return this.getBiome(x, 0, z);
    }

    @Override
    public Biome getBiome(final int x, final int y, final int z) {
        final Biome biome = org.bukkit.Registry.BIOME.get(NamespacedKey.fromString(this.biomeKey(x, y, z)));
        return biome != null ? biome : Biome.PLAINS;
    }

    @Override
    @SuppressWarnings("deprecation")
    public double getRawBiomeTemperature(final int x, final int z) {
        return this.getRawBiomeTemperature(x, 0, z);
    }

    @Override
    public double getRawBiomeTemperature(final int x, final int y, final int z) {
        if (!this.withTemperatures) {
            throw new IllegalStateException("ChunkSnapshot created without biome temperature. Please call getSnapshot with includeBiomeTempRain=true");
        }
        return TEMPERATURES.getOrDefault(this.biomeKey(x, y, z), 0.8);
    }

    @Override
    public long getCaptureFullTime() {
        return this.time;
    }

    @Override
    public boolean isSectionEmpty(final int sy) {
        if (sy < 0 || sy >= this.sections.length) {
            throw new IllegalArgumentException("y section " + sy + " is outside the world");
        }
        for (final String state : this.sections[sy].palette()) {
            if (!state.equals("minecraft:air") && !state.equals("minecraft:cave_air") && !state.equals("minecraft:void_air")) {
                return false;
            }
        }
        return true;
    }

    @Override
    public boolean contains(final BlockData data) {
        final String wanted = data.getAsString();
        for (final Section s : this.sections) {
            for (final String state : s.palette()) {
                if (state.equals(wanted)) {
                    return true;
                }
            }
        }
        return false;
    }

    @Override
    public boolean contains(final Biome biome) {
        if (!this.withBiomes && !this.withTemperatures) {
            throw new IllegalStateException("ChunkSnapshot created without biome. Please call getSnapshot with includeBiome=true");
        }
        final String wanted = biome.getKey().toString();
        for (final Section s : this.sections) {
            for (final String name : s.biomes()) {
                if (name.equals(wanted)) {
                    return true;
                }
            }
        }
        return false;
    }
}
