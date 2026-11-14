package io.papermc.paperrs.host;

import io.papermc.paper.registry.RegistryAccess;
import io.papermc.paper.registry.RegistryKey;
import io.papermc.paperrs.host.stub.AbstractUnsafeValues;
import java.util.logging.Logger;
import org.bukkit.Keyed;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.block.data.BlockData;
import org.bukkit.craftbukkit.legacy.CraftLegacy;
import org.bukkit.craftbukkit.util.ApiVersion;
import org.bukkit.material.MaterialData;
import org.bukkit.plugin.InvalidPluginException;
import org.bukkit.plugin.PluginDescriptionFile;

/** {@code UnsafeValues}: the API version gate, version numbers, class rewriting and legacy materials. */
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

    /**
     * Paper's class rewriting for plugins as they load ({@code Commodore}): fields renamed since
     * the plugin's api-version, enums that became registries, and {@code Material.values()}
     * without the legacy materials. Plugins without an api-version (written for 1.12 or older)
     * are loaded unchanged: converting them also needs Paper's rerouting of every method that
     * takes a {@code Material}, which the host does not have.
     */
    @Override
    public byte[] processClass(final PluginDescriptionFile pdf, final String path, final byte[] clazz) {
        if (pdf.getAPIVersion() == null) {
            return clazz;
        }
        try {
            return COMMODORE.convert(clazz, pdf.getName(), ApiVersion.getOrCreateVersion(pdf.getAPIVersion()), java.util.Set.of());
        } catch (Exception ex) {
            LOGGER.log(java.util.logging.Level.SEVERE, "Fatal error trying to convert " + pdf.getFullName() + ":" + path, ex);
            return clazz;
        }
    }

    private static final org.bukkit.craftbukkit.util.Commodore COMMODORE = new org.bukkit.craftbukkit.util.Commodore(compatibility -> false);

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
        return CraftLegacy.toLegacy(material);
    }

    @Override
    public Material fromLegacy(final Material material) {
        return CraftLegacy.fromLegacy(material);
    }

    @Override
    public Material fromLegacy(final MaterialData material) {
        return CraftLegacy.fromLegacy(material);
    }

    @Override
    public Material fromLegacy(final MaterialData material, final boolean itemPriority) {
        return CraftLegacy.fromLegacy(material, itemPriority);
    }

    @Override
    public BlockData fromLegacy(final Material material, final byte data) {
        return CraftLegacy.fromLegacyData(CraftLegacy.toLegacy(material), data);
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
