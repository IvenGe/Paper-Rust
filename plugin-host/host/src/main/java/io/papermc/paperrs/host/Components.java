package io.papermc.paperrs.host;

import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.ComponentLike;
import net.kyori.adventure.text.serializer.gson.GsonComponentSerializer;
import net.kyori.adventure.text.serializer.legacy.LegacyComponentSerializer;
import org.jetbrains.annotations.Nullable;

/** Text components cross the bridge as their JSON. */
public final class Components {
    private Components() {
    }

    public static @Nullable String toJson(@Nullable ComponentLike component) {
        return component == null ? null : GsonComponentSerializer.gson().serialize(component.asComponent());
    }

    /** Parses JSON sent by the server; {@code null} for a missing value. */
    public static @Nullable Component fromJson(@Nullable Object value) {
        if (!(value instanceof String json) || json.isEmpty()) {
            return null;
        }
        try {
            return GsonComponentSerializer.gson().deserialize(json);
        } catch (RuntimeException e) {
            return Component.text(json);
        }
    }

    public static Component legacy(String text) {
        return LegacyComponentSerializer.legacySection().deserialize(text);
    }

    public static String toLegacy(Component component) {
        return LegacyComponentSerializer.legacySection().serialize(component);
    }
}
