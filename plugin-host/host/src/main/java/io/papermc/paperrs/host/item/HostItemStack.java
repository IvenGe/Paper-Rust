package io.papermc.paperrs.host.item;

import com.google.common.base.Preconditions;
import io.papermc.paper.datacomponent.DataComponentType;
import io.papermc.paper.persistence.PersistentDataContainerView;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.Unsupported;
import io.papermc.paperrs.host.bridge.Codec;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.data.HostData;
import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.util.Base64;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.function.Consumer;
import java.util.function.Predicate;
import net.kyori.adventure.text.Component;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.persistence.PersistentDataContainer;
import org.jetbrains.annotations.Nullable;

/**
 * The item stack the API creates and the server's stacks become on the plugin host (Paper's
 * {@code CraftItemStack}): an item type, a count and the data component patch. Plain
 * {@code new ItemStack(...)} objects delegate to one of these.
 */
public final class HostItemStack extends ItemStack {
    private static final byte[] BYTES_MAGIC = {'P', 'R', 'S', 1};

    private Material type;
    private int amount;
    private ItemComponents components;
    /**
     * Where changes go when this stack mirrors an inventory slot (as CraftBukkit's mirror
     * stacks do: {@code getItemInMainHand().setAmount(...)} changes the player's item).
     */
    private @Nullable Consumer<HostItemStack> mirror;

    public HostItemStack(final Material type, final int amount) {
        this(type, amount, new ItemComponents());
    }

    private HostItemStack(final Material type, final int amount, final ItemComponents components) {
        this.type = type;
        this.amount = amount;
        this.components = components;
    }

    public static HostItemStack empty() {
        return new HostItemStack(Material.AIR, 0);
    }

    // --- conversion to and from the bridge ---------------------------------------------------

    /** A stack from the server's item value ({@code null} is the empty stack). */
    public static HostItemStack fromValue(final @Nullable Object value) {
        if (!(value instanceof Map<?, ?>)) {
            return empty();
        }
        final Map<String, Object> map = Values.map(value);
        final Material type = Material.matchMaterial(Values.str(map, "item", "minecraft:air"));
        if (type == null || type.isAir()) {
            return empty();
        }
        return new HostItemStack(type, Values.i32(map, "count", 1), ItemComponents.fromValue(map));
    }

    /** The bridge value of any stack ({@code null} for empty stacks). */
    public static @Nullable Map<String, Object> toValue(final @Nullable ItemStack stack) {
        if (stack == null) {
            return null;
        }
        final HostItemStack host = of(stack);
        if (host.isEmpty()) {
            return null;
        }
        final Map<String, Object> out = new LinkedHashMap<>();
        out.put("item", host.type.getKey().toString());
        out.put("count", (long) host.amount);
        host.components.toValue(out);
        return out;
    }

    /** The host stack behind any stack (a copy is never made). */
    public static HostItemStack of(final ItemStack stack) {
        if (stack instanceof HostItemStack host) {
            return host;
        }
        // A plain API ItemStack delegates to a host stack; reach it through clone(), which the
        // API forwards, and keep amount changes in step by copying back what callers see.
        final ItemStack delegate = stack.clone();
        if (delegate instanceof HostItemStack host) {
            return host;
        }
        throw new IllegalArgumentException("not a Paper-RS item stack: " + stack.getClass().getName());
    }

    /** A copy of any stack as a host stack. */
    public static HostItemStack copyOf(final @Nullable ItemStack stack) {
        if (stack == null) {
            return empty();
        }
        final HostItemStack host = of(stack);
        return host == stack ? host.clone() : host;
    }

    public ItemComponents components() {
        return this.components;
    }

    /** Makes this stack write itself back through {@code writeBack} whenever it changes. */
    public HostItemStack mirroring(final Consumer<HostItemStack> writeBack) {
        this.mirror = writeBack;
        return this;
    }

    private void changed() {
        if (this.mirror != null) {
            this.mirror.accept(this);
        }
    }

    // --- type and amount ------------------------------------------------------------------

    @Override
    public Material getType() {
        return this.type;
    }

    @Override
    public void setType(final Material type) {
        Preconditions.checkArgument(type != null, "Material cannot be null");
        if (type.isAir()) {
            this.type = Material.AIR;
            this.amount = 0;
            this.components = new ItemComponents();
        } else {
            Preconditions.checkArgument(type.isItem(), "Material %s is not an item", type);
            this.type = type;
        }
        this.changed();
    }

    @Override
    public ItemStack withType(final Material type) {
        final HostItemStack copy = this.clone();
        copy.setType(type);
        return copy;
    }

    @Override
    public int getAmount() {
        return this.amount;
    }

    @Override
    public void setAmount(final int amount) {
        this.amount = amount;
        this.changed();
    }

    @Override
    public int getMaxStackSize() {
        final Integer override = this.components.varInt(ItemComponents.MAX_STACK_SIZE);
        if (override != null) {
            return override;
        }
        final HostData.ItemInfo info = this.info();
        return info == null ? 64 : info.maxStackSize();
    }

    private HostData.@Nullable ItemInfo info() {
        return HostData.get().item(this.type.getKey().toString());
    }

    @Override
    public boolean isEmpty() {
        return this.type.isAir() || this.amount <= 0;
    }

    @Override
    @SuppressWarnings("deprecation")
    public void setDurability(final short durability) {
        this.components.setVarInt(ItemComponents.DAMAGE, durability > 0 ? (int) durability : null);
        this.changed();
    }

    @Override
    @SuppressWarnings("deprecation")
    public short getDurability() {
        final Integer damage = this.components.varInt(ItemComponents.DAMAGE);
        return damage == null ? 0 : damage.shortValue();
    }

    // --- meta -------------------------------------------------------------------------------

    @Override
    public ItemMeta getItemMeta() {
        if (this.type.isAir()) {
            return null;
        }
        return HostItemMeta.create(this.type, this.components.copy());
    }

    @Override
    public boolean hasItemMeta() {
        return !this.isEmpty() && !this.components.isEmpty();
    }

    @Override
    public boolean setItemMeta(final @Nullable ItemMeta itemMeta) {
        if (itemMeta == null) {
            this.components = new ItemComponents();
            this.changed();
            return true;
        }
        final HostItemMeta meta = HostItemMeta.implOf(itemMeta);
        if (this.type.isAir()) {
            return false;
        }
        this.components = meta.toComponents();
        this.changed();
        return true;
    }

    // --- enchantments ---------------------------------------------------------------------

    private static int enchantmentId(final Enchantment enchantment) {
        final List<String> ids = HostData.get().registry("minecraft:enchantment");
        final int id = ids == null ? -1 : ids.indexOf(enchantment.getKey().toString());
        if (id < 0) {
            throw new IllegalArgumentException("unknown enchantment " + enchantment.getKey());
        }
        return id;
    }

    static @Nullable Enchantment enchantment(final int id) {
        final List<String> ids = HostData.get().registry("minecraft:enchantment");
        return ids == null || id < 0 || id >= ids.size() ? null : Registry.ENCHANTMENT.get(NamespacedKey.fromString(ids.get(id)));
    }

    static Map<Enchantment, Integer> enchantments(final ItemComponents components, final String type) {
        final Map<Enchantment, Integer> out = new LinkedHashMap<>();
        components.enchantments(type).forEach((id, level) -> {
            final Enchantment e = enchantment(id);
            if (e != null) {
                out.put(e, level);
            }
        });
        return out;
    }

    static void putEnchantment(final ItemComponents components, final String type, final Enchantment enchantment, final int level) {
        final Map<Integer, Integer> map = components.enchantments(type);
        map.put(enchantmentId(enchantment), level);
        components.setEnchantments(type, map);
    }

    static int removeEnchantment(final ItemComponents components, final String type, final Enchantment enchantment) {
        final Map<Integer, Integer> map = components.enchantments(type);
        final Integer old = map.remove(enchantmentId(enchantment));
        components.setEnchantments(type, map);
        return old == null ? 0 : old;
    }

    @Override
    public boolean containsEnchantment(final Enchantment enchant) {
        return this.getEnchantmentLevel(enchant) > 0;
    }

    @Override
    public int getEnchantmentLevel(final Enchantment enchant) {
        return this.components.enchantments(ItemComponents.ENCHANTMENTS).getOrDefault(enchantmentId(enchant), 0);
    }

    @Override
    public Map<Enchantment, Integer> getEnchantments() {
        return enchantments(this.components, ItemComponents.ENCHANTMENTS);
    }

    @Override
    public void addUnsafeEnchantment(final Enchantment enchant, final int level) {
        putEnchantment(this.components, ItemComponents.ENCHANTMENTS, enchant, level);
        this.changed();
    }

    @Override
    public int removeEnchantment(final Enchantment enchant) {
        final int old = removeEnchantment(this.components, ItemComponents.ENCHANTMENTS, enchant);
        this.changed();
        return old;
    }

    @Override
    public void removeEnchantments() {
        this.components.unset(ItemComponents.ENCHANTMENTS);
        this.changed();
    }

    // --- persistent data --------------------------------------------------------------------

    @Override
    public PersistentDataContainerView getPersistentDataContainer() {
        return HostItemMeta.pdcOf(this.components);
    }

    @Override
    public boolean editPersistentDataContainer(final Consumer<PersistentDataContainer> consumer) {
        if (this.isEmpty()) {
            return false;
        }
        final HostPersistentDataContainer pdc = HostItemMeta.pdcOf(this.components);
        consumer.accept(pdc);
        HostItemMeta.storePdc(this.components, pdc);
        this.changed();
        return true;
    }

    // --- names --------------------------------------------------------------------------------

    @Override
    public Component effectiveName() {
        final String custom = this.components.nameJson();
        if (custom != null) {
            return Components.fromJson(custom);
        }
        final String itemName = this.components.itemNameJson();
        if (itemName != null) {
            return Components.fromJson(itemName);
        }
        return Component.translatable(this.translationKey());
    }

    @Override
    public String translationKey() {
        return this.type.translationKey();
    }

    @Override
    @SuppressWarnings("deprecation")
    public String getTranslationKey() {
        return this.translationKey();
    }

    @Override
    public boolean isRepairableBy(final ItemStack repairMaterial) {
        throw Unsupported.method("ItemStack#isRepairableBy");
    }

    @Override
    public int getMaxItemUseDuration(final org.bukkit.entity.LivingEntity entity) {
        throw Unsupported.method("ItemStack#getMaxItemUseDuration");
    }

    @Override
    public List<Component> computeTooltipLines(final io.papermc.paper.inventory.tooltip.TooltipContext tooltipContext, final org.bukkit.entity.@Nullable Player player) {
        throw Unsupported.method("ItemStack#computeTooltipLines");
    }

    // --- data components --------------------------------------------------------------------

    @Override
    public <T> @Nullable T getData(final DataComponentType.Valued<T> type) {
        return DataComponents.get(this.components, type);
    }

    @Override
    public boolean hasData(final DataComponentType type) {
        return this.components.has(type.getKey().toString());
    }

    /** The components this stack sets (its item's defaults are not known to the host yet). */
    @Override
    public Set<DataComponentType> getDataTypes() {
        final Set<DataComponentType> out = new java.util.LinkedHashSet<>();
        for (String type : this.components.addedTypes()) {
            final DataComponentType component = Registry.DATA_COMPONENT_TYPE.get(NamespacedKey.fromString(type));
            if (component != null) {
                out.add(component);
            }
        }
        return out;
    }

    @Override
    public <T> void setData(final DataComponentType.Valued<T> type, final T value) {
        DataComponents.set(this.components, type, value);
        this.changed();
    }

    @Override
    public void setData(final DataComponentType.NonValued type) {
        this.components.setUnit(type.getKey().toString(), true);
        this.changed();
    }

    @Override
    public void unsetData(final DataComponentType type) {
        this.components.remove(type.getKey().toString());
        this.changed();
    }

    @Override
    public void resetData(final DataComponentType type) {
        this.components.reset(type.getKey().toString());
        this.changed();
    }

    @Override
    public void copyDataFrom(final ItemStack source, final Predicate<DataComponentType> filter) {
        throw Unsupported.method("ItemStack#copyDataFrom");
    }

    @Override
    public boolean isDataOverridden(final DataComponentType type) {
        final String name = type.getKey().toString();
        return this.components.has(name) || this.components.isRemoved(name);
    }

    @Override
    public boolean matchesWithoutData(final ItemStack item, final Set<DataComponentType> excludeTypes, final boolean ignoreCount) {
        final HostItemStack other = of(item);
        if (other.type != this.type || !ignoreCount && other.amount != this.amount) {
            return false;
        }
        final ItemComponents a = this.components.copy();
        final ItemComponents b = other.components.copy();
        for (DataComponentType type : excludeTypes) {
            a.reset(type.getKey().toString());
            b.reset(type.getKey().toString());
        }
        return a.equals(b);
    }

    // --- serialisation ----------------------------------------------------------------------

    /**
     * Paper writes the components in their vanilla form here. Paper-RS stores its own bridge
     * form under {@code paper-rs:components}, which {@link #deserialize(Map)} reads back.
     */
    @Override
    public Map<String, Object> serialize() {
        final Map<String, Object> out = new LinkedHashMap<>();
        out.put("DataVersion", io.papermc.paperrs.host.HostVersions.DATA_VERSION);
        out.put("id", this.type.getKey().toString());
        out.put("count", this.amount);
        final Map<String, Object> value = new LinkedHashMap<>();
        this.components.toValue(value);
        if (!value.isEmpty()) {
            out.put("paper-rs:components", Base64.getEncoder().encodeToString(Codec.encodeValue(value)));
        }
        out.put("schema_version", 1);
        return out;
    }

    /** The inverse of {@link #serialize()}; vanilla component maps from Paper are not read. */
    public static ItemStack deserialize(final Map<String, Object> args) {
        final Material type = Material.matchMaterial(String.valueOf(args.get("id")));
        if (type == null || type.isAir()) {
            return empty();
        }
        final int count = args.get("count") instanceof Number n ? n.intValue() : 1;
        ItemComponents components = new ItemComponents();
        if (args.get("paper-rs:components") instanceof String encoded) {
            components = ItemComponents.fromValue(Values.map(Codec.decodeValue(Base64.getDecoder().decode(encoded))));
        } else if (args.containsKey("components")) {
            java.util.logging.Logger.getLogger("Paper-RS").warning("Item components serialized by Paper are not supported by Paper-RS yet; reading "
                + type.getKey() + " without them");
        }
        return new HostItemStack(type, count, components);
    }

    @Override
    public byte[] serializeAsBytes() {
        final byte[] value = Codec.encodeValue(toValue(this));
        final byte[] out = new byte[BYTES_MAGIC.length + value.length];
        System.arraycopy(BYTES_MAGIC, 0, out, 0, BYTES_MAGIC.length);
        System.arraycopy(value, 0, out, BYTES_MAGIC.length, value.length);
        return out;
    }

    /** The inverse of {@link #serializeAsBytes()}. */
    public static ItemStack deserializeBytes(final byte[] bytes) {
        for (int i = 0; i < BYTES_MAGIC.length; i++) {
            if (bytes.length <= i || bytes[i] != BYTES_MAGIC[i]) {
                throw Unsupported.feature("Reading item stacks serialized by Paper (only Paper-RS's own form is understood)");
            }
        }
        final byte[] value = new byte[bytes.length - BYTES_MAGIC.length];
        System.arraycopy(bytes, BYTES_MAGIC.length, value, 0, value.length);
        return fromValue(Codec.decodeValue(value));
    }

    // --- identity -----------------------------------------------------------------------------

    @Override
    public HostItemStack clone() {
        return new HostItemStack(this.type, this.amount, this.components.copy());
    }

    @Override
    public boolean isSimilar(final @Nullable ItemStack stack) {
        if (stack == null) {
            return false;
        }
        if (stack == this) {
            return true;
        }
        final HostItemStack other = of(stack);
        return this.type == other.type && this.components.equals(other.components);
    }

    @Override
    public boolean equals(final Object obj) {
        if (this == obj) {
            return true;
        }
        if (!(obj instanceof ItemStack stack)) {
            return false;
        }
        final HostItemStack other = of(stack);
        return this.amount == other.amount && this.isSimilar(other);
    }

    @Override
    public int hashCode() {
        int hash = 1;
        hash = hash * 31 + this.type.hashCode();
        hash = hash * 31 + this.amount;
        return hash * 31 + this.components.hashCode();
    }

    @Override
    public String toString() {
        return "ItemStack{" + this.type.name() + " x " + this.amount + (this.components.isEmpty() ? "" : ", " + this.components.addedTypes().toString().toLowerCase(Locale.ROOT)) + "}";
    }
}
