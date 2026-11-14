package io.papermc.paperrs.host.item;

import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.data.HostData;
import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;
import org.jetbrains.annotations.Nullable;

/**
 * The data component patch of an item stack, as the Rust server describes it (see
 * {@code api::item_value}): components added to the item's defaults, as their network
 * encoding, and the ids of default components removed.
 *
 * <p>The custom name, item name and lore arrive both as raw bytes and as JSON text; they stay
 * raw (so an unchanged stack goes back byte for byte and still stacks with its twins) until
 * plugin code changes them. Every other component is decoded on demand by the typed
 * accessors below and re-encoded when set; components nothing here understands pass through
 * untouched.
 */
public final class ItemComponents {
    public static final String CUSTOM_DATA = "minecraft:custom_data";
    public static final String MAX_STACK_SIZE = "minecraft:max_stack_size";
    public static final String MAX_DAMAGE = "minecraft:max_damage";
    public static final String DAMAGE = "minecraft:damage";
    public static final String UNBREAKABLE = "minecraft:unbreakable";
    public static final String CUSTOM_NAME = "minecraft:custom_name";
    public static final String ITEM_NAME = "minecraft:item_name";
    public static final String ITEM_MODEL = "minecraft:item_model";
    public static final String LORE = "minecraft:lore";
    public static final String RARITY = "minecraft:rarity";
    public static final String ENCHANTMENTS = "minecraft:enchantments";
    public static final String CUSTOM_MODEL_DATA = "minecraft:custom_model_data";
    public static final String TOOLTIP_DISPLAY = "minecraft:tooltip_display";
    public static final String REPAIR_COST = "minecraft:repair_cost";
    public static final String GLINT_OVERRIDE = "minecraft:enchantment_glint_override";
    public static final String ENCHANTABLE = "minecraft:enchantable";
    public static final String GLIDER = "minecraft:glider";
    public static final String TOOLTIP_STYLE = "minecraft:tooltip_style";
    public static final String STORED_ENCHANTMENTS = "minecraft:stored_enchantments";
    public static final String DYED_COLOR = "minecraft:dyed_color";
    public static final String WRITTEN_BOOK = "minecraft:written_book_content";

    /**
     * A written book's content, with the pages as JSON text: the server encodes text
     * components, so books cross the bridge like lore.
     */
    public record WrittenBook(String title, String author, int generation, List<String> pages, boolean resolved) {
        public WrittenBook {
            pages = List.copyOf(pages);
        }
    }

    /** Added components: type id to network bytes. */
    private final TreeMap<Integer, byte[]> raw = new TreeMap<>();
    /** Text components set here, as JSON (they replace the raw bytes of the same type). */
    private @Nullable String name;
    private @Nullable String itemName;
    private @Nullable List<String> lore;
    private @Nullable WrittenBook book;
    /** Default components removed from the stack. */
    private final TreeSet<Integer> removed = new TreeSet<>();

    public ItemComponents() {
    }

    public ItemComponents copy() {
        final ItemComponents out = new ItemComponents();
        this.raw.forEach((k, v) -> out.raw.put(k, v.clone()));
        out.name = this.name;
        out.itemName = this.itemName;
        out.lore = this.lore == null ? null : new ArrayList<>(this.lore);
        out.book = this.book;
        out.removed.addAll(this.removed);
        return out;
    }

    // --- registry ids ---------------------------------------------------------------------

    private static List<String> componentTypes() {
        return HostData.get().registry("minecraft:data_component_type");
    }

    public static int typeId(final String name) {
        final int id = componentTypes().indexOf(name);
        if (id < 0) {
            throw new IllegalArgumentException("unknown data component type " + name);
        }
        return id;
    }

    public static @Nullable String typeName(final int id) {
        final List<String> types = componentTypes();
        return id >= 0 && id < types.size() ? types.get(id) : null;
    }

    // --- bridge form ----------------------------------------------------------------------

    /** Reads the component fields of an item value sent by the server. */
    public static ItemComponents fromValue(final Map<String, Object> value) {
        final ItemComponents out = new ItemComponents();
        for (Object entry : Values.list(value.get("raw"))) {
            final List<Object> pair = Values.list(entry);
            out.raw.put(((Number) pair.get(0)).intValue(), (byte[]) pair.get(1));
        }
        // Text components come raw and as JSON. Both are kept: the JSON for reading, the bytes
        // so that an unchanged stack goes back exactly as it came (the server keeps the bytes
        // when the JSON still matches them).
        out.name = value.get("name") instanceof String s ? s : null;
        out.itemName = value.get("item_name") instanceof String s ? s : null;
        if (value.get("lore") instanceof List<?> lines) {
            final List<String> lore = new ArrayList<>();
            for (Object line : lines) {
                lore.add(String.valueOf(line));
            }
            out.lore = lore;
        }
        if (value.get("book") instanceof Map<?, ?> book) {
            final Map<String, Object> b = Values.map(book);
            final List<String> pages = new ArrayList<>();
            for (Object page : Values.list(b.get("pages"))) {
                pages.add(String.valueOf(page));
            }
            out.book = new WrittenBook(Values.str(b, "title", ""), Values.str(b, "author", ""), Values.i32(b, "generation", 0), pages,
                Values.bool(b, "resolved", false));
        }
        for (Object id : Values.list(value.get("removed"))) {
            out.removed.add(((Number) id).intValue());
        }
        return out;
    }

    /** Writes the component fields into an item value for the server. */
    public void toValue(final Map<String, Object> out) {
        final List<Object> raw = new ArrayList<>();
        this.raw.forEach((id, bytes) -> raw.add(List.of((long) id, bytes)));
        if (!raw.isEmpty()) {
            out.put("raw", raw);
        }
        if (this.name != null) {
            out.put("name", this.name);
        }
        if (this.itemName != null) {
            out.put("item_name", this.itemName);
        }
        if (this.lore != null) {
            out.put("lore", new ArrayList<Object>(this.lore));
        }
        if (this.book != null) {
            out.put("book", Values.of("title", this.book.title(), "author", this.book.author(), "generation", this.book.generation(),
                "pages", new ArrayList<Object>(this.book.pages()), "resolved", this.book.resolved()));
        }
        if (!this.removed.isEmpty()) {
            final List<Object> removed = new ArrayList<>();
            for (int id : this.removed) {
                removed.add((long) id);
            }
            out.put("removed", removed);
        }
    }

    public boolean isEmpty() {
        return this.raw.isEmpty() && this.name == null && this.itemName == null && this.lore == null && this.book == null && this.removed.isEmpty();
    }

    /** The type names of added components. */
    public Set<String> addedTypes() {
        final Set<String> out = new LinkedHashSet<>();
        for (int id : this.raw.keySet()) {
            out.add(typeName(id));
        }
        if (this.name != null) {
            out.add(CUSTOM_NAME);
        }
        if (this.itemName != null) {
            out.add(ITEM_NAME);
        }
        if (this.lore != null) {
            out.add(LORE);
        }
        if (this.book != null) {
            out.add(WRITTEN_BOOK);
        }
        return out;
    }

    public Set<String> removedTypes() {
        final Set<String> out = new LinkedHashSet<>();
        for (int id : this.removed) {
            out.add(typeName(id));
        }
        return out;
    }

    // --- generic access -------------------------------------------------------------------

    public boolean has(final String type) {
        return switch (type) {
            case CUSTOM_NAME -> this.name != null || this.raw.containsKey(typeId(type));
            case ITEM_NAME -> this.itemName != null || this.raw.containsKey(typeId(type));
            case LORE -> this.lore != null || this.raw.containsKey(typeId(type));
            case WRITTEN_BOOK -> this.book != null || this.raw.containsKey(typeId(type));
            default -> this.raw.containsKey(typeId(type));
        };
    }

    public boolean isRemoved(final String type) {
        return this.removed.contains(typeId(type));
    }

    public byte @Nullable [] raw(final String type) {
        return this.raw.get(typeId(type));
    }

    /** Sets a component to its network bytes ({@code null} unsets it). */
    public void setRaw(final String type, final byte @Nullable [] bytes) {
        final int id = typeId(type);
        this.clearText(type);
        if (bytes == null) {
            this.raw.remove(id);
        } else {
            this.raw.put(id, bytes);
            this.removed.remove(id);
        }
    }

    /** Removes an added component (the item's default applies again). */
    public void unset(final String type) {
        this.setRaw(type, null);
    }

    /** Removes a default component of the item. */
    public void remove(final String type) {
        this.unset(type);
        this.removed.add(typeId(type));
    }

    /** Neither adds nor removes the component. */
    public void reset(final String type) {
        this.unset(type);
        this.removed.remove(typeId(type));
    }

    private void clearText(final String type) {
        switch (type) {
            case CUSTOM_NAME -> this.name = null;
            case ITEM_NAME -> this.itemName = null;
            case LORE -> this.lore = null;
            case WRITTEN_BOOK -> this.book = null;
            default -> {
            }
        }
    }

    // --- text -----------------------------------------------------------------------------

    /** The custom name as JSON, or {@code null}. */
    public @Nullable String nameJson() {
        return this.name;
    }

    public @Nullable String itemNameJson() {
        return this.itemName;
    }

    public @Nullable List<String> loreJson() {
        return this.lore;
    }

    /** Whether a text component is only known as raw bytes (its JSON came with the value). */
    public boolean hasRawText(final String type) {
        return this.raw.containsKey(typeId(type));
    }

    public void setNameJson(final @Nullable String json) {
        this.raw.remove(typeId(CUSTOM_NAME));
        this.name = json;
    }

    public void setItemNameJson(final @Nullable String json) {
        this.raw.remove(typeId(ITEM_NAME));
        this.itemName = json;
    }

    public void setLoreJson(final @Nullable List<String> json) {
        this.raw.remove(typeId(LORE));
        this.lore = json == null ? null : new ArrayList<>(json);
    }

    public @Nullable WrittenBook writtenBook() {
        return this.book;
    }

    /** Sets (or with {@code null} unsets) the written book content. */
    public void setWrittenBook(final @Nullable WrittenBook book) {
        this.raw.remove(typeId(WRITTEN_BOOK));
        this.book = book;
        if (book != null) {
            this.removed.remove(typeId(WRITTEN_BOOK));
        }
    }

    // --- simple values ----------------------------------------------------------------------

    public @Nullable Integer varInt(final String type) {
        final byte[] bytes = this.raw(type);
        return bytes == null ? null : new In(bytes).varInt();
    }

    public void setVarInt(final String type, final @Nullable Integer value) {
        if (value == null) {
            this.unset(type);
            return;
        }
        final Out out = new Out();
        out.varInt(value);
        this.setRaw(type, out.toByteArray());
    }

    public @Nullable Boolean bool(final String type) {
        final byte[] bytes = this.raw(type);
        return bytes == null || bytes.length == 0 ? null : bytes[0] != 0;
    }

    public void setBool(final String type, final @Nullable Boolean value) {
        this.setRaw(type, value == null ? null : new byte[] {(byte) (value ? 1 : 0)});
    }

    /** A component without data ({@code minecraft:unbreakable}, {@code minecraft:glider}). */
    public void setUnit(final String type, final boolean present) {
        this.setRaw(type, present ? new byte[0] : null);
    }

    public @Nullable String identifier(final String type) {
        final byte[] bytes = this.raw(type);
        return bytes == null ? null : new In(bytes).string();
    }

    public void setIdentifier(final String type, final @Nullable String value) {
        if (value == null) {
            this.unset(type);
            return;
        }
        final Out out = new Out();
        out.string(value);
        this.setRaw(type, out.toByteArray());
    }

    public @Nullable Integer int32(final String type) {
        final byte[] bytes = this.raw(type);
        return bytes == null ? null : new In(bytes).int32();
    }

    public void setInt32(final String type, final @Nullable Integer value) {
        if (value == null) {
            this.unset(type);
            return;
        }
        final Out out = new Out();
        out.int32(value);
        this.setRaw(type, out.toByteArray());
    }

    // --- enchantments ---------------------------------------------------------------------

    /** {@code minecraft:enchantments} or {@code minecraft:stored_enchantments}: enchantment network id to level. */
    public static final String ATTRIBUTE_MODIFIERS = "minecraft:attribute_modifiers";

    /**
     * One entry of {@code minecraft:attribute_modifiers}: the attribute (registry id), the
     * modifier (id, amount, operation id), the slot group id, and how the tooltip shows it (0
     * default, 1 hidden, 2 the given text component).
     */
    public record AttributeEntry(int attribute, String id, double amount, int operation, int slot, int display, byte @org.jetbrains.annotations.Nullable [] text) {
    }

    /** The stack's own attribute modifiers, or null if it keeps its item's defaults. */
    public java.util.@org.jetbrains.annotations.Nullable List<AttributeEntry> attributeModifiers() {
        final byte[] bytes = this.raw(ATTRIBUTE_MODIFIERS);
        if (bytes == null) {
            return null;
        }
        final In in = new In(bytes);
        final int count = in.varInt();
        final java.util.List<AttributeEntry> out = new java.util.ArrayList<>(count);
        for (int i = 0; i < count; i++) {
            final int attribute = in.varInt();
            final String id = in.string();
            final double amount = in.f64();
            final int operation = in.varInt();
            final int slot = in.varInt();
            final int display = in.varInt();
            final byte[] text = display == 2 ? in.nbtTag() : null;
            out.add(new AttributeEntry(attribute, id, amount, operation, slot, display, text));
        }
        return out;
    }

    /** Sets the stack's attribute modifiers; null goes back to the item's defaults. */
    public void setAttributeModifiers(final java.util.@org.jetbrains.annotations.Nullable List<AttributeEntry> entries) {
        if (entries == null) {
            this.unset(ATTRIBUTE_MODIFIERS);
            return;
        }
        final Out out = new Out();
        out.varInt(entries.size());
        for (final AttributeEntry e : entries) {
            out.varInt(e.attribute());
            out.string(e.id());
            out.f64(e.amount());
            out.varInt(e.operation());
            out.varInt(e.slot());
            out.varInt(e.text() == null && e.display() == 2 ? 0 : e.display());
            if (e.display() == 2 && e.text() != null) {
                out.raw(e.text());
            }
        }
        this.setRaw(ATTRIBUTE_MODIFIERS, out.toByteArray());
    }

    public Map<Integer, Integer> enchantments(final String type) {
        final Map<Integer, Integer> out = new LinkedHashMap<>();
        final byte[] bytes = this.raw(type);
        if (bytes == null) {
            return out;
        }
        final In in = new In(bytes);
        final int count = in.varInt();
        for (int i = 0; i < count; i++) {
            final int id = in.varInt();
            out.put(id, in.varInt());
        }
        return out;
    }

    public void setEnchantments(final String type, final Map<Integer, Integer> enchantments) {
        if (enchantments.isEmpty()) {
            this.unset(type);
            return;
        }
        final Out out = new Out();
        out.varInt(enchantments.size());
        enchantments.forEach((id, level) -> {
            out.varInt(id);
            out.varInt(level);
        });
        this.setRaw(type, out.toByteArray());
    }

    // --- custom model data ----------------------------------------------------------------

    public record CustomModelData(List<Float> floats, List<Boolean> flags, List<String> strings, List<Integer> colors) {
    }

    public @Nullable CustomModelData customModelData() {
        final byte[] bytes = this.raw(CUSTOM_MODEL_DATA);
        if (bytes == null) {
            return null;
        }
        final In in = new In(bytes);
        final List<Float> floats = new ArrayList<>();
        for (int i = in.varInt(); i > 0; i--) {
            floats.add(in.float32());
        }
        final List<Boolean> flags = new ArrayList<>();
        for (int i = in.varInt(); i > 0; i--) {
            flags.add(in.bool());
        }
        final List<String> strings = new ArrayList<>();
        for (int i = in.varInt(); i > 0; i--) {
            strings.add(in.string());
        }
        final List<Integer> colors = new ArrayList<>();
        for (int i = in.varInt(); i > 0; i--) {
            colors.add(in.int32());
        }
        return new CustomModelData(floats, flags, strings, colors);
    }

    public void setCustomModelData(final @Nullable CustomModelData data) {
        if (data == null) {
            this.unset(CUSTOM_MODEL_DATA);
            return;
        }
        final Out out = new Out();
        out.varInt(data.floats().size());
        data.floats().forEach(out::float32);
        out.varInt(data.flags().size());
        data.flags().forEach(out::bool);
        out.varInt(data.strings().size());
        data.strings().forEach(out::string);
        out.varInt(data.colors().size());
        data.colors().forEach(out::int32);
        this.setRaw(CUSTOM_MODEL_DATA, out.toByteArray());
    }

    // --- tooltip display ------------------------------------------------------------------

    public record TooltipDisplay(boolean hideTooltip, Set<String> hidden) {
    }

    public TooltipDisplay tooltipDisplay() {
        final byte[] bytes = this.raw(TOOLTIP_DISPLAY);
        if (bytes == null) {
            return new TooltipDisplay(false, Set.of());
        }
        final In in = new In(bytes);
        final boolean hide = in.bool();
        final Set<String> hidden = new LinkedHashSet<>();
        for (int i = in.varInt(); i > 0; i--) {
            final String name = typeName(in.varInt());
            if (name != null) {
                hidden.add(name);
            }
        }
        return new TooltipDisplay(hide, hidden);
    }

    public void setTooltipDisplay(final TooltipDisplay display) {
        if (!display.hideTooltip() && display.hidden().isEmpty()) {
            this.unset(TOOLTIP_DISPLAY);
            return;
        }
        final Out out = new Out();
        out.bool(display.hideTooltip());
        out.varInt(display.hidden().size());
        for (String type : display.hidden()) {
            out.varInt(typeId(type));
        }
        this.setRaw(TOOLTIP_DISPLAY, out.toByteArray());
    }

    // --- custom data ----------------------------------------------------------------------

    public Nbt.Compound customData() {
        final byte[] bytes = this.raw(CUSTOM_DATA);
        return bytes == null ? new Nbt.Compound() : Nbt.readCompound(bytes);
    }

    public void setCustomData(final Nbt.Compound data) {
        this.setRaw(CUSTOM_DATA, data.isEmpty() ? null : Nbt.writeCompound(data));
    }

    // --- equality ---------------------------------------------------------------------------

    /**
     * The patch as compared: text components by their meaning (a name set here as JSON equals
     * the same name the server sent as bytes), everything else by its bytes.
     */
    private Map<Object, Object> comparable() {
        final Map<Object, Object> out = new java.util.HashMap<>();
        final Set<Integer> text = new java.util.HashSet<>();
        if (this.name != null) {
            out.put(CUSTOM_NAME, io.papermc.paperrs.host.Components.fromJson(this.name));
            text.add(typeId(CUSTOM_NAME));
        }
        if (this.itemName != null) {
            out.put(ITEM_NAME, io.papermc.paperrs.host.Components.fromJson(this.itemName));
            text.add(typeId(ITEM_NAME));
        }
        if (this.lore != null) {
            final List<Object> lines = new ArrayList<>();
            for (String line : this.lore) {
                lines.add(io.papermc.paperrs.host.Components.fromJson(line));
            }
            out.put(LORE, lines);
            text.add(typeId(LORE));
        }
        if (this.book != null) {
            final List<Object> pages = new ArrayList<>();
            for (String page : this.book.pages()) {
                pages.add(io.papermc.paperrs.host.Components.fromJson(page));
            }
            out.put(WRITTEN_BOOK, List.of(this.book.title(), this.book.author(), this.book.generation(), pages, this.book.resolved()));
            text.add(typeId(WRITTEN_BOOK));
        }
        this.raw.forEach((id, bytes) -> {
            if (!text.contains(id)) {
                out.put(id, java.nio.ByteBuffer.wrap(bytes));
            }
        });
        out.put("removed", this.removed);
        return out;
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof ItemComponents other && this.comparable().equals(other.comparable());
    }

    @Override
    public int hashCode() {
        return this.comparable().hashCode();
    }

    // --- binary helpers ---------------------------------------------------------------------

    /** Reads the network encoding of a component. */
    public static final class In {
        private final byte[] bytes;
        private int pos;

        public In(final byte[] bytes) {
            this.bytes = bytes;
        }

        public int u8() {
            if (this.pos >= this.bytes.length) {
                throw new IllegalArgumentException("component data ends early");
            }
            return this.bytes[this.pos++] & 0xFF;
        }

        public boolean bool() {
            return this.u8() != 0;
        }

        public int varInt() {
            int value = 0;
            for (int shift = 0; shift < 35; shift += 7) {
                final int b = this.u8();
                value |= (b & 0x7F) << shift;
                if ((b & 0x80) == 0) {
                    return value;
                }
            }
            throw new IllegalArgumentException("varint too long");
        }

        public int int32() {
            return this.u8() << 24 | this.u8() << 16 | this.u8() << 8 | this.u8();
        }

        public float float32() {
            return Float.intBitsToFloat(this.int32());
        }

        public long int64() {
            return (long) this.int32() << 32 | this.int32() & 0xFFFFFFFFL;
        }

        public java.util.UUID uuid() {
            return new java.util.UUID(this.int64(), this.int64());
        }

        public double f64() {
            return Double.longBitsToDouble(this.int64());
        }

        /** One network NBT tag (type byte and payload, no name), as its bytes. */
        public byte[] nbtTag() {
            final int start = this.pos;
            final int type = this.u8();
            this.skipPayload(type);
            return java.util.Arrays.copyOfRange(this.bytes, start, this.pos);
        }

        private void skipPayload(final int type) {
            switch (type) {
                case 0 -> { }
                case 1 -> this.pos += 1;
                case 2 -> this.pos += 2;
                case 3, 5 -> this.pos += 4;
                case 4, 6 -> this.pos += 8;
                case 7 -> this.pos += this.int32();
                case 8 -> this.pos += ((this.u8() << 8) | this.u8());
                case 9 -> {
                    final int inner = this.u8();
                    final int n = this.int32();
                    for (int i = 0; i < n; i++) {
                        this.skipPayload(inner);
                    }
                }
                case 10 -> {
                    int inner;
                    while ((inner = this.u8()) != 0) {
                        this.pos += ((this.u8() << 8) | this.u8());
                        this.skipPayload(inner);
                    }
                }
                case 11 -> this.pos += 4 * this.int32();
                case 12 -> this.pos += 8 * this.int32();
                default -> throw new IllegalStateException("bad NBT tag type " + type);
            }
        }

        public boolean atEnd() {
            return this.pos >= this.bytes.length;
        }

        public String string() {
            final int length = this.varInt();
            if (length < 0 || this.pos + length > this.bytes.length) {
                throw new IllegalArgumentException("bad string length " + length);
            }
            final String s = new String(this.bytes, this.pos, length, StandardCharsets.UTF_8);
            this.pos += length;
            return s;
        }
    }

    /** Writes the network encoding of a component. */
    public static final class Out extends ByteArrayOutputStream {
        public void varInt(int value) {
            while ((value & ~0x7F) != 0) {
                this.write((value & 0x7F) | 0x80);
                value >>>= 7;
            }
            this.write(value);
        }

        public void bool(final boolean b) {
            this.write(b ? 1 : 0);
        }

        public void int32(final int v) {
            this.write(v >>> 24);
            this.write(v >>> 16);
            this.write(v >>> 8);
            this.write(v);
        }

        public void float32(final float f) {
            this.int32(Float.floatToIntBits(f));
        }

        public void int64(final long v) {
            this.int32((int) (v >>> 32));
            this.int32((int) v);
        }

        public void uuid(final java.util.UUID id) {
            this.int64(id.getMostSignificantBits());
            this.int64(id.getLeastSignificantBits());
        }

        public void f64(final double v) {
            this.int64(Double.doubleToLongBits(v));
        }

        public void raw(final byte[] b) {
            this.write(b, 0, b.length);
        }

        public void string(final String s) {
            final byte[] b = s.getBytes(StandardCharsets.UTF_8);
            this.varInt(b.length);
            this.write(b, 0, b.length);
        }
    }
}
