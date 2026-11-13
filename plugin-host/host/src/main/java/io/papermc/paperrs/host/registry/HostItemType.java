package io.papermc.paperrs.host.registry;

import io.papermc.paperrs.host.data.HostData;
import io.papermc.paperrs.host.stub.AbstractItemType;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.block.BlockType;
import org.bukkit.inventory.meta.ItemMeta;
import org.jetbrains.annotations.Nullable;

/** An item type, described by the Rust server's item table. */
public final class HostItemType extends AbstractItemType<ItemMeta> {
    private final HostData.@Nullable ItemInfo info;

    public HostItemType(final NamespacedKey key, final int ordinal, final String translationKey) {
        super(key, ordinal, translationKey);
        this.info = HostData.get().item(key.toString());
    }

    @Override
    public Material asMaterial() {
        return Material.matchMaterial(this.key.toString());
    }

    @Override
    public boolean hasBlockType() {
        return this.info != null && this.info.block() != null;
    }

    @Override
    public @Nullable BlockType getBlockType() {
        return this.hasBlockType() ? Registry.BLOCK.get(NamespacedKey.fromString(this.info.block())) : null;
    }

    @Override
    public int getMaxStackSize() {
        return this.info == null ? 64 : this.info.maxStackSize();
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
    public org.bukkit.inventory.ItemType.Typed<ItemMeta> typed() {
        return this;
    }

    @Override
    @SuppressWarnings("unchecked")
    public <M extends ItemMeta> org.bukkit.inventory.ItemType.Typed<M> typed(final Class<M> itemMetaType) {
        return (org.bukkit.inventory.ItemType.Typed<M>) (Object) this;
    }
}
