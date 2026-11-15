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
 * values are plain types, custom model data and lore. Components with other API value types
 * (enchantments, tools, ...) are not supported yet and fail naming the component.
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
            case ItemComponents.CUSTOM_MODEL_DATA -> {
                final ItemComponents.CustomModelData data = components.customModelData();
                yield data == null ? null : HostCustomModelData.of(data);
            }
            case ItemComponents.LORE -> {
                final java.util.List<String> lines = components.loreJson();
                yield lines == null ? null : new HostItemLore(lines.stream().map(Components::fromJson).toList());
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
            case ItemComponents.CUSTOM_MODEL_DATA -> {
                final io.papermc.paper.datacomponent.item.CustomModelData data = (io.papermc.paper.datacomponent.item.CustomModelData) value;
                components.setCustomModelData(new ItemComponents.CustomModelData(data.floats(), data.flags(), data.strings(), data.colors().stream().map(org.bukkit.Color::asRGB).toList()));
            }
            case ItemComponents.LORE -> components.setLoreJson(((io.papermc.paper.datacomponent.item.ItemLore) value).lines().stream().map(Components::toJson).toList());
            default -> throw unsupported(type);
        }
    }
}
