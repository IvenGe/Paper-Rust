package io.papermc.paperrs.host.registry;

import io.papermc.paperrs.host.stub.AbstractArt;
import java.util.Map;
import net.kyori.adventure.key.Key;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.NamespacedKey;
import org.jspecify.annotations.Nullable;

/**
 * A painting variant ({@code CraftArt}): its size in blocks as vanilla's {@code PaintingVariants}
 * has it (the server's {@code world::hanging::painting_size}), and its title and author.
 */
public final class HostArt extends AbstractArt {
    /** Width and height in blocks of the vanilla variants. */
    private static final Map<String, int[]> SIZES = sizes();

    public HostArt(final NamespacedKey key, final int ordinal, final String translationKey) {
        super(key, ordinal, translationKey);
    }

    private static Map<String, int[]> sizes() {
        final Map<String, int[]> out = new java.util.HashMap<>();
        final Object[][] groups = {
            {1, 1, "alban", "aztec", "aztec2", "bomb", "kebab", "plant", "wasteland", "meditative"},
            {2, 1, "courbet", "creebet", "pool", "sea", "sunset"},
            {1, 2, "graham", "wanderer", "prairie_ride"},
            {2, 2, "bust", "match", "skull_and_roses", "stage", "void", "wither", "earth", "fire", "water", "wind", "baroque", "humble"},
            {4, 2, "fighters", "changing", "finding", "lowmist", "passage"},
            {4, 3, "skeleton", "donkey_kong"},
            {3, 3, "bouquet", "cavebird", "cotan", "endboss", "fern", "owlemons", "sunflowers", "tides", "dennis"},
            {3, 4, "backyard", "pond"},
            {4, 4, "burning_skull", "pigscene", "pointer", "orb", "unpacked"},
        };
        for (final Object[] group : groups) {
            for (int i = 2; i < group.length; i++) {
                out.put((String) group[i], new int[] {(Integer) group[0], (Integer) group[1]});
            }
        }
        return Map.copyOf(out);
    }

    private int[] size() {
        return SIZES.getOrDefault(this.key.getKey(), new int[] {1, 1});
    }

    @Override
    public int getBlockWidth() {
        return this.size()[0];
    }

    @Override
    public int getBlockHeight() {
        return this.size()[1];
    }

    @Override
    public int getId() {
        return this.ordinal;
    }

    @Override
    public Key assetId() {
        return this.key;
    }

    @Override
    public @Nullable Component title() {
        return Component.translatable("painting." + this.key.getNamespace() + "." + this.key.getKey() + ".title", NamedTextColor.YELLOW);
    }

    @Override
    public @Nullable Component author() {
        return Component.translatable("painting." + this.key.getNamespace() + "." + this.key.getKey() + ".author", NamedTextColor.GRAY);
    }
}
