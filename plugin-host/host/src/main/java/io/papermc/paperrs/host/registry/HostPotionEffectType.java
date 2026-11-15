package io.papermc.paperrs.host.registry;

import io.papermc.paperrs.host.Unsupported;
import io.papermc.paperrs.host.data.HostData;
import io.papermc.paperrs.host.stub.AbstractPotionEffectType;
import java.util.Map;
import org.bukkit.NamespacedKey;
import org.bukkit.potion.PotionEffect;
import org.bukkit.potion.PotionEffectTypeCategory;

/**
 * A potion effect type. Its category and whether it is instant come from the server's game
 * data; colours and attribute modifiers are not there yet.
 */
@SuppressWarnings({"deprecation", "removal"})
public final class HostPotionEffectType extends AbstractPotionEffectType {
    /** Bukkit's old names where they differ from the key (CraftPotionEffectType#getName). */
    private static final Map<String, String> LEGACY_NAMES = Map.ofEntries(
        Map.entry("slowness", "SLOW"), Map.entry("haste", "FAST_DIGGING"), Map.entry("mining_fatigue", "SLOW_DIGGING"),
        Map.entry("strength", "INCREASE_DAMAGE"), Map.entry("instant_health", "HEAL"), Map.entry("instant_damage", "HARM"),
        Map.entry("jump_boost", "JUMP"), Map.entry("nausea", "CONFUSION"), Map.entry("resistance", "DAMAGE_RESISTANCE"));
    /** The effects that had names before keys (ids 1 to 33). */
    private static final java.util.Set<String> OLD = java.util.Set.of("speed", "slowness", "haste", "mining_fatigue", "strength",
        "instant_health", "instant_damage", "jump_boost", "nausea", "regeneration", "resistance", "fire_resistance", "water_breathing",
        "invisibility", "blindness", "night_vision", "hunger", "weakness", "poison", "wither", "health_boost", "absorption", "saturation",
        "glowing", "levitation", "luck", "unluck", "slow_falling", "conduit_power", "dolphins_grace", "bad_omen", "hero_of_the_village", "darkness");

    public HostPotionEffectType(final NamespacedKey key, final int ordinal, final String translationKey) {
        super(key, ordinal, translationKey);
    }

    private String category() {
        final String category = HostData.get().effectCategory(this.key.toString());
        if (category == null) {
            throw Unsupported.feature("The category of " + this.key + " (the server has no vanilla game data)");
        }
        return category;
    }

    @Override
    public PotionEffectTypeCategory getCategory() {
        return PotionEffectTypeCategory.valueOf(this.category());
    }

    @Override
    public Category getEffectCategory() {
        return Category.valueOf(this.category());
    }

    @Override
    public boolean isInstant() {
        return HostData.get().isInstantEffect(this.key.toString());
    }

    @Override
    public double getDurationModifier() {
        return 1.0;
    }

    @Override
    public PotionEffect createEffect(final int duration, final int amplifier) {
        return new PotionEffect(this, this.isInstant() ? 1 : (int) (duration * this.getDurationModifier()), amplifier);
    }

    /** The registry's numeric id plus one, as Bukkit numbers effects. */
    @Override
    public int getId() {
        return this.ordinal + 1;
    }

    @Override
    public String getName() {
        final String path = this.key.getKey();
        if (!this.key.getNamespace().equals(NamespacedKey.MINECRAFT) || !OLD.contains(path)) {
            return this.key.toString();
        }
        return LEGACY_NAMES.getOrDefault(path, path.toUpperCase(java.util.Locale.ROOT));
    }
}
