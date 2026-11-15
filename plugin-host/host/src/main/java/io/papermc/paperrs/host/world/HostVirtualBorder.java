package io.papermc.paperrs.host.world;

import com.google.common.base.Preconditions;
import io.papermc.paperrs.host.bridge.Bridge;
import io.papermc.paperrs.host.bridge.Values;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.CopyOnWriteArrayList;
import org.bukkit.Location;
import org.bukkit.World;
import org.bukkit.WorldBorder;
import org.bukkit.craftbukkit.CraftServer;
import org.jspecify.annotations.Nullable;

/**
 * A border of no world ({@code Server#createWorldBorder}, CraftBukkit's virtual
 * {@code CraftWorldBorder}), which players given it see instead of their world's
 * ({@code Player#setWorldBorder}). It only shows on their clients: the server keeps the
 * world's border for damage and collisions.
 *
 * <p>The host keeps its settings; the server keeps a copy for each player who sees it and
 * moves those with the ticks, as Paper ticks a virtual border with its players. A moving
 * border's size is therefore read from a viewer's copy, and stays where it was while nobody
 * sees it. Each change goes to every viewer, as CraftPlayer's border listener sends it.
 */
public final class HostVirtualBorder implements WorldBorder {
    /** {@code WorldBorder.MAX_SIZE} and {@code MAX_CENTER_COORDINATE}. */
    private static final double MAX_SIZE = 5.9999968E7;
    private static final double MAX_CENTER = 2.9999984E7;

    private final CraftServer server;
    /** The players who see it, by UUID. */
    private final List<UUID> viewers = new CopyOnWriteArrayList<>();
    private double centerX;
    private double centerZ;
    private double size = MAX_SIZE;
    private double target = MAX_SIZE;
    private long ticksLeft;
    private double damageBuffer = 5.0;
    private double damageAmount = 0.2;
    private int warningDistance = 5;
    /** {@code WorldBorder.Settings.DEFAULT.warningTime()}, which CraftServer sets. */
    private int warningTicks = 300;

    public HostVirtualBorder(final CraftServer server) {
        this.server = server;
    }

    /** Shows the border to a player, whole (CraftPlayer's {@code setWorldBorder}). */
    public synchronized void addViewer(final UUID player) {
        this.refresh();
        this.viewers.remove(player);
        this.viewers.add(player);
        final Map<String, Object> border = Values.of("center_x", this.centerX, "center_z", this.centerZ,
            "damage_buffer", this.damageBuffer, "damage_amount", this.damageAmount,
            "warning_distance", this.warningDistance, "warning_time", this.warningTicks / 20);
        if (this.ticksLeft > 0) {
            border.put("from", this.size);
            border.put("size", this.target);
            border.put("ticks", this.ticksLeft);
        } else {
            border.put("size", this.size);
        }
        this.server.call("player.set_border", Values.of("uuid", player.toString(), "border", border, "initial", true));
    }

    /** The player no longer sees it (they were given another border, or left). */
    public void removeViewer(final UUID player) {
        this.viewers.remove(player);
    }

    /** Sends a change to each viewer's copy; viewers who have gone are forgotten. */
    private void send(final Object... kv) {
        final Map<String, Object> changes = Values.of(kv);
        final List<UUID> gone = new ArrayList<>();
        for (final UUID viewer : this.viewers) {
            try {
                this.server.call("player.set_border", Values.of("uuid", viewer.toString(), "border", changes));
            } catch (final Bridge.RemoteException e) {
                gone.add(viewer);
            }
        }
        this.viewers.removeAll(gone);
    }

    /** Catches up with a moving border: its size as a viewer's copy has it now. */
    private synchronized void refresh() {
        if (this.ticksLeft <= 0) {
            return;
        }
        for (final UUID viewer : this.viewers) {
            final Object state;
            try {
                state = this.server.call("player.border", Values.of("uuid", viewer.toString()));
            } catch (final Bridge.RemoteException e) {
                continue;
            }
            if (state instanceof Map<?, ?>) {
                final Map<String, Object> border = Values.map(state);
                this.size = Values.f64(border, "size", this.size);
                this.ticksLeft = Values.i64(border, "lerp_left", 0);
                if (this.ticksLeft <= 0) {
                    this.size = this.target;
                }
                return;
            }
        }
    }

    @Override
    public @Nullable World getWorld() {
        return null;
    }

    @Override
    public synchronized void reset() {
        this.setCenter(0, 0);
        this.setDamageAmount(0.2);
        this.setDamageBuffer(5.0);
        this.setWarningDistance(5);
        this.setWarningTimeTicks(300);
        this.setSize(MAX_SIZE);
    }

    @Override
    public synchronized double getSize() {
        this.refresh();
        return this.size;
    }

    @Override
    public synchronized void setSize(final double newSize) {
        Preconditions.checkArgument(newSize >= 1.0 && newSize <= this.getMaxSize(), "newSize must be between 1.0 and %s", this.getMaxSize());
        this.size = newSize;
        this.target = newSize;
        this.ticksLeft = 0;
        this.send("size", newSize);
    }

    @Override
    public synchronized void changeSize(final double newSize, final long ticks) {
        Preconditions.checkArgument(ticks >= 0 && ticks <= Integer.MAX_VALUE, "ticks must be between 0-%s", Integer.MAX_VALUE);
        Preconditions.checkArgument(newSize >= 1.0 && newSize <= this.getMaxSize(), "newSize must be between 1.0 and %s", this.getMaxSize());
        final double from = this.getSize();
        if (ticks <= 0 || from == newSize) {
            this.setSize(newSize);
            return;
        }
        this.target = newSize;
        this.ticksLeft = ticks;
        this.send("from", from, "size", newSize, "ticks", ticks);
    }

    @Override
    public synchronized Location getCenter() {
        return new Location(null, this.centerX, 0, this.centerZ);
    }

    @Override
    public synchronized void setCenter(final double x, final double z) {
        Preconditions.checkArgument(Math.abs(x) <= this.getMaxCenterCoordinate(), "x coordinate cannot be outside +- %s", this.getMaxCenterCoordinate());
        Preconditions.checkArgument(Math.abs(z) <= this.getMaxCenterCoordinate(), "z coordinate cannot be outside +- %s", this.getMaxCenterCoordinate());
        this.centerX = x;
        this.centerZ = z;
        this.send("center_x", x, "center_z", z);
    }

    @Override
    public void setCenter(final Location location) {
        this.setCenter(location.getX(), location.getZ());
    }

    @Override
    public synchronized double getDamageBuffer() {
        return this.damageBuffer;
    }

    @Override
    public synchronized void setDamageBuffer(final double blocks) {
        this.damageBuffer = blocks;
        this.send("damage_buffer", blocks);
    }

    @Override
    public synchronized double getDamageAmount() {
        return this.damageAmount;
    }

    @Override
    public synchronized void setDamageAmount(final double damage) {
        this.damageAmount = damage;
        this.send("damage_amount", damage);
    }

    @Override
    public synchronized int getWarningTimeTicks() {
        return this.warningTicks;
    }

    @Override
    public synchronized void setWarningTimeTicks(final int ticks) {
        Preconditions.checkArgument(ticks >= 0, "ticks cannot be lower than 0");
        this.warningTicks = ticks;
        this.send("warning_time", ticks / 20);
    }

    @Override
    public synchronized int getWarningDistance() {
        return this.warningDistance;
    }

    @Override
    public synchronized void setWarningDistance(final int distance) {
        this.warningDistance = distance;
        this.send("warning_distance", distance);
    }

    /** {@code WorldBorder.isWithinBounds(BlockPos)}, in any world: a virtual border has none. */
    @Override
    public boolean isInside(final Location location) {
        Preconditions.checkArgument(location != null, "location cannot be null");
        final double half = this.getSize() / 2;
        final double cx;
        final double cz;
        synchronized (this) {
            cx = this.centerX;
            cz = this.centerZ;
        }
        final double minX = Math.max(cx - half, -MAX_CENTER);
        final double maxX = Math.min(cx + half, MAX_CENTER);
        final double minZ = Math.max(cz - half, -MAX_CENTER);
        final double maxZ = Math.min(cz + half, MAX_CENTER);
        final int x = location.getBlockX();
        final int z = location.getBlockZ();
        return x >= Math.floor(minX) && x < Math.ceil(maxX) && z >= Math.floor(minZ) && z < Math.ceil(maxZ);
    }

    @Override
    public double getMaxSize() {
        return MAX_SIZE;
    }

    @Override
    public double getMaxCenterCoordinate() {
        return MAX_CENTER;
    }
}
