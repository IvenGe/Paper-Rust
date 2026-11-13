package io.papermc.paperrs.host.registry;

import io.papermc.paperrs.host.Unsupported;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.stub.AbstractEnchantment;
import java.util.List;
import java.util.Map;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import net.kyori.adventure.text.format.TextDecoration;
import org.bukkit.NamespacedKey;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.inventory.ItemStack;
import org.jetbrains.annotations.Nullable;

/**
 * An enchantment. What vanilla's data defines (maximum level, which items it applies to,
 * which enchantments exclude it, curses) comes from the server's game data in WELCOME;
 * without that data these methods fail with a named {@code UnsupportedOperationException}.
 */
public final class HostEnchantment extends AbstractEnchantment {
    private static volatile Map<String, Object> info = Map.of();

    public HostEnchantment(final NamespacedKey key, final int ordinal, final String translationKey) {
        super(key, ordinal, translationKey);
    }

    /** The enchantment table the server sent ({@code api::enchantment_info}). */
    public static void setInfo(final Map<String, Object> table) {
        info = table;
    }

    private Map<String, Object> data() {
        final Object entry = info.get(this.key.toString());
        if (!(entry instanceof Map<?, ?>)) {
            throw Unsupported.feature("Enchantment data for " + this.key + " (the server has no vanilla game data)");
        }
        return Values.map(entry);
    }

    @Override
    public int getMaxLevel() {
        return Values.i32(this.data(), "max_level", 1);
    }

    @Override
    public int getStartLevel() {
        return 1;
    }

    @Override
    public boolean isCursed() {
        return Values.bool(this.data(), "curse", false);
    }

    @Override
    public boolean isTreasure() {
        return Values.bool(this.data(), "treasure", false);
    }

    @Override
    public boolean conflictsWith(final Enchantment other) {
        if (other.equals(this)) {
            return false;
        }
        final List<Object> exclusive = Values.list(this.data().get("exclusive_set"));
        return exclusive.contains(other.getKey().toString());
    }

    @Override
    public boolean canEnchantItem(final @Nullable ItemStack item) {
        if (item == null) {
            return false;
        }
        if (item.getType().getKey().toString().equals("minecraft:book")) {
            return true;
        }
        return Values.list(this.data().get("supported_items")).contains(item.getType().getKey().toString());
    }

    @Override
    public Component displayName(final int level) {
        final Component name = Component.translatable(this.translationKey, this.isCursed() ? NamedTextColor.RED : NamedTextColor.GRAY);
        if (level == 1 && this.getMaxLevel() == 1) {
            return name.decoration(TextDecoration.ITALIC, false);
        }
        return name.append(Component.space()).append(Component.translatable("enchantment.level." + level)).decoration(TextDecoration.ITALIC, false);
    }

    @Override
    public String getName() {
        return this.key.getKey().toUpperCase(java.util.Locale.ROOT);
    }
}
