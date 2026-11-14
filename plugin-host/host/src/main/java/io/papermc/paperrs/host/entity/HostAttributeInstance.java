package io.papermc.paperrs.host.entity;

import io.papermc.paperrs.host.bridge.Values;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import net.kyori.adventure.key.Key;
import org.bukkit.NamespacedKey;
import org.bukkit.attribute.Attribute;
import org.bukkit.attribute.AttributeInstance;
import org.bukkit.attribute.AttributeModifier;
import org.bukkit.craftbukkit.CraftServer;
import org.jetbrains.annotations.Nullable;

/**
 * An attribute of a living entity or player ({@code CraftAttributeInstance}). Every read
 * asks the server, which owns the attribute map; changes apply there at once.
 */
public final class HostAttributeInstance implements AttributeInstance {
    private final CraftServer server;
    private final int entity;
    private final Attribute attribute;
    /** Reloads state the entity caches (a player's health) after a change. */
    private final Runnable changed;

    private HostAttributeInstance(final CraftServer server, final int entity, final Attribute attribute, final Runnable changed) {
        this.server = server;
        this.entity = entity;
        this.attribute = attribute;
        this.changed = changed;
    }

    private Map<String, Object> args(final Object... kv) {
        final Map<String, Object> args = Values.of(kv);
        args.put("id", this.entity);
        args.put("attribute", this.attribute.getKey().toString());
        return args;
    }

    /** {@code Attributable#getAttribute}: null if the entity does not have the attribute. */
    public static @Nullable AttributeInstance of(final CraftServer server, final int entity, final Attribute attribute, final Runnable changed) {
        if (attribute == null) {
            throw new IllegalArgumentException("Attribute cannot be null");
        }
        final HostAttributeInstance instance = new HostAttributeInstance(server, entity, attribute, changed);
        return instance.state() == null ? null : instance;
    }

    /** {@code Attributable#registerAttribute}. */
    public static void register(final CraftServer server, final int entity, final Attribute attribute) {
        if (attribute == null) {
            throw new IllegalArgumentException("Attribute cannot be null");
        }
        server.call("attribute.register", Values.of("id", entity, "attribute", attribute.getKey().toString()));
    }

    private @Nullable Map<String, Object> state() {
        final Object state = this.server.call("attribute.get", this.args());
        return state instanceof Map<?, ?> m ? Values.map(m) : null;
    }

    private Map<String, Object> live() {
        final Map<String, Object> state = this.state();
        if (state == null) {
            throw new IllegalStateException("The entity no longer has " + this.attribute.getKey());
        }
        return state;
    }

    @Override
    public Attribute getAttribute() {
        return this.attribute;
    }

    @Override
    public double getBaseValue() {
        return Values.f64(this.live(), "base", 0);
    }

    @Override
    public void setBaseValue(final double value) {
        this.server.call("attribute.set_base", this.args("base", value));
        this.changed.run();
    }

    @Override
    public double getValue() {
        return Values.f64(this.live(), "value", 0);
    }

    @Override
    public double getDefaultValue() {
        return Values.f64(this.live(), "default", 0);
    }

    private static AttributeModifier.Operation operation(final int id) {
        return switch (id) {
            case 1 -> AttributeModifier.Operation.ADD_SCALAR;
            case 2 -> AttributeModifier.Operation.MULTIPLY_SCALAR_1;
            default -> AttributeModifier.Operation.ADD_NUMBER;
        };
    }

    @Override
    public Collection<AttributeModifier> getModifiers() {
        final List<AttributeModifier> out = new ArrayList<>();
        for (final Object o : Values.list(this.live().get("modifiers"))) {
            final Map<String, Object> m = Values.map(o);
            final NamespacedKey key = NamespacedKey.fromString(Values.str(m, "id", "minecraft:unknown"));
            if (key != null) {
                out.add(new AttributeModifier(key, Values.f64(m, "amount", 0), operation(Values.i32(m, "operation", 0))));
            }
        }
        return out;
    }

    @Override
    public @Nullable AttributeModifier getModifier(final Key key) {
        final String id = key.asString();
        for (final AttributeModifier modifier : this.getModifiers()) {
            if (modifier.getKey().asString().equals(id)) {
                return modifier;
            }
        }
        return null;
    }

    @Override
    @SuppressWarnings("deprecation")
    public @Nullable AttributeModifier getModifier(final UUID uuid) {
        return this.getModifier(NamespacedKey.minecraft(uuid.toString()));
    }

    private void add(final AttributeModifier modifier, final boolean persistent) {
        if (modifier == null) {
            throw new IllegalArgumentException("modifier");
        }
        if (this.getModifier(modifier.getKey()) != null) {
            throw new IllegalArgumentException("Modifier is already applied on this attribute!");
        }
        this.server.call("attribute.add_modifier", this.args("modifier", modifier.getKey().asString(), "amount", modifier.getAmount(),
            "operation", modifier.getOperation().ordinal(), "persistent", persistent));
        this.changed.run();
    }

    @Override
    public void addModifier(final AttributeModifier modifier) {
        this.add(modifier, true);
    }

    @Override
    public void addTransientModifier(final AttributeModifier modifier) {
        this.add(modifier, false);
    }

    @Override
    public void removeModifier(final Key key) {
        this.server.call("attribute.remove_modifier", this.args("modifier", key.asString()));
        this.changed.run();
    }

    @Override
    @SuppressWarnings("deprecation")
    public void removeModifier(final UUID uuid) {
        this.removeModifier(NamespacedKey.minecraft(uuid.toString()));
    }

    @Override
    public void removeModifier(final AttributeModifier modifier) {
        this.removeModifier(modifier.getKey());
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostAttributeInstance other && other.entity == this.entity && other.attribute.equals(this.attribute);
    }

    @Override
    public int hashCode() {
        return 31 * this.entity + this.attribute.hashCode();
    }

    @Override
    public String toString() {
        return "CraftAttributeInstance{" + this.attribute.getKey() + " of " + this.entity + "}";
    }
}
