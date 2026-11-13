package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.HostProxies;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.inventory.Inventories;
import java.util.Map;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.World;
import org.bukkit.block.Barrel;
import org.bukkit.block.BlastFurnace;
import org.bukkit.block.Block;
import org.bukkit.block.BlockState;
import org.bukkit.block.Chest;
import org.bukkit.block.EnderChest;
import org.bukkit.block.Furnace;
import org.bukkit.block.Smoker;
import org.bukkit.event.inventory.InventoryType;
import org.bukkit.inventory.Inventory;
import org.jetbrains.annotations.Nullable;

/**
 * {@code Block#getState()}: the block at a position, implementing the block's state interface
 * for the containers Paper-RS supports ({@code Chest}, {@code Barrel}, {@code Furnace},
 * {@code Smoker}, {@code BlastFurnace}, {@code EnderChest}); other blocks are plain
 * {@code BlockState}s. Container inventories are live (the server keeps the items in the
 * block entity), so {@code update()} has nothing to write.
 */
public final class HostBlockState implements HostProxies.Backed {
    private final HostBlock block;
    private final Material type;
    private Object self;

    private HostBlockState(final HostBlock block, final Material type) {
        this.block = block;
        this.type = type;
    }

    static BlockState of(final HostBlock block) {
        final Material type = block.getType();
        final Class<? extends BlockState> iface = switch (type.getKey().getKey()) {
            case "chest", "trapped_chest" -> Chest.class;
            case "barrel" -> Barrel.class;
            case "furnace" -> Furnace.class;
            case "smoker" -> Smoker.class;
            case "blast_furnace" -> BlastFurnace.class;
            case "ender_chest" -> EnderChest.class;
            default -> BlockState.class;
        };
        return HostProxies.create(new HostBlockState(block, type), iface);
    }

    @Override
    public void bind(final Object self) {
        this.self = self;
    }

    public Block getBlock() {
        return this.block;
    }

    public Material getType() {
        return this.type;
    }

    public World getWorld() {
        return this.block.getWorld();
    }

    public int getX() {
        return this.block.getX();
    }

    public int getY() {
        return this.block.getY();
    }

    public int getZ() {
        return this.block.getZ();
    }

    public Location getLocation() {
        return this.block.getLocation();
    }

    public @Nullable Location getLocation(final @Nullable Location loc) {
        return this.block.getLocation(loc);
    }

    public byte getLightLevel() {
        return this.block.getLightLevel();
    }

    public boolean isPlaced() {
        return true;
    }

    public boolean update() {
        return true;
    }

    public boolean update(final boolean force) {
        return true;
    }

    public boolean update(final boolean force, final boolean applyPhysics) {
        return true;
    }

    private @Nullable Map<String, Object> container() {
        final HostWorld world = (HostWorld) this.block.getWorld();
        final Object info = world.server().call("inventory.of_block", Values.of("world", world.getName(), "x", this.getX(), "y", this.getY(), "z", this.getZ()));
        return info instanceof Map<?, ?> m ? Values.map(m) : null;
    }

    /** The container's inventory (both halves for a double chest). */
    public Inventory getInventory() {
        final Map<String, Object> info = this.container();
        if (info == null) {
            throw new IllegalStateException("The block at " + this.getLocation() + " is no longer a container");
        }
        return Inventories.block(((HostWorld) this.block.getWorld()).server(), Values.map(info.get("inventory")),
            Inventories.type(Values.str(info, "type", "CHEST")), Values.i32(info, "size", 27));
    }

    public Inventory getSnapshotInventory() {
        return this.getBlockInventory();
    }

    /** This block's own slots (one half of a double chest). */
    public Inventory getBlockInventory() {
        final HostWorld world = (HostWorld) this.block.getWorld();
        final InventoryType type = switch (this.type.getKey().getKey()) {
            case "barrel" -> InventoryType.BARREL;
            case "furnace" -> InventoryType.FURNACE;
            case "smoker" -> InventoryType.SMOKER;
            case "blast_furnace" -> InventoryType.BLAST_FURNACE;
            default -> InventoryType.CHEST;
        };
        return Inventories.block(world.server(), Values.of("kind", "block", "world", world.getName(), "x", this.getX(), "y", this.getY(), "z", this.getZ()),
            type, type == InventoryType.CHEST || type == InventoryType.BARREL ? 27 : 3);
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostBlockState other && other.block.equals(this.block);
    }

    @Override
    public int hashCode() {
        return this.block.hashCode();
    }

    @Override
    public String toString() {
        return "CraftBlockState{" + this.type + " at " + this.block + "}";
    }
}
