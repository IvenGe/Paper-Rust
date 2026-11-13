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
    public org.bukkit.inventory.ItemStack createItemStack() {
        return this.createItemStack(1);
    }

    @Override
    public org.bukkit.inventory.ItemStack createItemStack(final int amount) {
        final Material material = this.asMaterial();
        if (material == null || material.isAir()) {
            return io.papermc.paperrs.host.item.HostItemStack.empty();
        }
        return new io.papermc.paperrs.host.item.HostItemStack(material, amount);
    }

    @Override
    public org.bukkit.inventory.ItemStack createItemStack(final java.util.function.Consumer<? super ItemMeta> metaConfigurator) {
        return this.createItemStack(1, metaConfigurator);
    }

    @Override
    public org.bukkit.inventory.ItemStack createItemStack(final int amount, final java.util.function.@org.jetbrains.annotations.Nullable Consumer<? super ItemMeta> metaConfigurator) {
        final org.bukkit.inventory.ItemStack stack = this.createItemStack(amount);
        if (metaConfigurator != null) {
            stack.editMeta(metaConfigurator);
        }
        return stack;
    }

    @Override
    public Class<ItemMeta> getItemMetaClass() {
        final Material material = this.asMaterial();
        @SuppressWarnings("unchecked") final Class<ItemMeta> kind = (Class<ItemMeta>) (Class<?>) (material == null
            ? ItemMeta.class : io.papermc.paperrs.host.item.MetaKinds.of(material).getInterfaces()[0]);
        return kind;
    }

    @Override
    public short getMaxDurability() {
        return (short) (this.info == null ? 0 : this.info.maxDamage());
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
