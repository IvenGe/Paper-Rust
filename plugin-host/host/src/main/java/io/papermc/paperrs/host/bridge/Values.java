package io.papermc.paperrs.host.bridge;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;

/** Typed access to bridge values (maps with string keys, lists, numbers, strings). */
public final class Values {
    private Values() {
    }

    @SuppressWarnings("unchecked")
    public static Map<String, Object> map(Object v) {
        if (v instanceof Map<?, ?> m) {
            return (Map<String, Object>) m;
        }
        if (v == null) {
            return Map.of();
        }
        throw new IllegalArgumentException("expected a map, got " + v.getClass().getSimpleName());
    }

    @SuppressWarnings("unchecked")
    public static List<Object> list(Object v) {
        if (v instanceof List<?> l) {
            return (List<Object>) l;
        }
        if (v == null) {
            return List.of();
        }
        throw new IllegalArgumentException("expected a list, got " + v.getClass().getSimpleName());
    }

    public static String str(Map<String, Object> m, String key) {
        Object v = m.get(key);
        if (v == null) {
            throw new IllegalArgumentException("missing field " + key);
        }
        return v.toString();
    }

    public static String str(Map<String, Object> m, String key, String fallback) {
        Object v = m.get(key);
        return v == null ? fallback : v.toString();
    }

    public static long i64(Map<String, Object> m, String key) {
        Object v = m.get(key);
        if (v instanceof Number n) {
            return n.longValue();
        }
        throw new IllegalArgumentException("missing integer field " + key);
    }

    public static long i64(Map<String, Object> m, String key, long fallback) {
        return m.get(key) instanceof Number n ? n.longValue() : fallback;
    }

    public static int i32(Map<String, Object> m, String key) {
        return (int) i64(m, key);
    }

    public static int i32(Map<String, Object> m, String key, int fallback) {
        return (int) i64(m, key, fallback);
    }

    public static double f64(Map<String, Object> m, String key) {
        Object v = m.get(key);
        if (v instanceof Number n) {
            return n.doubleValue();
        }
        throw new IllegalArgumentException("missing number field " + key);
    }

    public static double f64(Map<String, Object> m, String key, double fallback) {
        return m.get(key) instanceof Number n ? n.doubleValue() : fallback;
    }

    public static boolean bool(Map<String, Object> m, String key, boolean fallback) {
        return m.get(key) instanceof Boolean b ? b : fallback;
    }

    public static UUID uuid(Map<String, Object> m, String key) {
        return UUID.fromString(str(m, key));
    }

    /** Builds a map from alternating keys and values: {@code of("a", 1, "b", "x")}. */
    public static Map<String, Object> of(Object... kv) {
        if (kv.length % 2 != 0) {
            throw new IllegalArgumentException("odd number of arguments");
        }
        Map<String, Object> m = new LinkedHashMap<>();
        for (int i = 0; i < kv.length; i += 2) {
            m.put((String) kv[i], kv[i + 1]);
        }
        return m;
    }
}
