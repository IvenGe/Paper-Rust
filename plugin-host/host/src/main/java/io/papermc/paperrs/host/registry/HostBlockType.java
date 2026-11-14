package io.papermc.paperrs.host.registry;

import io.papermc.paperrs.host.data.HostData;
import io.papermc.paperrs.host.stub.AbstractBlockType;
import io.papermc.paperrs.host.world.HostBlockData;
import java.util.Collection;
import java.util.function.Consumer;
import java.util.Locale;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.block.data.BlockData;
import org.bukkit.inventory.ItemType;
import org.jetbrains.annotations.Nullable;

/** A block type, described by the Rust server's block table. */
public final class HostBlockType extends AbstractBlockType<BlockData> {
    private final HostData.@Nullable BlockInfo info;

    public HostBlockType(final NamespacedKey key, final int ordinal, final String translationKey) {
        super(key, ordinal, translationKey);
        this.info = HostData.get().block(key.toString());
    }

    @Override
    public Material asMaterial() {
        return Material.matchMaterial(this.key.toString());
    }

    @Override
    public boolean hasItemType() {
        return this.info != null && this.info.item() != null;
    }

    @Override
    public @Nullable ItemType getItemType() {
        return this.hasItemType() ? Registry.ITEM.get(NamespacedKey.fromString(this.info.item())) : null;
    }

    @Override
    public boolean isAir() {
        final String path = this.key.getKey();
        return path.equals("air") || path.equals("cave_air") || path.equals("void_air");
    }

    @Override
    public float getHardness() {
        return this.info == null ? 0 : this.info.hardness();
    }

    @Override
    public float getBlastResistance() {
        return this.info == null ? 0 : this.info.blastResistance();
    }

    @Override
    public float getSlipperiness() {
        final String path = this.key.getKey();
        if (path.equals("blue_ice")) {
            return 0.989f;
        }
        if (path.equals("ice") || path.equals("packed_ice") || path.equals("frosted_ice")) {
            return 0.98f;
        }
        return path.equals("slime_block") ? 0.8f : 0.6f;
    }

    /** Blocks the player can walk through; the server has no shape data, so this goes by name. */
    private boolean passable() {
        if (this.isAir()) {
            return true;
        }
        final String path = this.key.getKey().toLowerCase(Locale.ROOT);
        for (String part : new String[] {"water", "lava", "torch", "sign", "button", "pressure_plate", "rail", "flower", "sapling",
            "short_grass", "tall_grass", "fern", "vine", "banner", "carpet", "mushroom", "dead_bush", "seagrass", "kelp", "coral_fan",
            "redstone_wire", "tripwire", "lever", "fire", "sugar_cane", "wheat", "carrots", "potatoes", "beetroots", "nether_wart",
            "cobweb", "light", "structure_void", "moss_carpet", "snow", "pink_petals", "leaf_litter", "bush", "roots", "sprouts"}) {
            if (path.contains(part)) {
                return !path.endsWith("_block") && !path.equals("snow_block");
            }
        }
        return false;
    }

    @Override
    public boolean isSolid() {
        return !this.passable();
    }

    @Override
    public boolean hasCollision() {
        return !this.passable();
    }

    @Override
    public boolean isOccluding() {
        if (this.passable()) {
            return false;
        }
        final String path = this.key.getKey();
        return !(path.contains("glass") || path.contains("leaves") || path.contains("slab") || path.contains("stairs")
            || path.contains("fence") || path.contains("wall") || path.contains("door") || path.contains("pane") || path.contains("bars"));
    }

    @Override
    public boolean hasGravity() {
        final String path = this.key.getKey();
        return path.equals("sand") || path.equals("red_sand") || path.equals("gravel") || path.endsWith("concrete_powder")
            || path.equals("anvil") || path.endsWith("_anvil") || path.equals("dragon_egg") || path.equals("suspicious_sand")
            || path.equals("suspicious_gravel") || path.equals("pointed_dripstone") || path.equals("scaffolding");
    }

    @Override
    public boolean isFlammable() {
        final String path = this.key.getKey();
        return path.contains("planks") || path.contains("log") || path.contains("wood") || path.contains("leaves") || path.contains("wool");
    }

    @Override
    public boolean isBurnable() {
        return this.isFlammable();
    }

    @Override
    public boolean isInteractable() {
        final String path = this.key.getKey();
        return path.contains("door") || path.contains("button") || path.contains("lever") || path.contains("chest")
            || path.contains("crafting_table") || path.contains("furnace") || path.contains("bed") || path.contains("anvil");
    }

    @Override
    public boolean isEnabledByFeature(final org.bukkit.World world) {
        return true;
    }

    @Override
    public String getTranslationKey() {
        return this.translationKey;
    }

    @Override
    public BlockData createBlockData() {
        return HostBlockData.defaults(this.key.toString());
    }

    @Override
    public BlockData createBlockData(final @Nullable String data) {
        return HostBlockData.parse(this.asMaterial(), data);
    }

    @Override
    public BlockData createBlockData(final @Nullable Consumer<? super BlockData> consumer) {
        final BlockData data = this.createBlockData();
        if (consumer != null) {
            consumer.accept(data);
        }
        return data;
    }

    @Override
    public Collection<BlockData> createBlockDataStates() {
        return HostBlockData.allStates(this.key.toString());
    }

    @Override
    @SuppressWarnings("unchecked")
    public Class<BlockData> getBlockDataClass() {
        return (Class<BlockData>) HostBlockData.dataClass(this.key.toString());
    }

    @Override
    public org.bukkit.block.BlockType.Typed<BlockData> typed() {
        return this;
    }

    @Override
    @SuppressWarnings("unchecked")
    public <B extends BlockData> org.bukkit.block.BlockType.Typed<B> typed(final Class<B> blockDataType) {
        return (org.bukkit.block.BlockType.Typed<B>) (Object) this;
    }
}
