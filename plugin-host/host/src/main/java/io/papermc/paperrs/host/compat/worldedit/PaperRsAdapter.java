package io.papermc.paperrs.host.compat.worldedit;

import com.sk89q.worldedit.blocks.BaseItemStack;
import com.sk89q.worldedit.bukkit.BukkitAdapter;
import com.sk89q.worldedit.bukkit.adapter.BukkitImplAdapter;
import com.sk89q.worldedit.entity.BaseEntity;
import com.sk89q.worldedit.internal.wna.WorldNativeAccess;
import com.sk89q.worldedit.math.BlockVector3;
import com.sk89q.worldedit.registry.state.BooleanProperty;
import com.sk89q.worldedit.registry.state.DirectionalProperty;
import com.sk89q.worldedit.registry.state.EnumProperty;
import com.sk89q.worldedit.registry.state.IntegerProperty;
import com.sk89q.worldedit.registry.state.Property;
import com.sk89q.worldedit.util.Direction;
import com.sk89q.worldedit.util.SideEffect;
import com.sk89q.worldedit.util.formatting.text.Component;
import com.sk89q.worldedit.util.formatting.text.TranslatableComponent;
import com.sk89q.worldedit.world.DataFixer;
import com.sk89q.worldedit.world.biome.BiomeType;
import com.sk89q.worldedit.world.biome.BiomeTypes;
import com.sk89q.worldedit.world.block.BaseBlock;
import com.sk89q.worldedit.world.block.BlockState;
import com.sk89q.worldedit.world.block.BlockType;
import com.sk89q.worldedit.world.entity.EntityType;
import com.sk89q.worldedit.world.entity.EntityTypes;
import com.sk89q.worldedit.world.item.ItemType;
import com.sk89q.worldedit.world.item.ItemTypes;
import com.sk89q.worldedit.world.registry.BlockMaterial;
import io.papermc.paperrs.host.data.HostData;
import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.EnumSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.OptionalInt;
import java.util.Set;
import java.util.TreeMap;
import org.bukkit.Bukkit;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.World;
import org.bukkit.block.data.BlockData;
import org.bukkit.entity.Entity;
import org.bukkit.entity.Player;
import org.bukkit.inventory.ItemStack;
import org.enginehub.linbus.stream.LinBinaryIO;
import org.enginehub.linbus.tree.LinCompoundTag;
import org.enginehub.linbus.tree.LinRootEntry;
import org.jetbrains.annotations.Nullable;

/**
 * WorldEdit's access to the world on Paper-RS. WorldEdit ships an adapter per Minecraft version
 * that reaches into the server's internal classes, and without one it cannot work (block
 * properties come only from the adapter). Paper-RS has no such classes, so the plugin host
 * gives WorldEdit this adapter (see {@code WorldEditCompat}), built on the server's block data
 * and the plugin bridge.
 *
 * <p>The host defines it in WorldEdit's own class loader, in WorldEdit's package, and names it
 * with WorldEdit's {@code worldedit.bukkit.adapter} property.
 */
public final class PaperRsAdapter implements BukkitImplAdapter {
    static org.bukkit.craftbukkit.CraftServer server() {
        return (org.bukkit.craftbukkit.CraftServer) Bukkit.getServer();
    }

    static Map<String, Object> args(final World world, final int x, final int y, final int z) {
        final Map<String, Object> args = new LinkedHashMap<>();
        args.put("world", world.getName());
        args.put("x", x);
        args.put("y", y);
        args.put("z", z);
        return args;
    }

    @Override
    public @Nullable DataFixer getDataFixer() {
        return null;
    }

    /** WorldEdit's block state for each state id of the server, made once. */
    private static final BlockState[] STATES = new BlockState[io.papermc.paperrs.host.data.HostData.get().stateCount()];

    private static BlockState state(final int id) {
        BlockState state = STATES[id];
        if (state == null) {
            state = BukkitAdapter.adapt(io.papermc.paperrs.host.world.HostBlockData.ofState(io.papermc.paperrs.host.data.HostData.get().stateToString(id)));
            STATES[id] = state;
        }
        return state;
    }

    @Override
    public BlockState getBlock(final Location location) {
        if (location.getWorld() instanceof io.papermc.paperrs.host.world.HostWorld world) {
            return state(world.stateId(location.getBlockX(), location.getBlockY(), location.getBlockZ()));
        }
        return BukkitAdapter.adapt(location.getBlock().getBlockData());
    }

    @Override
    public BaseBlock getFullBlock(final Location location) {
        final BlockState state = this.getBlock(location);
        final World world = location.getWorld();
        final int x = location.getBlockX(), y = location.getBlockY(), z = location.getBlockZ();
        // Most blocks have no block entity: only ask where there may be one.
        if (world instanceof io.papermc.paperrs.host.world.HostWorld host && !host.mayHaveBlockEntity(x, y, z)) {
            return state.toBaseBlock();
        }
        final LinCompoundTag nbt = blockEntity(world, x, y, z);
        return nbt == null ? state.toBaseBlock() : state.toBaseBlock(nbt);
    }

    /** The block entity at a block as saved, or null. */
    static @Nullable LinCompoundTag blockEntity(final World world, final int x, final int y, final int z) {
        // A read: it changes nothing, so the host's caches stay good.
        final Object bytes = server().read("world.block_entity", args(world, x, y, z));
        if (!(bytes instanceof byte[] data)) {
            return null;
        }
        try {
            return LinBinaryIO.readUsing(new DataInputStream(new ByteArrayInputStream(data)), LinRootEntry::readFrom).value();
        } catch (IOException e) {
            throw new IllegalStateException("Could not read the block entity at " + x + "," + y + "," + z, e);
        }
    }

    @Override
    public WorldNativeAccess<?, ?, ?> createWorldNativeAccess(final World world) {
        return new PaperRsWorldNativeAccess(world);
    }

    @Override
    public @Nullable BaseEntity getEntity(final Entity entity) {
        if (entity instanceof Player) {
            return null;
        }
        final EntityType type = EntityTypes.get(entity.getType().getKey().toString());
        return type == null ? null : new BaseEntity(type);
    }

    @Override
    public @Nullable Entity createEntity(final Location location, final BaseEntity state) {
        final org.bukkit.entity.EntityType type = org.bukkit.Registry.ENTITY_TYPE.get(org.bukkit.NamespacedKey.fromString(state.getType().id()));
        if (type == null || !type.isSpawnable()) {
            return null;
        }
        try {
            return location.getWorld().spawnEntity(location, type);
        } catch (RuntimeException e) {
            return null;
        }
    }

    @Override
    public Component getRichBlockName(final BlockType blockType) {
        return TranslatableComponent.of(Material.matchMaterial(blockType.id()) instanceof Material m && m.isBlock() ? m.getBlockTranslationKey() : "block." + blockType.id().replace(':', '.'));
    }

    @Override
    public Component getRichItemName(final ItemType itemType) {
        final Material material = Material.matchMaterial(itemType.id());
        return TranslatableComponent.of(material != null && material.isItem() ? material.getItemTranslationKey() : "item." + itemType.id().replace(':', '.'));
    }

    @Override
    public Component getRichItemName(final BaseItemStack itemStack) {
        return this.getRichItemName(itemStack.getType());
    }

    @Override
    public BlockMaterial getBlockMaterial(final BlockType blockType) {
        return PaperRsBlockMaterial.of(blockType.id());
    }

    /** The block's properties as WorldEdit's types, in the block's order. */
    @Override
    public Map<String, ? extends Property<?>> getProperties(final BlockType blockType) {
        final Map<String, Property<?>> out = new TreeMap<>();
        final HostData.BlockInfo info = HostData.get().block(blockType.id());
        if (info == null) {
            return out;
        }
        for (final HostData.Property p : info.properties()) {
            out.put(p.name(), property(p.name(), p.values()));
        }
        return out;
    }

    private static final Set<String> DIRECTIONS = Set.of("north", "south", "east", "west", "up", "down");

    private static Property<?> property(final String name, final List<String> values) {
        if (values.size() == 2 && values.contains("true") && values.contains("false")) {
            final List<Boolean> booleans = new ArrayList<>();
            for (final String v : values) {
                booleans.add(Boolean.parseBoolean(v));
            }
            return new BooleanProperty(name, booleans);
        }
        if (values.stream().allMatch(v -> v.chars().allMatch(Character::isDigit))) {
            final List<Integer> ints = new ArrayList<>();
            for (final String v : values) {
                ints.add(Integer.parseInt(v));
            }
            return new IntegerProperty(name, ints);
        }
        if (DIRECTIONS.containsAll(values)) {
            final List<Direction> directions = new ArrayList<>();
            for (final String v : values) {
                directions.add(Direction.valueOf(v.toUpperCase(Locale.ROOT)));
            }
            return new DirectionalProperty(name, directions);
        }
        return new EnumProperty(name, List.copyOf(values));
    }

    @Override
    public void sendFakeNBT(final Player player, final BlockVector3 pos, final LinCompoundTag nbtData) {
        // The structure block preview of selections is not shown.
    }

    @Override
    public void sendFakeOP(final Player player) {
        // Paper-RS does not send fake operator levels.
    }

    @Override
    public boolean canPlaceAt(final World world, final BlockVector3 position, final BlockState blockState) {
        return true;
    }

    @Override
    public ItemStack adapt(final BaseItemStack item) {
        final Material material = Material.matchMaterial(item.getType().id());
        return new ItemStack(material == null ? Material.AIR : material, item.getAmount());
    }

    @Override
    public BaseItemStack adapt(final ItemStack itemStack) {
        final ItemType type = ItemTypes.get(itemStack.getType().getKey().toString());
        return new BaseItemStack(type == null ? ItemTypes.AIR : type, itemStack.getAmount());
    }

    @Override
    public Set<SideEffect> getSupportedSideEffects() {
        // Neighbours and updates come with the server's physics; light follows changes anyway.
        return EnumSet.of(SideEffect.NEIGHBORS, SideEffect.UPDATE, SideEffect.LIGHTING, SideEffect.NETWORK);
    }

    @Override
    public OptionalInt getInternalBlockStateId(final BlockData data) {
        final int id = HostData.get().parseState(data.getAsString());
        return id < 0 ? OptionalInt.empty() : OptionalInt.of(id);
    }

    @Override
    public OptionalInt getInternalBlockStateId(final BlockState state) {
        final int id = HostData.get().parseState(state.getAsString());
        return id < 0 ? OptionalInt.empty() : OptionalInt.of(id);
    }

    @Override
    public boolean clearContainerBlockContents(final World world, final BlockVector3 pt) {
        // WorldEdit asks before every change; containers are block entities, and most blocks
        // have none.
        if (world instanceof io.papermc.paperrs.host.world.HostWorld host && !host.mayHaveBlockEntity(pt.x(), pt.y(), pt.z())) {
            return false;
        }
        final org.bukkit.block.BlockState state = world.getBlockAt(pt.x(), pt.y(), pt.z()).getState();
        if (!(state instanceof org.bukkit.inventory.InventoryHolder holder)) {
            return false;
        }
        org.bukkit.inventory.Inventory inventory = holder.getInventory();
        if (holder instanceof org.bukkit.block.Chest chest) {
            inventory = chest.getBlockInventory();
        }
        inventory.clear();
        return true;
    }

    @Override
    public BiomeType getBiome(final Location location) {
        final BiomeType biome = BiomeTypes.get(location.getBlock().getBiome().getKey().toString());
        return biome == null ? BiomeTypes.PLAINS : biome;
    }
}
