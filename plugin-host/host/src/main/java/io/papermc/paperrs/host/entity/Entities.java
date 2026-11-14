package io.papermc.paperrs.host.entity;

import io.papermc.paperrs.host.HostProxies;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.item.HostPersistentDataContainer;
import java.lang.ref.WeakReference;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.World;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.Entity;
import org.bukkit.entity.EntityType;
import org.bukkit.entity.Player;
import org.bukkit.persistence.PersistentDataContainer;
import org.jetbrains.annotations.Nullable;

/** Entity objects for the server's entity states and references. */
public final class Entities {
    /** One object per entity while plugins hold it, so identity comparisons work. */
    private static final Map<Integer, WeakReference<Entity>> KNOWN = new ConcurrentHashMap<>();
    /** Persistent data of entities, kept in memory (entities are not saved yet). */
    private static final Map<UUID, HostPersistentDataContainer> DATA = new ConcurrentHashMap<>();

    private Entities() {
    }

    /** The entity type of a state's {@code type} key. */
    public static EntityType type(final Map<String, Object> state) {
        final NamespacedKey key = NamespacedKey.fromString(Values.str(state, "type", "minecraft:pig"));
        final EntityType type = key == null ? null : Registry.ENTITY_TYPE.get(key);
        return type == null ? EntityType.UNKNOWN : type;
    }

    /** The entity for a state the server sent (see {@code entity::entity_value}). */
    public static Entity fromState(final CraftServer server, final Map<String, Object> state) {
        final int id = Values.i32(state, "id");
        final WeakReference<Entity> known = KNOWN.get(id);
        final Entity existing = known == null ? null : known.get();
        if (existing != null) {
            final HostEntity impl = HostProxies.implOf(existing, HostEntity.class);
            if (impl != null) {
                impl.updateState(state);
            }
            return existing;
        }
        final EntityType type = type(state);
        final HostEntity impl = new HostEntity(server, state, type);
        Class<? extends Entity> iface = type.getEntityClass();
        if (iface == null || !iface.isInterface()) {
            iface = Entity.class;
        }
        final Entity entity = HostProxies.create(impl, iface);
        KNOWN.put(id, new WeakReference<>(entity));
        if (KNOWN.size() > 4096) {
            KNOWN.values().removeIf(ref -> ref.get() == null);
        }
        return entity;
    }

    /**
     * A player or entity named by an event or state: {@code {player: uuid}} or
     * {@code {entity: id, value: state}}.
     */
    public static @Nullable Entity of(final CraftServer server, final @Nullable Map<String, Object> ref) {
        if (ref == null || ref.isEmpty()) {
            return null;
        }
        if (ref.get("player") instanceof String uuid) {
            return server.player(UUID.fromString(uuid));
        }
        if (ref.get("value") instanceof Map<?, ?> value) {
            return fromState(server, Values.map(value));
        }
        if (ref.get("entity") instanceof Number id) {
            return byId(server, id.intValue());
        }
        return null;
    }

    /** The entity with an entity id, asking the server for its state. */
    public static @Nullable Entity byId(final CraftServer server, final int id) {
        for (Player player : server.getOnlinePlayers()) {
            if (player.getEntityId() == id) {
                return player;
            }
        }
        final Object state = server.call("entity.state", Values.of("id", id));
        return state instanceof Map<?, ?> s ? fromState(server, Values.map(s)) : null;
    }

    /** The reference the server understands for an entity. */
    public static @Nullable Map<String, Object> ref(final @Nullable Entity entity) {
        if (entity == null) {
            return null;
        }
        if (entity instanceof Player player) {
            return Values.of("player", player.getUniqueId().toString());
        }
        return Values.of("entity", entity.getEntityId());
    }

    /** The non-player entities of a world matching the filter the server applies. */
    public static List<Entity> list(final CraftServer server, final World world, final @Nullable double[] box) {
        final Map<String, Object> args = Values.of("world", world.getName());
        if (box != null) {
            args.put("min", Values.of("x", box[0], "y", box[1], "z", box[2]));
            args.put("max", Values.of("x", box[3], "y", box[4], "z", box[5]));
        }
        final List<Entity> out = new ArrayList<>();
        for (Object state : Values.list(server.call("world.entities", args))) {
            out.add(fromState(server, Values.map(state)));
        }
        return out;
    }

    /**
     * The persistent data of a player or entity (named by {@code ref}): loaded from the server,
     * which saves it with the player or entity ({@code BukkitValues}), and sent back after
     * every change.
     */
    public static PersistentDataContainer data(final CraftServer server, final Map<String, Object> ref, final UUID uuid) {
        return DATA.computeIfAbsent(uuid, u -> {
            HostPersistentDataContainer pdc = new HostPersistentDataContainer();
            if (server.call("entity.pdc", Values.of("target", ref)) instanceof byte[] saved && saved.length > 0) {
                try {
                    pdc = new HostPersistentDataContainer(io.papermc.paperrs.host.item.Nbt.readCompound(saved));
                } catch (final RuntimeException e) {
                    pdc = new HostPersistentDataContainer();
                }
            }
            final HostPersistentDataContainer container = pdc;
            return container.onChange(() -> server.call("entity.set_pdc", Values.of("target", ref, "data", container.isEmpty() ? new byte[0] : io.papermc.paperrs.host.item.Nbt.writeCompound(container.tags()))));
        });
    }

    /** Forget everything (the plugin host is going away). */
    public static void clear() {
        KNOWN.clear();
    }
}
