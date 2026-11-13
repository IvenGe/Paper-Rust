package io.papermc.paperrs.host.item;

import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.HostProxies;
import io.papermc.paperrs.host.bridge.Codec;
import io.papermc.paperrs.host.data.HostData;
import java.util.ArrayList;
import java.util.Base64;
import java.util.EnumSet;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import net.kyori.adventure.text.Component;
import org.bukkit.Color;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.inventory.ItemFlag;
import org.bukkit.inventory.ItemRarity;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.persistence.PersistentDataContainer;
import org.jetbrains.annotations.Nullable;

/**
 * The implementation behind every item meta (Paper's {@code CraftMetaItem} and friends): a
 * copy of a stack's component patch with typed accessors. Plugins see it through a proxy
 * implementing the meta interfaces of the item ({@link MetaKinds}); methods not defined here
 * throw a named {@code UnsupportedOperationException}.
 */
public final class HostItemMeta implements HostProxies.Backed {
    private static final Map<ItemFlag, List<String>> FLAG_COMPONENTS = new LinkedHashMap<>();

    static {
        FLAG_COMPONENTS.put(ItemFlag.HIDE_ENCHANTS, List.of(ItemComponents.ENCHANTMENTS));
        FLAG_COMPONENTS.put(ItemFlag.HIDE_ATTRIBUTES, List.of("minecraft:attribute_modifiers"));
        FLAG_COMPONENTS.put(ItemFlag.HIDE_UNBREAKABLE, List.of(ItemComponents.UNBREAKABLE));
        FLAG_COMPONENTS.put(ItemFlag.HIDE_DESTROYS, List.of("minecraft:can_break"));
        FLAG_COMPONENTS.put(ItemFlag.HIDE_PLACED_ON, List.of("minecraft:can_place_on"));
        FLAG_COMPONENTS.put(ItemFlag.HIDE_ADDITIONAL_TOOLTIP, List.of("minecraft:banner_patterns", "minecraft:bees", "minecraft:block_entity_data",
            "minecraft:block_state", "minecraft:bundle_contents", "minecraft:charged_projectiles", "minecraft:container", "minecraft:container_loot",
            "minecraft:firework_explosion", "minecraft:fireworks", "minecraft:instrument", "minecraft:map_id", "minecraft:painting/variant",
            "minecraft:pot_decorations", "minecraft:potion_contents", "minecraft:tropical_fish/pattern", "minecraft:written_book_content"));
        FLAG_COMPONENTS.put(ItemFlag.HIDE_DYE, List.of(ItemComponents.DYED_COLOR));
        FLAG_COMPONENTS.put(ItemFlag.HIDE_ARMOR_TRIM, List.of("minecraft:trim"));
        FLAG_COMPONENTS.put(ItemFlag.HIDE_STORED_ENCHANTS, List.of(ItemComponents.STORED_ENCHANTMENTS));
    }

    private final Material type;
    private final ItemComponents components;
    private final Class<? extends ItemMeta> kind;
    private @Nullable HostPersistentDataContainer pdc;
    private Object self;

    private HostItemMeta(final Material type, final ItemComponents components, final Class<? extends ItemMeta> kind) {
        this.type = type;
        this.components = components;
        this.kind = kind;
    }

    /** A meta for an item of {@code type} holding {@code components} (owned by the meta). */
    public static ItemMeta create(final Material type, final ItemComponents components) {
        final Class<? extends ItemMeta> kind = MetaKinds.of(type);
        return HostProxies.create(new HostItemMeta(type, components, kind), kind);
    }

    /** The implementation behind a meta plugins hand back. */
    public static HostItemMeta implOf(final ItemMeta meta) {
        final HostItemMeta impl = HostProxies.implOf(meta, HostItemMeta.class);
        if (impl == null) {
            throw new IllegalArgumentException("not an item meta made by Paper-RS: " + meta.getClass().getName());
        }
        return impl;
    }

    @Override
    public void bind(final Object self) {
        this.self = self;
    }

    public Material type() {
        return this.type;
    }

    public Class<? extends ItemMeta> kind() {
        return this.kind;
    }

    /** The components with the persistent data written back. */
    public ItemComponents toComponents() {
        final ItemComponents out = this.components.copy();
        if (this.pdc != null) {
            storePdc(out, this.pdc);
        }
        return out;
    }

    /** The same meta for another item (the meta interfaces may change). */
    public ItemMeta asMetaFor(final Material material) {
        return create(material, this.toComponents());
    }

    // --- persistent data ------------------------------------------------------------------

    static HostPersistentDataContainer pdcOf(final ItemComponents components) {
        final Object values = components.customData().get(HostPersistentDataContainer.BUKKIT_KEY);
        return new HostPersistentDataContainer(values instanceof Nbt.Compound c ? c.copy() : new Nbt.Compound());
    }

    static void storePdc(final ItemComponents components, final HostPersistentDataContainer pdc) {
        final Nbt.Compound data = components.customData();
        if (pdc.isEmpty()) {
            data.remove(HostPersistentDataContainer.BUKKIT_KEY);
        } else {
            data.put(HostPersistentDataContainer.BUKKIT_KEY, pdc.tags().copy());
        }
        components.setCustomData(data);
    }

    public PersistentDataContainer getPersistentDataContainer() {
        if (this.pdc == null) {
            this.pdc = pdcOf(this.components);
        }
        return this.pdc;
    }

    // --- names and lore -----------------------------------------------------------------------

    public boolean hasCustomName() {
        return this.components.has(ItemComponents.CUSTOM_NAME);
    }

    public @Nullable Component customName() {
        return Components.fromJson(this.components.nameJson());
    }

    public void customName(final @Nullable Component name) {
        this.components.setNameJson(Components.toJson(name));
    }

    public boolean hasDisplayName() {
        return this.hasCustomName();
    }

    public @Nullable Component displayName() {
        return this.customName();
    }

    public void displayName(final @Nullable Component name) {
        this.customName(name);
    }

    public String getDisplayName() {
        final Component name = this.customName();
        return name == null ? "" : Components.toLegacy(name);
    }

    public void setDisplayName(final @Nullable String name) {
        this.customName(name == null || name.isEmpty() ? null : Components.legacy(name));
    }

    public boolean hasItemName() {
        return this.components.has(ItemComponents.ITEM_NAME);
    }

    public @Nullable Component itemName() {
        return Components.fromJson(this.components.itemNameJson());
    }

    public void itemName(final @Nullable Component name) {
        this.components.setItemNameJson(Components.toJson(name));
    }

    public String getItemName() {
        final Component name = this.itemName();
        return name == null ? "" : Components.toLegacy(name);
    }

    public void setItemName(final @Nullable String name) {
        this.itemName(name == null || name.isEmpty() ? null : Components.legacy(name));
    }

    public boolean hasLore() {
        return this.components.has(ItemComponents.LORE);
    }

    public @Nullable List<Component> lore() {
        final List<String> json = this.components.loreJson();
        if (json == null) {
            return null;
        }
        final List<Component> out = new ArrayList<>();
        for (String line : json) {
            out.add(Components.fromJson(line));
        }
        return out;
    }

    public void lore(final @Nullable List<? extends Component> lore) {
        if (lore == null || lore.isEmpty()) {
            this.components.setLoreJson(null);
            return;
        }
        final List<String> json = new ArrayList<>();
        for (Component line : lore) {
            json.add(Components.toJson(line == null ? Component.empty() : line));
        }
        this.components.setLoreJson(json);
    }

    public @Nullable List<String> getLore() {
        final List<Component> lore = this.lore();
        if (lore == null) {
            return null;
        }
        final List<String> out = new ArrayList<>();
        for (Component line : lore) {
            out.add(Components.toLegacy(line));
        }
        return out;
    }

    public void setLore(final @Nullable List<String> lore) {
        if (lore == null) {
            this.lore(null);
            return;
        }
        final List<Component> out = new ArrayList<>();
        for (String line : lore) {
            out.add(Components.legacy(line == null ? "" : line));
        }
        this.lore(out);
    }

    // --- enchantments ---------------------------------------------------------------------

    public boolean hasEnchants() {
        return !this.components.enchantments(ItemComponents.ENCHANTMENTS).isEmpty();
    }

    public boolean hasEnchant(final Enchantment enchantment) {
        return this.getEnchantLevel(enchantment) > 0;
    }

    public int getEnchantLevel(final Enchantment enchantment) {
        return HostItemStack.enchantments(this.components, ItemComponents.ENCHANTMENTS).getOrDefault(enchantment, 0);
    }

    public Map<Enchantment, Integer> getEnchants() {
        return HostItemStack.enchantments(this.components, ItemComponents.ENCHANTMENTS);
    }

    public boolean addEnchant(final Enchantment enchantment, final int level, final boolean ignoreLevelRestriction) {
        if (!ignoreLevelRestriction && (level < enchantment.getStartLevel() || level > enchantment.getMaxLevel())) {
            return false;
        }
        final int old = this.getEnchantLevel(enchantment);
        HostItemStack.putEnchantment(this.components, ItemComponents.ENCHANTMENTS, enchantment, level);
        return old != level;
    }

    public boolean removeEnchant(final Enchantment enchantment) {
        return HostItemStack.removeEnchantment(this.components, ItemComponents.ENCHANTMENTS, enchantment) > 0;
    }

    public void removeEnchantments() {
        this.components.unset(ItemComponents.ENCHANTMENTS);
    }

    public boolean hasConflictingEnchant(final Enchantment enchantment) {
        return conflicts(this.getEnchants(), enchantment);
    }

    private static boolean conflicts(final Map<Enchantment, Integer> present, final Enchantment enchantment) {
        for (Enchantment other : present.keySet()) {
            if (!other.equals(enchantment) && other.conflictsWith(enchantment)) {
                return true;
            }
        }
        return false;
    }

    // stored enchantments (enchanted books)

    public boolean hasStoredEnchants() {
        return !this.components.enchantments(ItemComponents.STORED_ENCHANTMENTS).isEmpty();
    }

    public boolean hasStoredEnchant(final Enchantment enchantment) {
        return this.getStoredEnchantLevel(enchantment) > 0;
    }

    public int getStoredEnchantLevel(final Enchantment enchantment) {
        return HostItemStack.enchantments(this.components, ItemComponents.STORED_ENCHANTMENTS).getOrDefault(enchantment, 0);
    }

    public Map<Enchantment, Integer> getStoredEnchants() {
        return HostItemStack.enchantments(this.components, ItemComponents.STORED_ENCHANTMENTS);
    }

    public boolean addStoredEnchant(final Enchantment enchantment, final int level, final boolean ignoreLevelRestriction) {
        if (!ignoreLevelRestriction && (level < enchantment.getStartLevel() || level > enchantment.getMaxLevel())) {
            return false;
        }
        final int old = this.getStoredEnchantLevel(enchantment);
        HostItemStack.putEnchantment(this.components, ItemComponents.STORED_ENCHANTMENTS, enchantment, level);
        return old != level;
    }

    public boolean removeStoredEnchant(final Enchantment enchantment) {
        return HostItemStack.removeEnchantment(this.components, ItemComponents.STORED_ENCHANTMENTS, enchantment) > 0;
    }

    public boolean hasConflictingStoredEnchant(final Enchantment enchantment) {
        return conflicts(this.getStoredEnchants(), enchantment);
    }

    // --- flags and tooltip -------------------------------------------------------------------

    public void addItemFlags(final ItemFlag... flags) {
        final ItemComponents.TooltipDisplay display = this.components.tooltipDisplay();
        final Set<String> hidden = new LinkedHashSet<>(display.hidden());
        for (ItemFlag flag : flags) {
            hidden.addAll(FLAG_COMPONENTS.getOrDefault(flag, List.of()));
        }
        this.components.setTooltipDisplay(new ItemComponents.TooltipDisplay(display.hideTooltip(), hidden));
    }

    public void removeItemFlags(final ItemFlag... flags) {
        final ItemComponents.TooltipDisplay display = this.components.tooltipDisplay();
        final Set<String> hidden = new LinkedHashSet<>(display.hidden());
        for (ItemFlag flag : flags) {
            FLAG_COMPONENTS.getOrDefault(flag, List.of()).forEach(hidden::remove);
        }
        this.components.setTooltipDisplay(new ItemComponents.TooltipDisplay(display.hideTooltip(), hidden));
    }

    public Set<ItemFlag> getItemFlags() {
        final Set<String> hidden = this.components.tooltipDisplay().hidden();
        final Set<ItemFlag> out = EnumSet.noneOf(ItemFlag.class);
        FLAG_COMPONENTS.forEach((flag, types) -> {
            if (hidden.containsAll(types)) {
                out.add(flag);
            }
        });
        return out;
    }

    public boolean hasItemFlag(final ItemFlag flag) {
        return this.components.tooltipDisplay().hidden().containsAll(FLAG_COMPONENTS.getOrDefault(flag, List.of()));
    }

    public boolean isHideTooltip() {
        return this.components.tooltipDisplay().hideTooltip();
    }

    public void setHideTooltip(final boolean hide) {
        this.components.setTooltipDisplay(new ItemComponents.TooltipDisplay(hide, this.components.tooltipDisplay().hidden()));
    }

    public boolean hasTooltipStyle() {
        return this.components.has(ItemComponents.TOOLTIP_STYLE);
    }

    public @Nullable NamespacedKey getTooltipStyle() {
        final String id = this.components.identifier(ItemComponents.TOOLTIP_STYLE);
        return id == null ? null : NamespacedKey.fromString(id);
    }

    public void setTooltipStyle(final @Nullable NamespacedKey key) {
        this.components.setIdentifier(ItemComponents.TOOLTIP_STYLE, key == null ? null : key.toString());
    }

    public boolean hasItemModel() {
        return this.components.has(ItemComponents.ITEM_MODEL);
    }

    public @Nullable NamespacedKey getItemModel() {
        final String id = this.components.identifier(ItemComponents.ITEM_MODEL);
        return id == null ? null : NamespacedKey.fromString(id);
    }

    public void setItemModel(final @Nullable NamespacedKey key) {
        this.components.setIdentifier(ItemComponents.ITEM_MODEL, key == null ? null : key.toString());
    }

    // --- simple components --------------------------------------------------------------------

    public boolean isUnbreakable() {
        return this.components.has(ItemComponents.UNBREAKABLE);
    }

    public void setUnbreakable(final boolean unbreakable) {
        this.components.setUnit(ItemComponents.UNBREAKABLE, unbreakable);
    }

    public boolean isGlider() {
        return this.components.has(ItemComponents.GLIDER);
    }

    public void setGlider(final boolean glider) {
        this.components.setUnit(ItemComponents.GLIDER, glider);
    }

    public boolean hasEnchantmentGlintOverride() {
        return this.components.has(ItemComponents.GLINT_OVERRIDE);
    }

    public @Nullable Boolean getEnchantmentGlintOverride() {
        return this.components.bool(ItemComponents.GLINT_OVERRIDE);
    }

    public void setEnchantmentGlintOverride(final @Nullable Boolean override) {
        this.components.setBool(ItemComponents.GLINT_OVERRIDE, override);
    }

    public boolean hasMaxStackSize() {
        return this.components.has(ItemComponents.MAX_STACK_SIZE);
    }

    public int getMaxStackSize() {
        final Integer size = this.components.varInt(ItemComponents.MAX_STACK_SIZE);
        if (size != null) {
            return size;
        }
        final HostData.ItemInfo info = HostData.get().item(this.type.getKey().toString());
        return info == null ? 64 : info.maxStackSize();
    }

    public void setMaxStackSize(final @Nullable Integer size) {
        if (size != null && (size < 1 || size > 99)) {
            throw new IllegalArgumentException("Max stack size must be between 1 and 99");
        }
        this.components.setVarInt(ItemComponents.MAX_STACK_SIZE, size);
    }

    public boolean hasRarity() {
        return this.components.has(ItemComponents.RARITY);
    }

    public ItemRarity getRarity() {
        final Integer id = this.components.varInt(ItemComponents.RARITY);
        if (id == null) {
            throw new IllegalStateException("We don't have rarity! Check hasRarity first!");
        }
        return ItemRarity.values()[id];
    }

    public void setRarity(final @Nullable ItemRarity rarity) {
        this.components.setVarInt(ItemComponents.RARITY, rarity == null ? null : rarity.ordinal());
    }

    public boolean hasEnchantable() {
        return this.components.has(ItemComponents.ENCHANTABLE);
    }

    public int getEnchantable() {
        final Integer value = this.components.varInt(ItemComponents.ENCHANTABLE);
        if (value == null) {
            throw new IllegalStateException("We don't have enchantable! Check hasEnchantable first!");
        }
        return value;
    }

    public void setEnchantable(final @Nullable Integer value) {
        this.components.setVarInt(ItemComponents.ENCHANTABLE, value);
    }

    public boolean hasCustomModelData() {
        final ItemComponents.CustomModelData data = this.components.customModelData();
        return data != null && !data.floats().isEmpty();
    }

    public int getCustomModelData() {
        final ItemComponents.CustomModelData data = this.components.customModelData();
        if (data == null || data.floats().isEmpty()) {
            throw new IllegalStateException("We don't have CustomModelData! Check hasCustomModelData first!");
        }
        return data.floats().getFirst().intValue();
    }

    public void setCustomModelData(final @Nullable Integer value) {
        this.components.setCustomModelData(value == null ? null : new ItemComponents.CustomModelData(List.of(value.floatValue()), List.of(), List.of(), List.of()));
    }

    public boolean hasCustomModelDataComponent() {
        return this.components.has(ItemComponents.CUSTOM_MODEL_DATA);
    }

    public boolean hasAttributeModifiers() {
        return this.components.has("minecraft:attribute_modifiers");
    }

    // --- Damageable and Repairable ------------------------------------------------------------

    public boolean hasDamage() {
        return this.getDamage() > 0;
    }

    public int getDamage() {
        final Integer damage = this.components.varInt(ItemComponents.DAMAGE);
        return damage == null ? 0 : damage;
    }

    public void setDamage(final int damage) {
        this.components.setVarInt(ItemComponents.DAMAGE, Math.max(0, damage));
    }

    public boolean hasDamageValue() {
        return this.components.has(ItemComponents.DAMAGE);
    }

    public void resetDamage() {
        this.components.unset(ItemComponents.DAMAGE);
    }

    public boolean hasMaxDamage() {
        return this.components.has(ItemComponents.MAX_DAMAGE);
    }

    public int getMaxDamage() {
        final Integer max = this.components.varInt(ItemComponents.MAX_DAMAGE);
        if (max == null) {
            throw new IllegalStateException("We don't have max_damage! Check hasMaxDamage first!");
        }
        return max;
    }

    public void setMaxDamage(final @Nullable Integer max) {
        this.components.setVarInt(ItemComponents.MAX_DAMAGE, max);
    }

    public boolean hasRepairCost() {
        return this.components.has(ItemComponents.REPAIR_COST);
    }

    public int getRepairCost() {
        final Integer cost = this.components.varInt(ItemComponents.REPAIR_COST);
        return cost == null ? 0 : cost;
    }

    public void setRepairCost(final int cost) {
        this.components.setVarInt(ItemComponents.REPAIR_COST, cost == 0 ? null : cost);
    }

    // --- leather colour -----------------------------------------------------------------------

    public Color getColor() {
        final Integer rgb = this.components.int32(ItemComponents.DYED_COLOR);
        return rgb == null ? Color.fromRGB(0xA06540) : Color.fromRGB(rgb & 0xFFFFFF);
    }

    public void setColor(final @Nullable Color color) {
        this.components.setInt32(ItemComponents.DYED_COLOR, color == null ? null : color.asRGB());
    }

    public boolean isDyed() {
        return this.components.has(ItemComponents.DYED_COLOR);
    }

    // --- object -------------------------------------------------------------------------------

    @SuppressWarnings("deprecation")
    public void setVersion(final int version) {
        // Item data is always current on Paper-RS.
    }

    public boolean hasBlockData() {
        return this.components.has("minecraft:block_state");
    }

    public Map<String, Object> serialize() {
        final Map<String, Object> out = new LinkedHashMap<>();
        out.put("meta-type", "PAPER_RS");
        out.put("type", this.type.getKey().toString());
        final Map<String, Object> value = new LinkedHashMap<>();
        this.toComponents().toValue(value);
        out.put("paper-rs:components", Base64.getEncoder().encodeToString(Codec.encodeValue(value)));
        return out;
    }

    @Override
    public Object clone() {
        return HostProxies.create(new HostItemMeta(this.type, this.toComponents(), this.kind), this.kind);
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostItemMeta other && this.toComponents().equals(other.toComponents());
    }

    @Override
    public int hashCode() {
        return this.toComponents().hashCode();
    }

    @Override
    public String toString() {
        return "ItemMeta{" + this.type.getKey() + ", " + this.toComponents().addedTypes() + "}";
    }
}
