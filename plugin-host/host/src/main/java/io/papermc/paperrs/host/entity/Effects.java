package io.papermc.paperrs.host.entity;

import io.papermc.paperrs.host.bridge.Values;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.Map;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.potion.PotionEffect;
import org.bukkit.potion.PotionEffectType;
import org.jetbrains.annotations.Nullable;

/**
 * The potion effects of players and mobs ({@code LivingEntity#addPotionEffect} and friends).
 * The server keeps them; every call asks it, so effects that ran out are gone.
 */
public final class Effects {
    private Effects() {
    }

    public static boolean add(final CraftServer server, final int id, final PotionEffect effect) {
        final Object result = server.call("entity.add_effect", Values.of("id", id, "effect", effect.getType().getKey().toString(),
            "duration", effect.getDuration(), "amplifier", effect.getAmplifier(),
            "ambient", effect.isAmbient(), "particles", effect.hasParticles(), "icon", effect.hasIcon()));
        return Boolean.TRUE.equals(result);
    }

    public static boolean addAll(final CraftServer server, final int id, final Collection<PotionEffect> effects) {
        boolean all = true;
        for (final PotionEffect effect : effects) {
            all &= add(server, id, effect);
        }
        return all;
    }

    public static List<PotionEffect> list(final CraftServer server, final int id) {
        final List<PotionEffect> out = new ArrayList<>();
        for (final Object o : Values.list(server.call("entity.effects", Values.of("id", id)))) {
            final Map<String, Object> m = Values.map(o);
            final NamespacedKey key = NamespacedKey.fromString(Values.str(m, "effect"));
            final PotionEffectType type = key == null ? null : Registry.MOB_EFFECT.get(key);
            if (type != null) {
                out.add(new PotionEffect(type, Values.i32(m, "duration", 0), Values.i32(m, "amplifier", 0),
                    Values.bool(m, "ambient", false), Values.bool(m, "particles", true), Values.bool(m, "icon", true)));
            }
        }
        return out;
    }

    public static @Nullable PotionEffect get(final CraftServer server, final int id, final PotionEffectType type) {
        for (final PotionEffect effect : list(server, id)) {
            if (effect.getType().equals(type)) {
                return effect;
            }
        }
        return null;
    }

    public static void remove(final CraftServer server, final int id, final PotionEffectType type) {
        server.call("entity.remove_effect", Values.of("id", id, "effect", type.getKey().toString()));
    }

    public static boolean clear(final CraftServer server, final int id) {
        return server.call("entity.clear_effects", Values.of("id", id)) instanceof Number n && n.longValue() > 0;
    }
}
