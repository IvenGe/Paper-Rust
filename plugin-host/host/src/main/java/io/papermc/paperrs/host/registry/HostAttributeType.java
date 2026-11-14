package io.papermc.paperrs.host.registry;

import io.papermc.paperrs.host.Unsupported;
import io.papermc.paperrs.host.bridge.Values;
import java.util.Locale;
import org.bukkit.Bukkit;
import org.bukkit.NamespacedKey;
import org.bukkit.attribute.Attribute;
import org.bukkit.craftbukkit.CraftServer;

/** An attribute of the registry; its default value comes from the server's attribute table. */
public final class HostAttributeType implements Attribute {
    private final NamespacedKey key;
    private final int ordinal;
    private final String translationKey;

    public HostAttributeType(final NamespacedKey key, final int ordinal, final String translationKey) {
        this.key = key;
        this.ordinal = ordinal;
        this.translationKey = translationKey;
    }

    @Override
    public double getDefaultValue() {
        final Object value = ((CraftServer) Bukkit.getServer()).call("attribute.default", Values.of("attribute", this.key.toString()));
        return value instanceof Number n ? n.doubleValue() : 0;
    }

    @Override
    public Attribute.Sentiment getSentiment() {
        throw Unsupported.method("Attribute#getSentiment");
    }

    @Override
    public NamespacedKey getKey() {
        return this.key;
    }

    @Override
    public String getTranslationKey() {
        return this.translationKey;
    }

    @Override
    public String translationKey() {
        return this.translationKey;
    }

    @Override
    public String name() {
        return this.key.getKey().toUpperCase(Locale.ROOT).replace('.', '_').replace('/', '_');
    }

    @Override
    public int ordinal() {
        return this.ordinal;
    }

    @Override
    public int compareTo(final Attribute other) {
        return Integer.compare(this.ordinal, other.ordinal());
    }

    @Override
    public boolean equals(final Object o) {
        return this == o;
    }

    @Override
    public int hashCode() {
        return this.key.hashCode();
    }

    @Override
    public String toString() {
        return "CraftAttribute{" + this.key + "}";
    }
}
