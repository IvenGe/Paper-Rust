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
import org.bukkit.inventory.meta.BookMeta;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.inventory.meta.MapMeta;
import org.bukkit.inventory.meta.PotionMeta;
import org.bukkit.inventory.meta.SuspiciousStewMeta;
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

    // --- colours: leather (dyed_color), potions (potion_contents) and maps (map_color) -------------

    private boolean is(final Class<?> meta) {
        return meta.isAssignableFrom(this.kind);
    }

    public @Nullable Color getColor() {
        if (this.is(PotionMeta.class)) {
            final Integer rgb = this.potion().color();
            return rgb == null ? null : Color.fromRGB(rgb & 0xFFFFFF);
        }
        if (this.is(MapMeta.class)) {
            final Integer rgb = this.components.int32(MetaCodecs.MAP_COLOR);
            return rgb == null ? null : Color.fromRGB(rgb & 0xFFFFFF);
        }
        final Integer rgb = this.components.int32(ItemComponents.DYED_COLOR);
        return rgb == null ? Color.fromRGB(0xA06540) : Color.fromRGB(rgb & 0xFFFFFF);
    }

    public void setColor(final @Nullable Color color) {
        if (this.is(PotionMeta.class)) {
            final MetaCodecs.PotionContents p = this.potion();
            this.setPotion(new MetaCodecs.PotionContents(p.potion(), color == null ? null : color.asRGB(), p.effects(), p.customName()));
        } else if (this.is(MapMeta.class)) {
            this.components.setInt32(MetaCodecs.MAP_COLOR, color == null ? null : color.asRGB());
        } else {
            this.components.setInt32(ItemComponents.DYED_COLOR, color == null ? null : color.asRGB());
        }
    }

    public boolean hasColor() {
        if (this.is(PotionMeta.class)) {
            return this.potion().color() != null;
        }
        return this.components.has(this.is(MapMeta.class) ? MetaCodecs.MAP_COLOR : ItemComponents.DYED_COLOR);
    }

    public boolean isDyed() {
        return this.components.has(ItemComponents.DYED_COLOR);
    }

    // --- skulls -------------------------------------------------------------------------------

    private MetaCodecs.@Nullable Profile profile() {
        final byte[] raw = this.components.raw(MetaCodecs.PROFILE);
        return raw == null ? null : MetaCodecs.readProfile(raw);
    }

    private void setProfile(final MetaCodecs.@Nullable Profile profile) {
        this.components.setRaw(MetaCodecs.PROFILE, profile == null ? null : MetaCodecs.writeProfile(profile));
    }

    private static MetaCodecs.Profile profileOf(final com.destroystokyo.paper.profile.PlayerProfile profile) {
        final List<MetaCodecs.Property> properties = new ArrayList<>();
        for (final com.destroystokyo.paper.profile.ProfileProperty p : profile.getProperties()) {
            properties.add(new MetaCodecs.Property(p.getName(), p.getValue(), p.getSignature()));
        }
        return MetaCodecs.Profile.of(profile.getName(), profile.getId(), properties);
    }

    public boolean hasOwner() {
        return this.profile() != null;
    }

    public @Nullable String getOwner() {
        final MetaCodecs.Profile p = this.profile();
        return p == null ? null : p.name();
    }

    public boolean setOwner(final @Nullable String name) {
        if (name != null && name.length() > 16) {
            return false;
        }
        final org.bukkit.entity.Player online = name == null ? null : org.bukkit.Bukkit.getPlayerExact(name);
        this.setProfile(name == null ? null : online != null ? profileOf(online.getPlayerProfile()) : MetaCodecs.Profile.of(name, null, List.of()));
        return true;
    }

    public org.bukkit.@Nullable OfflinePlayer getOwningPlayer() {
        final MetaCodecs.Profile p = this.profile();
        if (p == null) {
            return null;
        }
        if (p.id() != null) {
            return org.bukkit.Bukkit.getOfflinePlayer(p.id());
        }
        return p.name() == null ? null : org.bukkit.Bukkit.getOfflinePlayer(p.name());
    }

    public boolean setOwningPlayer(final org.bukkit.@Nullable OfflinePlayer owner) {
        if (owner == null) {
            this.setProfile(null);
        } else if (owner instanceof org.bukkit.entity.Player player) {
            this.setProfile(profileOf(player.getPlayerProfile()));
        } else {
            this.setProfile(MetaCodecs.Profile.of(owner.getName(), owner.getUniqueId(), List.of()));
        }
        return true;
    }

    public com.destroystokyo.paper.profile.@Nullable PlayerProfile getPlayerProfile() {
        final MetaCodecs.Profile p = this.profile();
        if (p == null) {
            return null;
        }
        final io.papermc.paperrs.host.HostPlayerProfile out = new io.papermc.paperrs.host.HostPlayerProfile(p.id(), p.name());
        for (final MetaCodecs.Property property : p.properties()) {
            out.setProperty(new com.destroystokyo.paper.profile.ProfileProperty(property.name(), property.value(), property.signature()));
        }
        return out;
    }

    public void setPlayerProfile(final com.destroystokyo.paper.profile.@Nullable PlayerProfile profile) {
        this.setProfile(profile == null ? null : profileOf(profile));
    }

    @SuppressWarnings("deprecation")
    public org.bukkit.profile.@Nullable PlayerProfile getOwnerProfile() {
        return this.getPlayerProfile();
    }

    @SuppressWarnings("deprecation")
    public void setOwnerProfile(final org.bukkit.profile.@Nullable PlayerProfile profile) {
        if (profile == null) {
            this.setProfile(null);
        } else if (profile instanceof com.destroystokyo.paper.profile.PlayerProfile paper) {
            this.setPlayerProfile(paper);
        } else {
            this.setProfile(MetaCodecs.Profile.of(profile.getName(), profile.getUniqueId(), List.of()));
        }
    }

    public @Nullable NamespacedKey getNoteBlockSound() {
        final String sound = this.components.identifier(MetaCodecs.NOTE_BLOCK_SOUND);
        return sound == null ? null : NamespacedKey.fromString(sound);
    }

    public void setNoteBlockSound(final @Nullable NamespacedKey sound) {
        this.components.setIdentifier(MetaCodecs.NOTE_BLOCK_SOUND, sound == null ? null : sound.toString());
    }

    // --- potions and suspicious stew -----------------------------------------------------------

    private static List<String> registry(final String id) {
        return HostData.get().registry(id);
    }

    private static int effectId(final org.bukkit.potion.PotionEffectType type) {
        final int id = registry("minecraft:mob_effect").indexOf(type.getKey().toString());
        if (id < 0) {
            throw new IllegalArgumentException("Unknown effect " + type.getKey());
        }
        return id;
    }

    private static org.bukkit.potion.@Nullable PotionEffectType effectType(final int id) {
        final List<String> effects = registry("minecraft:mob_effect");
        return id >= 0 && id < effects.size() ? org.bukkit.Registry.MOB_EFFECT.get(NamespacedKey.fromString(effects.get(id))) : null;
    }

    private static org.bukkit.potion.@Nullable PotionEffect effectOf(final MetaCodecs.Effect e) {
        final org.bukkit.potion.PotionEffectType type = effectType(e.type());
        final MetaCodecs.Detail d = e.detail();
        return type == null ? null : new org.bukkit.potion.PotionEffect(type, d.duration(), d.amplifier(), d.ambient(), d.particles(), d.icon());
    }

    private static MetaCodecs.Effect effectOf(final org.bukkit.potion.PotionEffect e) {
        return new MetaCodecs.Effect(effectId(e.getType()), new MetaCodecs.Detail(e.getAmplifier(), e.getDuration(), e.isAmbient(), e.hasParticles(), e.hasIcon(), null));
    }

    private MetaCodecs.PotionContents potion() {
        final byte[] raw = this.components.raw(MetaCodecs.POTION_CONTENTS);
        return raw == null ? MetaCodecs.PotionContents.EMPTY : MetaCodecs.readPotion(raw);
    }

    private void setPotion(final MetaCodecs.PotionContents p) {
        this.components.setRaw(MetaCodecs.POTION_CONTENTS, p.isEmpty() ? null : MetaCodecs.writePotion(p));
    }

    @SuppressWarnings("deprecation")
    public void setBasePotionData(final org.bukkit.potion.@Nullable PotionData data) {
        this.setBasePotionType(data == null ? null : data.getType());
    }

    @SuppressWarnings("deprecation")
    public org.bukkit.potion.@Nullable PotionData getBasePotionData() {
        final org.bukkit.potion.PotionType type = this.getBasePotionType();
        return type == null ? null : new org.bukkit.potion.PotionData(type);
    }

    public org.bukkit.potion.@Nullable PotionType getBasePotionType() {
        final Integer id = this.potion().potion();
        final List<String> potions = registry("minecraft:potion");
        if (id == null || id < 0 || id >= potions.size()) {
            return null;
        }
        final NamespacedKey key = NamespacedKey.fromString(potions.get(id));
        for (final org.bukkit.potion.PotionType type : org.bukkit.potion.PotionType.values()) {
            if (type.getKey().equals(key)) {
                return type;
            }
        }
        return null;
    }

    public void setBasePotionType(final org.bukkit.potion.@Nullable PotionType type) {
        final MetaCodecs.PotionContents p = this.potion();
        final Integer id = type == null ? null : registry("minecraft:potion").indexOf(type.getKey().toString());
        if (id != null && id < 0) {
            throw new IllegalArgumentException("Unknown potion " + type.getKey());
        }
        this.setPotion(new MetaCodecs.PotionContents(id, p.color(), p.effects(), p.customName()));
    }

    public boolean hasBasePotionType() {
        return this.potion().potion() != null;
    }

    public boolean hasCustomEffects() {
        return this.is(SuspiciousStewMeta.class) ? this.components.has(MetaCodecs.STEW_EFFECTS) && !this.stew().isEmpty() : !this.potion().effects().isEmpty();
    }

    public List<org.bukkit.potion.PotionEffect> getCustomEffects() {
        final List<org.bukkit.potion.PotionEffect> out = new ArrayList<>();
        if (this.is(SuspiciousStewMeta.class)) {
            for (final MetaCodecs.StewEffect e : this.stew()) {
                final org.bukkit.potion.PotionEffectType type = effectType(e.type());
                if (type != null) {
                    out.add(new org.bukkit.potion.PotionEffect(type, e.duration(), 0));
                }
            }
            return List.copyOf(out);
        }
        for (final MetaCodecs.Effect e : this.potion().effects()) {
            final org.bukkit.potion.PotionEffect effect = effectOf(e);
            if (effect != null) {
                out.add(effect);
            }
        }
        return List.copyOf(out);
    }

    public List<org.bukkit.potion.PotionEffect> getAllEffects() {
        final List<org.bukkit.potion.PotionEffect> out = new ArrayList<>();
        final org.bukkit.potion.PotionType base = this.getBasePotionType();
        if (base != null) {
            out.addAll(base.getPotionEffects());
        }
        out.addAll(this.getCustomEffects());
        return List.copyOf(out);
    }

    public boolean addCustomEffect(final org.bukkit.potion.PotionEffect effect, final boolean overwrite) {
        if (effect == null) {
            throw new IllegalArgumentException("Potion effect cannot be null");
        }
        if (this.is(SuspiciousStewMeta.class)) {
            final List<MetaCodecs.StewEffect> stew = new ArrayList<>(this.stew());
            final int id = effectId(effect.getType());
            for (int i = 0; i < stew.size(); i++) {
                if (stew.get(i).type() == id) {
                    if (!overwrite || stew.get(i).duration() == effect.getDuration()) {
                        return false;
                    }
                    stew.set(i, new MetaCodecs.StewEffect(id, effect.getDuration()));
                    this.setStew(stew);
                    return true;
                }
            }
            stew.add(new MetaCodecs.StewEffect(id, effect.getDuration()));
            this.setStew(stew);
            return true;
        }
        final MetaCodecs.PotionContents p = this.potion();
        final List<MetaCodecs.Effect> effects = new ArrayList<>(p.effects());
        final MetaCodecs.Effect added = effectOf(effect);
        for (int i = 0; i < effects.size(); i++) {
            if (effects.get(i).type() == added.type()) {
                final MetaCodecs.Detail old = effects.get(i).detail();
                if (!overwrite || old.amplifier() == effect.getAmplifier() && old.duration() == effect.getDuration() && old.ambient() == effect.isAmbient()) {
                    return false;
                }
                effects.set(i, added);
                this.setPotion(new MetaCodecs.PotionContents(p.potion(), p.color(), effects, p.customName()));
                return true;
            }
        }
        effects.add(added);
        this.setPotion(new MetaCodecs.PotionContents(p.potion(), p.color(), effects, p.customName()));
        return true;
    }

    public boolean addCustomEffect(final io.papermc.paper.potion.SuspiciousEffectEntry entry, final boolean overwrite) {
        return this.addCustomEffect(new org.bukkit.potion.PotionEffect(entry.effect(), entry.duration(), 0), overwrite);
    }

    public boolean removeCustomEffect(final org.bukkit.potion.PotionEffectType type) {
        final int id = effectId(type);
        if (this.is(SuspiciousStewMeta.class)) {
            final List<MetaCodecs.StewEffect> stew = new ArrayList<>(this.stew());
            final boolean removed = stew.removeIf(e -> e.type() == id);
            this.setStew(stew);
            return removed;
        }
        final MetaCodecs.PotionContents p = this.potion();
        final List<MetaCodecs.Effect> effects = new ArrayList<>(p.effects());
        final boolean removed = effects.removeIf(e -> e.type() == id);
        this.setPotion(new MetaCodecs.PotionContents(p.potion(), p.color(), effects, p.customName()));
        return removed;
    }

    public boolean hasCustomEffect(final org.bukkit.potion.PotionEffectType type) {
        final int id = effectId(type);
        return this.is(SuspiciousStewMeta.class) ? this.stew().stream().anyMatch(e -> e.type() == id) : this.potion().effects().stream().anyMatch(e -> e.type() == id);
    }

    public boolean setMainEffect(final org.bukkit.potion.PotionEffectType type) {
        final MetaCodecs.PotionContents p = this.potion();
        final List<MetaCodecs.Effect> effects = new ArrayList<>(p.effects());
        final int id = effectId(type);
        int index = -1;
        for (int i = 0; i < effects.size(); i++) {
            if (effects.get(i).type() == id) {
                index = i;
            }
        }
        if (index <= 0) {
            return false;
        }
        java.util.Collections.swap(effects, 0, index);
        this.setPotion(new MetaCodecs.PotionContents(p.potion(), p.color(), effects, p.customName()));
        return true;
    }

    public boolean clearCustomEffects() {
        if (this.is(SuspiciousStewMeta.class)) {
            final boolean had = this.hasCustomEffects();
            this.components.unset(MetaCodecs.STEW_EFFECTS);
            return had;
        }
        final MetaCodecs.PotionContents p = this.potion();
        this.setPotion(new MetaCodecs.PotionContents(p.potion(), p.color(), List.of(), p.customName()));
        return !p.effects().isEmpty();
    }

    /** The potion's colour: its own, or else Paper's mix of its effects' colours (which needs effect colours Paper-RS does not have). */
    public Color computeEffectiveColor() {
        final Color color = this.hasColor() ? this.getColor() : null;
        if (color != null) {
            return color;
        }
        if (this.getAllEffects().isEmpty()) {
            return Color.fromRGB(0x385DC6);
        }
        throw io.papermc.paperrs.host.Unsupported.method("PotionMeta#computeEffectiveColor (for a potion without a custom colour)");
    }

    public boolean hasCustomPotionName() {
        return this.potion().customName() != null;
    }

    public @Nullable String getCustomPotionName() {
        return this.potion().customName();
    }

    public void setCustomPotionName(final @Nullable String name) {
        final MetaCodecs.PotionContents p = this.potion();
        this.setPotion(new MetaCodecs.PotionContents(p.potion(), p.color(), p.effects(), name));
    }

    private List<MetaCodecs.StewEffect> stew() {
        final byte[] raw = this.components.raw(MetaCodecs.STEW_EFFECTS);
        return raw == null ? List.of() : MetaCodecs.readStew(raw);
    }

    private void setStew(final List<MetaCodecs.StewEffect> effects) {
        this.components.setRaw(MetaCodecs.STEW_EFFECTS, effects.isEmpty() ? null : MetaCodecs.writeStew(effects));
    }

    // --- fireworks ------------------------------------------------------------------------------

    private static final org.bukkit.FireworkEffect.Type[] SHAPES = {
        org.bukkit.FireworkEffect.Type.BALL, org.bukkit.FireworkEffect.Type.BALL_LARGE, org.bukkit.FireworkEffect.Type.STAR,
        org.bukkit.FireworkEffect.Type.CREEPER, org.bukkit.FireworkEffect.Type.BURST};

    private static org.bukkit.FireworkEffect effectOf(final MetaCodecs.Explosion e) {
        final org.bukkit.FireworkEffect.Builder b = org.bukkit.FireworkEffect.builder()
            .with(e.shape() >= 0 && e.shape() < SHAPES.length ? SHAPES[e.shape()] : org.bukkit.FireworkEffect.Type.BALL)
            .flicker(e.twinkle()).trail(e.trail());
        for (final int c : e.colors()) {
            b.withColor(Color.fromRGB(c & 0xFFFFFF));
        }
        for (final int c : e.fades()) {
            b.withFade(Color.fromRGB(c & 0xFFFFFF));
        }
        return b.build();
    }

    private static MetaCodecs.Explosion explosionOf(final org.bukkit.FireworkEffect e) {
        final List<Integer> colors = new ArrayList<>();
        for (final Color c : e.getColors()) {
            colors.add(c.asRGB());
        }
        final List<Integer> fades = new ArrayList<>();
        for (final Color c : e.getFadeColors()) {
            fades.add(c.asRGB());
        }
        return new MetaCodecs.Explosion(java.util.Arrays.asList(SHAPES).indexOf(e.getType()), colors, fades, e.hasTrail(), e.hasFlicker());
    }

    private MetaCodecs.Fireworks fireworks() {
        final byte[] raw = this.components.raw(MetaCodecs.FIREWORKS);
        return raw == null ? new MetaCodecs.Fireworks(0, List.of()) : MetaCodecs.readFireworks(raw);
    }

    private void setFireworks(final MetaCodecs.Fireworks f) {
        this.components.setRaw(MetaCodecs.FIREWORKS, MetaCodecs.writeFireworks(f));
    }

    public void addEffect(final org.bukkit.FireworkEffect effect) {
        if (effect == null) {
            throw new IllegalArgumentException("FireworkEffect cannot be null");
        }
        final MetaCodecs.Fireworks f = this.fireworks();
        final List<MetaCodecs.Explosion> explosions = new ArrayList<>(f.explosions());
        explosions.add(explosionOf(effect));
        this.setFireworks(new MetaCodecs.Fireworks(f.flight(), explosions));
    }

    public void addEffects(final org.bukkit.FireworkEffect... effects) {
        for (final org.bukkit.FireworkEffect effect : effects) {
            this.addEffect(effect);
        }
    }

    public void addEffects(final Iterable<org.bukkit.FireworkEffect> effects) {
        for (final org.bukkit.FireworkEffect effect : effects) {
            this.addEffect(effect);
        }
    }

    public List<org.bukkit.FireworkEffect> getEffects() {
        final List<org.bukkit.FireworkEffect> out = new ArrayList<>();
        for (final MetaCodecs.Explosion e : this.fireworks().explosions()) {
            out.add(effectOf(e));
        }
        return List.copyOf(out);
    }

    public int getEffectsSize() {
        return this.fireworks().explosions().size();
    }

    public void removeEffect(final int index) {
        final MetaCodecs.Fireworks f = this.fireworks();
        final List<MetaCodecs.Explosion> explosions = new ArrayList<>(f.explosions());
        explosions.remove(index);
        this.setFireworks(new MetaCodecs.Fireworks(f.flight(), explosions));
    }

    public void clearEffects() {
        this.setFireworks(new MetaCodecs.Fireworks(this.fireworks().flight(), List.of()));
    }

    public boolean hasEffects() {
        return !this.fireworks().explosions().isEmpty();
    }

    public boolean hasPower() {
        return this.components.has(MetaCodecs.FIREWORKS);
    }

    public int getPower() {
        return this.fireworks().flight();
    }

    public void setPower(final int power) {
        if (power < 0 || power > 255) {
            throw new IllegalArgumentException("Power cannot be less than 0 or greater than 255: " + power);
        }
        this.setFireworks(new MetaCodecs.Fireworks(power, this.fireworks().explosions()));
    }

    public void setEffect(final org.bukkit.@Nullable FireworkEffect effect) {
        this.components.setRaw(MetaCodecs.FIREWORK_EXPLOSION, effect == null ? null : MetaCodecs.writeExplosion(explosionOf(effect)));
    }

    public boolean hasEffect() {
        return this.components.has(MetaCodecs.FIREWORK_EXPLOSION);
    }

    public org.bukkit.@Nullable FireworkEffect getEffect() {
        final byte[] raw = this.components.raw(MetaCodecs.FIREWORK_EXPLOSION);
        return raw == null ? null : effectOf(MetaCodecs.readExplosion(raw));
    }

    // --- banners --------------------------------------------------------------------------------

    private List<MetaCodecs.Layer> layers() {
        final byte[] raw = this.components.raw(MetaCodecs.BANNER_PATTERNS);
        return raw == null ? List.of() : MetaCodecs.readBanner(raw);
    }

    private static org.bukkit.block.banner.Pattern patternOf(final MetaCodecs.Layer layer) {
        final List<String> patterns = registry("minecraft:banner_pattern");
        final org.bukkit.block.banner.PatternType type = layer.pattern() >= 0 && layer.pattern() < patterns.size()
            ? org.bukkit.Registry.BANNER_PATTERN.get(NamespacedKey.fromString(patterns.get(layer.pattern()))) : null;
        if (type == null) {
            throw io.papermc.paperrs.host.Unsupported.feature("Banner patterns defined inline (" + layer.assetId() + ")");
        }
        return new org.bukkit.block.banner.Pattern(org.bukkit.DyeColor.values()[layer.color()], type);
    }

    private static MetaCodecs.Layer layerOf(final org.bukkit.block.banner.Pattern pattern) {
        final int id = registry("minecraft:banner_pattern").indexOf(pattern.getPattern().getKey().toString());
        if (id < 0) {
            throw new IllegalArgumentException("Unknown banner pattern " + pattern.getPattern().getKey());
        }
        return new MetaCodecs.Layer(id, null, null, pattern.getColor().ordinal());
    }

    public List<org.bukkit.block.banner.Pattern> getPatterns() {
        final List<org.bukkit.block.banner.Pattern> out = new ArrayList<>();
        for (final MetaCodecs.Layer layer : this.layers()) {
            out.add(patternOf(layer));
        }
        return out;
    }

    public void setPatterns(final List<org.bukkit.block.banner.Pattern> patterns) {
        final List<MetaCodecs.Layer> layers = new ArrayList<>();
        for (final org.bukkit.block.banner.Pattern pattern : patterns) {
            layers.add(layerOf(pattern));
        }
        this.components.setRaw(MetaCodecs.BANNER_PATTERNS, layers.isEmpty() ? null : MetaCodecs.writeBanner(layers));
    }

    public void addPattern(final org.bukkit.block.banner.Pattern pattern) {
        final List<org.bukkit.block.banner.Pattern> patterns = this.getPatterns();
        patterns.add(pattern);
        this.setPatterns(patterns);
    }

    public org.bukkit.block.banner.Pattern getPattern(final int index) {
        return this.getPatterns().get(index);
    }

    public org.bukkit.block.banner.Pattern removePattern(final int index) {
        final List<org.bukkit.block.banner.Pattern> patterns = this.getPatterns();
        final org.bukkit.block.banner.Pattern removed = patterns.remove(index);
        this.setPatterns(patterns);
        return removed;
    }

    public void setPattern(final int index, final org.bukkit.block.banner.Pattern pattern) {
        final List<org.bukkit.block.banner.Pattern> patterns = this.getPatterns();
        patterns.set(index, pattern);
        this.setPatterns(patterns);
    }

    public int numberOfPatterns() {
        return this.layers().size();
    }

    // --- block data (block_state) -------------------------------------------------------------

    public org.bukkit.block.data.BlockData getBlockData(final Material material) {
        final org.bukkit.block.data.BlockData data = io.papermc.paperrs.host.world.HostBlockData.defaults(material);
        final byte[] raw = this.components.raw(MetaCodecs.BLOCK_STATE);
        if (raw == null) {
            return data;
        }
        final StringBuilder props = new StringBuilder();
        MetaCodecs.readBlockState(raw).forEach((k, v) -> {
            // Properties the block does not have are ignored, as in vanilla.
            try {
                io.papermc.paperrs.host.world.HostBlockData.parse(material, "[" + k + "=" + v + "]");
                props.append(props.isEmpty() ? "" : ",").append(k).append('=').append(v);
            } catch (IllegalArgumentException e) {
                // skip
            }
        });
        return props.isEmpty() ? data : io.papermc.paperrs.host.world.HostBlockData.parse(material, "[" + props + "]");
    }

    public void setBlockData(final org.bukkit.block.data.@Nullable BlockData data) {
        if (data == null) {
            this.components.unset(MetaCodecs.BLOCK_STATE);
            return;
        }
        // Like CraftBlockData#toStates: only the properties the data was given.
        final String state = io.papermc.paperrs.host.world.HostBlockData.impl(data).getAsString(true);
        final Map<String, String> properties = new LinkedHashMap<>();
        final int bracket = state.indexOf('[');
        if (bracket >= 0) {
            for (final String pair : state.substring(bracket + 1, state.length() - 1).split(",")) {
                final int eq = pair.indexOf('=');
                properties.put(pair.substring(0, eq), pair.substring(eq + 1));
            }
        }
        this.components.setRaw(MetaCodecs.BLOCK_STATE, properties.isEmpty() ? null : MetaCodecs.writeBlockState(properties));
    }

    // --- books ----------------------------------------------------------------------------------

    private boolean written() {
        return this.is(BookMeta.class);
    }

    private ItemComponents.WrittenBook book() {
        final ItemComponents.WrittenBook book = this.components.writtenBook();
        return book != null ? book : new ItemComponents.WrittenBook("", "", 0, List.of(), false);
    }

    private void setBook(final ItemComponents.WrittenBook book) {
        this.components.setWrittenBook(book);
    }

    /** The pages as text components (a writable book's are plain text). */
    public List<Component> pages() {
        final List<Component> out = new ArrayList<>();
        if (this.written()) {
            for (final String json : this.book().pages()) {
                out.add(Components.fromJson(json));
            }
        } else {
            for (final String page : this.writablePages()) {
                out.add(Component.text(page));
            }
        }
        return out;
    }

    private List<String> writablePages() {
        final byte[] raw = this.components.raw(MetaCodecs.WRITABLE_BOOK);
        return raw == null ? List.of() : MetaCodecs.readPages(raw);
    }

    private void setPagesText(final List<Component> pages) {
        if (pages.size() > 100) {
            throw new IllegalArgumentException("Book cannot have more than 100 pages");
        }
        if (this.written()) {
            final ItemComponents.WrittenBook b = this.book();
            final List<String> json = new ArrayList<>();
            for (final Component page : pages) {
                json.add(Components.toJson(page));
            }
            this.setBook(new ItemComponents.WrittenBook(b.title(), b.author(), b.generation(), json, b.resolved()));
        } else {
            final List<String> plain = new ArrayList<>();
            for (final Component page : pages) {
                plain.add(Components.toLegacy(page));
            }
            this.components.setRaw(MetaCodecs.WRITABLE_BOOK, plain.isEmpty() ? null : MetaCodecs.writePages(plain));
        }
    }

    public boolean hasPages() {
        return !this.pages().isEmpty();
    }

    public int getPageCount() {
        return this.pages().size();
    }

    /** {@code getPage} counts from 1. */
    public String getPage(final int page) {
        final List<Component> pages = this.pages();
        if (page < 1 || page > pages.size()) {
            throw new IllegalArgumentException("Invalid page number " + page + "/" + pages.size());
        }
        return this.written() ? Components.toLegacy(pages.get(page - 1)) : this.writablePages().get(page - 1);
    }

    public void setPage(final int page, final String data) {
        this.page(page, this.written() ? Components.legacy(data) : Component.text(data));
    }

    public List<String> getPages() {
        if (!this.written()) {
            return List.copyOf(this.writablePages());
        }
        final List<String> out = new ArrayList<>();
        for (final Component page : this.pages()) {
            out.add(Components.toLegacy(page));
        }
        return List.copyOf(out);
    }

    public void setPages(final List<String> pages) {
        final List<Component> out = new ArrayList<>();
        for (final String page : pages) {
            out.add(this.written() ? Components.legacy(page) : Component.text(page));
        }
        this.setPagesText(out);
    }

    public void setPages(final String... pages) {
        this.setPages(List.of(pages));
    }

    public void addPage(final String... pages) {
        final List<String> all = new ArrayList<>(this.getPages());
        all.addAll(List.of(pages));
        this.setPages(all);
    }

    public Component page(final int page) {
        final List<Component> pages = this.pages();
        if (page < 1 || page > pages.size()) {
            throw new IllegalArgumentException("Invalid page number " + page + "/" + pages.size());
        }
        return pages.get(page - 1);
    }

    public void page(final int page, final Component data) {
        final List<Component> pages = new ArrayList<>(this.pages());
        if (page < 1 || page > pages.size()) {
            throw new IllegalArgumentException("Invalid page number " + page + "/" + pages.size());
        }
        pages.set(page - 1, data == null ? Component.empty() : data);
        this.setPagesText(pages);
    }

    public void addPages(final Component... pages) {
        final List<Component> all = new ArrayList<>(this.pages());
        all.addAll(List.of(pages));
        this.setPagesText(all);
    }

    public BookMeta pages(final List<Component> pages) {
        this.setPagesText(pages);
        return (BookMeta) this.self;
    }

    public boolean hasTitle() {
        return this.components.writtenBook() != null && !this.book().title().isEmpty();
    }

    public @Nullable String getTitle() {
        return this.hasTitle() ? this.book().title() : null;
    }

    public boolean setTitle(final @Nullable String title) {
        if (title != null && title.length() > 32) {
            return false;
        }
        final ItemComponents.WrittenBook b = this.book();
        this.setBook(new ItemComponents.WrittenBook(title == null ? "" : title, b.author(), b.generation(), b.pages(), b.resolved()));
        return true;
    }

    public @Nullable Component title() {
        return this.hasTitle() ? Components.legacy(this.book().title()) : null;
    }

    public BookMeta title(final @Nullable Component title) {
        this.setTitle(title == null ? null : Components.toLegacy(title));
        return (BookMeta) this.self;
    }

    public boolean hasAuthor() {
        return this.components.writtenBook() != null && !this.book().author().isEmpty();
    }

    public @Nullable String getAuthor() {
        return this.hasAuthor() ? this.book().author() : null;
    }

    public void setAuthor(final @Nullable String author) {
        final ItemComponents.WrittenBook b = this.book();
        this.setBook(new ItemComponents.WrittenBook(b.title(), author == null ? "" : author, b.generation(), b.pages(), b.resolved()));
    }

    public @Nullable Component author() {
        return this.hasAuthor() ? Components.legacy(this.book().author()) : null;
    }

    public BookMeta author(final @Nullable Component author) {
        this.setAuthor(author == null ? null : Components.toLegacy(author));
        return (BookMeta) this.self;
    }

    public boolean hasGeneration() {
        return this.components.writtenBook() != null;
    }

    public BookMeta.@Nullable Generation getGeneration() {
        return this.hasGeneration() ? BookMeta.Generation.values()[Math.min(this.book().generation(), BookMeta.Generation.values().length - 1)] : null;
    }

    public void setGeneration(final BookMeta.@Nullable Generation generation) {
        final ItemComponents.WrittenBook b = this.book();
        this.setBook(new ItemComponents.WrittenBook(b.title(), b.author(), generation == null ? 0 : generation.ordinal(), b.pages(), b.resolved()));
    }

    // --- maps and ominous bottles ---------------------------------------------------------------

    public boolean hasMapId() {
        return this.components.has(MetaCodecs.MAP_ID);
    }

    public int getMapId() {
        final Integer id = this.components.varInt(MetaCodecs.MAP_ID);
        return id == null ? -1 : id;
    }

    public void setMapId(final int id) {
        this.components.setVarInt(MetaCodecs.MAP_ID, id);
    }

    public boolean hasAmplifier() {
        return this.components.has(MetaCodecs.OMINOUS_BOTTLE_AMPLIFIER);
    }

    public int getAmplifier() {
        final Integer amplifier = this.components.varInt(MetaCodecs.OMINOUS_BOTTLE_AMPLIFIER);
        return amplifier == null ? 0 : amplifier;
    }

    public void setAmplifier(final int amplifier) {
        if (amplifier < 0 || amplifier > 4) {
            throw new IllegalArgumentException("Amplifier must be in range [0, 4] but is " + amplifier);
        }
        this.components.setVarInt(MetaCodecs.OMINOUS_BOTTLE_AMPLIFIER, amplifier);
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
