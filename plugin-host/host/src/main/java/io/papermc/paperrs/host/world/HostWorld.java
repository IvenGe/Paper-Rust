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

    @Override
    public boolean hasStorm() {
        return false;
    }

    @Override
    public boolean isThundering() {
        return false;
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
