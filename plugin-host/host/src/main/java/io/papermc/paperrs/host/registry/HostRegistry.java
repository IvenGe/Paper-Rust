package io.papermc.paperrs.host.registry;

import io.papermc.paper.registry.RegistryKey;
import io.papermc.paper.registry.tag.Tag;
import io.papermc.paper.registry.tag.TagKey;
import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.logging.Logger;
import java.util.stream.Stream;
import org.bukkit.Keyed;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.jspecify.annotations.Nullable;

/**
 * A registry whose entries are created on first use and then cached.
 *
 * <p>Entries are created lazily because creating one may initialise its API class (for
 * example {@code Enchantment}), whose static initialiser asks this registry for all of its
 * constants. {@link #lockReferenceHolders()} initialises that class up front, on the main
 * thread, while nothing else touches the registry, so the constants and later lookups always
 * return the same object.
 */
public final class HostRegistry<T extends Keyed> implements Registry<T> {
    private static final Logger LOGGER = Logger.getLogger("Registry");

    /** Creates the entry for a key; {@code ordinal} is the network id. */
    @FunctionalInterface
    public interface Factory<T> {
        T create(NamespacedKey key, int ordinal);
    }

    private final RegistryKey<T> registryKey;
    private final Class<?> holder;
    private final List<NamespacedKey> keys;
    private final Map<NamespacedKey, Integer> ordinals = new HashMap<>();
    private final Factory<T> factory;
    private final Map<NamespacedKey, T> cache = new HashMap<>();
    private final Map<String, List<NamespacedKey>> tags;
    private boolean locked;

    public HostRegistry(RegistryKey<T> registryKey, Class<?> holder, List<String> ids, Factory<T> factory, Map<String, List<String>> tags) {
        this.registryKey = registryKey;
        this.holder = holder;
        this.keys = new ArrayList<>(ids.size());
        for (String id : ids) {
            NamespacedKey key = NamespacedKey.fromString(id);
            this.ordinals.put(key, this.keys.size());
            this.keys.add(key);
        }
        this.factory = factory;
        this.tags = new HashMap<>();
        tags.forEach((tag, values) -> this.tags.put(tag, values.stream().map(NamespacedKey::fromString).toList()));
    }

    public RegistryKey<T> registryKey() {
        return this.registryKey;
    }

    /** Initialises the API class that holds this registry's constants. */
    public synchronized void lockReferenceHolders() {
        if (this.locked) {
            return;
        }
        try {
            Class.forName(this.holder.getName(), true, this.holder.getClassLoader());
        } catch (ClassNotFoundException e) {
            throw new IllegalStateException("cannot initialise " + this.holder.getName(), e);
        }
        this.locked = true;
    }

    @Override
    public synchronized @Nullable T get(NamespacedKey key) {
        T existing = this.cache.get(key);
        if (existing != null) {
            return existing;
        }
        Integer ordinal = this.ordinals.get(key);
        if (ordinal == null) {
            if (!this.locked) {
                // An API constant this version's data does not have. Hand out a placeholder
                // so the holder class can still initialise, as Paper does.
                LOGGER.warning("Registry " + this.registryKey.key() + " has no entry " + key + "; using a placeholder");
                ordinal = -1;
            } else {
                return null;
            }
        }
        T created = this.factory.create(key, ordinal);
        // Creating the entry may have re-entered this method through the holder's initialiser.
        T raced = this.cache.putIfAbsent(key, created);
        return raced != null ? raced : created;
    }

    @Override
    public @Nullable NamespacedKey getKey(T value) {
        return value.getKey();
    }

    @Override
    public boolean hasTag(TagKey<T> key) {
        return this.tags.containsKey(key.key().asString());
    }

    @Override
    public Tag<T> getTag(TagKey<T> key) {
        List<NamespacedKey> values = this.tags.get(key.key().asString());
        if (values == null) {
            throw new java.util.NoSuchElementException("No tag " + key + " in " + this.registryKey);
        }
        return new HostTag<>(key, this.registryKey, values);
    }

    @Override
    public Collection<Tag<T>> getTags() {
        List<Tag<T>> out = new ArrayList<>();
        for (Map.Entry<String, List<NamespacedKey>> e : this.tags.entrySet()) {
            out.add(new HostTag<>(TagKey.create(this.registryKey, NamespacedKey.fromString(e.getKey())), this.registryKey, e.getValue()));
        }
        return out;
    }

    @Override
    public Stream<T> stream() {
        return this.keys.stream().map(this::get);
    }

    @Override
    public Stream<NamespacedKey> keyStream() {
        return this.keys.stream();
    }

    @Override
    public int size() {
        return this.keys.size();
    }

    @Override
    public Iterator<T> iterator() {
        return this.stream().iterator();
    }

    @Override
    public String toString() {
        return "HostRegistry{" + this.registryKey.key() + "}";
    }
}
