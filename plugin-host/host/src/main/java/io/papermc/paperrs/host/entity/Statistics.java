package io.papermc.paperrs.host.entity;

import io.papermc.paperrs.host.bridge.Values;
import java.util.EnumMap;
import java.util.HashMap;
import java.util.Locale;
import java.util.Map;
import java.util.UUID;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.Statistic;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.EntityType;
import org.jetbrains.annotations.Nullable;

/**
 * A player's {@link Statistic}s, online or not, as Paper's {@code CraftStatistic} reads and
 * writes them: the server counts each statistic by a vanilla stat type ({@code minecraft:custom},
 * {@code minecraft:mined}, ...) and a key (a custom stat, block, item or entity type).
 */
public final class Statistics {
    /** Stat types of the typed statistics. */
    private static final Map<Statistic, String> TYPES = new EnumMap<>(Statistic.class);
    /** Custom stats whose names differ from the statistic's. */
    private static final Map<Statistic, String> RENAMED = new EnumMap<>(Statistic.class);
    private static final Map<String, Statistic> BY_TYPE = new HashMap<>();
    private static final Map<String, Statistic> BY_CUSTOM_KEY = new HashMap<>();

    static {
        TYPES.put(Statistic.BREAK_ITEM, "minecraft:broken");
        TYPES.put(Statistic.CRAFT_ITEM, "minecraft:crafted");
        TYPES.put(Statistic.DROP, "minecraft:dropped");
        TYPES.put(Statistic.KILL_ENTITY, "minecraft:killed");
        TYPES.put(Statistic.ENTITY_KILLED_BY, "minecraft:killed_by");
        TYPES.put(Statistic.MINE_BLOCK, "minecraft:mined");
        TYPES.put(Statistic.PICKUP, "minecraft:picked_up");
        TYPES.put(Statistic.USE_ITEM, "minecraft:used");
        RENAMED.put(Statistic.ARMOR_CLEANED, "clean_armor");
        RENAMED.put(Statistic.BANNER_CLEANED, "clean_banner");
        RENAMED.put(Statistic.DROP_COUNT, "drop");
        RENAMED.put(Statistic.CAKE_SLICES_EATEN, "eat_cake_slice");
        RENAMED.put(Statistic.ITEM_ENCHANTED, "enchant_item");
        RENAMED.put(Statistic.CAULDRON_FILLED, "fill_cauldron");
        RENAMED.put(Statistic.DISPENSER_INSPECTED, "inspect_dispenser");
        RENAMED.put(Statistic.DROPPER_INSPECTED, "inspect_dropper");
        RENAMED.put(Statistic.HOPPER_INSPECTED, "inspect_hopper");
        RENAMED.put(Statistic.BEACON_INTERACTION, "interact_with_beacon");
        RENAMED.put(Statistic.BREWINGSTAND_INTERACTION, "interact_with_brewingstand");
        RENAMED.put(Statistic.CRAFTING_TABLE_INTERACTION, "interact_with_crafting_table");
        RENAMED.put(Statistic.FURNACE_INTERACTION, "interact_with_furnace");
        RENAMED.put(Statistic.CHEST_OPENED, "open_chest");
        RENAMED.put(Statistic.ENDERCHEST_OPENED, "open_enderchest");
        RENAMED.put(Statistic.SHULKER_BOX_OPENED, "open_shulker_box");
        RENAMED.put(Statistic.NOTEBLOCK_PLAYED, "play_noteblock");
        RENAMED.put(Statistic.RECORD_PLAYED, "play_record");
        RENAMED.put(Statistic.PLAY_ONE_MINUTE, "play_time");
        RENAMED.put(Statistic.FLOWER_POTTED, "pot_flower");
        RENAMED.put(Statistic.TRAPPED_CHEST_TRIGGERED, "trigger_trapped_chest");
        RENAMED.put(Statistic.NOTEBLOCK_TUNED, "tune_noteblock");
        RENAMED.put(Statistic.CAULDRON_USED, "use_cauldron");
        for (Statistic statistic : Statistic.values()) {
            if (statistic.getType() == Statistic.Type.UNTYPED) {
                BY_CUSTOM_KEY.put(customKey(statistic), statistic);
            } else {
                BY_TYPE.put(TYPES.get(statistic), statistic);
            }
        }
    }

    private Statistics() {
    }

    private static String customKey(final Statistic statistic) {
        return "minecraft:" + RENAMED.getOrDefault(statistic, statistic.name().toLowerCase(Locale.ROOT));
    }

    // --- the key of each kind of statistic, checked as CraftStatistic checks it ----------------

    private static String untyped(final Statistic statistic) {
        check(statistic != null, "Statistic cannot be null");
        check(statistic.getType() == Statistic.Type.UNTYPED, "Must supply additional parameter for this statistic");
        return customKey(statistic);
    }

    private static String material(final Statistic statistic, final Material material) {
        if (statistic.getType() == Statistic.Type.BLOCK) {
            check(material.isBlock(), "Material " + material + " must be a block");
        } else if (statistic.getType() == Statistic.Type.ITEM) {
            check(material.isItem(), "Material " + material + " must be an item");
        } else {
            throw new IllegalArgumentException("This statistic does not take a Material parameter");
        }
        check(!material.isLegacy(), "The supplied Material " + material + " does not have a corresponding statistic");
        return material.getKey().toString();
    }

    private static String entity(final Statistic statistic, final EntityType entityType) {
        check(statistic.getType() == Statistic.Type.ENTITY, "This statistic does not take an EntityType parameter");
        check(entityType != EntityType.UNKNOWN, "The supplied EntityType " + entityType + " does not have a corresponding statistic");
        return entityType.getKey().toString();
    }

    private static void check(final boolean ok, final String message) {
        if (!ok) {
            throw new IllegalArgumentException(message);
        }
    }

    private static void checkAmount(final int amount) {
        check(amount > 0, "Amount must be greater than 0");
    }

    // --- reads and writes -----------------------------------------------------------------------

    private static int read(final CraftServer server, final UUID player, final String type, final String key) {
        final Object value = server.call("player.statistic", Values.of("uuid", player.toString(), "type", type, "key", key));
        return value instanceof Number n ? n.intValue() : 0;
    }

    private static void write(final CraftServer server, final UUID player, final String type, final String key, final int value) {
        server.call("player.set_statistic", Values.of("uuid", player.toString(), "type", type, "key", key, "value", value));
    }

    public static int get(final CraftServer server, final UUID player, final Statistic statistic) {
        return read(server, player, "minecraft:custom", untyped(statistic));
    }

    public static void set(final CraftServer server, final UUID player, final Statistic statistic, final int value) {
        final String key = untyped(statistic);
        check(value >= 0, "Value must be greater than or equal to 0");
        write(server, player, "minecraft:custom", key, value);
    }

    public static void add(final CraftServer server, final UUID player, final Statistic statistic, final int amount) {
        set(server, player, statistic, get(server, player, statistic) + amount);
    }

    public static int get(final CraftServer server, final UUID player, final Statistic statistic, final Material material) {
        check(statistic != null, "Statistic cannot be null");
        check(material != null, "Material cannot be null");
        return read(server, player, TYPES.getOrDefault(statistic, ""), material(statistic, material));
    }

    public static void set(final CraftServer server, final UUID player, final Statistic statistic, final Material material, final int value) {
        check(statistic != null, "Statistic cannot be null");
        check(material != null, "Material cannot be null");
        check(value >= 0, "Value must be greater than or equal to 0");
        write(server, player, TYPES.getOrDefault(statistic, ""), material(statistic, material), value);
    }

    public static void add(final CraftServer server, final UUID player, final Statistic statistic, final Material material, final int amount) {
        set(server, player, statistic, material, get(server, player, statistic, material) + amount);
    }

    public static int get(final CraftServer server, final UUID player, final Statistic statistic, final EntityType entityType) {
        check(statistic != null, "Statistic cannot be null");
        check(entityType != null, "EntityType cannot be null");
        return read(server, player, TYPES.getOrDefault(statistic, ""), entity(statistic, entityType));
    }

    public static void set(final CraftServer server, final UUID player, final Statistic statistic, final EntityType entityType, final int value) {
        check(statistic != null, "Statistic cannot be null");
        check(entityType != null, "EntityType cannot be null");
        check(value >= 0, "Value must be greater than or equal to 0");
        write(server, player, TYPES.getOrDefault(statistic, ""), entity(statistic, entityType), value);
    }

    public static void add(final CraftServer server, final UUID player, final Statistic statistic, final EntityType entityType, final int amount) {
        set(server, player, statistic, entityType, get(server, player, statistic, entityType) + amount);
    }

    /** Checks an increment or decrement amount before anything else, as CraftStatistic does. */
    public static int amount(final int amount) {
        checkAmount(amount);
        return amount;
    }

    // --- the statistic a vanilla stat type and key name, for PlayerStatisticIncrementEvent ------

    public static @Nullable Statistic statistic(final String type, final String key) {
        return "minecraft:custom".equals(type) ? BY_CUSTOM_KEY.get(key) : BY_TYPE.get(type);
    }

    public static @Nullable Material material(final String key) {
        return Material.matchMaterial(key);
    }

    public static @Nullable EntityType entityType(final String key) {
        final NamespacedKey namespaced = NamespacedKey.fromString(key);
        return namespaced == null ? null : Registry.ENTITY_TYPE.get(namespaced);
    }
}
