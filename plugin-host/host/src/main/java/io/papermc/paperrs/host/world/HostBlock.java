package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.stub.AbstractBlock;
import java.util.Map;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.World;
import org.bukkit.block.Block;
import org.bukkit.block.BlockFace;
import org.bukkit.craftbukkit.CraftServer;

/** A block position in a world. Reads and writes go to the Rust server. */
public final class HostBlock extends AbstractBlock {
    private final HostWorld world;
    private final int x;
    private final int y;
    private final int z;

    public HostBlock(final HostWorld world, final int x, final int y, final int z) {
        this.world = world;
        this.x = x;
        this.y = y;
        this.z = z;
    }

    private Map<String, Object> query() {
        return Values.map(this.world.server().call("world.get_block", Values.of("world", this.world.getName(), "x", this.x, "y", this.y, "z", this.z)));
    }

    /** The block state as {@code minecraft:name[property=value,...]}. */
    public String stateString() {
        return Values.str(this.query(), "state", "minecraft:air");
    }

    @Override
    public int getX() {
        return this.x;
    }

    @Override
    public int getY() {
        return this.y;
    }

    @Override
    public int getZ() {
        return this.z;
    }

    @Override
    public World getWorld() {
        return this.world;
    }

    @Override
    public Location getLocation() {
        return new Location(this.world, this.x, this.y, this.z);
    }

    @Override
    public Location getLocation(final Location loc) {
        if (loc != null) {
            loc.setWorld(this.world);
            loc.set(this.x, this.y, this.z);
            loc.setYaw(0);
            loc.setPitch(0);
        }
        return loc;
    }

    @Override
    public Material getType() {
        final String state = this.stateString();
        final int bracket = state.indexOf('[');
        final Material material = Material.matchMaterial(bracket < 0 ? state : state.substring(0, bracket));
        return material == null ? Material.AIR : material;
    }

    @Override
    public void setType(final Material type) {
        this.setType(type, true);
    }

    @Override
    public void setType(final Material type, final boolean applyPhysics) {
        if (!type.isBlock()) {
            throw new IllegalArgumentException(type + " is not a block");
        }
        this.world.server().call("world.set_block",
            Values.of("world", this.world.getName(), "x", this.x, "y", this.y, "z", this.z, "state", type.getKey().toString()));
    }

    @Override
    public org.bukkit.block.BlockState getState() {
        return HostBlockState.of(this);
    }

    @Override
    public org.bukkit.block.BlockState getState(final boolean useSnapshot) {
        return HostBlockState.of(this);
    }

    @Override
    public Block getRelative(final int modX, final int modY, final int modZ) {
        return new HostBlock(this.world, this.x + modX, this.y + modY, this.z + modZ);
    }

    @Override
    public Block getRelative(final BlockFace face) {
        return this.getRelative(face, 1);
    }

    @Override
    public Block getRelative(final BlockFace face, final int distance) {
        return this.getRelative(face.getModX() * distance, face.getModY() * distance, face.getModZ() * distance);
    }

    @Override
    public byte getLightLevel() {
        return (byte) Math.max(this.getLightFromSky(), this.getLightFromBlocks());
    }

    @Override
    public byte getLightFromSky() {
        return (byte) Values.i32(this.query(), "sky_light", 0);
    }

    @Override
    public byte getLightFromBlocks() {
        return (byte) Values.i32(this.query(), "block_light", 0);
    }

    @Override
    public boolean isEmpty() {
        return this.getType().isAir();
    }

    @Override
    public boolean isLiquid() {
        final Material type = this.getType();
        return type == Material.WATER || type == Material.LAVA;
    }

    @Override
    public boolean isSolid() {
        return this.getType().isSolid();
    }

    @Override
    public String translationKey() {
        return this.getType().translationKey();
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostBlock b && b.world.equals(this.world) && b.x == this.x && b.y == this.y && b.z == this.z;
    }

    @Override
    public int hashCode() {
        return ((this.y & 0xFFF) << 20 | (this.x & 0x3FF) << 10 | (this.z & 0x3FF)) ^ this.world.hashCode();
    }

    @Override
    public String toString() {
        return "CraftBlock{pos=" + this.x + "," + this.y + "," + this.z + ",world=" + this.world.getName() + "}";
    }
}
