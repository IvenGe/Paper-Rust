package io.papermc.paperrs.host.registry;

import io.papermc.paper.datacomponent.DataComponentType;
import io.papermc.paper.datacomponent.DataComponentTypes;
import io.papermc.paper.registry.RegistryAccess;
import io.papermc.paper.registry.RegistryKey;
import io.papermc.paperrs.host.data.HostData;
import io.papermc.paperrs.host.stub.*;
import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import org.bukkit.Keyed;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.entity.EntityType;
import org.jetbrains.annotations.Nullable;

/**
 * Every registry plugins can reach, built from the Rust server's data tables.
 *
 * <p>The API finds this class through {@code META-INF/services}; there is one instance per
 * JVM ({@link #instance()}). Entries are created lazily (see {@link HostRegistry}) and
 * {@link #lockReferenceHolders()} must run on the main thread before plugin code does.
 */
public final class HostRegistryAccess implements RegistryAccess {
    /** How to create each registry; filled before any registry object exists. */
    private final Map<RegistryKey<?>, java.util.function.Supplier<Registry<?>>> definitions = new LinkedHashMap<>();
    private final Map<RegistryKey<?>, Registry<?>> registries = new java.util.concurrent.ConcurrentHashMap<>();
    private final Map<Class<?>, RegistryKey<?>> byClass = new HashMap<>();
    private HostData data;
    private boolean built;

    public HostRegistryAccess() {
        // Registries are built on first use: creating one initialises org.bukkit.Registry,
        // whose initialiser looks this provider up, so it must not happen while the service
        // loader is still constructing it.
    }

    private synchronized void build() {
        if (this.built) {
            return;
        }
        this.built = true;
        this.data = HostData.get();
        this.register(RegistryKey.GAME_EVENT, org.bukkit.GameEvent.class, "game_event", HostGameEvent::new);
        this.register(RegistryKey.STRUCTURE_TYPE, org.bukkit.generator.structure.StructureType.class, "structure_type", HostStructureType::new);
        this.register(RegistryKey.MOB_EFFECT, org.bukkit.potion.PotionEffectType.class, "effect", HostPotionEffectType::new);
        this.register(RegistryKey.BLOCK, org.bukkit.block.BlockType.class, "block", HostBlockType::new);
        this.register(RegistryKey.ITEM, org.bukkit.inventory.ItemType.class, "item", HostItemType::new);
        this.register(RegistryKey.VILLAGER_PROFESSION, org.bukkit.entity.Villager.Profession.class, "entity.minecraft.villager", HostVillagerProfession::new);
        this.register(RegistryKey.POINT_OF_INTEREST_TYPE, io.papermc.paper.entity.poi.PoiTypes.class, "poi", HostPoiType::new);
        this.register(RegistryKey.VILLAGER_TYPE, org.bukkit.entity.Villager.Type.class, "villager_type", HostVillagerType::new);
        this.register(RegistryKey.MAP_DECORATION_TYPE, org.bukkit.map.MapCursor.Type.class, "map_decoration", HostMapCursorType::new);
        this.register(RegistryKey.MENU, org.bukkit.inventory.MenuType.class, "container", HostMenuType::new);
        this.register(RegistryKey.ATTRIBUTE, org.bukkit.attribute.Attribute.class, "attribute.name", HostAttribute::new);
        this.register(RegistryKey.FLUID, org.bukkit.Fluid.class, "fluid", HostFluid::new);
        this.register(RegistryKey.SOUND_EVENT, org.bukkit.Sound.class, "subtitles", HostSound::new);
        this.registerDataComponents();
        this.register(RegistryKey.GAME_RULE, org.bukkit.GameRule.class, "gamerule", HostGameRule::new);
        this.register(RegistryKey.BIOME, org.bukkit.block.Biome.class, "biome", HostBiome::new);
        this.register(RegistryKey.STRUCTURE, org.bukkit.generator.structure.Structure.class, "structure", HostStructure::new);
        this.register(RegistryKey.TRIM_MATERIAL, org.bukkit.inventory.meta.trim.TrimMaterial.class, "trim_material", HostTrimMaterial::new);
        this.register(RegistryKey.TRIM_PATTERN, org.bukkit.inventory.meta.trim.TrimPattern.class, "trim_pattern", HostTrimPattern::new);
        this.register(RegistryKey.DAMAGE_TYPE, org.bukkit.damage.DamageType.class, "death.attack", HostDamageType::new);
        this.register(RegistryKey.WOLF_VARIANT, org.bukkit.entity.Wolf.Variant.class, "wolf_variant", HostWolfVariant::new);
        this.register(RegistryKey.WOLF_SOUND_VARIANT, org.bukkit.entity.Wolf.SoundVariant.class, "wolf_sound_variant", HostWolfSoundVariant::new);
        this.register(RegistryKey.ENCHANTMENT, org.bukkit.enchantments.Enchantment.class, "enchantment", HostEnchantment::new);
        this.register(RegistryKey.JUKEBOX_SONG, org.bukkit.JukeboxSong.class, "jukebox_song", HostJukeboxSong::new);
        this.register(RegistryKey.BANNER_PATTERN, org.bukkit.block.banner.PatternType.class, "block.minecraft.banner", HostPatternType::new);
        this.register(RegistryKey.DECORATED_POT_PATTERN, io.papermc.paper.block.pot.PotPatternTypes.class, "decorated_pot_pattern", HostPotPatternType::new);
        this.register(RegistryKey.PAINTING_VARIANT, org.bukkit.Art.class, "painting", HostArt::new);
        this.register(RegistryKey.INSTRUMENT, org.bukkit.MusicInstrument.class, "instrument", HostMusicInstrument::new);
        this.register(RegistryKey.CAT_VARIANT, org.bukkit.entity.Cat.Type.class, "cat_variant", HostCatType::new);
        this.register(RegistryKey.CAT_SOUND_VARIANT, org.bukkit.entity.Cat.SoundVariant.class, "cat_sound_variant", HostCatSoundVariant::new);
        this.register(RegistryKey.FROG_VARIANT, org.bukkit.entity.Frog.Variant.class, "frog_variant", HostFrogVariant::new);
        this.register(RegistryKey.CHICKEN_VARIANT, org.bukkit.entity.Chicken.Variant.class, "chicken_variant", HostChickenVariant::new);
        this.register(RegistryKey.CHICKEN_SOUND_VARIANT, org.bukkit.entity.Chicken.SoundVariant.class, "chicken_sound_variant", HostChickenSoundVariant::new);
        this.register(RegistryKey.COW_VARIANT, org.bukkit.entity.Cow.Variant.class, "cow_variant", HostCowVariant::new);
        this.register(RegistryKey.COW_SOUND_VARIANT, org.bukkit.entity.Cow.SoundVariant.class, "cow_sound_variant", HostCowSoundVariant::new);
        this.register(RegistryKey.PIG_VARIANT, org.bukkit.entity.Pig.Variant.class, "pig_variant", HostPigVariant::new);
        this.register(RegistryKey.PIG_SOUND_VARIANT, org.bukkit.entity.Pig.SoundVariant.class, "pig_sound_variant", HostPigSoundVariant::new);
        this.register(RegistryKey.ZOMBIE_NAUTILUS_VARIANT, org.bukkit.entity.ZombieNautilus.Variant.class, "zombie_nautilus_variant", HostZombieNautilusVariant::new);
        this.register(RegistryKey.SULFUR_CUBE_ARCHETYPE, org.bukkit.entity.SulfurCube.Archetype.class, "sulfur_cube_archetype", HostSulfurCubeArchetype::new);
        this.register(RegistryKey.DIALOG, io.papermc.paper.dialog.Dialog.class, "dialog", HostDialog::new);
        this.definitions.put(RegistryKey.ENTITY_TYPE, () -> new HostEnumRegistry<>(EntityType.class, type -> type != EntityType.UNKNOWN));
        this.definitions.put(RegistryKey.PARTICLE_TYPE, () -> new HostEnumRegistry<>(org.bukkit.Particle.class, particle -> true));
        this.definitions.put(RegistryKey.POTION, () -> new HostEnumRegistry<>(org.bukkit.potion.PotionType.class, potion -> true));

        // Registry fields the API resolves by class (deprecated lookups).
        this.byClass.put(org.bukkit.Art.class, RegistryKey.PAINTING_VARIANT);
        this.byClass.put(org.bukkit.block.banner.PatternType.class, RegistryKey.BANNER_PATTERN);
        this.byClass.put(org.bukkit.block.Biome.class, RegistryKey.BIOME);
        this.byClass.put(org.bukkit.entity.Cat.Type.class, RegistryKey.CAT_VARIANT);
        this.byClass.put(org.bukkit.enchantments.Enchantment.class, RegistryKey.ENCHANTMENT);
        this.byClass.put(org.bukkit.MusicInstrument.class, RegistryKey.INSTRUMENT);
        this.byClass.put(org.bukkit.generator.structure.Structure.class, RegistryKey.STRUCTURE);
        this.byClass.put(org.bukkit.inventory.meta.trim.TrimMaterial.class, RegistryKey.TRIM_MATERIAL);
        this.byClass.put(org.bukkit.inventory.meta.trim.TrimPattern.class, RegistryKey.TRIM_PATTERN);
        this.byClass.put(org.bukkit.damage.DamageType.class, RegistryKey.DAMAGE_TYPE);
        this.byClass.put(org.bukkit.JukeboxSong.class, RegistryKey.JUKEBOX_SONG);
        this.byClass.put(org.bukkit.entity.Frog.Variant.class, RegistryKey.FROG_VARIANT);
        this.byClass.put(org.bukkit.entity.Wolf.Variant.class, RegistryKey.WOLF_VARIANT);
    }

    public static HostRegistryAccess instance() {
        return (HostRegistryAccess) RegistryAccess.registryAccess();
    }

    private <T extends Keyed> void register(final RegistryKey<T> key, final Class<?> holder, final String translationPrefix, final HostFactory<T> factory) {
        final String id = key.key().asString();
        final List<String> ids = this.data.registry(id) == null ? List.of() : this.data.registry(id);
        final Map<String, List<String>> tags = this.data.tags(id);
        this.definitions.put(key, () -> new HostRegistry<>(key, holder, ids,
            (entry, ordinal) -> factory.create(entry, ordinal, translationKey(translationPrefix, entry)), tags));
    }

    /** Data component types are valued or not depending on how {@code DataComponentTypes} declares them. */
    @SuppressWarnings({"rawtypes", "unchecked"})
    private void registerDataComponents() {
        final Set<String> nonValued = new HashSet<>();
        for (Field field : DataComponentTypes.class.getDeclaredFields()) {
            if (Modifier.isStatic(field.getModifiers()) && field.getType() == DataComponentType.NonValued.class) {
                nonValued.add(field.getName().toLowerCase(Locale.ROOT));
            }
        }
        this.register((RegistryKey) RegistryKey.DATA_COMPONENT_TYPE, DataComponentTypes.class, "data_component",
            (HostFactory) (key, ordinal, translationKey) -> nonValued.contains(key.getKey())
                ? new HostNonValuedComponentType(key, ordinal, translationKey)
                : new HostValuedComponentType<>(key, ordinal, translationKey));
    }

    private static String translationKey(final String prefix, final NamespacedKey key) {
        return prefix + "." + key.getNamespace() + "." + key.getKey().replace('/', '.');
    }

    @Override
    @SuppressWarnings({"unchecked", "removal"})
    public <T extends Keyed> @Nullable Registry<T> getRegistry(final Class<T> type) {
        this.build();
        final RegistryKey<?> key = this.byClass.get(type);
        return key == null ? null : (Registry<T>) this.registry(key);
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T extends Keyed> Registry<T> getRegistry(final RegistryKey<T> registryKey) {
        this.build();
        final Registry<?> registry = this.registry(registryKey);
        if (registry == null) {
            throw new IllegalArgumentException(registryKey + " is not available on Paper-RS");
        }
        return (Registry<T>) registry;
    }

    /**
     * The registry for {@code key}, created on first use. Creating one can initialise
     * {@code org.bukkit.Registry}, which looks up other registries (and possibly this one)
     * from inside this method; the first instance stored wins.
     */
    private @Nullable Registry<?> registry(final RegistryKey<?> key) {
        final Registry<?> existing = this.registries.get(key);
        if (existing != null) {
            return existing;
        }
        final java.util.function.Supplier<Registry<?>> definition = this.definitions.get(key);
        if (definition == null) {
            return null;
        }
        final Registry<?> created = definition.get();
        final Registry<?> raced = this.registries.putIfAbsent(key, created);
        return raced != null ? raced : created;
    }

    /**
     * Initialises every class that holds registry constants, on the calling (main) thread,
     * before plugin code can race it.
     */
    public void lockReferenceHolders() {
        this.build();
        for (RegistryKey<?> key : this.definitions.keySet()) {
            this.registry(key);
        }
        for (Registry<?> registry : this.registries.values()) {
            if (registry instanceof HostRegistry<?> host) {
                host.lockReferenceHolders();
            }
        }
    }

    @FunctionalInterface
    private interface HostFactory<T> {
        T create(NamespacedKey key, int ordinal, String translationKey);
    }
}
