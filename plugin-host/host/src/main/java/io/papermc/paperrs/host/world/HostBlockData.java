package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.Unsupported;
import io.papermc.paperrs.host.data.HostData;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.lang.reflect.ParameterizedType;
import java.lang.reflect.Proxy;
import java.lang.reflect.Type;
import java.util.ArrayList;
import java.util.EnumSet;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.TreeSet;
import org.bukkit.Instrument;
import org.bukkit.Material;
import org.bukkit.Note;
import org.bukkit.block.BlockFace;
import org.bukkit.block.BlockType;
import org.bukkit.block.data.Bisected;
import org.bukkit.block.data.BlockData;
import org.jetbrains.annotations.Nullable;

/**
 * {@code BlockData}: a block state as a block name and property values, like
 * {@code CraftBlockData}. Each block's data is a proxy implementing the interface Paper gives
 * that block (the type argument of its {@code BlockType.Typed} constant: {@code Stairs} for
 * {@code oak_stairs}, {@code Ageable} for {@code wheat}), and the interface's getters and
 * setters read and write the block's properties ({@code getAge} is {@code age},
 * {@code isInWall} is {@code in_wall}, {@code getHeight(BlockFace)} is the property named after
 * the face). Methods about the block rather than its properties (sound groups, placement
 * checks, ...) throw {@link Unsupported}.
 */
public final class HostBlockData {
    private final HostData.BlockInfo block;
    /** Every property's value, in the block's (alphabetical) property order. */
    private final LinkedHashMap<String, String> values;
    /** The properties given explicitly, as {@code matches}, {@code merge} and {@code getAsString(true)} need. */
    private final Set<String> explicit;

    private HostBlockData(final HostData.BlockInfo block, final LinkedHashMap<String, String> values, final Set<String> explicit) {
        this.block = block;
        this.values = values;
        this.explicit = explicit;
    }

    // --- making them ----------------------------------------------------------------------------

    private static HostData.BlockInfo info(final String name) {
        final String key = (name.contains(":") ? name : "minecraft:" + name).toLowerCase(Locale.ROOT);
        final HostData.BlockInfo info = HostData.get().block(key);
        if (info == null) {
            throw new IllegalArgumentException("Unknown block " + name);
        }
        return info;
    }

    /** The default state of a block ({@code Material#createBlockData()}). */
    public static BlockData defaults(final Material material) {
        if (!material.isBlock()) {
            throw new IllegalArgumentException(material + " is not a block");
        }
        return defaults(material.getKey().toString());
    }

    /** The default state of the block with this key. */
    public static BlockData defaults(final String block) {
        final HostData.BlockInfo info = info(block);
        return proxy(new HostBlockData(info, new LinkedHashMap<>(HostData.get().properties(info.defaultState())), new HashSet<>()));
    }

    /**
     * {@code Bukkit.createBlockData}: {@code minecraft:oak_stairs[facing=east]}, or with a
     * material just the properties ({@code [facing=east]}). Properties not given keep their
     * default values.
     */
    public static BlockData parse(final @Nullable Material material, final @Nullable String data) {
        final String text = data == null ? "" : data.trim();
        String name = text;
        String props = "";
        final int bracket = text.indexOf('[');
        if (bracket >= 0) {
            if (!text.endsWith("]")) {
                throw new IllegalArgumentException("Could not parse data: " + data);
            }
            name = text.substring(0, bracket);
            props = text.substring(bracket + 1, text.length() - 1);
        }
        if (name.isEmpty()) {
            if (material == null) {
                throw new IllegalArgumentException("Could not parse data: " + data);
            }
            name = material.getKey().toString();
        }
        final HostData.BlockInfo info = info(name);
        if (material != null && !info.name().equals(material.getKey().toString())) {
            throw new IllegalArgumentException("Could not parse data: " + data + " (it is not " + material + ")");
        }
        final LinkedHashMap<String, String> values = new LinkedHashMap<>(HostData.get().properties(info.defaultState()));
        final Set<String> explicit = new HashSet<>();
        if (!props.isBlank()) {
            for (final String pair : props.split(",")) {
                final int eq = pair.indexOf('=');
                final String key = eq < 0 ? pair.trim() : pair.substring(0, eq).trim();
                final String value = eq < 0 ? "" : pair.substring(eq + 1).trim();
                final HostData.Property property = property(info, key);
                if (property == null || !property.values().contains(value)) {
                    throw new IllegalArgumentException("Could not parse data: " + data + " (" + key + "=" + value + ")");
                }
                values.put(key, value);
                explicit.add(key);
            }
        }
        return proxy(new HostBlockData(info, values, explicit));
    }

    /** A block state read from the world, as {@code minecraft:name[property=value,...]}. */
    public static BlockData ofState(final String state) {
        final BlockData data = parse(null, state);
        final HostBlockData impl = impl(data);
        impl.explicit.addAll(impl.values.keySet());
        return data;
    }

    /** Every state of a block ({@code BlockType#createBlockDataStates()}). */
    public static List<BlockData> allStates(final String block) {
        final HostData.BlockInfo info = info(block);
        final List<BlockData> out = new ArrayList<>(info.stateCount());
        for (int state = info.minState(); state < info.minState() + info.stateCount(); state++) {
            final LinkedHashMap<String, String> values = new LinkedHashMap<>(HostData.get().properties(state));
            out.add(proxy(new HostBlockData(info, values, new HashSet<>(values.keySet()))));
        }
        return out;
    }

    /** The implementation behind a {@code BlockData} made here. */
    public static HostBlockData impl(final BlockData data) {
        if (data != null && Proxy.isProxyClass(data.getClass()) && Proxy.getInvocationHandler(data) instanceof Handler handler) {
            return handler.impl;
        }
        throw new IllegalArgumentException("Not block data of this server: " + data);
    }

    private static @Nullable HostData.Property property(final HostData.BlockInfo info, final String name) {
        for (final HostData.Property p : info.properties()) {
            if (p.name().equals(name)) {
                return p;
            }
        }
        return null;
    }

    private HostBlockData copy() {
        return new HostBlockData(this.block, new LinkedHashMap<>(this.values), new HashSet<>(this.explicit));
    }

    // --- the API interface of each block --------------------------------------------------------

    private static volatile Map<String, Class<? extends BlockData>> dataClasses;

    /**
     * The API interface of a block's data: {@code BlockType} declares a
     * {@code BlockType.Typed<Stairs> OAK_STAIRS} constant for every block, generated from the
     * same mapping {@code CraftBlockData} uses. (Only the declarations are read; reading the
     * constants would need the registries.)
     */
    @SuppressWarnings("unchecked")
    public static Class<? extends BlockData> dataClass(final String block) {
        Map<String, Class<? extends BlockData>> classes = dataClasses;
        if (classes == null) {
            classes = new HashMap<>();
            for (final Field field : BlockType.class.getFields()) {
                if (Modifier.isStatic(field.getModifiers()) && field.getGenericType() instanceof ParameterizedType type
                    && type.getRawType() == BlockType.Typed.class && type.getActualTypeArguments()[0] instanceof Class<?> data) {
                    classes.put("minecraft:" + field.getName().toLowerCase(Locale.ROOT), (Class<? extends BlockData>) data);
                }
            }
            dataClasses = classes;
        }
        return classes.getOrDefault(block, BlockData.class);
    }

    private static BlockData proxy(final HostBlockData impl) {
        return (BlockData) Proxy.newProxyInstance(HostBlockData.class.getClassLoader(), new Class<?>[] {dataClass(impl.block.name())}, new Handler(impl));
    }

    // --- what BlockData does itself -----------------------------------------------------------------

    public String blockName() {
        return this.block.name();
    }

    public Material getMaterial() {
        final Material material = Material.matchMaterial(this.block.name());
        return material == null ? Material.AIR : material;
    }

    public String getAsString(final boolean hideUnspecified) {
        final StringBuilder out = new StringBuilder(this.block.name());
        final List<String> shown = new ArrayList<>();
        this.values.forEach((k, v) -> {
            if (!hideUnspecified || this.explicit.contains(k)) {
                shown.add(k + "=" + v);
            }
        });
        if (!shown.isEmpty()) {
            out.append('[').append(String.join(",", shown)).append(']');
        }
        return out.toString();
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostBlockData other && other.block.name().equals(this.block.name()) && other.values.equals(this.values);
    }

    @Override
    public int hashCode() {
        return Objects.hash(this.block.name(), this.values);
    }

    @Override
    public String toString() {
        return "CraftBlockData{" + this.getAsString(false) + "}";
    }

    // --- property access ---------------------------------------------------------------------------

    private void set(final String name, final String value) {
        final HostData.Property property = property(this.block, name);
        if (property == null) {
            throw new IllegalArgumentException(this.block.name() + " has no property " + name);
        }
        if (!property.values().contains(value)) {
            throw new IllegalArgumentException("Invalid " + name + " " + value + " for " + this.block.name() + " (allowed: " + property.values() + ")");
        }
        this.values.put(name, value);
        this.explicit.add(name);
    }

    private int bound(final String name, final boolean max) {
        final HostData.Property property = property(this.block, name);
        if (property == null) {
            throw new IllegalArgumentException(this.block.name() + " has no property " + name);
        }
        return property.values().stream().mapToInt(Integer::parseInt).reduce(max ? Integer::max : Integer::min).orElse(0);
    }

    /** {@code HoneyLevel} -> {@code honey_level}. */
    private static String snake(final String camel) {
        final StringBuilder out = new StringBuilder();
        for (int i = 0; i < camel.length(); i++) {
            final char c = camel.charAt(i);
            if (Character.isUpperCase(c)) {
                if (i > 0) {
                    out.append('_');
                }
                out.append(Character.toLowerCase(c));
            } else {
                out.append(c);
            }
        }
        return out.toString();
    }

    /** The property a getter or setter suffix ({@code InWall}, {@code HasBook}, {@code Eye}) means, if the block has it. */
    private @Nullable String propertyFor(final String suffix) {
        final String name = snake(suffix);
        for (final String candidate : new String[] {name, "has_" + name, name.startsWith("has_") ? name.substring(4) : name}) {
            if (this.values.containsKey(candidate)) {
                return candidate;
            }
        }
        return null;
    }

    /** {@code Rotatable}: rotation 0 is south, 4 west, 8 north, 12 east. */
    private static final BlockFace[] ROTATIONS = {
        BlockFace.SOUTH, BlockFace.SOUTH_SOUTH_WEST, BlockFace.SOUTH_WEST, BlockFace.WEST_SOUTH_WEST, BlockFace.WEST, BlockFace.WEST_NORTH_WEST,
        BlockFace.NORTH_WEST, BlockFace.NORTH_NORTH_WEST, BlockFace.NORTH, BlockFace.NORTH_NORTH_EAST, BlockFace.NORTH_EAST, BlockFace.EAST_NORTH_EAST,
        BlockFace.EAST, BlockFace.EAST_SOUTH_EAST, BlockFace.SOUTH_EAST, BlockFace.SOUTH_SOUTH_EAST};

    /** Note block instruments whose API name is not the vanilla one in capitals. */
    private static final Map<String, String> INSTRUMENTS = Map.of("harp", "PIANO", "basedrum", "BASS_DRUM", "snare", "SNARE_DRUM", "hat", "STICKS", "bass", "BASS_GUITAR");

    /** A property value as the type a getter returns. */
    @SuppressWarnings({"unchecked", "rawtypes"})
    private static Object read(final String value, final Type generic) {
        final Class<?> type = generic instanceof ParameterizedType p ? (Class<?>) p.getRawType() : (Class<?>) generic;
        if (type == boolean.class || type == Boolean.class) {
            return Boolean.parseBoolean(value);
        }
        if (type == int.class || type == Integer.class) {
            return Integer.parseInt(value);
        }
        if (type == String.class) {
            return value;
        }
        if (type == Bisected.Half.class) {
            return value.equals("upper") || value.equals("top") ? Bisected.Half.TOP : Bisected.Half.BOTTOM;
        }
        if (type == Instrument.class) {
            return Instrument.valueOf(INSTRUMENTS.getOrDefault(value, value.toUpperCase(Locale.ROOT)));
        }
        if (type.isEnum()) {
            return Enum.valueOf((Class<Enum>) type, value.toUpperCase(Locale.ROOT));
        }
        throw new IllegalArgumentException("Cannot read " + value + " as " + type.getName());
    }

    /** A setter's argument as a value of property {@code name}. */
    private String write(final String name, final Object value) {
        Objects.requireNonNull(value, name);
        if (value instanceof Bisected.Half half) {
            final HostData.Property property = property(this.block, name);
            final boolean door = property != null && property.values().contains("upper");
            return half == Bisected.Half.TOP ? (door ? "upper" : "top") : (door ? "lower" : "bottom");
        }
        if (value instanceof Instrument instrument) {
            for (final Map.Entry<String, String> e : INSTRUMENTS.entrySet()) {
                if (e.getValue().equals(instrument.name())) {
                    return e.getKey();
                }
            }
            return instrument.name().toLowerCase(Locale.ROOT);
        }
        if (value instanceof Enum<?> e) {
            return e.name().toLowerCase(Locale.ROOT);
        }
        return String.valueOf(value);
    }

    /** The allowed values of a property, as the element type of a getter returning a set of them. */
    private Set<Object> allowed(final String name, final Type generic) {
        final Type element = generic instanceof ParameterizedType p ? p.getActualTypeArguments()[0] : Object.class;
        final Set<Object> out = new LinkedHashSet<>();
        for (final String v : property(this.block, name).values()) {
            out.add(read(v, element));
        }
        return out;
    }

    private static final List<BlockFace> SIDES = List.of(BlockFace.NORTH, BlockFace.EAST, BlockFace.SOUTH, BlockFace.WEST, BlockFace.UP, BlockFace.DOWN);

    /** {@code MultipleFacing}: the faces with a true/false property (or only those set). */
    private Set<BlockFace> booleanFaces(final boolean onlySet) {
        final Set<BlockFace> out = EnumSet.noneOf(BlockFace.class);
        for (final BlockFace face : SIDES) {
            final String v = this.values.get(face.name().toLowerCase(Locale.ROOT));
            if (v != null && (v.equals("true") || v.equals("false")) && (!onlySet || v.equals("true"))) {
                out.add(face);
            }
        }
        return out;
    }

    private Set<Integer> numbered(final String prefix, final String suffix) {
        final Set<Integer> out = new TreeSet<>();
        for (int i = 0; this.values.containsKey(prefix + i + suffix); i++) {
            if (this.values.get(prefix + i + suffix).equals("true")) {
                out.add(i);
            }
        }
        return out;
    }

    private int count(final String prefix, final String suffix) {
        int i = 0;
        while (this.values.containsKey(prefix + i + suffix)) {
            i++;
        }
        return i;
    }

    /** Runs a {@code BlockData} method on this state; {@code self} is the proxy. */
    private Object invoke(final Object self, final Method method, final Object[] args) throws Throwable {
        final String name = method.getName();
        final int argc = args == null ? 0 : args.length;
        switch (name) {
            case "getMaterial", "getPlacementMaterial" -> {
                return this.getMaterial();
            }
            case "getAsString" -> {
                return this.getAsString(argc == 1 && (boolean) args[0]);
            }
            case "clone" -> {
                return proxy(this.copy());
            }
            case "matches" -> {
                // The same block, with every property the other data was given equal.
                if (args[0] == null) {
                    return false;
                }
                final HostBlockData other = impl((BlockData) args[0]);
                if (!other.block.name().equals(this.block.name())) {
                    return false;
                }
                for (final String key : other.explicit) {
                    if (!Objects.equals(other.values.get(key), this.values.get(key))) {
                        return false;
                    }
                }
                return true;
            }
            case "merge" -> {
                final HostBlockData other = impl((BlockData) args[0]);
                if (!other.block.name().equals(this.block.name())) {
                    throw new IllegalArgumentException("Data not created for same block type: " + other.block.name() + " into " + this.block.name());
                }
                final HostBlockData merged = this.copy();
                for (final String key : other.explicit) {
                    merged.values.put(key, other.values.get(key));
                    merged.explicit.add(key);
                }
                return proxy(merged);
            }
            case "copyTo" -> {
                // The properties both blocks have, where the value is valid for the target.
                final HostBlockData other = impl((BlockData) args[0]);
                this.values.forEach((k, v) -> {
                    final HostData.Property p = property(other.block, k);
                    if (p != null && p.values().contains(v)) {
                        other.values.put(k, v);
                    }
                });
                return null;
            }
            case "isOccluding" -> {
                return this.getMaterial().isOccluding();
            }
            // MultipleFacing and Directional.
            case "hasFace" -> {
                return "true".equals(this.values.get(((BlockFace) args[0]).name().toLowerCase(Locale.ROOT)));
            }
            case "getFaces" -> {
                if (this.values.containsKey("facing")) {
                    final Set<BlockFace> faces = EnumSet.noneOf(BlockFace.class);
                    for (final String v : property(this.block, "facing").values()) {
                        faces.add(BlockFace.valueOf(v.toUpperCase(Locale.ROOT)));
                    }
                    return faces;
                }
                return this.booleanFaces(true);
            }
            case "getAllowedFaces" -> {
                if (this.block.name().equals("minecraft:redstone_wire")) {
                    return EnumSet.of(BlockFace.NORTH, BlockFace.EAST, BlockFace.SOUTH, BlockFace.WEST);
                }
                return this.booleanFaces(false);
            }
            case "getAxes" -> {
                return this.allowed("axis", method.getGenericReturnType());
            }
            case "getRotation" -> {
                return ROTATIONS[Integer.parseInt(this.values.get("rotation"))];
            }
            case "setRotation" -> {
                for (int i = 0; i < ROTATIONS.length; i++) {
                    if (ROTATIONS[i] == args[0]) {
                        this.set("rotation", Integer.toString(i));
                        return null;
                    }
                }
                throw new IllegalArgumentException("Illegal rotation " + args[0]);
            }
            case "getAttachedFace" -> {
                return read(this.values.get("face"), method.getGenericReturnType());
            }
            case "setAttachedFace" -> {
                this.set("face", this.write("face", args[0]));
                return null;
            }
            case "getNote" -> {
                return new Note(Integer.parseInt(this.values.get("note")));
            }
            case "setNote" -> {
                this.set("note", Integer.toString(((Note) args[0]).getId()));
                return null;
            }
            // BrewingStand and ChiseledBookshelf: numbered properties.
            case "hasBottle" -> {
                return "true".equals(this.values.get("has_bottle_" + args[0]));
            }
            case "setBottle" -> {
                this.set("has_bottle_" + args[0], String.valueOf(args[1]));
                return null;
            }
            case "getBottles" -> {
                return this.numbered("has_bottle_", "");
            }
            case "getMaximumBottles" -> {
                return this.count("has_bottle_", "");
            }
            case "isSlotOccupied" -> {
                return "true".equals(this.values.get("slot_" + args[0] + "_occupied"));
            }
            case "setSlotOccupied" -> {
                this.set("slot_" + args[0] + "_occupied", String.valueOf(args[1]));
                return null;
            }
            case "getOccupiedSlots" -> {
                return this.numbered("slot_", "_occupied");
            }
            case "getMaximumOccupiedSlots" -> {
                return this.count("slot_", "_occupied");
            }
            default -> {
            }
        }
        // Deprecated aliases and conveniences (SculkSensor#getPhase, Vault#getTrialSpawnerState,
        // CreakingHeart#isActive, ...) are default methods over the real accessors.
        if (method.isDefault()) {
            return InvocationHandler.invokeDefault(self, method, args);
        }
        // Wall#getHeight(BlockFace), RedstoneWire#getFace(BlockFace) and their setters.
        if (argc == 1 && args[0] instanceof BlockFace face && name.startsWith("get") && this.values.containsKey(face.name().toLowerCase(Locale.ROOT))) {
            return read(this.values.get(face.name().toLowerCase(Locale.ROOT)), method.getGenericReturnType());
        }
        if (argc == 2 && args[0] instanceof BlockFace face && name.startsWith("set") && this.values.containsKey(face.name().toLowerCase(Locale.ROOT))) {
            final String key = face.name().toLowerCase(Locale.ROOT);
            this.set(key, this.write(key, args[1]));
            return null;
        }
        if (argc == 0 && name.startsWith("getMaximum") && this.propertyFor(name.substring(10)) != null) {
            return this.bound(this.propertyFor(name.substring(10)), true);
        }
        if (argc == 0 && name.startsWith("getMinimum") && this.propertyFor(name.substring(10)) != null) {
            return this.bound(this.propertyFor(name.substring(10)), false);
        }
        if (argc == 0 && method.getReturnType() != void.class) {
            final String suffix = name.startsWith("get") || name.startsWith("has") ? name.substring(3) : name.startsWith("is") ? name.substring(2) : null;
            final String key = suffix == null ? null : this.propertyFor(name.startsWith("has") ? "Has" + suffix : suffix);
            if (key != null) {
                return read(this.values.get(key), method.getGenericReturnType());
            }
            // getShapes, getVerticalDirections: the allowed values of shape, vertical_direction.
            if (suffix != null && Set.class.isAssignableFrom(method.getReturnType()) && suffix.endsWith("s")) {
                final String singular = this.propertyFor(suffix.substring(0, suffix.length() - 1));
                if (singular != null) {
                    return this.allowed(singular, method.getGenericReturnType());
                }
            }
        }
        if (argc == 1 && name.startsWith("set")) {
            final String key = this.propertyFor(name.substring(3));
            if (key != null) {
                this.set(key, this.write(key, args[0]));
                return null;
            }
        }
        throw Unsupported.method(method.getDeclaringClass().getSimpleName() + "#" + name);
    }

    private static final class Handler implements InvocationHandler {
        private final HostBlockData impl;

        Handler(final HostBlockData impl) {
            this.impl = impl;
        }

        @Override
        public Object invoke(final Object proxy, final Method method, final Object[] args) throws Throwable {
            if (method.getDeclaringClass() == Object.class) {
                return switch (method.getName()) {
                    case "equals" -> args[0] instanceof BlockData other && Proxy.isProxyClass(other.getClass())
                        && Proxy.getInvocationHandler(other) instanceof Handler h && h.impl.equals(this.impl);
                    case "hashCode" -> this.impl.hashCode();
                    default -> this.impl.toString();
                };
            }
            return this.impl.invoke(proxy, method, args);
        }
    }
}
