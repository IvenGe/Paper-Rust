package io.papermc.paperrs.host.item;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import org.junit.jupiter.api.Test;

/** The component encodings behind the special item metas, by layout and round trip. */
class MetaCodecsTest {
    @Test
    void potionContentsLayout() {
        // A potion with id 5 and nothing else: as the server's brewing writes it.
        final MetaCodecs.PotionContents plain = new MetaCodecs.PotionContents(5, null, List.of(), null);
        assertArrayEquals(new byte[] {1, 5, 0, 0, 0}, MetaCodecs.writePotion(plain));
        assertEquals(plain, MetaCodecs.readPotion(new byte[] {1, 5, 0, 0, 0}));

        final MetaCodecs.Detail weaker = new MetaCodecs.Detail(0, 600, false, true, true, null);
        final MetaCodecs.PotionContents full = new MetaCodecs.PotionContents(null, 0xFF0000,
            List.of(new MetaCodecs.Effect(1, new MetaCodecs.Detail(2, -1, true, false, true, weaker))), "Brew");
        assertEquals(full, MetaCodecs.readPotion(MetaCodecs.writePotion(full)));
    }

    @Test
    void profiles() {
        // A partial profile with just a name: kind 0, name present, no id, no properties, no skin overrides.
        final MetaCodecs.Profile named = MetaCodecs.Profile.of("Alice", null, List.of());
        assertArrayEquals(new byte[] {0, 1, 5, 'A', 'l', 'i', 'c', 'e', 0, 0, 0, 0, 0, 0}, MetaCodecs.writeProfile(named));

        final UUID id = UUID.fromString("853c80ef-3c37-49fd-aa49-938b674adae6");
        final MetaCodecs.Profile textured = MetaCodecs.Profile.of("jeb_", id, List.of(new MetaCodecs.Property("textures", "e30=", "sig")));
        assertEquals(true, textured.complete(), "a profile with textures needs no lookup");
        assertEquals(textured, MetaCodecs.readProfile(MetaCodecs.writeProfile(textured)));

        final MetaCodecs.Profile skin = new MetaCodecs.Profile(false, null, id, List.of(), "minecraft:entity/player/wide/steve", null, null, 1);
        assertEquals(skin, MetaCodecs.readProfile(MetaCodecs.writeProfile(skin)));
    }

    @Test
    void fireworksBannersStatesPagesAndStew() {
        final MetaCodecs.Explosion star = new MetaCodecs.Explosion(2, List.of(0xFF0000, 0x00FF00), List.of(0x0000FF), true, false);
        assertArrayEquals(new byte[] {2, 2, 0, -1, 0, 0, 0, 0, -1, 0, 1, 0, 0, 0, -1, 1, 0}, MetaCodecs.writeExplosion(star));
        final MetaCodecs.Fireworks rocket = new MetaCodecs.Fireworks(3, List.of(star, star));
        assertEquals(rocket, MetaCodecs.readFireworks(MetaCodecs.writeFireworks(rocket)));

        final List<MetaCodecs.Layer> layers = List.of(new MetaCodecs.Layer(4, null, null, 14), new MetaCodecs.Layer(-1, "minecraft:custom", "block.custom", 0));
        assertArrayEquals(new byte[] {1, 5, 14}, MetaCodecs.writeBanner(List.of(layers.get(0))), "a registry id is sent plus one");
        assertEquals(layers, MetaCodecs.readBanner(MetaCodecs.writeBanner(layers)));

        final Map<String, String> state = new LinkedHashMap<>();
        state.put("facing", "east");
        state.put("half", "top");
        assertEquals(state, MetaCodecs.readBlockState(MetaCodecs.writeBlockState(state)));

        assertArrayEquals(new byte[] {1, 2, 'h', 'i', 0}, MetaCodecs.writePages(List.of("hi")));
        assertEquals(List.of("one", "two"), MetaCodecs.readPages(MetaCodecs.writePages(List.of("one", "two"))));

        final List<MetaCodecs.StewEffect> stew = List.of(new MetaCodecs.StewEffect(14, 160));
        assertEquals(stew, MetaCodecs.readStew(MetaCodecs.writeStew(stew)));
    }
}
