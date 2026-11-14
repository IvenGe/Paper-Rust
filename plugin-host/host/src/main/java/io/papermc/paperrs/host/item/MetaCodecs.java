package io.papermc.paperrs.host.item;

import io.papermc.paperrs.host.item.ItemComponents.In;
import io.papermc.paperrs.host.item.ItemComponents.Out;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import org.jetbrains.annotations.Nullable;

/**
 * The network encodings of the components behind the special item metas (profiles, potion
 * contents, fireworks, banner patterns, block states, book pages, stew effects), as the
 * protocol's slot data describes them. Written books are encoded by the server instead (their
 * pages are text components), see {@link ItemComponents.WrittenBook}.
 */
public final class MetaCodecs {
    public static final String PROFILE = "minecraft:profile";
    public static final String LODESTONE_TRACKER = "minecraft:lodestone_tracker";
    public static final String TROPICAL_FISH_PATTERN = "minecraft:tropical_fish/pattern";
    public static final String TROPICAL_FISH_BASE_COLOR = "minecraft:tropical_fish/base_color";
    public static final String TROPICAL_FISH_PATTERN_COLOR = "minecraft:tropical_fish/pattern_color";
    public static final String AXOLOTL_VARIANT = "minecraft:axolotl/variant";
    public static final String NOTE_BLOCK_SOUND = "minecraft:note_block_sound";
    public static final String POTION_CONTENTS = "minecraft:potion_contents";
    public static final String STEW_EFFECTS = "minecraft:suspicious_stew_effects";
    public static final String FIREWORKS = "minecraft:fireworks";
    public static final String FIREWORK_EXPLOSION = "minecraft:firework_explosion";
    public static final String BANNER_PATTERNS = "minecraft:banner_patterns";
    public static final String BLOCK_STATE = "minecraft:block_state";
    public static final String WRITABLE_BOOK = "minecraft:writable_book_content";
    public static final String MAP_ID = "minecraft:map_id";
    public static final String MAP_COLOR = "minecraft:map_color";
    public static final String OMINOUS_BOTTLE_AMPLIFIER = "minecraft:ominous_bottle_amplifier";

    private MetaCodecs() {
    }

    private static @Nullable String optString(final In in) {
        return in.bool() ? in.string() : null;
    }

    private static void optString(final Out out, final @Nullable String s) {
        out.bool(s != null);
        if (s != null) {
            out.string(s);
        }
    }

    // --- profiles ---------------------------------------------------------------------------

    public record Property(String name, String value, @Nullable String signature) {
    }

    /**
     * A resolvable profile: complete (name and id given, nothing left to look up) or partial
     * (the client resolves the rest), with optional skin overrides.
     */
    public record Profile(boolean complete, @Nullable String name, @Nullable UUID id, List<Property> properties,
                          @Nullable String body, @Nullable String cape, @Nullable String elytra, @Nullable Integer model) {
        public Profile {
            properties = List.copyOf(properties);
        }

        public static Profile of(final @Nullable String name, final @Nullable UUID id, final List<Property> properties) {
            // A profile with textures is complete; otherwise the client looks the skin up.
            final boolean textures = properties.stream().anyMatch(p -> p.name().equals("textures"));
            return new Profile(textures && name != null && id != null, name, id, properties, null, null, null, null);
        }
    }

    public static Profile readProfile(final byte[] bytes) {
        final In in = new In(bytes);
        final boolean complete = in.bool();
        final String name;
        final UUID id;
        if (complete) {
            id = in.uuid();
            name = in.string();
        } else {
            name = optString(in);
            id = in.bool() ? in.uuid() : null;
        }
        final List<Property> properties = new ArrayList<>();
        for (int i = 0, n = in.varInt(); i < n; i++) {
            properties.add(new Property(in.string(), in.string(), optString(in)));
        }
        final String body = optString(in);
        final String cape = optString(in);
        final String elytra = optString(in);
        final Integer model = in.bool() ? in.varInt() : null;
        return new Profile(complete, name, id, properties, body, cape, elytra, model);
    }

    public static byte[] writeProfile(final Profile p) {
        final Out out = new Out();
        out.bool(p.complete());
        if (p.complete()) {
            out.uuid(p.id());
            out.string(p.name());
        } else {
            optString(out, p.name());
            out.bool(p.id() != null);
            if (p.id() != null) {
                out.uuid(p.id());
            }
        }
        out.varInt(p.properties().size());
        for (final Property property : p.properties()) {
            out.string(property.name());
            out.string(property.value());
            optString(out, property.signature());
        }
        optString(out, p.body());
        optString(out, p.cape());
        optString(out, p.elytra());
        out.bool(p.model() != null);
        if (p.model() != null) {
            out.varInt(p.model());
        }
        return out.toByteArray();
    }

    // --- potion contents and stew effects -----------------------------------------------------

    /** An effect's settings; {@code hidden} is the weaker effect it replaced, kept for later. */
    public record Detail(int amplifier, int duration, boolean ambient, boolean particles, boolean icon, @Nullable Detail hidden) {
    }

    /** An effect: its {@code minecraft:mob_effect} id and settings. */
    public record Effect(int type, Detail detail) {
    }

    public record PotionContents(@Nullable Integer potion, @Nullable Integer color, List<Effect> effects, @Nullable String customName) {
        public PotionContents {
            effects = List.copyOf(effects);
        }

        public static final PotionContents EMPTY = new PotionContents(null, null, List.of(), null);

        public boolean isEmpty() {
            return this.potion == null && this.color == null && this.effects.isEmpty() && this.customName == null;
        }
    }

    private static Detail readDetail(final In in) {
        return new Detail(in.varInt(), in.varInt(), in.bool(), in.bool(), in.bool(), in.bool() ? readDetail(in) : null);
    }

    private static void writeDetail(final Out out, final Detail d) {
        out.varInt(d.amplifier());
        out.varInt(d.duration());
        out.bool(d.ambient());
        out.bool(d.particles());
        out.bool(d.icon());
        out.bool(d.hidden() != null);
        if (d.hidden() != null) {
            writeDetail(out, d.hidden());
        }
    }

    public static PotionContents readPotion(final byte[] bytes) {
        final In in = new In(bytes);
        final Integer potion = in.bool() ? in.varInt() : null;
        final Integer color = in.bool() ? in.int32() : null;
        final List<Effect> effects = new ArrayList<>();
        for (int i = 0, n = in.varInt(); i < n; i++) {
            effects.add(new Effect(in.varInt(), readDetail(in)));
        }
        return new PotionContents(potion, color, effects, optString(in));
    }

    public static byte[] writePotion(final PotionContents p) {
        final Out out = new Out();
        out.bool(p.potion() != null);
        if (p.potion() != null) {
            out.varInt(p.potion());
        }
        out.bool(p.color() != null);
        if (p.color() != null) {
            out.int32(p.color());
        }
        out.varInt(p.effects().size());
        for (final Effect e : p.effects()) {
            out.varInt(e.type());
            writeDetail(out, e.detail());
        }
        optString(out, p.customName());
        return out.toByteArray();
    }

    /** A suspicious stew effect: {@code minecraft:mob_effect} id and duration. */
    public record StewEffect(int type, int duration) {
    }

    public static List<StewEffect> readStew(final byte[] bytes) {
        final In in = new In(bytes);
        final List<StewEffect> out = new ArrayList<>();
        for (int i = 0, n = in.varInt(); i < n; i++) {
            out.add(new StewEffect(in.varInt(), in.varInt()));
        }
        return out;
    }

    public static byte[] writeStew(final List<StewEffect> effects) {
        final Out out = new Out();
        out.varInt(effects.size());
        for (final StewEffect e : effects) {
            out.varInt(e.type());
            out.varInt(e.duration());
        }
        return out.toByteArray();
    }

    // --- fireworks ------------------------------------------------------------------------------

    /** Shapes: 0 small ball, 1 large ball, 2 star, 3 creeper, 4 burst. Colours are RGB. */
    public record Explosion(int shape, List<Integer> colors, List<Integer> fades, boolean trail, boolean twinkle) {
        public Explosion {
            colors = List.copyOf(colors);
            fades = List.copyOf(fades);
        }
    }

    public record Fireworks(int flight, List<Explosion> explosions) {
        public Fireworks {
            explosions = List.copyOf(explosions);
        }
    }

    private static List<Integer> readInts(final In in) {
        final List<Integer> out = new ArrayList<>();
        for (int i = 0, n = in.varInt(); i < n; i++) {
            out.add(in.int32());
        }
        return out;
    }

    private static void writeInts(final Out out, final List<Integer> ints) {
        out.varInt(ints.size());
        for (final int i : ints) {
            out.int32(i);
        }
    }

    private static Explosion readExplosion(final In in) {
        return new Explosion(in.varInt(), readInts(in), readInts(in), in.bool(), in.bool());
    }

    private static void writeExplosion(final Out out, final Explosion e) {
        out.varInt(e.shape());
        writeInts(out, e.colors());
        writeInts(out, e.fades());
        out.bool(e.trail());
        out.bool(e.twinkle());
    }

    public static Explosion readExplosion(final byte[] bytes) {
        return readExplosion(new In(bytes));
    }

    public static byte[] writeExplosion(final Explosion e) {
        final Out out = new Out();
        writeExplosion(out, e);
        return out.toByteArray();
    }

    public static Fireworks readFireworks(final byte[] bytes) {
        final In in = new In(bytes);
        final int flight = in.varInt();
        final List<Explosion> explosions = new ArrayList<>();
        for (int i = 0, n = in.varInt(); i < n; i++) {
            explosions.add(readExplosion(in));
        }
        return new Fireworks(flight, explosions);
    }

    public static byte[] writeFireworks(final Fireworks f) {
        final Out out = new Out();
        out.varInt(f.flight());
        out.varInt(f.explosions().size());
        for (final Explosion e : f.explosions()) {
            writeExplosion(out, e);
        }
        return out.toByteArray();
    }

    // --- banner patterns ------------------------------------------------------------------------

    /**
     * A banner layer: a {@code minecraft:banner_pattern} id (or -1 for a pattern defined inline
     * by its asset id and translation key) and a dye colour (0 white ... 15 black).
     */
    public record Layer(int pattern, @Nullable String assetId, @Nullable String translationKey, int color) {
    }

    public static List<Layer> readBanner(final byte[] bytes) {
        final In in = new In(bytes);
        final List<Layer> out = new ArrayList<>();
        for (int i = 0, n = in.varInt(); i < n; i++) {
            final int id = in.varInt();
            if (id == 0) {
                final String asset = in.string();
                final String key = in.string();
                out.add(new Layer(-1, asset, key, in.varInt()));
            } else {
                out.add(new Layer(id - 1, null, null, in.varInt()));
            }
        }
        return out;
    }

    public static byte[] writeBanner(final List<Layer> layers) {
        final Out out = new Out();
        out.varInt(layers.size());
        for (final Layer layer : layers) {
            if (layer.pattern() < 0) {
                out.varInt(0);
                out.string(layer.assetId());
                out.string(layer.translationKey());
            } else {
                out.varInt(layer.pattern() + 1);
            }
            out.varInt(layer.color());
        }
        return out.toByteArray();
    }

    // --- block states and book pages ----------------------------------------------------------

    public static Map<String, String> readBlockState(final byte[] bytes) {
        final In in = new In(bytes);
        final Map<String, String> out = new LinkedHashMap<>();
        for (int i = 0, n = in.varInt(); i < n; i++) {
            out.put(in.string(), in.string());
        }
        return out;
    }

    public static byte[] writeBlockState(final Map<String, String> properties) {
        final Out out = new Out();
        out.varInt(properties.size());
        properties.forEach((k, v) -> {
            out.string(k);
            out.string(v);
        });
        return out.toByteArray();
    }

    /** The raw pages of a writable book (the chat-filtered versions are dropped). */
    public static List<String> readPages(final byte[] bytes) {
        final In in = new In(bytes);
        final List<String> out = new ArrayList<>();
        for (int i = 0, n = in.varInt(); i < n; i++) {
            out.add(in.string());
            optString(in);
        }
        return out;
    }

    public static byte[] writePages(final List<String> pages) {
        final Out out = new Out();
        out.varInt(pages.size());
        for (final String page : pages) {
            out.string(page);
            out.bool(false);
        }
        return out.toByteArray();
    }
}
