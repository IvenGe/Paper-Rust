package io.papermc.paperrs.host.item;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import net.kyori.adventure.key.Key;
import org.bukkit.NamespacedKey;
import org.bukkit.persistence.ListPersistentDataType;
import org.bukkit.persistence.PersistentDataAdapterContext;
import org.bukkit.persistence.PersistentDataContainer;
import org.bukkit.persistence.PersistentDataType;
import org.jetbrains.annotations.Nullable;

/**
 * A persistent data container stored as NBT, as in Paper: each key holds the tag of the
 * value's primitive form ({@code PersistentDataType#getPrimitiveType}). On items it lives in
 * {@code minecraft:custom_data} under {@code PublicBukkitValues}.
 */
public final class HostPersistentDataContainer implements PersistentDataContainer {
    /** The key item custom data keeps plugin values under, as in CraftBukkit. */
    public static final String BUKKIT_KEY = "PublicBukkitValues";
    private static final PersistentDataAdapterContext CONTEXT = HostPersistentDataContainer::new;

    private final Nbt.Compound tags;

    public HostPersistentDataContainer() {
        this(new Nbt.Compound());
    }

    public HostPersistentDataContainer(final Nbt.Compound tags) {
        this.tags = tags;
    }

    /** The values as an NBT compound (live). */
    /** Called after every change (an entity's container is saved with the entity). */
    private @Nullable Runnable onChange;

    public HostPersistentDataContainer onChange(final @Nullable Runnable listener) {
        this.onChange = listener;
        return this;
    }

    private void changed() {
        if (this.onChange != null) {
            this.onChange.run();
        }
    }

    public Nbt.Compound tags() {
        return this.tags;
    }

    // --- primitive conversion -------------------------------------------------------------

    private static Object wrap(final PersistentDataType<?, ?> type, final Object primitive) {
        if (type instanceof ListPersistentDataType<?, ?> list) {
            final Nbt.NbtList out = new Nbt.NbtList(Nbt.END);
            for (Object element : (List<?>) primitive) {
                out.add(wrap(list.elementType(), element));
            }
            return out;
        }
        return wrapPrimitive(primitive);
    }

    private static Object wrapPrimitive(final Object primitive) {
        return switch (primitive) {
            case Byte b -> b;
            case Short s -> s;
            case Integer i -> i;
            case Long l -> l;
            case Float f -> f;
            case Double d -> d;
            case String s -> s;
            case byte[] b -> b.clone();
            case int[] i -> i.clone();
            case long[] l -> l.clone();
            case HostPersistentDataContainer c -> c.tags.copy();
            case PersistentDataContainer[] containers -> {
                final Nbt.NbtList out = new Nbt.NbtList(Nbt.COMPOUND);
                for (PersistentDataContainer c : containers) {
                    out.add(((HostPersistentDataContainer) c).tags.copy());
                }
                yield out;
            }
            case List<?> list -> {
                final Nbt.NbtList out = new Nbt.NbtList(Nbt.END);
                for (Object element : list) {
                    out.add(wrapPrimitive(element));
                }
                yield out;
            }
            default -> throw new IllegalArgumentException("Could not find a valid tag type for " + primitive.getClass().getName());
        };
    }

    private static boolean matches(final PersistentDataType<?, ?> type, final Object tag) {
        if (type instanceof ListPersistentDataType<?, ?> list) {
            if (!(tag instanceof Nbt.NbtList nbtList)) {
                return false;
            }
            for (Object element : nbtList) {
                if (!matches(list.elementType(), element)) {
                    return false;
                }
            }
            return true;
        }
        final Class<?> primitive = type.getPrimitiveType();
        return switch (tag) {
            case Byte b -> primitive == Byte.class;
            case Short s -> primitive == Short.class;
            case Integer i -> primitive == Integer.class;
            case Long l -> primitive == Long.class;
            case Float f -> primitive == Float.class;
            case Double d -> primitive == Double.class;
            case String s -> primitive == String.class;
            case byte[] b -> primitive == byte[].class;
            case int[] i -> primitive == int[].class;
            case long[] l -> primitive == long[].class;
            case Nbt.Compound c -> primitive == PersistentDataContainer.class;
            case Nbt.NbtList l -> primitive == PersistentDataContainer[].class && (l.isEmpty() || l.elementType() == Nbt.COMPOUND) || List.class.isAssignableFrom(primitive);
            default -> false;
        };
    }

    private static Object unwrap(final PersistentDataType<?, ?> type, final Object tag) {
        if (type instanceof ListPersistentDataType<?, ?> list) {
            final List<Object> out = new ArrayList<>();
            for (Object element : (Nbt.NbtList) tag) {
                out.add(unwrap(list.elementType(), element));
            }
            return out;
        }
        return switch (tag) {
            case byte[] b -> b.clone();
            case int[] i -> i.clone();
            case long[] l -> l.clone();
            case Nbt.Compound c -> new HostPersistentDataContainer(c.copy());
            case Nbt.NbtList l -> {
                final PersistentDataContainer[] out = new PersistentDataContainer[l.size()];
                for (int i = 0; i < out.length; i++) {
                    out[i] = new HostPersistentDataContainer(((Nbt.Compound) l.get(i)).copy());
                }
                yield out;
            }
            default -> tag;
        };
    }

    // --- the API ----------------------------------------------------------------------------

    @Override
    @SuppressWarnings({"unchecked", "rawtypes"})
    public <P, C> void set(final NamespacedKey key, final PersistentDataType<P, C> type, final C value) {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(type, "type");
        Objects.requireNonNull(value, "value");
        final P primitive = type.toPrimitive(value, CONTEXT);
        this.tags.put(key.toString(), wrap((PersistentDataType) type, primitive));
        this.changed();
    }

    @Override
    public void remove(final NamespacedKey key) {
        if (this.tags.remove(key.toString()) != null) {
            this.changed();
        }
    }

    @Override
    public void readFromBytes(final byte[] bytes, final boolean clear) throws IOException {
        if (clear) {
            this.tags.clear();
        }
        try (DataInputStream in = new DataInputStream(new ByteArrayInputStream(bytes))) {
            final int type = in.readUnsignedByte();
            if (type != Nbt.COMPOUND) {
                throw new IOException("not a persistent data container");
            }
            in.readUTF();
            final byte[] rest = in.readAllBytes();
            final byte[] root = new byte[rest.length + 1];
            root[0] = Nbt.COMPOUND;
            System.arraycopy(rest, 0, root, 1, rest.length);
            this.tags.putAll(Nbt.readCompound(root));
        }
        this.changed();
    }

    @Override
    public <P, C> boolean has(final NamespacedKey key, final PersistentDataType<P, C> type) {
        final Object tag = this.tags.get(key.toString());
        return tag != null && matches(type, tag);
    }

    @Override
    public boolean has(final NamespacedKey key) {
        return this.tags.containsKey(key.toString());
    }

    @Override
    @SuppressWarnings({"unchecked", "rawtypes"})
    public <P, C> @Nullable C get(final NamespacedKey key, final PersistentDataType<P, C> type) {
        final Object tag = this.tags.get(key.toString());
        if (tag == null) {
            return null;
        }
        if (!matches(type, tag)) {
            throw new IllegalArgumentException("The found tag instance (" + tag.getClass().getSimpleName() + ") cannot store " + type.getPrimitiveType().getSimpleName());
        }
        return type.fromPrimitive((P) unwrap((PersistentDataType) type, tag), CONTEXT);
    }

    @Override
    public <P, C> C getOrDefault(final NamespacedKey key, final PersistentDataType<P, C> type, final C defaultValue) {
        final C value = this.get(key, type);
        return value != null ? value : defaultValue;
    }

    @Override
    public @Nullable PersistentDataType<?, ?> getPrimitiveStorageType(final Key key) {
        final Object tag = this.tags.get(key.asString());
        if (tag == null) {
            return null;
        }
        return switch (tag) {
            case Byte b -> PersistentDataType.BYTE;
            case Short s -> PersistentDataType.SHORT;
            case Integer i -> PersistentDataType.INTEGER;
            case Long l -> PersistentDataType.LONG;
            case Float f -> PersistentDataType.FLOAT;
            case Double d -> PersistentDataType.DOUBLE;
            case String s -> PersistentDataType.STRING;
            case byte[] b -> PersistentDataType.BYTE_ARRAY;
            case int[] i -> PersistentDataType.INTEGER_ARRAY;
            case long[] l -> PersistentDataType.LONG_ARRAY;
            case Nbt.Compound c -> PersistentDataType.TAG_CONTAINER;
            default -> PersistentDataType.TAG_CONTAINER_ARRAY;
        };
    }

    @Override
    public Set<NamespacedKey> getKeys() {
        final Set<NamespacedKey> keys = new LinkedHashSet<>();
        for (String key : this.tags.keySet()) {
            final NamespacedKey parsed = NamespacedKey.fromString(key);
            if (parsed != null) {
                keys.add(parsed);
            }
        }
        return keys;
    }

    @Override
    public boolean isEmpty() {
        return this.tags.isEmpty();
    }

    @Override
    public void copyTo(final PersistentDataContainer other, final boolean replace) {
        final Nbt.Compound target = ((HostPersistentDataContainer) other).tags;
        for (Map.Entry<String, Object> e : this.tags.entrySet()) {
            if (replace || !target.containsKey(e.getKey())) {
                target.put(e.getKey(), Nbt.copy(e.getValue()));
            }
        }
    }

    @Override
    public PersistentDataAdapterContext getAdapterContext() {
        return CONTEXT;
    }

    @Override
    public byte[] serializeToBytes() throws IOException {
        // Vanilla's NbtIo.write form: a named root tag with an empty name.
        final byte[] network = Nbt.writeCompound(this.tags);
        final ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        try (DataOutputStream out = new DataOutputStream(bytes)) {
            out.writeByte(Nbt.COMPOUND);
            out.writeUTF("");
            out.write(network, 1, network.length - 1);
        }
        return bytes.toByteArray();
    }

    @Override
    public int getSize() {
        return this.tags.size();
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostPersistentDataContainer other && Nbt.equal(this.tags, other.tags);
    }

    @Override
    public int hashCode() {
        return Arrays.deepHashCode(this.tags.keySet().toArray());
    }

    @Override
    public String toString() {
        return "HostPersistentDataContainer" + this.tags.keySet();
    }
}
