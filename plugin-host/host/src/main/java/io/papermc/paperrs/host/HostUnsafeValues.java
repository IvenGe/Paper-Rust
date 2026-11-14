package io.papermc.paperrs.host;

import io.papermc.paper.registry.RegistryAccess;
import io.papermc.paper.registry.RegistryKey;
import io.papermc.paperrs.host.stub.AbstractUnsafeValues;
import java.util.logging.Logger;
import org.bukkit.Keyed;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.craftbukkit.util.ApiVersion;
import org.bukkit.plugin.InvalidPluginException;
import org.bukkit.plugin.PluginDescriptionFile;

/** {@code UnsafeValues}: the API version gate and version numbers. */
public final class HostUnsafeValues extends AbstractUnsafeValues {
    private static final Logger LOGGER = Logger.getLogger("Minecraft");

    @Override
    public void checkSupported(final PluginDescriptionFile pdf) throws InvalidPluginException {
        final String version = pdf.getAPIVersion();
        if (version == null) {
            // Plugins without api-version predate 1.13; their material names are not remapped here.
            LOGGER.warning("Legacy plugin " + pdf.getFullName() + " does not specify an api-version.");
            return;
        }
        if (!this.isSupportedApiVersion(version)) {
            throw new InvalidPluginException("Unsupported API version " + version);
        }
    }

    @Override
    public boolean isSupportedApiVersion(final String apiVersion) {
        if (apiVersion == null) {
            return false;
        }
        final ApiVersion version;
        try {
            version = ApiVersion.getOrCreateVersion(apiVersion);
        } catch (IllegalArgumentException e) {
            return false;
        }
        return !version.isNewerThan(ApiVersion.CURRENT);
    }

    @Override
    public byte[] processClass(final PluginDescriptionFile pdf, final String path, final byte[] clazz) {
        // Paper rewrites old plugin bytecode here (Commodore); Paper-RS loads classes unchanged.
        return clazz;
    }

    @Override
    public int getDataVersion() {
        return HostVersions.DATA_VERSION;
    }

    @Override
    public int getProtocolVersion() {
        return HostVersions.PROTOCOL_VERSION;
    }

    @Override
    public String getMainLevelName() {
        return "world";
    }

    @Override
    public Material toLegacy(final Material material) {
        return material;
    }

    @Override
    public Material fromLegacy(final Material material) {
        return material;
    }

    @Override
    public Material getMaterial(final String material, final int version) {
        return Material.matchMaterial(material);
    }

    @Override
    public org.bukkit.inventory.ItemStack deserializeStack(final java.util.Map<String, Object> args) {
        return io.papermc.paperrs.host.item.HostItemStack.deserialize(args);
    }

    @Override
    public <B extends Keyed> B get(final RegistryKey<B> registry, final NamespacedKey key) {
        return RegistryAccess.registryAccess().getRegistry(registry).get(key);
    }

    /** {@code PotionType} data: the potion's effects, and whether stronger or longer versions exist. */
    @Override
    public org.bukkit.potion.PotionType.InternalPotionData getInternalPotionData(final NamespacedKey key) {
        final io.papermc.paperrs.host.data.HostData data = io.papermc.paperrs.host.data.HostData.get();
        final java.util.List<io.papermc.paperrs.host.data.HostData.PotionEffectInfo> effects = data.potionEffects(key.toString());
        if (effects == null) {
            throw new IllegalArgumentException("Unknown potion " + key);
        }
        final java.util.List<org.bukkit.potion.PotionEffect> list = new java.util.ArrayList<>();
        for (final io.papermc.paperrs.host.data.HostData.PotionEffectInfo e : effects) {
            final org.bukkit.potion.PotionEffectType type = org.bukkit.Registry.MOB_EFFECT.get(NamespacedKey.fromString(e.effect()));
            if (type != null) {
                list.add(new org.bukkit.potion.PotionEffect(type, e.duration(), e.amplifier()));
            }
        }
        final java.util.List<org.bukkit.potion.PotionEffect> potionEffects = java.util.List.copyOf(list);
        final boolean instant = effects.stream().anyMatch(e -> data.isInstantEffect(e.effect()));
        final boolean upgradeable = data.potionEffects(key.getNamespace() + ":strong_" + key.getKey()) != null;
        final boolean extendable = data.potionEffects(key.getNamespace() + ":long_" + key.getKey()) != null;
        return new org.bukkit.potion.PotionType.InternalPotionData() {
            @Override
            public org.bukkit.potion.PotionEffectType getEffectType() {
                return potionEffects.isEmpty() ? null : potionEffects.get(0).getType();
            }

            @Override
            public java.util.List<org.bukkit.potion.PotionEffect> getPotionEffects() {
                return potionEffects;
            }

            @Override
            public boolean isInstant() {
                return instant;
            }

            @Override
            public boolean isUpgradeable() {
                return upgradeable;
            }

            @Override
            public boolean isExtendable() {
                return extendable;
            }

            @Override
            public int getMaxLevel() {
                return upgradeable ? 2 : 1;
            }
        };
    }
}
