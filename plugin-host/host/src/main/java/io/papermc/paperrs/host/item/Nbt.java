package io.papermc.paperrs.host.item;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Named Binary Tag values, in the network form item components use (an unnamed root tag).
 *
 * <p>Tags map to Java as: byte {@link Byte}, short {@link Short}, int {@link Integer}, long
 * {@link Long}, float {@link Float}, double {@link Double}, string {@link String}, byte array
 * {@code byte[]}, int array {@code int[]}, long array {@code long[]}, list {@link NbtList},
 * compound {@link Compound}. Reading and writing keep every type, so data that passes
 * through the plugin host unchanged is written back byte for byte.
 */
public final class Nbt {
    public static final int END = 0;
    public static final int BYTE = 1;
    public static final int SHORT = 2;
    public static final int INT = 3;
    public static final int LONG = 4;
    public static final int FLOAT = 5;
    public static final int DOUBLE = 6;
    public static final int BYTE_ARRAY = 7;
    public static final int STRING = 8;
    public static final int LIST = 9;
    public static final int COMPOUND = 10;
    public static final int INT_ARRAY = 11;
    public static final int LONG_ARRAY = 12;

    private static final int MAX_DEPTH = 512;

    private Nbt() {
    }

    /** A compound tag: names to tags, in insertion order. */
    public static final class Compound extends LinkedHashMap<String, Object> {
        public Compound() {
        }

        public Compound(final Map<String, Object> other) {
            super(other);
        }

        /** A deep copy. */
        public Compound copy() {
            final Compound out = new Compound();
            this.forEach((k, v) -> out.put(k, Nbt.copy(v)));
            return out;
        }
    }

    /** A list tag with its element type (kept for empty lists). */
    public static final class NbtList extends ArrayList<Object> {
        private int elementType;

        public NbtList(final int elementType) {
            this.elementType = elementType;
        }

        public int elementType() {
            return this.isEmpty() ? this.elementType : typeOf(this.getFirst());
        }

        public NbtList copy() {
            final NbtList out = new NbtList(this.elementType);
            for (Object v : this) {
                out.add(Nbt.copy(v));
            }
            return out;
        }
    }

    /** The tag type id of a value. */
    public static int typeOf(final Object value) {
        return switch (value) {
            case Byte b -> BYTE;
            case Short s -> SHORT;
            case Integer i -> INT;
            case Long l -> LONG;
            case Float f -> FLOAT;
            case Double d -> DOUBLE;
            case byte[] b -> BYTE_ARRAY;
            case String s -> STRING;
            case NbtList l -> LIST;
            case Compound c -> COMPOUND;
            case int[] i -> INT_ARRAY;
            case long[] l -> LONG_ARRAY;
            default -> throw new IllegalArgumentException("not an NBT value: " + value.getClass().getName());
        };
    }

    /** A deep copy of a tag value. */
    public static Object copy(final Object value) {
        return switch (value) {
            case byte[] b -> b.clone();
            case int[] i -> i.clone();
            case long[] l -> l.clone();
            case NbtList l -> l.copy();
            case Compound c -> c.copy();
            default -> value;
        };
    }

    /** Structural equality of two tag values (arrays by content). */
    public static boolean equal(final Object a, final Object b) {
        if (a instanceof byte[] x && b instanceof byte[] y) {
            return Arrays.equals(x, y);
        }
        if (a instanceof int[] x && b instanceof int[] y) {
            return Arrays.equals(x, y);
        }
        if (a instanceof long[] x && b instanceof long[] y) {
            return Arrays.equals(x, y);
        }
        if (a instanceof Map<?, ?> x && b instanceof Map<?, ?> y) {
            if (x.size() != y.size()) {
                return false;
            }
            for (Map.Entry<?, ?> e : x.entrySet()) {
                if (!y.containsKey(e.getKey()) || !equal(e.getValue(), y.get(e.getKey()))) {
                    return false;
                }
            }
            return true;
        }
        if (a instanceof List<?> x && b instanceof List<?> y) {
            if (x.size() != y.size()) {
                return false;
            }
            for (int i = 0; i < x.size(); i++) {
                if (!equal(x.get(i), y.get(i))) {
                    return false;
                }
            }
            return true;
        }
        return Objects.equals(a, b);
    }

    // --- reading --------------------------------------------------------------------------

    /** Reads an unnamed root compound (the network form); empty input is an empty compound. */
    public static Compound readCompound(final byte[] bytes) {
        if (bytes.length == 0) {
            return new Compound();
        }
        try (DataInputStream in = new DataInputStream(new ByteArrayInputStream(bytes))) {
            final int type = in.readUnsignedByte();
            if (type == END) {
                return new Compound();
            }
            if (type != COMPOUND) {
                throw new IOException("root tag is not a compound (type " + type + ")");
            }
            return (Compound) readPayload(in, COMPOUND, 0);
        } catch (IOException e) {
            throw new UncheckedIOException("malformed NBT", e);
        }
    }

    private static Object readPayload(final DataInputStream in, final int type, final int depth) throws IOException {
        if (depth > MAX_DEPTH) {
            throw new IOException("NBT nested too deeply");
        }
        return switch (type) {
            case BYTE -> in.readByte();
            case SHORT -> in.readShort();
            case INT -> in.readInt();
            case LONG -> in.readLong();
            case FLOAT -> in.readFloat();
            case DOUBLE -> in.readDouble();
            case BYTE_ARRAY -> {
                final byte[] b = new byte[checkedLength(in.readInt())];
                in.readFully(b);
                yield b;
            }
            case STRING -> in.readUTF();
            case LIST -> {
                final int elementType = in.readUnsignedByte();
                final int length = checkedLength(in.readInt());
                final NbtList list = new NbtList(elementType);
                for (int i = 0; i < length; i++) {
                    list.add(readPayload(in, elementType, depth + 1));
                }
                yield list;
            }
            case COMPOUND -> {
                final Compound c = new Compound();
                while (true) {
                    final int t = in.readUnsignedByte();
                    if (t == END) {
                        break;
                    }
                    final String name = in.readUTF();
                    c.put(name, readPayload(in, t, depth + 1));
                }
                yield c;
            }
            case INT_ARRAY -> {
                final int[] a = new int[checkedLength(in.readInt())];
                for (int i = 0; i < a.length; i++) {
                    a[i] = in.readInt();
                }
                yield a;
            }
            case LONG_ARRAY -> {
                final long[] a = new long[checkedLength(in.readInt())];
                for (int i = 0; i < a.length; i++) {
                    a[i] = in.readLong();
                }
                yield a;
            }
            default -> throw new IOException("unknown NBT tag type " + type);
        };
    }

    private static int checkedLength(final int length) throws IOException {
        if (length < 0 || length > 16 * 1024 * 1024) {
            throw new IOException("bad NBT length " + length);
        }
        return length;
    }

    // --- writing --------------------------------------------------------------------------

    /** Writes a compound as an unnamed root tag (the network form). */
    public static byte[] writeCompound(final Compound compound) {
        final ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        try (DataOutputStream out = new DataOutputStream(bytes)) {
            out.writeByte(COMPOUND);
            writePayload(out, compound);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
        return bytes.toByteArray();
    }

    private static void writePayload(final DataOutputStream out, final Object value) throws IOException {
        switch (value) {
            case Byte b -> out.writeByte(b);
            case Short s -> out.writeShort(s);
            case Integer i -> out.writeInt(i);
            case Long l -> out.writeLong(l);
            case Float f -> out.writeFloat(f);
            case Double d -> out.writeDouble(d);
            case byte[] b -> {
                out.writeInt(b.length);
                out.write(b);
            }
            case String s -> out.writeUTF(s);
            case NbtList l -> {
                out.writeByte(l.isEmpty() ? l.elementType : typeOf(l.getFirst()));
                out.writeInt(l.size());
                for (Object v : l) {
                    writePayload(out, v);
                }
            }
            case Compound c -> {
                for (Map.Entry<String, Object> e : c.entrySet()) {
                    out.writeByte(typeOf(e.getValue()));
                    out.writeUTF(e.getKey());
                    writePayload(out, e.getValue());
                }
                out.writeByte(END);
            }
            case int[] a -> {
                out.writeInt(a.length);
                for (int i : a) {
                    out.writeInt(i);
                }
            }
            case long[] a -> {
                out.writeInt(a.length);
                for (long l : a) {
                    out.writeLong(l);
                }
            }
            default -> throw new IllegalArgumentException("not an NBT value: " + value.getClass().getName());
        }
    }
}
