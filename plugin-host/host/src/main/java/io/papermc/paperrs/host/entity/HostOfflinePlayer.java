package io.papermc.paperrs.host.entity;

import com.destroystokyo.paper.profile.PlayerProfile;
import io.papermc.paperrs.host.HostPlayerProfile;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.stub.AbstractOfflinePlayer;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import org.bukkit.Location;
import org.bukkit.World;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.Player;
import org.jetbrains.annotations.Nullable;

/**
 * A player who may not be online ({@code Bukkit.getOfflinePlayer}), as Paper's
 * {@code CraftOfflinePlayer}: what the server knows from the player data files, the name
 * cache and the operator, ban and whitelist lists. Each question asks the server, so the
 * answers stay current.
 */
public final class HostOfflinePlayer extends AbstractOfflinePlayer {
    private final CraftServer server;
    private final UUID uuid;
    private @Nullable String name;

    public HostOfflinePlayer(final CraftServer server, final UUID uuid, final @Nullable String name) {
        this.server = server;
        this.uuid = uuid;
        this.name = name;
    }

    private Map<String, Object> info() {
        final Map<String, Object> args = Values.of("uuid", this.uuid.toString());
        if (this.name != null) {
            args.put("name", this.name);
        }
        final Map<String, Object> info = Values.map(this.server.call("offline.get", args));
        if (info.get("name") instanceof String known) {
            this.name = known;
        }
        return info;
    }

    @Override
    public UUID getUniqueId() {
        return this.uuid;
    }

    @Override
    public @Nullable String getName() {
        final Player online = this.getPlayer();
        if (online != null) {
            return online.getName();
        }
        if (this.name == null) {
            this.info();
        }
        return this.name;
    }

    @Override
    public PlayerProfile getPlayerProfile() {
        return new HostPlayerProfile(this.uuid, this.getName());
    }

    @Override
    public @Nullable Player getPlayer() {
        return this.server.getPlayer(this.uuid);
    }

    @Override
    public boolean isOnline() {
        return this.getPlayer() != null;
    }

    @Override
    public boolean isConnected() {
        return this.isOnline();
    }

    @Override
    public boolean hasPlayedBefore() {
        return Values.bool(this.info(), "played_before", false);
    }

    @Override
    public long getFirstPlayed() {
        return Values.i64(this.info(), "first_played", 0);
    }

    @Override
    @SuppressWarnings("deprecation")
    public long getLastPlayed() {
        return Values.i64(this.info(), "last_played", 0);
    }

    @Override
    public long getLastLogin() {
        return Values.i64(this.info(), "last_login", 0);
    }

    @Override
    public long getLastSeen() {
        return Values.i64(this.info(), "last_seen", 0);
    }

    @Override
    public boolean isOp() {
        return Values.bool(this.info(), "op", false);
    }

    @Override
    public void setOp(final boolean value) {
        this.server.call("offline.set_op", Values.of("uuid", this.uuid.toString(), "name", this.getName(), "op", value));
        final Player online = this.getPlayer();
        if (online != null) {
            online.recalculatePermissions();
        }
    }

    @Override
    public boolean isBanned() {
        return Values.bool(this.info(), "banned", false);
    }

    @Override
    public boolean isWhitelisted() {
        return Values.bool(this.info(), "whitelisted", false);
    }

    @Override
    public void setWhitelisted(final boolean value) {
        this.server.call("offline.set_whitelisted", Values.of("uuid", this.uuid.toString(), "name", this.getName(), "value", value));
    }

    /** Where the player was when they last left, from their player data. */
    @Override
    public @Nullable Location getLocation() {
        final Player online = this.getPlayer();
        if (online != null) {
            return online.getLocation();
        }
        final Map<String, Object> info = this.info();
        final List<Object> pos = Values.list(info.get("position"));
        if (pos.size() != 3) {
            return null;
        }
        final String worldName = Values.str(info, "world", "world");
        World world = this.server.getWorld(worldName);
        if (world == null) {
            for (World w : this.server.getWorlds()) {
                if (w.getKey().toString().equals(worldName)) {
                    world = w;
                }
            }
        }
        return new Location(world, ((Number) pos.get(0)).doubleValue(), ((Number) pos.get(1)).doubleValue(), ((Number) pos.get(2)).doubleValue());
    }

    @Override
    public Map<String, Object> serialize() {
        final Map<String, Object> out = new LinkedHashMap<>();
        out.put("UUID", this.uuid.toString());
        return out;
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof org.bukkit.OfflinePlayer other && this.uuid.equals(other.getUniqueId());
    }

    @Override
    public int hashCode() {
        return this.uuid.hashCode();
    }

    @Override
    public String toString() {
        return "CraftOfflinePlayer[UUID=" + this.uuid + "]";
    }

    // --- statistics -------------------------------------------------------------------------

    @Override
    public int getStatistic(final org.bukkit.Statistic statistic) {
        return Statistics.get(this.server, this.uuid, statistic);
    }

    @Override
    public void setStatistic(final org.bukkit.Statistic statistic, final int newValue) {
        Statistics.set(this.server, this.uuid, statistic, newValue);
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic) {
        Statistics.add(this.server, this.uuid, statistic, Statistics.amount(1));
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, Statistics.amount(amount));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic) {
        Statistics.add(this.server, this.uuid, statistic, -Statistics.amount(1));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, -Statistics.amount(amount));
    }

    @Override
    public int getStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material) {
        return Statistics.get(this.server, this.uuid, statistic, material);
    }

    @Override
    public void setStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material, final int newValue) {
        Statistics.set(this.server, this.uuid, statistic, material, newValue);
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material) {
        Statistics.add(this.server, this.uuid, statistic, material, Statistics.amount(1));
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, material, Statistics.amount(amount));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material) {
        Statistics.add(this.server, this.uuid, statistic, material, -Statistics.amount(1));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, material, -Statistics.amount(amount));
    }

    @Override
    public int getStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType) {
        return Statistics.get(this.server, this.uuid, statistic, entityType);
    }

    @Override
    public void setStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType, final int newValue) {
        Statistics.set(this.server, this.uuid, statistic, entityType, newValue);
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType) {
        Statistics.add(this.server, this.uuid, statistic, entityType, Statistics.amount(1));
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, entityType, Statistics.amount(amount));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType) {
        Statistics.add(this.server, this.uuid, statistic, entityType, -Statistics.amount(1));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, entityType, -Statistics.amount(amount));
    }
}
