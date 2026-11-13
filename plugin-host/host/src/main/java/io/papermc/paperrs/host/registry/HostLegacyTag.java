package io.papermc.paperrs.host.registry;

import java.util.Collections;
import java.util.LinkedHashSet;
import java.util.Set;
import org.bukkit.Keyed;
import org.bukkit.NamespacedKey;

/** An {@link org.bukkit.Tag} (the Bukkit-era tag API, e.g. {@code Tag.LOGS}) over fixed values. */
public final class HostLegacyTag<T extends Keyed> implements org.bukkit.Tag<T> {
    private final NamespacedKey key;
    private final Set<T> values;

    public HostLegacyTag(NamespacedKey key, Set<T> values) {
        this.key = key;
        this.values = Collections.unmodifiableSet(new LinkedHashSet<>(values));
    }

    @Override
    public boolean isTagged(T item) {
        return this.values.contains(item);
    }

    @Override
    public Set<T> getValues() {
        return this.values;
    }

    @Override
    public NamespacedKey getKey() {
        return this.key;
    }

    @Override
    public String toString() {
        return "Tag{" + this.key + ", " + this.values.size() + " values}";
    }
}
