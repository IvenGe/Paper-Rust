package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.stub.AbstractWorld;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.UUID;
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

    @Override
    public List<Entity> getEntities() {
        return new ArrayList<>(this.getPlayers());
    }

    @Override
    public List<LivingEntity> getLivingEntities() {
        return new ArrayList<>(this.getPlayers());
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
