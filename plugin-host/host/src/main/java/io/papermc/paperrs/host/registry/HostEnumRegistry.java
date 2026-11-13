package io.papermc.paperrs.host.registry;

import java.util.function.Predicate;
import org.bukkit.Keyed;
import org.bukkit.Registry;

/** A registry over an API enum (entity types, particles, potions). */
final class HostEnumRegistry<T extends Enum<T> & Keyed> extends Registry.SimpleRegistry<T> {
    HostEnumRegistry(final Class<T> type, final Predicate<T> predicate) {
        super(type, predicate);
    }
}
