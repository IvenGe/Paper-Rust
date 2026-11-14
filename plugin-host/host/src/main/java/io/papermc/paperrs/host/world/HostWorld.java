package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.entity.Entities;
import io.papermc.paperrs.host.item.HostItemStack;
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
    private final UUID uid;
    private final NamespacedKey key;
    private final Environment environment;
    private final int minY;
    private final int height;
    private final long seed;
    private final int seaLevel;

    public HostWorld(final CraftServer server, final Map<String, Object> info) {
        this.server = server;
        this.name = Values.str(info, "name");
        this.uid = UUID.nameUUIDFromBytes(("paper-rs:world:" + this.name).getBytes(StandardCharsets.UTF_8));
        this.key = NamespacedKey.fromString(Values.str(info, "dimension", "minecraft:overworld"));
        this.environment = switch (Values.str(info, "environment", "NORMAL")) {
            case "NETHER" -> Environment.NETHER;
            case "THE_END" -> Environment.THE_END;
            default -> Environment.NORMAL;
        };
        this.minY = Values.i32(info, "min_y", -64);
        this.height = Values.i32(info, "height", 384);
        this.seed = Values.i64(info, "seed", 0);
        this.seaLevel = Values.i32(info, "sea_level", 63);
    }

    public CraftServer server() {
        return this.server;
    }

    private Map<String, Object> info() {
        return Values.map(this.server.call("world.info", Values.of("world", this.name)));
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

    private static EntityType typeOf(final Class<? extends Entity> clazz) {
        for (EntityType type : EntityType.values()) {
            if (type.getEntityClass() == clazz) {
                return type;
            }
        }
        throw new IllegalArgumentException("Cannot spawn an entity for " + clazz.getName());
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

    @Override
    public boolean getPVP() {
        return true;
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
