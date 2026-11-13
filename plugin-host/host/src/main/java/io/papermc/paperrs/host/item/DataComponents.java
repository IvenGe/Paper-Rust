package io.papermc.paperrs.host.item;

import io.papermc.paper.datacomponent.DataComponentType;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.Unsupported;
import net.kyori.adventure.key.Key;
import net.kyori.adventure.text.Component;
import org.bukkit.inventory.ItemRarity;
import org.jetbrains.annotations.Nullable;

/**
 * Paper's data component API ({@code ItemStack#getData} and friends) for the components whose
 * values are plain types. Components with their own API value types (lore, enchantments,
 * tools, ...) are not supported yet and fail naming the component.
 */
final class DataComponents {
    private DataComponents() {
    }

    private static Unsupported unsupported(final DataComponentType type) {
        return Unsupported.feature("The data component " + type.getKey().asString() + " in the data component API");
    }

    @SuppressWarnings("unchecked")
    static <T> @Nullable T get(final ItemComponents components, final DataComponentType.Valued<T> type) {
        final String name = type.getKey().asString();
        return (T) switch (name) {
            case ItemComponents.MAX_STACK_SIZE, ItemComponents.MAX_DAMAGE, ItemComponents.DAMAGE, ItemComponents.REPAIR_COST -> components.varInt(name);
            case ItemComponents.CUSTOM_NAME -> Components.fromJson(components.nameJson());
            case ItemComponents.ITEM_NAME -> Components.fromJson(components.itemNameJson());
            case ItemComponents.GLINT_OVERRIDE -> components.bool(name);
            case ItemComponents.ITEM_MODEL, ItemComponents.TOOLTIP_STYLE -> {
                final String id = components.identifier(name);
                yield id == null ? null : Key.key(id);
            }
            case ItemComponents.RARITY -> {
                final Integer id = components.varInt(name);
                yield id == null ? null : ItemRarity.values()[id];
            }
            default -> throw unsupported(type);
        };
    }

    static <T> void set(final ItemComponents components, final DataComponentType.Valued<T> type, final T value) {
        final String name = type.getKey().asString();
        switch (name) {
            case ItemComponents.MAX_STACK_SIZE, ItemComponents.MAX_DAMAGE, ItemComponents.DAMAGE, ItemComponents.REPAIR_COST -> components.setVarInt(name, (Integer) value);
            case ItemComponents.CUSTOM_NAME -> components.setNameJson(Components.toJson((Component) value));
            case ItemComponents.ITEM_NAME -> components.setItemNameJson(Components.toJson((Component) value));
            case ItemComponents.GLINT_OVERRIDE -> components.setBool(name, (Boolean) value);
            case ItemComponents.ITEM_MODEL, ItemComponents.TOOLTIP_STYLE -> components.setIdentifier(name, ((Key) value).asString());
            case ItemComponents.RARITY -> components.setVarInt(name, ((ItemRarity) value).ordinal());
            default -> throw unsupported(type);
        }
    }
}
