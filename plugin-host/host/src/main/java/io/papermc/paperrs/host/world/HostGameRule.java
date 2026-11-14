package io.papermc.paperrs.host.world;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import java.util.function.Function;
import org.bukkit.GameRule;
import org.bukkit.NamespacedKey;
import org.jetbrains.annotations.Nullable;

/**
 * A game rule (an entry of the {@code minecraft:game_rule} registry), as Paper's
 * {@code CraftGameRule}: named by its key ({@code minecraft:keep_inventory}), a boolean or an
 * integer rule with the default of the server's game rule table. The Bukkit constants of rules
 * vanilla replaced by others ({@code DO_FIRE_TICK} by the fire spread radius) are {@link Legacy}
 * rules, which convert values on their way to and from the server.
 */
public class HostGameRule<T> extends GameRule<T> {
    /** Bounds of the integer rules (filled by {@link #defaults}, so declared first). */
    private static final Map<String, int[]> BOUNDS = new HashMap<>();
    /** Default values by rule name, from the bundled game rule table. */
    private static final Map<String, Object> DEFAULTS = defaults();

    protected final NamespacedKey key;
    private final String translationKey;

    public HostGameRule(final NamespacedKey key, final int ordinal, final String translationKey) {
        this.key = key;
        this.translationKey = translationKey;
    }

    @Override
    public NamespacedKey getKey() {
        return this.key;
    }

    /** {@code Holder#getRegisteredName}: the rule's key. */
    @Override
    public String getName() {
        return this.key.asString();
    }

    @Override
    public String translationKey() {
        return this.translationKey;
    }

    @SuppressWarnings("unchecked")
    @Override
    public Class<T> getType() {
        return (Class<T>) (this.serverDefault() instanceof Boolean ? Boolean.class : Integer.class);
    }

    @Override
    public T getDefaultValue() {
        return this.fromServer(this.serverDefault());
    }

    /** The default as the server keeps it. */
    public Object serverDefault() {
        final Object value = DEFAULTS.get(this.key.asString());
        return value != null ? value : Boolean.FALSE;
    }

    /** The value the server keeps for one the API gives (the same, except for legacy rules). */
    public Object toServer(final T value) {
        final Object def = this.serverDefault();
        if (def instanceof Boolean ? !(value instanceof Boolean) : !(value instanceof Integer)) {
            throw new IllegalArgumentException("Invalid value: " + value + " (not a " + (def instanceof Boolean ? "boolean" : "integer") + ")");
        }
        final int[] bounds = BOUNDS.get(this.key.asString());
        if (bounds != null && value instanceof Integer i && (i < bounds[0] || i > bounds[1])) {
            throw new IllegalArgumentException("Invalid value: " + value + " (Value must be within range [" + bounds[0] + ";" + bounds[1] + "])");
        }
        return value;
    }

    /** The value the API shows for one the server keeps. */
    @SuppressWarnings("unchecked")
    public T fromServer(final @Nullable Object value) {
        return (T) normalize(value, this.serverDefault());
    }

    /** A value from the server as the rule's own type (numbers arrive as longs). */
    static @Nullable Object normalize(final @Nullable Object value, final Object def) {
        if (value instanceof Number n && def instanceof Integer) {
            return n.intValue();
        }
        return value;
    }

    /** Parses a value as {@code /gamerule} takes it; null if it is not one. */
    public @Nullable Object parseServerValue(final String text) {
        if (this.serverDefault() instanceof Boolean) {
            return "true".equals(text) ? Boolean.TRUE : "false".equals(text) ? Boolean.FALSE : null;
        }
        try {
            final int value = Integer.parseInt(text);
            final int[] bounds = BOUNDS.get(this.key.asString());
            return bounds == null || value >= bounds[0] && value <= bounds[1] ? value : null;
        } catch (final NumberFormatException e) {
            return null;
        }
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostGameRule<?> other && other.key.equals(this.key);
    }

    @Override
    public int hashCode() {
        return this.key.hashCode();
    }

    @Override
    public String toString() {
        return "CraftGameRule{" + this.key + "}";
    }

    private static Map<String, Object> defaults() {
        final Map<String, Object> out = new HashMap<>();
        try (InputStream in = HostGameRule.class.getClassLoader().getResourceAsStream("paper-rs-data/game_rules.json")) {
            if (in == null) {
                return out;
            }
            final JsonObject rules = JsonParser.parseReader(new InputStreamReader(in, StandardCharsets.UTF_8)).getAsJsonObject().getAsJsonObject("rules");
            for (final Map.Entry<String, JsonElement> rule : rules.entrySet()) {
                final JsonObject info = rule.getValue().getAsJsonObject();
                final JsonElement def = info.get("default");
                if (def.getAsJsonPrimitive().isBoolean()) {
                    out.put(rule.getKey(), def.getAsBoolean());
                } else {
                    out.put(rule.getKey(), def.getAsInt());
                    BOUNDS.put(rule.getKey(), new int[] {info.has("min") ? info.get("min").getAsInt() : Integer.MIN_VALUE, info.has("max") ? info.get("max").getAsInt() : Integer.MAX_VALUE});
                }
            }
        } catch (final IOException | RuntimeException e) {
            throw new IllegalStateException("could not read the bundled game rule table", e);
        }
        return out;
    }

    /**
     * A Bukkit constant for a rule vanilla replaced ({@code LegacyGameRuleWrapper}): its own
     * type, with values converted to and from those of the rule the server keeps.
     */
    public static final class Legacy<L, M> extends HostGameRule<L> {
        private final Function<L, M> fromLegacy;
        private final Function<M, L> toLegacy;
        private final Class<L> type;

        public Legacy(final GameRule<M> modern, final Function<L, M> fromLegacy, final Function<M, L> toLegacy, final Class<L> type) {
            super(modern.getKey(), -1, modern.translationKey());
            this.fromLegacy = fromLegacy;
            this.toLegacy = toLegacy;
            this.type = type;
        }

        @Override
        public Class<L> getType() {
            return this.type;
        }

        @Override
        public Object toServer(final L value) {
            return this.fromLegacy.apply(value);
        }

        @SuppressWarnings("unchecked")
        @Override
        public L fromServer(final @Nullable Object value) {
            return this.toLegacy.apply((M) normalize(value, this.serverDefault()));
        }
    }
}
