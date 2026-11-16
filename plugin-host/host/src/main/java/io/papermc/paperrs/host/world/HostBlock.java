package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.stub.AbstractBlock;
import java.util.Map;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.World;
import org.bukkit.block.Block;
import org.bukkit.block.BlockFace;
import org.bukkit.block.data.BlockData;
import org.jetbrains.annotations.Nullable;

/** A block position in a world. Reads and writes go to the Rust server. */
public final class HostBlock extends AbstractBlock {
    private final HostWorld world;
    private final int x;
    private final int y;
    private final int z;
    /**
     * The state an event shows for a change the server makes only once the event allows it
     * ({@code BlockPlaceEvent#getBlockPlaced}); null to read the world.
     */
    private @Nullable String pending;

    @Override
    public org.bukkit.block.Biome getBiome() {
        return this.world.getBiome(this.x, this.y, this.z);
    }

    @Override
    public org.bukkit.Chunk getChunk() {
        return this.world.getChunkAt(this);
    }

    public HostBlock(final HostWorld world, final int x, final int y, final int z) {
        this.world = world;
        this.x = x;
        this.y = y;
        this.z = z;
    }

    private Map<String, Object> query() {
        // A read: it changes nothing, so the host's caches stay good.
        return Values.map(this.world.server().read("world.get_block", Values.of("world", this.world.getName(), "x", this.x, "y", this.y, "z", this.z)));
    }

    /** The block's state and whether it has a block entity, as {@code world.get_block} tells. */
    Map<String, Object> info() {
        final String state = this.pending;
        return state != null ? Values.of("state", state, "block_entity", false) : this.query();
    }

    /** The block state as {@code minecraft:name[property=value,...]}. */
    public String stateString() {
        final String state = this.pending;
        return state != null ? state : this.world.blockState(this.x, this.y, this.z);
    }

    /** Shows {@code state} instead of the world's block until {@link #settle()} or a change. */
    public HostBlock pending(final String state) {
        this.pending = state;
        return this;
    }

    public void settle() {
        this.pending = null;
    }

    /** Sets the block state ({@code minecraft:name[property=value,...]}). */
    public void setState(final String state, final boolean applyPhysics) {
        this.pending = null;
        if (!applyPhysics && this.world.server().isPrimaryThread()) {
            // Without physics only this block changes and nothing fires events: the change
            // goes out with the next batch rather than as a call there and back.
            final int id = io.papermc.paperrs.host.data.HostData.get().parseState(state);
            if (id >= 0) {
                this.world.setBlockQuietly(this.x, this.y, this.z, id);
                return;
            }
        }
        this.world.server().call("world.set_block",
            Values.of("world", this.world.getName(), "x", this.x, "y", this.y, "z", this.z, "state", state, "physics", applyPhysics));
    }

    @Override
    public BlockData getBlockData() {
        return HostBlockData.ofState(this.stateString());
    }

    @Override
    public void setBlockData(final BlockData data) {
        this.setBlockData(data, true);
    }

    @Override
    public void setBlockData(final BlockData data, final boolean applyPhysics) {
        if (data == null) {
            throw new IllegalArgumentException("BlockData cannot be null");
        }
        this.setState(HostBlockData.impl(data).getAsString(false), applyPhysics);
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
        this.setState(type.getKey().toString(), applyPhysics);
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
    public boolean isPassable() {
        return !Values.bool(this.query(), "collision", false);
    }

    @Override
    public boolean isCollidable() {
        return Values.bool(this.query(), "collision", false);
    }

    @Override
    public org.bukkit.block.PistonMoveReaction getPistonMoveReaction() {
        return org.bukkit.block.PistonMoveReaction.valueOf(Values.str(this.query(), "piston", "MOVE"));
    }

    /** The legacy data value of the block ({@code CraftMagicNumbers#toLegacyData}). */
    @Override
    @SuppressWarnings("deprecation")
    public byte getData() {
        return org.bukkit.craftbukkit.legacy.CraftLegacy.toLegacyData(this.getBlockData());
    }

    // --- breaking ---------------------------------------------------------------------------

    @Override
    public boolean breakNaturally() {
        return this.breakNaturally(null, false, false, false);
    }

    @Override
    public boolean breakNaturally(final org.bukkit.inventory.@Nullable ItemStack tool) {
        return this.breakNaturally(tool, false, false, false);
    }

    @Override
    public boolean breakNaturally(final boolean triggerEffect, final boolean dropExperience) {
        return this.breakNaturally(null, triggerEffect, dropExperience, false);
    }

    @Override
    public boolean breakNaturally(final org.bukkit.inventory.@Nullable ItemStack tool, final boolean triggerEffect, final boolean dropExperience) {
        return this.breakNaturally(tool, triggerEffect, dropExperience, false);
    }

    /**
     * As {@code CraftBlock#breakNaturally}: what the block yields drops (with a tool, nothing
     * if it is the wrong one; with none, whatever tool the block needs), with its experience if
     * asked, the break effect if asked, and the block is removed. True if it dropped anything.
     */
    @Override
    public boolean breakNaturally(final org.bukkit.inventory.@Nullable ItemStack tool, final boolean triggerEffect, final boolean dropExperience, final boolean forceEffect) {
        this.pending = null;
        return Boolean.TRUE.equals(this.world.server().call("world.break_naturally", Values.of("world", this.world.getName(), "x", this.x, "y", this.y, "z", this.z,
            "tool", tool == null ? null : io.papermc.paperrs.host.item.HostItemStack.toValue(tool), "effect", triggerEffect, "experience", dropExperience)));
    }

    @Override
    public java.util.Collection<org.bukkit.inventory.ItemStack> getDrops() {
        return this.getDrops(null);
    }

    @Override
    public java.util.Collection<org.bukkit.inventory.ItemStack> getDrops(final org.bukkit.inventory.@Nullable ItemStack tool) {
        final java.util.List<org.bukkit.inventory.ItemStack> drops = new java.util.ArrayList<>();
        for (final Object item : Values.list(this.world.server().call("world.block_drops", Values.of("world", this.world.getName(), "x", this.x, "y", this.y, "z", this.z,
            "tool", tool == null ? null : io.papermc.paperrs.host.item.HostItemStack.toValue(tool))))) {
            drops.add(io.papermc.paperrs.host.item.HostItemStack.fromValue(item));
        }
        return drops;
    }

    @Override
    public java.util.Collection<org.bukkit.inventory.ItemStack> getDrops(final org.bukkit.inventory.@Nullable ItemStack tool, final org.bukkit.entity.@Nullable Entity entity) {
        return this.getDrops(tool);
    }

    // --- metadata (kept by the world, as CraftBlock does) ------------------------------------

    @Override
    public void setMetadata(final String key, final org.bukkit.metadata.MetadataValue value) {
        this.world.blockMetadata().setMetadata(this, key, value);
    }

    @Override
    public java.util.List<org.bukkit.metadata.MetadataValue> getMetadata(final String key) {
        return this.world.blockMetadata().getMetadata(this, key);
    }

    @Override
    public boolean hasMetadata(final String key) {
        return this.world.blockMetadata().hasMetadata(this, key);
    }

    @Override
    public void removeMetadata(final String key, final org.bukkit.plugin.Plugin plugin) {
        this.world.blockMetadata().removeMetadata(this, key, plugin);
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
