package io.papermc.paperrs.host.bridge;

import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

/**
 * Wire format of the plugin bridge; the Java twin of the Rust crate {@code paper-bridge}.
 *
 * <pre>
 * frame   := u32 length (big endian) | u8 kind | body
 * HELLO   (1) := value                  WELCOME (2) := value
 * CALL    (3) := uvarint id | u8 flags | string method | value
 * RETURN  (4) := uvarint id | u8 status (0 ok, 1 error) | value
 * NOTIFY  (5) := string method | value
 * value   := 0 null | 1 false | 2 true | 3 zigzag varint | 4 f64 | 5 string | 6 bytes
 *          | 7 uvarint n, value*n | 8 uvarint n, (string value)*n
 * </pre>
 *
 * Values map to Java as: {@code null}, {@link Boolean}, {@link Long}, {@link Double},
 * {@link String}, {@code byte[]}, {@link List}, {@link Map} with {@link String} keys. Any
 * integral {@link Number} encodes as an integer and any other {@link Number} as a double.
 * Map keys are written in sorted order so equal values always encode to equal bytes.
 */
public final class Codec {
    public static final int HELLO = 1;
    public static final int WELCOME = 2;
    public static final int CALL = 3;
    public static final int RETURN = 4;
    public static final int NOTIFY = 5;

    /** CALL flag: the caller holds the main-thread baton; see {@link Bridge}. */
    public static final int FLAG_SYNC = 1;

    public static final int MAX_FRAME_LEN = 64 * 1024 * 1024;

    private Codec() {
    }

    /** One decoded frame. Unused fields are zero or null. */
    public record Frame(int kind, long id, int flags, boolean ok, String method, Object value) {
        public static Frame hello(Object value) {
            return new Frame(HELLO, 0, 0, true, null, value);
        }

        public static Frame welcome(Object value) {
            return new Frame(WELCOME, 0, 0, true, null, value);
        }

        public static Frame call(long id, int flags, String method, Object args) {
            return new Frame(CALL, id, flags, true, method, args);
        }

        public static Frame ret(long id, boolean ok, Object value) {
            return new Frame(RETURN, id, 0, ok, null, value);
        }

        public static Frame notify(String method, Object args) {
            return new Frame(NOTIFY, 0, 0, true, method, args);
        }
    }

    // --- encoding --------------------------------------------------------------------------

    private static final class Out extends ByteArrayOutputStream {
        Out(int size) {
            super(size);
        }

        void uvarint(long v) {
            while ((v & ~0x7FL) != 0) {
                this.write((int) ((v & 0x7F) | 0x80));
                v >>>= 7;
            }
            this.write((int) v);
        }

        void str(String s) {
            byte[] b = s.getBytes(StandardCharsets.UTF_8);
            this.uvarint(b.length);
            this.write(b, 0, b.length);
        }

        void f64(double d) {
            long bits = Double.doubleToRawLongBits(d);
            for (int shift = 56; shift >= 0; shift -= 8) {
                this.write((int) (bits >>> shift) & 0xFF);
            }
        }

        byte[] buffer() {
            return this.buf;
        }
    }

    private static void value(Out out, Object v) {
        switch (v) {
            case null -> out.write(0);
            case Boolean b -> out.write(b ? 2 : 1);
            case Long l -> integer(out, l);
            case Integer i -> integer(out, i);
            case Short s -> integer(out, s);
            case Byte b -> integer(out, b);
            case Number n -> {
                out.write(4);
                out.f64(n.doubleValue());
            }
            case String s -> {
                out.write(5);
                out.str(s);
            }
            case Character c -> {
                out.write(5);
                out.str(String.valueOf(c));
            }
            case byte[] b -> {
                out.write(6);
                out.uvarint(b.length);
                out.write(b, 0, b.length);
            }
            case List<?> list -> {
                out.write(7);
                out.uvarint(list.size());
                for (Object item : list) {
                    value(out, item);
                }
            }
            case Map<?, ?> map -> {
                out.write(8);
                out.uvarint(map.size());
                Map<String, Object> sorted = new TreeMap<>();
                for (Map.Entry<?, ?> e : map.entrySet()) {
                    sorted.put((String) e.getKey(), e.getValue());
                }
                for (Map.Entry<String, Object> e : sorted.entrySet()) {
                    out.str(e.getKey());
                    value(out, e.getValue());
                }
            }
            case Enum<?> e -> {
                out.write(5);
                out.str(e.name());
            }
            default -> throw new IllegalArgumentException("cannot encode " + v.getClass().getName() + " on the plugin bridge");
        }
    }

    private static void integer(Out out, long i) {
        out.write(3);
        out.uvarint((i << 1) ^ (i >> 63));
    }

    /** The complete frame including its length prefix. */
    public static byte[] encode(Frame frame) {
        Out out = new Out(64);
        out.write(0);
        out.write(0);
        out.write(0);
        out.write(0);
        out.write(frame.kind());
        switch (frame.kind()) {
            case HELLO, WELCOME -> value(out, frame.value());
            case CALL -> {
                out.uvarint(frame.id());
                out.write(frame.flags());
                out.str(frame.method());
                value(out, frame.value());
            }
            case RETURN -> {
                out.uvarint(frame.id());
                out.write(frame.ok() ? 0 : 1);
                value(out, frame.value());
            }
            case NOTIFY -> {
                out.str(frame.method());
                value(out, frame.value());
            }
            default -> throw new IllegalArgumentException("unknown frame kind " + frame.kind());
        }
        int len = out.size() - 4;
        byte[] bytes = out.buffer();
        bytes[0] = (byte) (len >>> 24);
        bytes[1] = (byte) (len >>> 16);
        bytes[2] = (byte) (len >>> 8);
        bytes[3] = (byte) len;
        return java.util.Arrays.copyOf(bytes, out.size());
    }

    // --- decoding --------------------------------------------------------------------------

    private static final class In {
        private final byte[] buf;
        private int pos;
        private int depth;

        In(byte[] buf) {
            this.buf = buf;
        }

        int u8() {
            if (this.pos >= this.buf.length) {
                throw new IllegalArgumentException("truncated bridge frame");
            }
            return this.buf[this.pos++] & 0xFF;
        }

        long uvarint() {
            long v = 0;
            for (int shift = 0; shift < 70; shift += 7) {
                int b = this.u8();
                v |= (long) (b & 0x7F) << shift;
                if ((b & 0x80) == 0) {
                    return v;
                }
            }
            throw new IllegalArgumentException("varint too long");
        }

        int len() {
            long n = this.uvarint();
            if (n > this.buf.length - this.pos) {
                throw new IllegalArgumentException("length exceeds frame");
            }
            return (int) n;
        }

        String str() {
            int n = this.len();
            String s = new String(this.buf, this.pos, n, StandardCharsets.UTF_8);
            this.pos += n;
            return s;
        }

        Object value() {
            if (this.depth > 128) {
                throw new IllegalArgumentException("value nested too deeply");
            }
            int tag = this.u8();
            return switch (tag) {
                case 0 -> null;
                case 1 -> Boolean.FALSE;
                case 2 -> Boolean.TRUE;
                case 3 -> {
                    long z = this.uvarint();
                    yield (z >>> 1) ^ -(z & 1);
                }
                case 4 -> {
                    long bits = 0;
                    for (int i = 0; i < 8; i++) {
                        bits = (bits << 8) | this.u8();
                    }
                    yield Double.longBitsToDouble(bits);
                }
                case 5 -> this.str();
                case 6 -> {
                    int n = this.len();
                    byte[] b = java.util.Arrays.copyOfRange(this.buf, this.pos, this.pos + n);
                    this.pos += n;
                    yield b;
                }
                case 7 -> {
                    int n = this.len();
                    this.depth++;
                    List<Object> list = new ArrayList<>(n);
                    for (int i = 0; i < n; i++) {
                        list.add(this.value());
                    }
                    this.depth--;
                    yield list;
                }
                case 8 -> {
                    int n = this.len();
                    this.depth++;
                    Map<String, Object> map = new LinkedHashMap<>();
                    for (int i = 0; i < n; i++) {
                        String key = this.str();
                        map.put(key, this.value());
                    }
                    this.depth--;
                    yield map;
                }
                default -> throw new IllegalArgumentException("unknown value tag " + tag);
            };
        }

        void finish() {
            if (this.pos != this.buf.length) {
                throw new IllegalArgumentException("trailing bytes in bridge frame");
            }
        }
    }

    /** Decodes a frame body (the bytes after the length prefix). */
    public static Frame decode(byte[] body) {
        In in = new In(body);
        int kind = in.u8();
        Frame frame = switch (kind) {
            case HELLO -> Frame.hello(in.value());
            case WELCOME -> Frame.welcome(in.value());
            case CALL -> {
                long id = in.uvarint();
                int flags = in.u8();
                String method = in.str();
                yield Frame.call(id, flags, method, in.value());
            }
            case RETURN -> {
                long id = in.uvarint();
                int status = in.u8();
                yield Frame.ret(id, status == 0, in.value());
            }
            case NOTIFY -> {
                String method = in.str();
                yield Frame.notify(method, in.value());
            }
            default -> throw new IllegalArgumentException("unknown frame kind " + kind);
        };
        in.finish();
        return frame;
    }

    /** Reads one frame; returns null on a clean end of stream. */
    public static Frame read(InputStream stream) throws IOException {
        DataInputStream in = stream instanceof DataInputStream d ? d : new DataInputStream(stream);
        int b0 = in.read();
        if (b0 < 0) {
            return null;
        }
        int len;
        try {
            len = (b0 << 24) | (in.readUnsignedByte() << 16) | (in.readUnsignedByte() << 8) | in.readUnsignedByte();
        } catch (EOFException e) {
            throw new IOException("stream ended inside a frame header", e);
        }
        if (len < 0 || len > MAX_FRAME_LEN) {
            throw new IOException("bridge frame of " + len + " bytes exceeds the limit");
        }
        byte[] body = new byte[len];
        in.readFully(body);
        return decode(body);
    }
}
