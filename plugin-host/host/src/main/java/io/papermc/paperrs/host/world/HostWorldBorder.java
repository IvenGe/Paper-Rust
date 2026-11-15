package io.papermc.paperrs.host.world;

import com.google.common.base.Preconditions;
import io.papermc.paper.event.world.border.WorldBorderBoundsChangeEvent;
import io.papermc.paper.event.world.border.WorldBorderCenterChangeEvent;
import io.papermc.paperrs.host.bridge.Values;
import java.util.Map;
import org.bukkit.Location;
import org.bukkit.World;
import org.bukkit.WorldBorder;
import org.bukkit.event.HandlerList;

/**
 * A world's border, kept by the Rust server: reads ask it for the border as it is now (so a
 * border moving to a new size reads its current size), changes go to it and to the world's
 * players who see it. The checks are {@code CraftWorldBorder}'s; changes fire Paper's border events
 * as {@code WorldBorder}'s setters do.
 */
public final class HostWorldBorder implements WorldBorder {
    /** {@code WorldBorder.MAX_SIZE} and {@code MAX_CENTER_COORDINATE}. */
    private static final double MAX_SIZE = 5.9999968E7;
    private static final double MAX_CENTER = 2.9999984E7;

    private final HostWorld world;

    HostWorldBorder(final HostWorld world) {
        this.world = world;
    }

    private Map<String, Object> state() {
        return Values.map(Values.map(this.world.server().call("world.info", Values.of("world", this.world.getName()))).get("border"));
    }

    private void set(final Object... kv) {
        final Map<String, Object> args = Values.of(kv);
        args.put("world", this.world.getName());
        this.world.server().call("world.set_border", args);
    }

    @Override
    public World getWorld() {
        return this.world;
    }

    /**
     * Paper's {@code WorldBorder.applySettings(Settings.DEFAULT)}: each setting through its
     * setter, so plugins hear of the center and size as they change.
     */
    @Override
    public void reset() {
        this.setCenter(0, 0);
        this.set("damage_amount", 0.2, "damage_buffer", 5.0, "warning_distance", 5, "warning_time", 15);
        this.resize(MAX_SIZE);
    }

    @Override
    public double getSize() {
        return Values.f64(this.state(), "size", MAX_SIZE);
    }

    @Override
    public void setSize(final double newSize) {
        Preconditions.checkArgument(newSize >= 1.0 && newSize <= this.getMaxSize(), "newSize must be between 1.0 and %s", this.getMaxSize());
        this.resize(newSize);
    }

    @Override
    public void changeSize(final double newSize, final long ticks) {
        Preconditions.checkArgument(ticks >= 0 && ticks <= Integer.MAX_VALUE, "ticks must be between 0-%s", Integer.MAX_VALUE);
        Preconditions.checkArgument(newSize >= 1.0 && newSize <= this.getMaxSize(), "newSize must be between 1.0 and %s", this.getMaxSize());
        if (ticks > 0) {
            this.lerp(this.getSize(), newSize, ticks);
        } else {
            this.resize(newSize);
        }
    }

    /**
     * Paper's {@code WorldBorder.setSize}: plugins hear of it ({@code WorldBorderBoundsChangeEvent})
     * and may stop it, change the size, or make it a move.
     */
    private void resize(final double size) {
        if (!listened(WorldBorderBoundsChangeEvent.getHandlerList())) {
            this.set("size", size);
            return;
        }
        final WorldBorderBoundsChangeEvent event = new WorldBorderBoundsChangeEvent(this.world, this, WorldBorderBoundsChangeEvent.Type.INSTANT_MOVE, this.getSize(), size, 0);
        if (!event.callEvent()) {
            return;
        }
        if (event.getType() == WorldBorderBoundsChangeEvent.Type.STARTED_MOVE && event.getDurationTicks() > 0) {
            this.lerp(event.getOldSize(), event.getNewSize(), event.getDurationTicks());
            return;
        }
        this.set("size", event.getNewSize());
    }

    /** Paper's {@code WorldBorder.lerpSizeBetween}, with the same event. */
    private void lerp(final double from, double to, long ticks) {
        if (listened(WorldBorderBoundsChangeEvent.getHandlerList())) {
            final WorldBorderBoundsChangeEvent event = new WorldBorderBoundsChangeEvent(this.world, this,
                from == to ? WorldBorderBoundsChangeEvent.Type.INSTANT_MOVE : WorldBorderBoundsChangeEvent.Type.STARTED_MOVE, from, to, ticks);
            if (!event.callEvent()) {
                return;
            }
            to = event.getNewSize();
            ticks = event.getDurationTicks();
        }
        if (from == to || ticks <= 0) {
            this.set("size", to);
        } else {
            this.set("from", from, "size", to, "ticks", ticks);
        }
    }

    private static boolean listened(final HandlerList handlers) {
        return handlers.getRegisteredListeners().length > 0;
    }

    @Override
    public Location getCenter() {
        final Map<String, Object> state = this.state();
        return new Location(this.world, Values.f64(state, "center_x", 0), 0, Values.f64(state, "center_z", 0));
    }

    /** With Paper's {@code WorldBorderCenterChangeEvent}, which may stop or move it. */
    @Override
    public void setCenter(final double x, final double z) {
        Preconditions.checkArgument(Math.abs(x) <= this.getMaxCenterCoordinate(), "x coordinate cannot be outside +- %s", this.getMaxCenterCoordinate());
        Preconditions.checkArgument(Math.abs(z) <= this.getMaxCenterCoordinate(), "z coordinate cannot be outside +- %s", this.getMaxCenterCoordinate());
        if (!listened(WorldBorderCenterChangeEvent.getHandlerList())) {
            this.set("center_x", x, "center_z", z);
            return;
        }
        final WorldBorderCenterChangeEvent event = new WorldBorderCenterChangeEvent(this.world, this, this.getCenter(), new Location(this.world, x, 0, z));
        if (event.callEvent()) {
            this.set("center_x", event.getNewCenter().getX(), "center_z", event.getNewCenter().getZ());
        }
    }

    @Override
    public void setCenter(final Location location) {
        this.setCenter(location.getX(), location.getZ());
    }

    @Override
    public double getDamageBuffer() {
        return Values.f64(this.state(), "damage_buffer", 5.0);
    }

    @Override
    public void setDamageBuffer(final double blocks) {
        this.set("damage_buffer", blocks);
    }

    @Override
    public double getDamageAmount() {
        return Values.f64(this.state(), "damage_amount", 0.2);
    }

    @Override
    public void setDamageAmount(final double damage) {
        this.set("damage_amount", damage);
    }

    @Override
    public int getWarningTimeTicks() {
        return Values.i32(this.state(), "warning_time", 15) * 20;
    }

    @Override
    public void setWarningTimeTicks(final int ticks) {
        Preconditions.checkArgument(ticks >= 0, "ticks cannot be lower than 0");
        this.set("warning_time", ticks / 20);
    }

    @Override
    public int getWarningDistance() {
        return Values.i32(this.state(), "warning_distance", 5);
    }

    @Override
    public void setWarningDistance(final int distance) {
        this.set("warning_distance", distance);
    }

    @Override
    public boolean isInside(final Location location) {
        Preconditions.checkArgument(location != null, "location cannot be null");
        if (location.getWorld() != null && !location.getWorld().equals(this.world)) {
            return false;
        }
        // WorldBorder.isWithinBounds(BlockPos): the block's corner against the bounds.
        final Map<String, Object> state = this.state();
        final double half = Values.f64(state, "size", MAX_SIZE) / 2;
        final double cx = Values.f64(state, "center_x", 0);
        final double cz = Values.f64(state, "center_z", 0);
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

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostWorldBorder b && b.world.equals(this.world);
    }

    @Override
    public int hashCode() {
        return this.world.hashCode();
    }
}
