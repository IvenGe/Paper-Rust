package io.papermc.paperrs.host.item;

import io.papermc.paperrs.host.stub.AbstractItemFactory;
import java.util.function.UnaryOperator;
import net.kyori.adventure.key.Key;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.event.HoverEvent;
import org.bukkit.Color;
import org.bukkit.Material;
import org.bukkit.entity.EntityType;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.meta.ItemMeta;
import org.jetbrains.annotations.Nullable;

/** {@code Bukkit.getItemFactory()}: item metas and stack helpers. */
public final class HostItemFactory extends AbstractItemFactory {
    public static final HostItemFactory INSTANCE = new HostItemFactory();

    private HostItemFactory() {
    }

    @Override
    public @Nullable ItemMeta getItemMeta(final Material material) {
        if (material.isAir() || !material.isItem()) {
            return null;
        }
        return HostItemMeta.create(material, new ItemComponents());
    }

    @Override
    public boolean isApplicable(final @Nullable ItemMeta meta, final @Nullable ItemStack stack) {
        return stack != null && this.isApplicable(meta, stack.getType());
    }

    @Override
    public boolean isApplicable(final @Nullable ItemMeta meta, final @Nullable Material material) {
        if (meta == null || material == null || material.isAir()) {
            return false;
        }
        return HostItemMeta.implOf(meta).kind() == MetaKinds.of(material);
    }

    @Override
    public boolean equals(final @Nullable ItemMeta meta1, final @Nullable ItemMeta meta2) {
        final ItemComponents a = meta1 == null ? new ItemComponents() : HostItemMeta.implOf(meta1).toComponents();
        final ItemComponents b = meta2 == null ? new ItemComponents() : HostItemMeta.implOf(meta2).toComponents();
        return a.equals(b);
    }

    @Override
    public @Nullable ItemMeta asMetaFor(final ItemMeta meta, final ItemStack stack) {
        return this.asMetaFor(meta, stack.getType());
    }

    @Override
    public @Nullable ItemMeta asMetaFor(final ItemMeta meta, final Material material) {
        if (material.isAir()) {
            return null;
        }
        return HostItemMeta.implOf(meta).asMetaFor(material);
    }

    @Override
    public Color getDefaultLeatherColor() {
        return Color.fromRGB(0xA06540);
    }

    @Override
    public @Nullable Material getSpawnEgg(final EntityType type) {
        if (type.getKey() == null) {
            return null;
        }
        return Material.matchMaterial(type.getKey().getNamespace() + ":" + type.getKey().getKey() + "_spawn_egg");
    }

    @Override
    public Component displayName(final ItemStack stack) {
        return stack.effectiveName();
    }

    @Override
    @SuppressWarnings("deprecation")
    public String getI18NDisplayName(final ItemStack stack) {
        return io.papermc.paperrs.host.Components.toLegacy(stack.effectiveName());
    }

    @Override
    public ItemStack ensureServerConversions(final ItemStack stack) {
        return stack;
    }

    @Override
    public HoverEvent<HoverEvent.ShowItem> asHoverEvent(final ItemStack stack, final UnaryOperator<HoverEvent.ShowItem> op) {
        return HoverEvent.showItem(op.apply(HoverEvent.ShowItem.showItem(Key.key(stack.getType().getKey().toString()), stack.getAmount())));
    }
}
