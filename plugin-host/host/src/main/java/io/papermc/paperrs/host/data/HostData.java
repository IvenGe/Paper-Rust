package io.papermc.paperrs.host.data;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import org.jetbrains.annotations.Nullable;

/**
 * The version data tables, read from the same JSON files the Rust server is built from
 * (bundled into the jar under {@code paper-rs-data/}).
 */
public final class HostData {
    /** A block with its state id range and properties (in state-id order). */
    public record BlockInfo(String name, int id, int minState, int stateCount, int defaultState,
                            List<Property> properties, String item, float hardness, float blastResistance) {
    }

    public record Property(String name, List<String> values) {
    }

    public record ItemInfo(String name, int id, int maxStackSize, String block, int maxDamage) {
    }

    private static HostData instance;

    /** Registry id -> entry ids in network order, for built-in and synchronised registries. */
    private final Map<String, List<String>> registries = new HashMap<>();
    private final List<BlockInfo> blocks = new ArrayList<>();
    private final Map<String, BlockInfo> blocksByName = new HashMap<>();
    private final int[] stateToBlock;
    private final List<ItemInfo> items = new ArrayList<>();
    private final Map<String, ItemInfo> itemsByName = new HashMap<>();
    /** Registry id -> tag id -> entry ids. */
    private final Map<String, Map<String, List<String>>> tags = new HashMap<>();
    /** Potion -> its effects (effect, duration, amplifier). */
    private final Map<String, List<PotionEffectInfo>> potions = new HashMap<>();
    private final Set<String> instantEffects = new java.util.HashSet<>();

    /** An effect a potion gives: {@code minecraft:mob_effect} name, duration in ticks, amplifier. */
    public record PotionEffectInfo(String effect, int duration, int amplifier) {
    }

    private HostData() {
        JsonObject builtin = read("paper-rs-data/registries.json").getAsJsonObject();
        for (Map.Entry<String, JsonElement> e : builtin.entrySet()) {
            this.registries.put(e.getKey(), strings(e.getValue().getAsJsonArray()));
        }
        JsonObject synced = read("paper-rs-data/synced_registries.json").getAsJsonObject().getAsJsonObject("entries");
        for (Map.Entry<String, JsonElement> e : synced.entrySet()) {
            this.registries.put(e.getKey(), strings(e.getValue().getAsJsonArray()));
        }
        JsonObject extra = read("paper-rs/extra-registries.json").getAsJsonObject();
        for (Map.Entry<String, JsonElement> e : extra.entrySet()) {
            this.registries.putIfAbsent(e.getKey(), strings(e.getValue().getAsJsonArray()));
        }

        JsonObject blocksJson = read("paper-rs-data/blocks.json").getAsJsonObject();
        this.stateToBlock = new int[blocksJson.get("state_count").getAsInt()];
        for (JsonElement el : blocksJson.getAsJsonArray("blocks")) {
            JsonObject b = el.getAsJsonObject();
            List<Property> props = new ArrayList<>();
            int count = 1;
            for (JsonElement p : b.getAsJsonArray("properties")) {
                JsonObject po = p.getAsJsonObject();
                List<String> values = strings(po.getAsJsonArray("values"));
                props.add(new Property(po.get("name").getAsString(), List.copyOf(values)));
                count *= values.size();
            }
            BlockInfo info = new BlockInfo(
                b.get("name").getAsString(), b.get("id").getAsInt(), b.get("min_state").getAsInt(), count,
                b.get("default_state").getAsInt(), List.copyOf(props),
                b.get("item").isJsonNull() ? null : b.get("item").getAsString(),
                floatOr(b.get("hardness")), floatOr(b.get("blast_resistance")));
            this.blocks.add(info);
            this.blocksByName.put(info.name(), info);
            for (int s = info.minState(); s < info.minState() + count; s++) {
                this.stateToBlock[s] = info.id();
            }
        }

        JsonObject itemComponents = read("paper-rs-data/item_components.json").getAsJsonObject().getAsJsonObject("items");
        for (JsonElement el : read("paper-rs-data/items.json").getAsJsonObject().getAsJsonArray("items")) {
            JsonObject i = el.getAsJsonObject();
            String name = i.get("name").getAsString();
            JsonObject components = itemComponents.getAsJsonObject(name);
            int maxDamage = components != null && components.has("max_damage") ? components.get("max_damage").getAsInt() : 0;
            ItemInfo info = new ItemInfo(name, i.get("id").getAsInt(), i.get("max_stack_size").getAsInt(),
                i.get("block").isJsonNull() ? null : i.get("block").getAsString(), maxDamage);
            this.items.add(info);
            this.itemsByName.put(info.name(), info);
        }

        for (Map.Entry<String, JsonElement> potion : read("paper-rs-data/potions.json").getAsJsonObject().getAsJsonObject("potions").entrySet()) {
            List<PotionEffectInfo> effects = new ArrayList<>();
            for (JsonElement effect : potion.getValue().getAsJsonArray()) {
                JsonArray e = effect.getAsJsonArray();
                effects.add(new PotionEffectInfo(e.get(0).getAsString(), e.get(1).getAsInt(), e.get(2).getAsInt()));
            }
            this.potions.put(potion.getKey(), List.copyOf(effects));
        }
        for (Map.Entry<String, JsonElement> effect : read("paper-rs-data/effects.json").getAsJsonObject().getAsJsonObject("effects").entrySet()) {
            if (effect.getValue().getAsJsonObject().get("instantaneous").getAsBoolean()) {
                this.instantEffects.add(effect.getKey());
            }
        }

        JsonObject tagsJson = read("paper-rs-data/tags.json").getAsJsonObject();
        for (Map.Entry<String, JsonElement> registry : tagsJson.entrySet()) {
            List<String> names = this.registries.get(registry.getKey());
            if (names == null) {
                continue;
            }
            Map<String, List<String>> byTag = new LinkedHashMap<>();
            for (Map.Entry<String, JsonElement> tag : registry.getValue().getAsJsonObject().entrySet()) {
                List<String> entries = new ArrayList<>();
                for (JsonElement id : tag.getValue().getAsJsonArray()) {
                    int index = id.getAsInt();
                    if (index >= 0 && index < names.size()) {
                        entries.add(names.get(index));
                    }
                }
                byTag.put(tag.getKey(), List.copyOf(entries));
            }
            this.tags.put(registry.getKey(), byTag);
        }
    }

    public static synchronized HostData get() {
        if (instance == null) {
            instance = new HostData();
        }
        return instance;
    }

    private static float floatOr(JsonElement e) {
        return e == null || e.isJsonNull() ? Float.NaN : e.getAsFloat();
    }

    private static List<String> strings(JsonArray array) {
        List<String> out = new ArrayList<>(array.size());
        for (JsonElement e : array) {
            out.add(e.getAsString());
        }
        return out;
    }

    private static JsonElement read(String resource) {
        try (InputStream in = HostData.class.getClassLoader().getResourceAsStream(resource)) {
            if (in == null) {
                throw new IllegalStateException("missing bundled data file " + resource);
            }
            return JsonParser.parseReader(new InputStreamReader(in, StandardCharsets.UTF_8));
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    /** Entry ids of a registry ({@code minecraft:block}), or null if unknown. */
    public List<String> registry(String id) {
        return this.registries.get(id);
    }

    public List<BlockInfo> blocks() {
        return this.blocks;
    }

    public BlockInfo block(String name) {
        return this.blocksByName.get(name);
    }

    public BlockInfo blockOfState(int state) {
        if (state < 0 || state >= this.stateToBlock.length) {
            return this.blocks.get(0);
        }
        return this.blocks.get(this.stateToBlock[state]);
    }

    public int stateCount() {
        return this.stateToBlock.length;
    }

    public List<ItemInfo> items() {
        return this.items;
    }

    public ItemInfo item(String name) {
        return this.itemsByName.get(name);
    }

    public ItemInfo item(int id) {
        return id >= 0 && id < this.items.size() ? this.items.get(id) : null;
    }

    /** The effects of a potion ({@code minecraft:strong_healing}), or null if there is no such potion. */
    public @Nullable List<PotionEffectInfo> potionEffects(String potion) {
        return this.potions.get(potion);
    }

    public boolean isInstantEffect(String effect) {
        return this.instantEffects.contains(effect);
    }

    /** Tags of a registry ({@code minecraft:block}); empty if it has none. */
    public Map<String, List<String>> tags(String registry) {
        return this.tags.getOrDefault(registry, Map.of());
    }

    // --- block states ------------------------------------------------------------------------

    /** Property values of a state, in the block's property order. */
    public Map<String, String> properties(int state) {
        BlockInfo b = this.blockOfState(state);
        Map<String, String> out = new LinkedHashMap<>();
        int index = state - b.minState();
        String[] values = new String[b.properties().size()];
        for (int i = b.properties().size() - 1; i >= 0; i--) {
            Property p = b.properties().get(i);
            values[i] = p.values().get(index % p.values().size());
            index /= p.values().size();
        }
        for (int i = 0; i < values.length; i++) {
            out.put(b.properties().get(i).name(), values[i]);
        }
        return out;
    }

    /** The state of the same block with one property changed, or -1 if invalid. */
    public int withProperty(int state, String name, String value) {
        BlockInfo b = this.blockOfState(state);
        int stride = 1;
        for (int i = b.properties().size() - 1; i >= 0; i--) {
            Property p = b.properties().get(i);
            if (p.name().equals(name)) {
                int target = p.values().indexOf(value);
                if (target < 0) {
                    return -1;
                }
                int current = (state - b.minState()) / stride % p.values().size();
                return state - current * stride + target * stride;
            }
            stride *= p.values().size();
        }
        return -1;
    }

    /** {@code minecraft:oak_stairs[facing=north,...]}. */
    public String stateToString(int state) {
        BlockInfo b = this.blockOfState(state);
        if (b.properties().isEmpty()) {
            return b.name();
        }
        StringBuilder sb = new StringBuilder(b.name()).append('[');
        boolean first = true;
        for (Map.Entry<String, String> e : this.properties(state).entrySet()) {
            if (!first) {
                sb.append(',');
            }
            first = false;
            sb.append(e.getKey()).append('=').append(e.getValue());
        }
        return sb.append(']').toString();
    }

    /** Parses {@code name} or {@code name[k=v,...]}; -1 if invalid. */
    public int parseState(String text) {
        String t = text.trim();
        String name = t;
        String props = null;
        int bracket = t.indexOf('[');
        if (bracket >= 0) {
            if (!t.endsWith("]")) {
                return -1;
            }
            name = t.substring(0, bracket);
            props = t.substring(bracket + 1, t.length() - 1);
        }
        if (!name.contains(":")) {
            name = "minecraft:" + name;
        }
        BlockInfo b = this.blocksByName.get(name);
        if (b == null) {
            return -1;
        }
        int state = b.defaultState();
        if (props != null && !props.isBlank()) {
            for (String pair : props.split(",")) {
                int eq = pair.indexOf('=');
                if (eq < 0) {
                    return -1;
                }
                state = this.withProperty(state, pair.substring(0, eq).trim(), pair.substring(eq + 1).trim());
                if (state < 0) {
                    return -1;
                }
            }
        }
        return state;
    }
}
