package io.papermc.paperrs.host.registry;

import io.papermc.paper.registry.RegistryKey;
import io.papermc.paper.registry.TypedKey;
import io.papermc.paper.registry.tag.Tag;
import io.papermc.paper.registry.tag.TagKey;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import org.bukkit.Keyed;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;

/** A tag of a registry: a fixed set of entry keys. */
final class HostTag<T extends Keyed> implements Tag<T> {
    private final TagKey<T> tagKey;
    private final RegistryKey<T> registryKey;
    private final List<TypedKey<T>> values;

    HostTag(TagKey<T> tagKey, RegistryKey<T> registryKey, List<NamespacedKey> keys) {
        this.tagKey = tagKey;
        this.registryKey = registryKey;
        List<TypedKey<T>> values = new ArrayList<>(keys.size());
        for (NamespacedKey key : keys) {
            values.add(TypedKey.create(registryKey, key));
        }
        this.values = List.copyOf(values);
    }

    @Override
    public TagKey<T> tagKey() {
        return this.tagKey;
    }

    @Override
    public RegistryKey<T> registryKey() {
        return this.registryKey;
    }

    @Override
    public Collection<TypedKey<T>> values() {
        return this.values;
    }

    @Override
    public Collection<T> resolve(Registry<T> registry) {
        List<T> out = new ArrayList<>(this.values.size());
        for (TypedKey<T> key : this.values) {
            T value = registry.get(key);
            if (value != null) {
                out.add(value);
            }
        }
        return out;
    }

    @Override
    public boolean contains(TypedKey<T> key) {
        return this.values.contains(key);
    }
}
