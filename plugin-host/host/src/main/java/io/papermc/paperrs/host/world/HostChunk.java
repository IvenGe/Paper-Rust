package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.Unsupported;
import io.papermc.paperrs.host.bridge.Values;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.function.Predicate;
import org.bukkit.Chunk;
import org.bukkit.ChunkSnapshot;
import org.bukkit.World;
import org.bukkit.block.Block;
import org.bukkit.block.BlockState;
import org.bukkit.entity.Entity;
import org.bukkit.entity.Player;
import org.bukkit.persistence.PersistentDataContainer;
import org.bukkit.plugin.Plugin;

/**
 * A chunk of a world, by position (as {@code CraftChunk}, which also only holds the world and
 * coordinates): what it holds is read from the world when asked.
 */
public final class HostChunk implements Chunk {
    private final HostWorld world;
    private final int x;
    private final int z;

    HostChunk(final HostWorld world, final int x, final int z) {
        this.world = world;
        this.x = x;
        this.z = z;
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
    public World getWorld() {
        return this.world;
    }

    @Override
    public Block getBlock(final int x, final int y, final int z) {
        return this.world.getBlockAt((this.x << 4) | (x & 15), y, (this.z << 4) | (z & 15));
    }

    @Override
    public ChunkSnapshot getChunkSnapshot(final boolean includeMaxBlockY, final boolean includeBiome, final boolean includeBiomeTempRain, final boolean includeBeehives) {
        throw Unsupported.method("Chunk#getChunkSnapshot");
    }

    @Override
    public boolean isEntitiesLoaded() {
        return this.isLoaded();
    }

    @Override
    public Entity[] getEntities() {
        final List<Entity> out = new ArrayList<>();
        for (final Entity e : this.world.getEntities()) {
            if (e.getLocation().getBlockX() >> 4 == this.x && e.getLocation().getBlockZ() >> 4 == this.z) {
                out.add(e);
            }
        }
        return out.toArray(Entity[]::new);
    }

    @Override
    public BlockState[] getTileEntities(final boolean useSnapshot) {
        return this.getTileEntities(block -> true, useSnapshot).toArray(BlockState[]::new);
    }

    @Override
    public Collection<BlockState> getTileEntities(final Predicate<? super Block> blockPredicate, final boolean useSnapshot) {
        final List<BlockState> out = new ArrayList<>();
        for (final Object pos : Values.list(this.world.server().call("world.chunk_block_entities", Values.of("world", this.world.getName(), "x", this.x, "z", this.z)))) {
            final List<Object> p = Values.list(pos);
            final Block block = this.world.getBlockAt(((Number) p.get(0)).intValue(), ((Number) p.get(1)).intValue(), ((Number) p.get(2)).intValue());
            if (blockPredicate.test(block)) {
                out.add(block.getState(useSnapshot));
            }
        }
        return out;
    }

    @Override
    public boolean isGenerated() {
        return this.world.isChunkGenerated(this.x, this.z);
    }

    @Override
    public boolean isLoaded() {
        return this.world.isChunkLoaded(this.x, this.z);
    }

    @Override
    public boolean load(final boolean generate) {
        return this.world.loadChunk(this.x, this.z, generate);
    }

    @Override
    public boolean load() {
        return this.world.loadChunk(this.x, this.z, true);
    }

    @Override
    public boolean unload(final boolean save) {
        return this.world.unloadChunk(this.x, this.z, save);
    }

    @Override
    public boolean unload() {
        return this.world.unloadChunk(this.x, this.z);
    }

    /** {@code WorldgenRandom.seedSlimeChunk(x, z, seed, 987234911L).nextInt(10) == 0}. */
    @Override
    public boolean isSlimeChunk() {
        final long seed = this.world.getSeed() + (long) (this.x * this.x * 4987142) + (long) (this.x * 5947611)
            + (long) (this.z * this.z) * 4392871L + (long) (this.z * 389711) ^ 987234911L;
        return new java.util.Random(seed).nextInt(10) == 0;
    }

    @Override
    public boolean isForceLoaded() {
        return this.world.isChunkForceLoaded(this.x, this.z);
    }

    @Override
    public void setForceLoaded(final boolean forced) {
        this.world.setChunkForceLoaded(this.x, this.z, forced);
    }

    @Override
    public boolean addPluginChunkTicket(final Plugin plugin) {
        return this.world.addPluginChunkTicket(this.x, this.z, plugin);
    }

    @Override
    public boolean removePluginChunkTicket(final Plugin plugin) {
        return this.world.removePluginChunkTicket(this.x, this.z, plugin);
    }

    @Override
    public Collection<Plugin> getPluginChunkTickets() {
        return this.world.getPluginChunkTickets(this.x, this.z);
    }

    @Override
    public long getInhabitedTime() {
        return 0;
    }

    @Override
    public void setInhabitedTime(final long ticks) {
        com.google.common.base.Preconditions.checkArgument(ticks >= 0, "ticks cannot be negative");
    }

    @Override
    public boolean contains(final org.bukkit.block.data.BlockData block) {
        throw Unsupported.method("Chunk#contains");
    }

    @Override
    public boolean contains(final org.bukkit.block.Biome biome) {
        throw Unsupported.method("Chunk#contains");
    }

    @Override
    public LoadLevel getLoadLevel() {
        if (!this.isLoaded()) {
            return LoadLevel.UNLOADED;
        }
        return Boolean.TRUE.equals(this.world.server().call("world.chunk_in_use", Values.of("world", this.world.getName(), "x", this.x, "z", this.z))) ? LoadLevel.ENTITY_TICKING : LoadLevel.BORDER;
    }

    @Override
    public Collection<org.bukkit.generator.structure.GeneratedStructure> getStructures() {
        throw Unsupported.method("Chunk#getStructures");
    }

    @Override
    public Collection<org.bukkit.generator.structure.GeneratedStructure> getStructures(final org.bukkit.generator.structure.Structure structure) {
        throw Unsupported.method("Chunk#getStructures");
    }

    @Override
    public Collection<Player> getPlayersSeeingChunk() {
        return this.world.getPlayersSeeingChunk(this.x, this.z);
    }

    @Override
    public PersistentDataContainer getPersistentDataContainer() {
        throw Unsupported.method("Chunk#getPersistentDataContainer");
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostChunk c && c.x == this.x && c.z == this.z && c.world.equals(this.world);
    }

    @Override
    public int hashCode() {
        return (this.world.hashCode() * 31 + this.x) * 31 + this.z;
    }

    @Override
    public String toString() {
        return "CraftChunk{x=" + this.x + "z=" + this.z + "}";
    }
}
