package io.papermc.paperrs.host;

import io.papermc.paper.command.brigadier.CommandSourceStack;
import io.papermc.paper.entity.poi.PoiType;
import io.papermc.paper.plugin.lifecycle.event.LifecycleEventManager;
import io.papermc.paper.plugin.lifecycle.event.PaperLifecycleEventManager;
import io.papermc.paperrs.host.stub.AbstractInternalAPIBridge;
import io.papermc.paperrs.host.stub.HostBiome;
import io.papermc.paperrs.host.stub.HostGameRule;
import java.util.function.BooleanSupplier;
import java.util.function.Function;
import java.util.function.Predicate;
import net.kyori.adventure.text.flattener.ComponentFlattener;
import org.bukkit.GameRule;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.Sound;
import org.bukkit.damage.DamageEffect;
import org.bukkit.entity.EntityType;
import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.java.JavaPlugin;

/** The internal bridge the API calls for objects only the server can make. */
public final class HostInternalAPIBridge extends AbstractInternalAPIBridge {

    @Override
    public LifecycleEventManager<Plugin> createPluginLifecycleEventManager(final JavaPlugin plugin, final BooleanSupplier registrationCheck) {
        return new PaperLifecycleEventManager<>(plugin, registrationCheck);
    }

    @Override
    public Predicate<CommandSourceStack> restricted(final Predicate<CommandSourceStack> predicate, final boolean restricted) {
        return predicate;
    }

    @Override
    public <MODERN, LEGACY> GameRule<LEGACY> legacyGameRuleBridge(final GameRule<MODERN> rule, final Function<LEGACY, MODERN> fromLegacy, final Function<MODERN, LEGACY> toLegacy, final Class<LEGACY> legacyClass) {
        if (rule == null) {
            // GameRules is still initialising (GameRule must be initialised first).
            throw new IllegalStateException("game rules are initialised in the wrong order");
        }
        return new HostGameRule<>(rule.getKey(), -1, rule.translationKey());
    }

    @Override
    public org.bukkit.block.Biome constructLegacyCustomBiome() {
        return new HostBiome(NamespacedKey.minecraft("custom"), -1, "biome.minecraft.custom");
    }

    @Override
    public DamageEffect getDamageEffect(final String key) {
        final String sound = switch (key) {
            case "thorns" -> "enchant.thorns.hit";
            case "drowning" -> "entity.player.hurt_drown";
            case "burning" -> "entity.player.hurt_on_fire";
            case "poking" -> "entity.player.hurt_sweet_berry_bush";
            case "freezing" -> "entity.player.hurt_freeze";
            default -> "entity.player.hurt";
        };
        return new DamageEffect() {
            @Override
            public Sound getSound() {
                return Registry.SOUNDS.get(NamespacedKey.minecraft(sound));
            }
        };
    }

    @Override
    public PoiType.Occupancy createOccupancy(final String name) {
        return new PoiType.Occupancy() {
            @Override
            public String toString() {
                return name;
            }
        };
    }

    @Override
    public ComponentFlattener componentFlattener() {
        return ComponentFlattener.basic();
    }

    @Override
    public org.bukkit.damage.DamageSource.Builder createDamageSourceBuilder(final org.bukkit.damage.DamageType type) {
        return new io.papermc.paperrs.host.entity.HostDamageSource.Builder(type);
    }

    @Override
    public <C> io.papermc.paper.persistence.PersistentDataKey<C> createPersistentDataKey(final net.kyori.adventure.key.Key key, final org.bukkit.persistence.PersistentDataType<?, C> type) {
        final org.bukkit.NamespacedKey namespaced = new org.bukkit.NamespacedKey(key.namespace(), key.value());
        return new io.papermc.paper.persistence.PersistentDataKey<>() {
            @Override
            public org.bukkit.NamespacedKey key() {
                return namespaced;
            }

            @Override
            public org.bukkit.persistence.PersistentDataType<?, C> dataType() {
                return type;
            }
        };
    }

    @Override
    public org.bukkit.inventory.ItemStack createEmptyStack() {
        return io.papermc.paperrs.host.item.HostItemStack.empty();
    }

    @Override
    public org.bukkit.inventory.ItemStack deserializeItem(final byte[] bytes) {
        return io.papermc.paperrs.host.item.HostItemStack.deserializeBytes(bytes);
    }

    @Override
    public String getTranslationKey(final EntityType type) {
        return "entity." + type.getKey().getNamespace() + "." + type.getKey().getKey();
    }
}
