package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.entity.Entities;
import io.papermc.paperrs.host.item.HostItemStack;
import io.papermc.paperrs.host.item.HostPersistentDataContainer;
import io.papermc.paperrs.host.item.Nbt;
import io.papermc.paper.event.world.WorldGameRuleChangeEvent;
import io.papermc.paperrs.host.stub.AbstractWorld;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.function.Consumer;
import java.util.function.Predicate;
import org.bukkit.entity.EntityType;
import org.bukkit.entity.Item;
import org.bukkit.event.entity.CreatureSpawnEvent;
import org.bukkit.inventory.ItemStack;
import org.bukkit.util.BoundingBox;
import org.jetbrains.annotations.Nullable;
import org.bukkit.Difficulty;
import org.bukkit.Location;
import org.bukkit.NamespacedKey;
import org.bukkit.block.Block;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.Entity;
import org.bukkit.entity.LivingEntity;
import org.bukkit.entity.Player;

/** A world of the Rust server. Block and time queries go over the bridge. */
public final class HostWorld extends AbstractWorld {
    private final CraftServer server;
    private final String name;

    /**
     * Sections of blocks read at once (`world.section`): reading block after block, as
     * plugins scanning an area do, costs one call per section instead of one per block. A
     * section is good while nothing crosses the bridge (see {@code CraftServer#epoch}).
     */
    private final Map<Long, CachedSection> sections = new java.util.concurrent.ConcurrentHashMap<>();

    private record CachedSection(long epoch, String[] palette, byte @Nullable [] indices) {
    }
    private final UUID uid;
    private final NamespacedKey key;
    private final Environment environment;
    private final int minY;
    private final int height;
    private final long seed;
    private final int seaLevel;
    /** What the world's dimension type says (the server's {@code dimension_type} map). */
    private final Map<String, Object> dimensionType;

    public HostWorld(final CraftServer server, final Map<String, Object> info) {
        this.server = server;
        this.name = Values.str(info, "name");
        this.uid = UUID.nameUUIDFromBytes(("paper-rs:world:" + this.name).getBytes(StandardCharsets.UTF_8));
        // Its own key (Paper's level key): the dimension's for the server's own worlds.
        this.key = NamespacedKey.fromString(Values.str(info, "key", Values.str(info, "dimension", "minecraft:overworld")));
        this.environment = switch (Values.str(info, "environment", "NORMAL")) {
            case "NETHER" -> Environment.NETHER;
            case "THE_END" -> Environment.THE_END;
            default -> Environment.NORMAL;
        };
        this.minY = Values.i32(info, "min_y", -64);
        this.height = Values.i32(info, "height", 384);
        this.seed = Values.i64(info, "seed", 0);
        this.seaLevel = Values.i32(info, "sea_level", 63);
        this.dimensionType = info.get("dimension_type") instanceof Map<?, ?> m ? Values.map(m) : Map.of();
    }

    // --- the dimension type ---------------------------------------------------------------------

    @Override
    public double getCoordinateScale() {
        return Values.f64(this.dimensionType, "coordinate_scale", 1);
    }

    @Override
    public boolean hasSkyLight() {
        return Values.bool(this.dimensionType, "has_skylight", true);
    }

    @Override
    public boolean hasCeiling() {
        return Values.bool(this.dimensionType, "has_ceiling", false);
    }

    /** Water evaporates and lava is fast, as in the Nether. */
    @Override
    public boolean isUltraWarm() {
        return Values.bool(this.dimensionType, "ultrawarm", false);
    }

    @Override
    public boolean isFixedTime() {
        return Values.bool(this.dimensionType, "fixed_time", false);
    }

    @Override
    public boolean isPiglinSafe() {
        return Values.bool(this.dimensionType, "piglin_safe", false);
    }

    @Override
    public boolean isRespawnAnchorWorks() {
        return Values.bool(this.dimensionType, "respawn_anchor_works", false);
    }

    @Override
    public boolean hasRaids() {
        return Values.bool(this.dimensionType, "raids", true);
    }

    @Override
    public boolean isBedWorks() {
        return Values.bool(this.dimensionType, "bed_works", true);
    }

    public CraftServer server() {
        return this.server;
    }

    private Map<String, Object> info() {
        return Values.map(this.server.call("world.info", Values.of("world", this.name)));
    }

    /** The state of a block, as {@code minecraft:name[property=value,...]}, through the section cache. */
    public String blockState(final int x, final int y, final int z) {
        final long key = ((long) (x >> 4) & 0x3FFFFFL) << 42 | ((long) (z >> 4) & 0x3FFFFFL) << 20 | ((long) (y >> 4) & 0xFFFFFL);
        final long epoch = this.server.epoch();
        CachedSection section = this.sections.get(key);
        if (section == null || section.epoch() != epoch) {
            final Map<String, Object> read = Values.map(this.server.read("world.section", Values.of("world", this.name, "x", x, "y", y, "z", z)));
            final List<Object> palette = Values.list(read.get("palette"));
            final String[] states = new String[palette.size()];
            for (int i = 0; i < states.length; i++) {
                states[i] = String.valueOf(palette.get(i));
            }
            section = new CachedSection(epoch, states, read.get("indices") instanceof byte[] bytes ? bytes : null);
            if (this.sections.size() >= 256) {
                this.sections.clear();
            }
            this.sections.put(key, section);
        }
        final byte[] indices = section.indices();
        if (indices == null) {
            return section.palette()[0];
        }
        final int i = ((y & 15) << 8 | (z & 15) << 4 | (x & 15)) * 2;
        return section.palette()[(indices[i] & 0xFF) | (indices[i + 1] & 0xFF) << 8];
    }

    @Override
    public String getName() {
        return this.name;
    }

    @Override
    public UUID getUID() {
        return this.uid;
    }

    @Override
    public NamespacedKey getKey() {
        return this.key;
    }

    @Override
    public Environment getEnvironment() {
        return this.environment;
    }

    @Override
    public int getMinHeight() {
        return this.minY;
    }

    @Override
    public int getMaxHeight() {
        return this.minY + this.height;
    }

    @Override
    public int getLogicalHeight() {
        return this.height;
    }

    @Override
    public int getSeaLevel() {
        return this.seaLevel;
    }

    @Override
    public long getSeed() {
        return this.seed;
    }

    @Override
    public boolean isNatural() {
        return this.environment == Environment.NORMAL;
    }

    @Override
    public Block getBlockAt(final int x, final int y, final int z) {
        return new HostBlock(this, x, y, z);
    }

    @Override
    public org.bukkit.block.data.BlockData getBlockData(final int x, final int y, final int z) {
        return this.getBlockAt(x, y, z).getBlockData();
    }

    @Override
    public void setBlockData(final int x, final int y, final int z, final org.bukkit.block.data.BlockData data) {
        this.getBlockAt(x, y, z).setBlockData(data);
    }

    @Override
    public org.bukkit.Material getType(final int x, final int y, final int z) {
        return this.getBlockAt(x, y, z).getType();
    }

    @Override
    public int getHighestBlockYAt(final int x, final int z) {
        final Object y = this.server.call("world.highest_block_y", Values.of("world", this.name, "x", x, "z", z));
        return y instanceof Number n ? n.intValue() : this.minY;
    }

    @Override
    public int getHighestBlockYAt(final Location location) {
        return this.getHighestBlockYAt(location.getBlockX(), location.getBlockZ());
    }

    @Override
    public Location getSpawnLocation() {
        final Map<String, Object> info = this.info();
        final List<Object> spawn = Values.list(info.get("spawn"));
        return new Location(this, ((Number) spawn.get(0)).intValue() + 0.5, ((Number) spawn.get(1)).intValue(), ((Number) spawn.get(2)).intValue() + 0.5,
            (float) Values.f64(info, "spawn_yaw", 0), 0);
    }

    @Override
    public boolean setSpawnLocation(final Location location) {
        return this.setSpawnLocation(location.getBlockX(), location.getBlockY(), location.getBlockZ(), location.getYaw());
    }

    @Override
    public boolean setSpawnLocation(final int x, final int y, final int z, final float angle) {
        this.server.call("world.set_spawn", Values.of("world", this.name, "x", x, "y", y, "z", z, "yaw", (double) angle));
        return true;
    }

    @Override
    public long getTime() {
        return Math.floorMod(Values.i64(this.info(), "time", 0), 24000L);
    }

    @Override
    public void setTime(final long time) {
        final long full = this.getFullTime();
        final long base = full - Math.floorMod(full, 24000L);
        this.setFullTime(base + Math.floorMod(time, 24000L));
    }

    @Override
    public long getFullTime() {
        return Values.i64(this.info(), "time", 0);
    }

    @Override
    public void setFullTime(final long time) {
        this.server.call("world.set_time", Values.of("world", this.name, "time", time));
    }

    @Override
    public boolean isChunkLoaded(final int x, final int z) {
        return Boolean.TRUE.equals(this.server.call("world.is_chunk_loaded", Values.of("world", this.name, "x", x, "z", z)));
    }

    @Override
    public List<Player> getPlayers() {
        final List<Player> out = new ArrayList<>();
        for (Player player : this.server.getOnlinePlayers()) {
            if (player.getWorld() == this) {
                out.add(player);
            }
        }
        return out;
    }

    // --- entities -----------------------------------------------------------------------------

    @Override
    public List<Entity> getEntities() {
        final List<Entity> out = new ArrayList<>(this.getPlayers());
        out.addAll(Entities.list(this.server, this, null));
        return out;
    }

    @Override
    public List<LivingEntity> getLivingEntities() {
        final List<LivingEntity> out = new ArrayList<>();
        for (Entity entity : this.getEntities()) {
            if (entity instanceof LivingEntity living) {
                out.add(living);
            }
        }
        return out;
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T extends Entity> Collection<T> getEntitiesByClass(final Class<T> type) {
        final List<T> out = new ArrayList<>();
        for (Entity entity : this.getEntities()) {
            if (type.isInstance(entity)) {
                out.add((T) entity);
            }
        }
        return out;
    }

    @Override
    @SafeVarargs
    @SuppressWarnings("unchecked")
    public final <T extends Entity> Collection<T> getEntitiesByClass(final Class<T>... types) {
        return (Collection<T>) this.getEntitiesByClasses(types);
    }

    @Override
    public Collection<Entity> getEntitiesByClasses(final Class<?>... types) {
        final List<Entity> out = new ArrayList<>();
        for (Entity entity : this.getEntities()) {
            for (Class<?> type : types) {
                if (type.isInstance(entity)) {
                    out.add(entity);
                    break;
                }
            }
        }
        return out;
    }

    @Override
    public @Nullable Entity getEntity(final UUID uuid) {
        for (Entity entity : this.getEntities()) {
            if (entity.getUniqueId().equals(uuid)) {
                return entity;
            }
        }
        return null;
    }

    @Override
    public Collection<Entity> getNearbyEntities(final Location location, final double x, final double y, final double z, final @Nullable Predicate<? super Entity> filter) {
        return this.getNearbyEntities(new BoundingBox(location.getX() - x, location.getY() - y, location.getZ() - z, location.getX() + x, location.getY() + y, location.getZ() + z), filter);
    }

    @Override
    public Collection<Entity> getNearbyEntities(final BoundingBox box, final @Nullable Predicate<? super Entity> filter) {
        final List<Entity> out = new ArrayList<>();
        for (Player player : this.getPlayers()) {
            if (player.getBoundingBox().overlaps(box) && (filter == null || filter.test(player))) {
                out.add(player);
            }
        }
        for (Entity entity : Entities.list(this.server, this, new double[] {box.getMinX(), box.getMinY(), box.getMinZ(), box.getMaxX(), box.getMaxY(), box.getMaxZ()})) {
            if (filter == null || filter.test(entity)) {
                out.add(entity);
            }
        }
        return out;
    }

    private Entity spawnState(final Location location, final EntityType type, final CreatureSpawnEvent.SpawnReason reason) {
        if (type == EntityType.PLAYER || type == EntityType.UNKNOWN || type.getKey() == null) {
            throw new IllegalArgumentException("Cannot spawn an entity of type " + type);
        }
        final Map<String, Object> state = Values.map(this.server.call("world.spawn_entity", Values.of("world", this.name, "type", type.getKey().toString(),
            "x", location.getX(), "y", location.getY(), "z", location.getZ(), "yaw", (double) location.getYaw(), "pitch", (double) location.getPitch(),
            "reason", reason.name())));
        return Entities.fromState(this.server, state);
    }

    /**
     * The entity type spawned for an API class. Classes several types share stand for the one
     * CraftBukkit spawns for them ({@code Fireball} a large fireball, {@code AbstractArrow} an
     * arrow, ...); otherwise the one type whose class it is.
     */
    public static EntityType typeOf(final Class<? extends Entity> clazz) {
        for (EntityType type : EntityType.values()) {
            if (type.getEntityClass() == clazz) {
                return type;
            }
        }
        final EntityType preferred = clazz == org.bukkit.entity.Fireball.class || clazz == org.bukkit.entity.SizedFireball.class ? EntityType.FIREBALL
            : clazz == org.bukkit.entity.AbstractArrow.class || clazz == org.bukkit.entity.Projectile.class ? EntityType.ARROW
            : clazz == org.bukkit.entity.ThrownPotion.class ? EntityType.SPLASH_POTION
            : clazz == org.bukkit.entity.Minecart.class ? EntityType.MINECART
            : clazz == org.bukkit.entity.Boat.class ? EntityType.OAK_BOAT
            : clazz == org.bukkit.entity.ChestBoat.class ? EntityType.OAK_CHEST_BOAT
            : null;
        if (preferred != null) {
            return preferred;
        }
        EntityType only = null;
        for (EntityType type : EntityType.values()) {
            if (type.getEntityClass() != null && clazz.isAssignableFrom(type.getEntityClass())) {
                if (only != null) {
                    only = null;
                    break;
                }
                only = type;
            }
        }
        if (only == null) {
            throw new IllegalArgumentException("Cannot spawn an entity for " + clazz.getName());
        }
        return only;
    }

    @Override
    public Entity spawnEntity(final Location location, final EntityType type, final boolean randomizeData) {
        return this.spawnState(location, type, CreatureSpawnEvent.SpawnReason.CUSTOM);
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T extends Entity> T spawn(final Location location, final Class<T> clazz, final boolean randomizeData, final @Nullable Consumer<? super T> function) {
        final T entity = (T) this.spawnState(location, typeOf(clazz), CreatureSpawnEvent.SpawnReason.CUSTOM);
        if (function != null) {
            function.accept(entity);
        }
        return entity;
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T extends Entity> T spawn(final Location location, final Class<T> clazz, final @Nullable Consumer<? super T> function, final CreatureSpawnEvent.SpawnReason reason) {
        final T entity = (T) this.spawnState(location, typeOf(clazz), reason);
        if (function != null) {
            function.accept(entity);
        }
        return entity;
    }

    @Override
    public <T extends LivingEntity> T spawn(final Location location, final Class<T> clazz, final CreatureSpawnEvent.SpawnReason reason, final boolean randomizeData, final @Nullable Consumer<? super T> function) {
        return this.spawn(location, clazz, function, reason);
    }

    private Item drop(final Location location, final ItemStack item, final boolean natural, final @Nullable Consumer<? super Item> function) {
        if (item == null || item.isEmpty()) {
            throw new IllegalArgumentException("Cannot drop air");
        }
        final Map<String, Object> state = Values.map(this.server.call("world.drop_item", Values.of("world", this.name, "x", location.getX(), "y", location.getY(),
            "z", location.getZ(), "item", HostItemStack.toValue(item), "natural", natural)));
        final Item entity = (Item) Entities.fromState(this.server, state);
        if (function != null) {
            function.accept(entity);
        }
        return entity;
    }

    @Override
    public Item dropItem(final Location location, final ItemStack item, final @Nullable Consumer<? super Item> function) {
        return this.drop(location, item, false, function);
    }

    @Override
    public Item dropItemNaturally(final Location location, final ItemStack item, final @Nullable Consumer<? super Item> function) {
        return this.drop(location, item, true, function);
    }

    @Override
    public Difficulty getDifficulty() {
        return this.server.difficulty();
    }

    /** All worlds share the server's difficulty: this sets it for every world. */
    @Override
    public void setDifficulty(final Difficulty difficulty) {
        if (difficulty == null) {
            throw new IllegalArgumentException("Difficulty cannot be null");
        }
        this.server.setDifficulty(difficulty);
    }

    // --- weather (CraftWorld: the events first, then the server's weather) -----------------

    @Override
    public boolean hasStorm() {
        return Values.bool(this.info(), "raining", false);
    }

    @Override
    public void setStorm(final boolean hasStorm) {
        if (this.hasStorm() == hasStorm) {
            this.weather("raining", hasStorm, "rain_time", 0, "clear_time", 0);
            return;
        }
        final org.bukkit.event.weather.WeatherChangeEvent event = new org.bukkit.event.weather.WeatherChangeEvent(this, hasStorm, org.bukkit.event.weather.WeatherChangeEvent.Cause.PLUGIN);
        if (!event.callEvent()) {
            return;
        }
        this.weather("raining", hasStorm, "rain_time", 0, "clear_time", 0);
    }

    @Override
    public int getWeatherDuration() {
        return Values.i32(this.info(), "rain_time", 0);
    }

    @Override
    public void setWeatherDuration(final int duration) {
        this.weather("rain_time", duration);
    }

    @Override
    public boolean isThundering() {
        return Values.bool(this.info(), "thundering", false);
    }

    @Override
    public void setThundering(final boolean thundering) {
        if (this.isThundering() != thundering) {
            final org.bukkit.event.weather.ThunderChangeEvent event = new org.bukkit.event.weather.ThunderChangeEvent(this, thundering, org.bukkit.event.weather.ThunderChangeEvent.Cause.PLUGIN);
            if (!event.callEvent()) {
                return;
            }
        }
        this.weather("thundering", thundering, "thunder_time", 0, "clear_time", 0);
    }

    @Override
    public int getThunderDuration() {
        return Values.i32(this.info(), "thunder_time", 0);
    }

    @Override
    public void setThunderDuration(final int duration) {
        this.weather("thunder_time", duration);
    }

    @Override
    public boolean isClearWeather() {
        return !this.hasStorm() && !this.isThundering();
    }

    @Override
    public void setClearWeatherDuration(final int duration) {
        this.weather("clear_time", duration);
    }

    @Override
    public int getClearWeatherDuration() {
        return Values.i32(this.info(), "clear_time", 0);
    }

    private void weather(final Object... kv) {
        final Map<String, Object> args = Values.of(kv);
        args.put("world", this.name);
        this.server.call("world.set_weather", args);
    }

    private final HostWorldBorder border = new HostWorldBorder(this);

    // --- biomes ------------------------------------------------------------------------------

    @Override
    public org.bukkit.block.Biome getBiome(final int x, final int y, final int z) {
        final Object biome = this.server.call("world.get_biome", Values.of("world", this.name, "x", x, "y", y, "z", z));
        final org.bukkit.block.Biome found = org.bukkit.Registry.BIOME.get(NamespacedKey.fromString(String.valueOf(biome)));
        return found != null ? found : org.bukkit.block.Biome.PLAINS;
    }

    @Override
    public org.bukkit.block.Biome getBiome(final Location location) {
        return this.getBiome(location.getBlockX(), location.getBlockY(), location.getBlockZ());
    }

    // --- chunks (CraftWorld) ----------------------------------------------------------------

    private Map<String, Object> chunkArgs(final int x, final int z, final Object... kv) {
        final Map<String, Object> args = Values.of(kv);
        args.put("world", this.name);
        args.put("x", x);
        args.put("z", z);
        return args;
    }

    /** Loads (or generates) the chunk now, as {@code CraftWorld.getChunkAt}. */
    @Override
    public org.bukkit.Chunk getChunkAt(final int x, final int z) {
        this.server.call("world.load_chunk", this.chunkArgs(x, z, "generate", true));
        return new HostChunk(this, x, z);
    }

    @Override
    public org.bukkit.Chunk getChunkAt(final int x, final int z, final boolean generate) {
        return generate ? this.getChunkAt(x, z) : new HostChunk(this, x, z);
    }

    @Override
    public org.bukkit.Chunk getChunkAt(final Block block) {
        com.google.common.base.Preconditions.checkArgument(block != null, "null block");
        return this.getChunkAt(block.getX() >> 4, block.getZ() >> 4);
    }

    @Override
    public boolean isChunkLoaded(final org.bukkit.Chunk chunk) {
        com.google.common.base.Preconditions.checkArgument(chunk != null, "null chunk");
        return this.isChunkLoaded(chunk.getX(), chunk.getZ());
    }

    @Override
    public org.bukkit.Chunk[] getLoadedChunks() {
        final List<Object> loaded = Values.list(this.server.call("world.loaded_chunks", Values.of("world", this.name)));
        final org.bukkit.Chunk[] out = new org.bukkit.Chunk[loaded.size()];
        for (int i = 0; i < out.length; i++) {
            final List<Object> p = Values.list(loaded.get(i));
            out[i] = new HostChunk(this, ((Number) p.get(0)).intValue(), ((Number) p.get(1)).intValue());
        }
        return out;
    }

    @Override
    public int getChunkCount() {
        return Values.list(this.server.call("world.loaded_chunks", Values.of("world", this.name))).size();
    }

    @Override
    public void loadChunk(final org.bukkit.Chunk chunk) {
        this.loadChunk(chunk.getX(), chunk.getZ(), true);
    }

    @Override
    public boolean isChunkGenerated(final int x, final int z) {
        return Boolean.TRUE.equals(this.server.call("world.is_chunk_generated", this.chunkArgs(x, z)));
    }

    @Override
    public boolean isChunkInUse(final int x, final int z) {
        return Boolean.TRUE.equals(this.server.call("world.chunk_in_use", this.chunkArgs(x, z)));
    }

    @Override
    public boolean loadChunk(final int x, final int z, final boolean generate) {
        return Boolean.TRUE.equals(this.server.call("world.load_chunk", this.chunkArgs(x, z, "generate", generate)));
    }

    /** The chunk goes when nothing keeps it (the next unload pass); in use, it stays. */
    @Override
    public boolean unloadChunk(final int x, final int z, final boolean save) {
        return !this.isChunkLoaded(x, z) || !this.isChunkInUse(x, z);
    }

    @Override
    public boolean unloadChunkRequest(final int x, final int z) {
        return true;
    }

    @Override
    public boolean refreshChunk(final int x, final int z) {
        return true;
    }

    @Override
    public Collection<Player> getPlayersSeeingChunk(final org.bukkit.Chunk chunk) {
        return this.getPlayersSeeingChunk(chunk.getX(), chunk.getZ());
    }

    @Override
    public Collection<Player> getPlayersSeeingChunk(final int x, final int z) {
        final List<Player> out = new ArrayList<>();
        for (final Object uuid : Values.list(this.server.call("world.players_seeing_chunk", this.chunkArgs(x, z)))) {
            final Player player = this.server.getPlayer(UUID.fromString(uuid.toString()));
            if (player != null) {
                out.add(player);
            }
        }
        return out;
    }

    private List<Object> forceLoad(final int x, final int z, final @Nullable Boolean forced) {
        final Map<String, Object> args = this.chunkArgs(x, z);
        if (forced != null) {
            args.put("forced", forced);
        }
        return Values.list(this.server.call("world.force_load", args));
    }

    @Override
    public boolean isChunkForceLoaded(final int x, final int z) {
        for (final Object p : this.forceLoad(x, z, null)) {
            final List<Object> pos = Values.list(p);
            if (((Number) pos.get(0)).intValue() == x && ((Number) pos.get(1)).intValue() == z) {
                return true;
            }
        }
        return false;
    }

    @Override
    public void setChunkForceLoaded(final int x, final int z, final boolean forced) {
        this.forceLoad(x, z, forced);
    }

    @Override
    public Collection<org.bukkit.Chunk> getForceLoadedChunks() {
        final List<org.bukkit.Chunk> out = new ArrayList<>();
        for (final Object p : this.forceLoad(0, 0, null)) {
            final List<Object> pos = Values.list(p);
            out.add(new HostChunk(this, ((Number) pos.get(0)).intValue(), ((Number) pos.get(1)).intValue()));
        }
        return out;
    }

    /** Plugin chunk tickets by chunk key; the server holds one ticket per chunk with any. */
    private final Map<Long, java.util.Set<org.bukkit.plugin.Plugin>> pluginTickets = new java.util.HashMap<>();

    @Override
    public boolean addPluginChunkTicket(final int x, final int z, final org.bukkit.plugin.Plugin plugin) {
        com.google.common.base.Preconditions.checkArgument(plugin != null, "null plugin");
        com.google.common.base.Preconditions.checkArgument(plugin.isEnabled(), "plugin is not enabled");
        final java.util.Set<org.bukkit.plugin.Plugin> plugins = this.pluginTickets.computeIfAbsent(org.bukkit.Chunk.getChunkKey(x, z), k -> new java.util.HashSet<>());
        if (!plugins.add(plugin)) {
            return false;
        }
        if (plugins.size() == 1) {
            this.server.call("world.chunk_ticket", this.chunkArgs(x, z, "add", true));
        }
        return true;
    }

    @Override
    public boolean removePluginChunkTicket(final int x, final int z, final org.bukkit.plugin.Plugin plugin) {
        com.google.common.base.Preconditions.checkNotNull(plugin, "null plugin");
        final long key = org.bukkit.Chunk.getChunkKey(x, z);
        final java.util.Set<org.bukkit.plugin.Plugin> plugins = this.pluginTickets.get(key);
        if (plugins == null || !plugins.remove(plugin)) {
            return false;
        }
        if (plugins.isEmpty()) {
            this.pluginTickets.remove(key);
            this.server.call("world.chunk_ticket", this.chunkArgs(x, z, "add", false));
        }
        return true;
    }

    @Override
    public void removePluginChunkTickets(final org.bukkit.plugin.Plugin plugin) {
        com.google.common.base.Preconditions.checkNotNull(plugin, "null plugin");
        for (final long key : List.copyOf(this.pluginTickets.keySet())) {
            this.removePluginChunkTicket((int) key, (int) (key >> 32), plugin);
        }
    }

    @Override
    public Collection<org.bukkit.plugin.Plugin> getPluginChunkTickets(final int x, final int z) {
        return List.copyOf(this.pluginTickets.getOrDefault(org.bukkit.Chunk.getChunkKey(x, z), java.util.Set.of()));
    }

    @Override
    public Map<org.bukkit.plugin.Plugin, Collection<org.bukkit.Chunk>> getPluginChunkTickets() {
        final Map<org.bukkit.plugin.Plugin, Collection<org.bukkit.Chunk>> out = new java.util.HashMap<>();
        for (final Map.Entry<Long, java.util.Set<org.bukkit.plugin.Plugin>> e : this.pluginTickets.entrySet()) {
            final org.bukkit.Chunk chunk = new HostChunk(this, (int) (long) e.getKey(), (int) (e.getKey() >> 32));
            for (final org.bukkit.plugin.Plugin plugin : e.getValue()) {
                out.computeIfAbsent(plugin, p -> new ArrayList<>()).add(chunk);
            }
        }
        return out;
    }

    @Override
    public Collection<org.bukkit.Chunk> getIntersectingChunks(final BoundingBox box) {
        final List<org.bukkit.Chunk> out = new ArrayList<>();
        for (int x = (int) Math.floor(box.getMinX()) >> 4; x <= (int) Math.floor(box.getMaxX()) >> 4; x++) {
            for (int z = (int) Math.floor(box.getMinZ()) >> 4; z <= (int) Math.floor(box.getMaxZ()) >> 4; z++) {
                out.add(this.getChunkAt(x, z));
            }
        }
        return out;
    }

    /** The chunk is loaded on the main thread now, and the callback gets it right after. */
    @Override
    public void getChunkAtAsync(final int x, final int z, final boolean gen, final boolean urgent, final Consumer<? super org.bukkit.Chunk> cb) {
        if (!gen && !this.isChunkGenerated(x, z)) {
            cb.accept(null);
            return;
        }
        cb.accept(this.getChunkAt(x, z));
    }

    @Override
    public void getChunksAtAsync(final int minX, final int minZ, final int maxX, final int maxZ, final boolean urgent, final Runnable cb) {
        for (int x = minX; x <= maxX; x++) {
            for (int z = minZ; z <= maxZ; z++) {
                this.getChunkAt(x, z);
            }
        }
        cb.run();
    }

    @Override
    public org.bukkit.WorldBorder getWorldBorder() {
        return this.border;
    }

    // --- game rules ---------------------------------------------------------------------------

    /** The value the server keeps for a rule; null for a rule there is not. */
    private @Nullable Object ruleValue(final org.bukkit.GameRule<?> rule) {
        return this.server.call("world.game_rule", Values.of("world", this.name, "rule", rule.getKey().asString()));
    }

    private static HostGameRule<?> hostRule(final org.bukkit.GameRule<?> rule) {
        if (!(rule instanceof HostGameRule<?> host)) {
            throw new IllegalArgumentException("not a game rule of this server: " + rule);
        }
        return host;
    }

    /**
     * Sets a rule as Paper does ({@code CraftEventFactory.handleGameRuleSet}): plugins see a
     * {@link WorldGameRuleChangeEvent} with the value as text first, may change or cancel it.
     */
    private boolean changeRule(final HostGameRule<?> rule, final Object serverValue) {
        final String text = String.valueOf(serverValue);
        final WorldGameRuleChangeEvent event = new WorldGameRuleChangeEvent(this, null, rule, text);
        if (!event.callEvent()) {
            return false;
        }
        Object value = serverValue;
        if (!event.getValue().equals(text)) {
            value = rule.parseServerValue(event.getValue());
            if (value == null) {
                throw new IllegalArgumentException("Invalid value: " + event.getValue());
            }
        }
        this.server.call("world.set_game_rule", Values.of("world", this.name, "rule", rule.getKey().asString(), "value", value));
        return true;
    }

    @Override
    public String[] getGameRules() {
        return Values.list(this.server.call("world.game_rules", Values.of("world", this.name))).stream().map(String::valueOf).toArray(String[]::new);
    }

    @Override
    public boolean isGameRule(final String rule) {
        if (rule == null || rule.isEmpty()) {
            throw new IllegalArgumentException("String rule cannot be " + (rule == null ? "null" : "empty"));
        }
        final org.bukkit.GameRule<?> bukkit = org.bukkit.GameRule.getByName(rule);
        return bukkit != null && this.ruleValue(bukkit) != null;
    }

    @Override
    public @Nullable String getGameRuleValue(final @Nullable String rule) {
        if (rule == null) {
            return null;
        }
        final org.bukkit.GameRule<?> bukkit = org.bukkit.GameRule.getByName(rule);
        if (bukkit == null) {
            throw new IllegalArgumentException("Unknown gamerule: " + rule);
        }
        return String.valueOf(this.ruleValue(bukkit));
    }

    @Override
    public boolean setGameRuleValue(final @Nullable String rule, final @Nullable String value) {
        if (rule == null || value == null) {
            return false;
        }
        final org.bukkit.GameRule<?> bukkit = org.bukkit.GameRule.getByName(rule);
        if (bukkit == null || this.ruleValue(bukkit) == null) {
            return false;
        }
        final HostGameRule<?> host = hostRule(bukkit);
        final Object parsed = host.parseServerValue(value);
        if (parsed == null) {
            throw new IllegalArgumentException("Invalid value: " + value);
        }
        return this.changeRule(host, parsed);
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T> T getGameRuleValue(final org.bukkit.GameRule<T> rule) {
        if (rule == null) {
            throw new IllegalArgumentException("GameRule cannot be null");
        }
        return ((HostGameRule<T>) hostRule(rule)).fromServer(this.ruleValue(rule));
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T> boolean setGameRule(final org.bukkit.GameRule<T> rule, final T newValue) {
        if (rule == null || newValue == null) {
            throw new IllegalArgumentException(rule == null ? "GameRule cannot be null" : "GameRule value cannot be null");
        }
        if (this.ruleValue(rule) == null) {
            return false;
        }
        final HostGameRule<T> host = (HostGameRule<T>) hostRule(rule);
        return this.changeRule(host, host.toServer(newValue));
    }

    @Override
    public boolean getPVP() {
        return this.getGameRuleValue(org.bukkit.GameRules.PVP);
    }

    @Override
    public void setPVP(final boolean pvp) {
        this.setGameRule(org.bukkit.GameRules.PVP, pvp);
    }

    // --- sounds ---------------------------------------------------------------------------------

    private void sound(final double x, final double y, final double z, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final @Nullable Long seed) {
        final Map<String, Object> args = Values.of("world", this.name, "sound", sound, "category", category.ordinal(), "x", x, "y", y, "z", z, "volume", (double) volume, "pitch", (double) pitch);
        if (seed != null) {
            args.put("seed", seed);
        }
        this.server.call("world.play_sound", args);
    }

    private static String soundKey(final org.bukkit.Sound sound) {
        return ((net.kyori.adventure.key.Keyed) sound).key().asString();
    }

    @Override
    public void playSound(final Location location, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch) {
        this.sound(location.getX(), location.getY(), location.getZ(), sound, category, volume, pitch, null);
    }

    @Override
    public void playSound(final Location location, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final long seed) {
        this.sound(location.getX(), location.getY(), location.getZ(), sound, category, volume, pitch, seed);
    }

    @Override
    public void playSound(final Location location, final org.bukkit.Sound sound, final org.bukkit.SoundCategory category, final float volume, final float pitch) {
        this.sound(location.getX(), location.getY(), location.getZ(), soundKey(sound), category, volume, pitch, null);
    }

    @Override
    public void playSound(final Location location, final org.bukkit.Sound sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final long seed) {
        this.sound(location.getX(), location.getY(), location.getZ(), soundKey(sound), category, volume, pitch, seed);
    }

    @Override
    public void playSound(final Entity entity, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch) {
        final Location at = entity.getLocation();
        this.sound(at.getX(), at.getY(), at.getZ(), sound, category, volume, pitch, null);
    }

    @Override
    public void playSound(final Entity entity, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final long seed) {
        final Location at = entity.getLocation();
        this.sound(at.getX(), at.getY(), at.getZ(), sound, category, volume, pitch, seed);
    }

    @Override
    public void playSound(final Entity entity, final org.bukkit.Sound sound, final org.bukkit.SoundCategory category, final float volume, final float pitch) {
        final Location at = entity.getLocation();
        this.sound(at.getX(), at.getY(), at.getZ(), soundKey(sound), category, volume, pitch, null);
    }

    @Override
    public void playSound(final Entity entity, final org.bukkit.Sound sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final long seed) {
        final Location at = entity.getLocation();
        this.sound(at.getX(), at.getY(), at.getZ(), soundKey(sound), category, volume, pitch, seed);
    }

    // --- particles, effects, explosions and lightning ----------------------------------------

    /** A particle's options as the server takes them ({@code CraftParticle}'s data). */
    public static Map<String, Object> particleData(final org.bukkit.Particle particle, final @Nullable Object data) {
        if (data != null && !particle.getDataType().isInstance(data)) {
            throw new IllegalArgumentException("data (" + data.getClass() + ") should be " + particle.getDataType());
        }
        if (data == null && particle.getDataType() != Void.class) {
            throw new IllegalArgumentException("missing required data " + particle.getDataType());
        }
        final Map<String, Object> out = new java.util.LinkedHashMap<>();
        switch (data) {
            case null -> {
            }
            case org.bukkit.Particle.DustTransition transition -> {
                out.put("color", transition.getColor().asRGB());
                out.put("to_color", transition.getToColor().asRGB());
                out.put("scale", (double) transition.getSize());
            }
            case org.bukkit.Particle.DustOptions dust -> {
                out.put("color", dust.getColor().asRGB());
                out.put("scale", (double) dust.getSize());
            }
            case org.bukkit.block.data.BlockData block -> out.put("state", block.getAsString());
            case org.bukkit.inventory.ItemStack item -> out.put("item", io.papermc.paperrs.host.item.HostItemStack.toValue(item));
            case org.bukkit.Color color -> out.put("color", color.asARGB());
            case org.bukkit.Particle.Trail trail -> {
                out.put("target_x", trail.getTarget().getX());
                out.put("target_y", trail.getTarget().getY());
                out.put("target_z", trail.getTarget().getZ());
                out.put("color", trail.getColor().asRGB());
                out.put("duration", trail.getDuration());
            }
            case org.bukkit.Particle.Spell spell -> {
                out.put("color", spell.getColor().asARGB());
                out.put("power", (double) spell.getPower());
            }
            case org.bukkit.Vibration vibration -> {
                switch (vibration.getDestination()) {
                    case org.bukkit.Vibration.Destination.EntityDestination entity -> out.put("entity", entity.getEntity().getEntityId());
                    case org.bukkit.Vibration.Destination.BlockDestination block -> {
                        out.put("target_x", block.getLocation().getX());
                        out.put("target_y", block.getLocation().getY());
                        out.put("target_z", block.getLocation().getZ());
                    }
                    default -> {
                    }
                }
                out.put("ticks", vibration.getArrivalTime());
            }
            case Float value -> out.put(particle == org.bukkit.Particle.SCULK_CHARGE ? "roll" : "power", (double) value);
            case Integer value -> out.put("delay", value);
            default -> {
            }
        }
        return out;
    }

    @Override
    public <T> void spawnParticle(final org.bukkit.Particle particle, final @Nullable List<Player> receivers, final @Nullable Player sender, final double x, final double y, final double z,
                                  final int count, final double offsetX, final double offsetY, final double offsetZ, final double speedX, final double speedY, final double speedZ,
                                  final @Nullable T data, final boolean force, final org.bukkit.Particle.RandomizationType randomization) {
        final Map<String, Object> args = Values.of("world", this.name, "particle", particle.getKey().asString(), "data", particleData(particle, data),
            "x", x, "y", y, "z", z, "count", count, "offset_x", offsetX, "offset_y", offsetY, "offset_z", offsetZ,
            "speed_x", speedX, "speed_y", speedY, "speed_z", speedZ, "force", force, "randomization", randomization.ordinal());
        if (receivers != null) {
            args.put("to", receivers.stream().filter(p -> sender == null || p.canSee(sender)).map(p -> p.getUniqueId().toString()).toList());
        }
        this.server.call("world.particles", args);
    }

    /** {@code CraftWorld#playEffect}: the level event to the players within {@code radius}. */
    @Override
    @SuppressWarnings("deprecation")
    public void playEffect(final Location location, final org.bukkit.Effect effect, final int data, final int radius) {
        this.server.call("world.level_event", Values.of("world", this.name, "x", location.getBlockX(), "y", location.getBlockY(), "z", location.getBlockZ(),
            "at_x", location.getX(), "at_y", location.getY(), "at_z", location.getZ(), "event", effect.getId(), "data", data, "radius", (double) radius));
    }

    /** Data objects as {@code CraftEffect#getDataValue} turns them into a number (a block as its state). */
    @Override
    @SuppressWarnings("deprecation")
    public <T> void playEffect(final Location location, final org.bukkit.Effect effect, final @Nullable T data, final int radius) {
        if (data != null && (effect.getData() == null || !effect.getData().isAssignableFrom(data.getClass()))) {
            throw new IllegalArgumentException(data.getClass().getName() + " data cannot be used for the " + effect + " effect");
        }
        final Map<String, Object> args = Values.of("world", this.name, "x", location.getBlockX(), "y", location.getBlockY(), "z", location.getBlockZ(),
            "at_x", location.getX(), "at_y", location.getY(), "at_z", location.getZ(), "event", effect.getId(), "radius", (double) radius);
        switch (data) {
            case null -> args.put("data", 0);
            case org.bukkit.block.data.BlockData block -> args.put("state", block.getAsString());
            case org.bukkit.Material material when material.isBlock() -> args.put("state", material.createBlockData().getAsString());
            case org.bukkit.Color color -> args.put("data", color.asRGB());
            case org.bukkit.block.BlockFace face -> args.put("data", switch (face) {
                case DOWN -> 0;
                case UP -> 1;
                case NORTH -> 2;
                case SOUTH -> 3;
                case WEST -> 4;
                default -> 5;
            });
            case Integer value -> args.put("data", value);
            case Boolean value -> args.put("data", value ? 1 : 0);
            default -> args.put("data", 0);
        }
        this.server.call("world.level_event", args);
    }

    @Override
    public boolean createExplosion(final double x, final double y, final double z, final float power, final boolean setFire, final boolean breakBlocks, final @Nullable Entity source) {
        return Boolean.TRUE.equals(this.server.call("world.explode", Values.of("world", this.name, "x", x, "y", y, "z", z, "power", (double) power, "fire", setFire,
            "break_blocks", breakBlocks, "source", source == null ? null : source.getEntityId())));
    }

    @Override
    public boolean createExplosion(final Location location, final float power, final boolean setFire, final boolean breakBlocks, final @Nullable Entity source) {
        return this.createExplosion(location.getX(), location.getY(), location.getZ(), power, setFire, breakBlocks, source);
    }

    @Override
    public boolean createExplosion(final @Nullable Entity source, final Location location, final float power, final boolean setFire, final boolean breakBlocks, final boolean excludeSourceFromDamage) {
        return this.createExplosion(location.getX(), location.getY(), location.getZ(), power, setFire, breakBlocks, source);
    }

    private org.bukkit.entity.LightningStrike lightning(final Location location, final boolean effect) {
        final Object bolt = this.server.call("world.strike_lightning", Values.of("world", this.name, "x", location.getX(), "y", location.getY(), "z", location.getZ(), "effect", effect));
        return bolt instanceof Map<?, ?> state ? (org.bukkit.entity.LightningStrike) Entities.fromState(this.server, Values.map(state)) : null;
    }

    @Override
    public org.bukkit.entity.LightningStrike strikeLightning(final Location location) {
        return this.lightning(location, false);
    }

    /** Only the flash and the thunder: no fire, no harm. */
    @Override
    public org.bukkit.entity.LightningStrike strikeLightningEffect(final Location location) {
        return this.lightning(location, true);
    }

    // --- settings and counts ---------------------------------------------------------------------

    private final Map<org.bukkit.entity.SpawnCategory, Integer> spawnLimits = new java.util.EnumMap<>(org.bukkit.entity.SpawnCategory.class);
    private final Map<org.bukkit.entity.SpawnCategory, Integer> ticksPerSpawns = new java.util.EnumMap<>(org.bukkit.entity.SpawnCategory.class);
    private boolean allowAnimals = true;
    private boolean voidDamage = true;
    private float voidDamageAmount = 4;
    private double voidDamageOffset = -64;

    @Override
    public boolean canGenerateStructures() {
        return this.server.getGenerateStructures();
    }

    @Override
    public boolean isHardcore() {
        return this.server.isHardcore();
    }

    @Override
    public int getViewDistance() {
        return this.server.getViewDistance();
    }

    @Override
    public int getSimulationDistance() {
        return this.server.getSimulationDistance();
    }

    @Override
    public int getSendViewDistance() {
        return this.server.getViewDistance();
    }

    private Map<String, Object> counts() {
        return Values.map(this.server.call("world.counts", Values.of("world", this.name)));
    }

    @Override
    public int getEntityCount() {
        return Values.i32(this.counts(), "entities", 0);
    }

    @Override
    public int getPlayerCount() {
        return Values.i32(this.counts(), "players", 0);
    }

    @Override
    public int getTileEntityCount() {
        return Values.i32(this.counts(), "block_entities", 0);
    }

    @Override
    public long getGameTime() {
        return this.getFullTime();
    }

    /** {@code isBrightOutside}: the sky is not yet dark (the time of day without weather). */
    @Override
    public boolean isDayTime() {
        final long time = this.getTime() % 24000;
        return !this.hasCeiling() && !this.isFixedTime() && (time < 12542 || time > 23460);
    }

    @Override
    public io.papermc.paper.world.MoonPhase getMoonPhase() {
        return io.papermc.paper.world.MoonPhase.getPhase(this.getFullTime() / 24000L);
    }

    @Override
    public org.bukkit.block.BlockState getBlockState(final int x, final int y, final int z) {
        return this.getBlockAt(x, y, z).getState();
    }

    /**
     * Plugins' block populators: vanilla generation makes the chunks, then each new chunk is
     * handed to these ({@code populate(World, Random, Chunk)}) before {@code ChunkPopulateEvent},
     * as in CraftBukkit's {@code LevelChunk.loadCallback}.
     */
    private final List<org.bukkit.generator.BlockPopulator> populators = new java.util.concurrent.CopyOnWriteArrayList<>();

    @Override
    public List<org.bukkit.generator.BlockPopulator> getPopulators() {
        return this.populators;
    }

    /** Runs the populators on a newly generated chunk, with CraftBukkit's random. */
    public void populate(final int x, final int z) {
        if (this.populators.isEmpty()) {
            return;
        }
        final java.util.Random random = new java.util.Random(this.seed);
        final long xRand = random.nextLong() / 2L * 2L + 1L;
        final long zRand = random.nextLong() / 2L * 2L + 1L;
        random.setSeed((long) x * xRand + (long) z * zRand ^ this.seed);
        final org.bukkit.Chunk chunk = this.getChunkAt(x, z, false);
        for (final org.bukkit.generator.BlockPopulator populator : this.populators) {
            populator.populate(this, random, chunk);
        }
    }

    @Override
    public org.bukkit.generator.@Nullable BiomeProvider getBiomeProvider() {
        return null;
    }

    @Override
    public boolean getAllowAnimals() {
        return this.allowAnimals;
    }

    /** Monsters follow the {@code spawn_monsters} game rule. */
    @Override
    public boolean getAllowMonsters() {
        return this.getGameRuleValue(org.bukkit.GameRules.SPAWN_MONSTERS);
    }

    /**
     * Monsters through the {@code spawn_monsters} game rule; whether animals spawn is kept by
     * the host only (the server's spawning has no such switch yet).
     */
    @Override
    public void setSpawnFlags(final boolean allowMonsters, final boolean allowAnimals) {
        this.allowAnimals = allowAnimals;
        if (this.getAllowMonsters() != allowMonsters) {
            this.setGameRule(org.bukkit.GameRules.SPAWN_MONSTERS, allowMonsters);
        }
    }

    /** The server's limits unless a plugin set this world's (kept by the host, not yet used by the server's spawning). */
    @Override
    public int getSpawnLimit(final org.bukkit.entity.SpawnCategory category) {
        final Integer own = this.spawnLimits.get(category);
        return own != null && own >= 0 ? own : this.server.getSpawnLimit(category);
    }

    @Override
    public void setSpawnLimit(final org.bukkit.entity.SpawnCategory category, final int limit) {
        this.spawnLimits.put(category, limit);
    }

    @Override
    public long getTicksPerSpawns(final org.bukkit.entity.SpawnCategory category) {
        final Integer own = this.ticksPerSpawns.get(category);
        return own != null && own >= 0 ? own : this.server.getTicksPerSpawns(category);
    }

    @Override
    public void setTicksPerSpawns(final org.bukkit.entity.SpawnCategory category, final int ticks) {
        this.ticksPerSpawns.put(category, ticks);
    }

    /** Paper's per-world void damage settings, kept by the host (the server hurts below the world as vanilla does). */
    @Override
    public boolean isVoidDamageEnabled() {
        return this.voidDamage;
    }

    @Override
    public void setVoidDamageEnabled(final boolean enabled) {
        this.voidDamage = enabled;
    }

    @Override
    public float getVoidDamageAmount() {
        return this.voidDamageAmount;
    }

    @Override
    public void setVoidDamageAmount(final float amount) {
        this.voidDamageAmount = amount;
    }

    @Override
    public double getVoidDamageMinBuildHeightOffset() {
        return this.voidDamageOffset;
    }

    @Override
    public void setVoidDamageMinBuildHeightOffset(final double offset) {
        this.voidDamageOffset = offset;
    }

    // --- saving, plugin data, metadata and plugin messages ----------------------------------

    private boolean autoSave = true;
    private @Nullable HostPersistentDataContainer pdc;
    private final org.bukkit.craftbukkit.metadata.BlockMetadataStore blockMetadata = new org.bukkit.craftbukkit.metadata.BlockMetadataStore(this);

    @Override
    public void save(final boolean flush) {
        this.server.call("world.save", Values.of("world", this.name, "flush", flush));
    }

    /** The world's folder ({@code getWorldFolder} too): the server's, or where it would be. */
    @Override
    public java.nio.file.Path getWorldPath() {
        final Object folder = this.info().get("folder");
        return folder instanceof String path ? java.nio.file.Path.of(path) : this.server.getWorldContainer().toPath().resolve(this.name);
    }

    /** Kept by the host: the server saves its worlds on its own schedule. */
    @Override
    public boolean isAutoSave() {
        return this.autoSave;
    }

    @Override
    public void setAutoSave(final boolean value) {
        this.autoSave = value;
    }

    @Override
    @SuppressWarnings("deprecation")
    public org.bukkit.WorldType getWorldType() {
        return "minecraft:flat".equals(this.server.getWorldType()) && this.environment == Environment.NORMAL ? org.bukkit.WorldType.FLAT : org.bukkit.WorldType.NORMAL;
    }

    /** Worlds are generated by the server; plugins cannot give them generators yet. */
    @Override
    public org.bukkit.generator.@Nullable ChunkGenerator getGenerator() {
        return null;
    }

    /** Saved with the world, as Paper saves it (its {@code paper:persistent_data_container} data). */
    @Override
    public org.bukkit.persistence.PersistentDataContainer getPersistentDataContainer() {
        if (this.pdc == null) {
            HostPersistentDataContainer container = new HostPersistentDataContainer();
            if (this.server.call("world.pdc", Values.of("world", this.name)) instanceof byte[] saved && saved.length > 0) {
                container = new HostPersistentDataContainer(Nbt.readCompound(saved));
            }
            final HostPersistentDataContainer data = container;
            this.pdc = data.onChange(() -> this.server.call("world.set_pdc", Values.of("world", this.name, "data", data.isEmpty() ? new byte[0] : Nbt.writeCompound(data.tags()))));
        }
        return this.pdc;
    }

    /** Metadata of the blocks of this world ({@code CraftWorld#getBlockMetadata}). */
    public org.bukkit.craftbukkit.metadata.BlockMetadataStore blockMetadata() {
        return this.blockMetadata;
    }

    @Override
    public void setMetadata(final String key, final org.bukkit.metadata.MetadataValue value) {
        this.server.worldMetadata().setMetadata(this, key, value);
    }

    @Override
    public List<org.bukkit.metadata.MetadataValue> getMetadata(final String key) {
        return this.server.worldMetadata().getMetadata(this, key);
    }

    @Override
    public boolean hasMetadata(final String key) {
        return this.server.worldMetadata().hasMetadata(this, key);
    }

    @Override
    public void removeMetadata(final String key, final org.bukkit.plugin.Plugin plugin) {
        this.server.worldMetadata().removeMetadata(this, key, plugin);
    }

    /** To every player in the world ({@code CraftWorld#sendPluginMessage}). */
    @Override
    public void sendPluginMessage(final org.bukkit.plugin.Plugin source, final String channel, final byte[] message) {
        org.bukkit.plugin.messaging.StandardMessenger.validatePluginMessage(this.server.getMessenger(), source, channel, message);
        for (final Player player : this.getPlayers()) {
            player.sendPluginMessage(source, channel, message);
        }
    }

    @Override
    public java.util.Set<String> getListeningPluginChannels() {
        final java.util.Set<String> channels = new java.util.HashSet<>();
        for (final Player player : this.getPlayers()) {
            channels.addAll(player.getListeningPluginChannels());
        }
        return channels;
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostWorld w && w.uid.equals(this.uid);
    }

    @Override
    public int hashCode() {
        return this.uid.hashCode();
    }

    @Override
    public String toString() {
        return "CraftWorld{name=" + this.name + "}";
    }
}
