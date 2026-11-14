package org.bukkit.craftbukkit.legacy;

import com.google.common.base.Preconditions;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import io.papermc.paperrs.host.data.HostData;
import io.papermc.paperrs.host.world.HostBlockData;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.HashMap;
import java.util.Locale;
import java.util.Map;
import java.util.logging.Logger;
import org.bukkit.Material;
import org.bukkit.block.data.BlockData;
import org.bukkit.entity.EntityType;
import org.bukkit.material.MaterialData;
import org.jetbrains.annotations.Nullable;

/**
 * Bukkit's legacy materials (the {@code LEGACY_} constants, the numeric ids before 1.13) and
 * what they are now. The tables and lookups are those of Paper's {@code CraftLegacy}, which
 * fills them by running Minecraft's data fixers over every legacy id and data value; here they
 * are read from {@code legacy.json} ({@code id:data} to 26.3 block states and items, built by
 * {@code tools/datagen/legacy.py}). Block states are kept as state ids.
 *
 * @deprecated legacy use only
 */
@Deprecated
public final class CraftLegacy {
    private static final Logger LOGGER = Logger.getLogger("Minecraft");

    private static final Map<Byte, Material> SPAWN_EGGS = new HashMap<>();
    private static final Map<MaterialData, Material> materialToItem = new HashMap<>(16384);
    private static final Map<Material, MaterialData> itemToMaterial = new HashMap<>(1024);
    private static final Map<MaterialData, Integer> materialToState = new HashMap<>(4096);
    private static final Map<Integer, MaterialData> stateToMaterial = new HashMap<>(4096);
    private static final Map<MaterialData, Material> materialToBlock = new HashMap<>(4096);
    private static final Map<Material, MaterialData> blockToMaterial = new HashMap<>(1024);

    private CraftLegacy() {
    }

    public static Material toLegacy(Material material) {
        if (material == null || material.isLegacy()) {
            return material;
        }

        return CraftLegacy.toLegacyData(material).getItemType();
    }

    public static MaterialData toLegacyData(Material material) {
        return CraftLegacy.toLegacyData(material, false);
    }

    public static MaterialData toLegacyData(Material material, boolean itemPriority) {
        Preconditions.checkArgument(!material.isLegacy(), "toLegacy on legacy Material");
        MaterialData mappedData = null;

        if (itemPriority) {
            mappedData = CraftLegacy.itemToMaterial.get(material);
        }

        if (mappedData == null && material.isBlock()) {
            final HostData.BlockInfo block = HostData.get().block(material.getKey().toString());

            // Try exact match first
            mappedData = block == null ? null : CraftLegacy.stateToMaterial.get(block.defaultState());
            // Fallback to any block
            if (mappedData == null) {
                mappedData = CraftLegacy.blockToMaterial.get(material);
                // Fallback to matching item
                if (mappedData == null && block != null && block.item() != null) {
                    mappedData = CraftLegacy.itemToMaterial.get(CraftLegacy.material(block.item()));
                }
            }
        } else if (!itemPriority) {
            mappedData = CraftLegacy.itemToMaterial.get(material);
        }

        return (mappedData == null) ? new MaterialData(Material.LEGACY_AIR) : mappedData;
    }

    /** The block state of a legacy material and data value (Paper returns Minecraft's state). */
    public static BlockData fromLegacyData(Material material, byte data) {
        Preconditions.checkArgument(material.isLegacy(), "fromLegacyData on modern Material");

        MaterialData materialData = new MaterialData(material, data);

        // Try exact match first
        Integer converted = CraftLegacy.materialToState.get(materialData);
        if (converted != null) {
            return HostBlockData.ofState(HostData.get().stateToString(converted));
        }

        // Fallback to any block
        Material convertedBlock = CraftLegacy.materialToBlock.get(materialData);
        if (convertedBlock != null) {
            return HostBlockData.defaults(convertedBlock);
        }

        // Return air
        return HostBlockData.defaults(Material.AIR);
    }

    /** The item of a legacy material and data value (Paper returns Minecraft's item). */
    public static Material fromLegacyData(Material material, short data) {
        Preconditions.checkArgument(material.isLegacy(), "fromLegacyData on modern Material. Did you forget to define a modern (1.13+) api-version in your plugin.yml?");

        MaterialData materialData = new MaterialData(material, (byte) data);

        // First try matching item
        Material convertedItem = CraftLegacy.materialToItem.get(materialData);
        if (convertedItem != null) {
            return convertedItem;
        }

        // Fallback to matching block
        if (CraftLegacy.isBlock(material)) {
            // Try exact match first
            Integer converted = CraftLegacy.materialToState.get(materialData);
            if (converted != null) {
                return CraftLegacy.blockItem(CraftLegacy.blockMaterial(converted));
            }

            // Fallback to any block
            Material convertedBlock = CraftLegacy.materialToBlock.get(materialData);
            if (convertedBlock != null) {
                return CraftLegacy.blockItem(convertedBlock);
            }
        }

        // Return air
        return Material.AIR;
    }

    public static byte toLegacyData(BlockData state) {
        return CraftLegacy.toLegacy(state).getData();
    }

    public static Material toLegacyMaterial(BlockData state) {
        return CraftLegacy.toLegacy(state).getItemType();
    }

    public static MaterialData toLegacy(BlockData state) {
        MaterialData mappedData;

        // Try exact match first
        mappedData = CraftLegacy.stateToMaterial.get(HostData.get().parseState(state.getAsString()));
        // Fallback to any block
        if (mappedData == null) {
            mappedData = CraftLegacy.blockToMaterial.get(state.getMaterial());
        }

        return (mappedData == null) ? new MaterialData(Material.LEGACY_AIR) : mappedData;
    }

    public static Material fromLegacy(Material material) {
        if (material == null || !material.isLegacy()) {
            return material;
        }

        return CraftLegacy.fromLegacy(new MaterialData(material));
    }

    public static Material fromLegacy(MaterialData materialData) {
        return CraftLegacy.fromLegacy(materialData, false);
    }

    public static Material fromLegacy(MaterialData materialData, boolean itemPriority) {
        Material material = materialData.getItemType();
        if (material == null || !material.isLegacy()) {
            return material;
        }

        Material mappedData = null;

        // Try item first
        if (itemPriority) {
            mappedData = CraftLegacy.materialToItem.get(materialData);
        }

        if (mappedData == null) {
            // Try exact match first
            Integer iblock = CraftLegacy.materialToState.get(materialData);
            if (iblock != null) {
                mappedData = CraftLegacy.blockMaterial(iblock);
            }

            // Fallback to any block
            if (mappedData == null) {
                mappedData = CraftLegacy.materialToBlock.get(materialData);
            }
        }

        // Fallback to matching item
        if (!itemPriority && mappedData == null) {
            mappedData = CraftLegacy.materialToItem.get(materialData);
        }

        return (mappedData == null) ? Material.AIR : mappedData;
    }

    public static Material[] values() {
        Material[] values = Material.values();
        return Arrays.copyOfRange(values, Material.LEGACY_AIR.ordinal(), values.length);
    }

    public static Material valueOf(String name) {
        return (name.startsWith(Material.LEGACY_PREFIX)) ? Material.valueOf(name) : Material.valueOf(Material.LEGACY_PREFIX + name);
    }

    public static Material getMaterial(String name) {
        return (name.startsWith(Material.LEGACY_PREFIX)) ? Material.getMaterial(name) : Material.getMaterial(Material.LEGACY_PREFIX + name);
    }

    public static Material matchMaterial(String name) {
        return (name.startsWith(Material.LEGACY_PREFIX)) ? Material.matchMaterial(name) : Material.matchMaterial(Material.LEGACY_PREFIX + name);
    }

    public static int ordinal(Material material) {
        Preconditions.checkArgument(material.isLegacy(), "ordinal on modern Material");

        return material.ordinal() - Material.LEGACY_AIR.ordinal();
    }

    public static String name(Material material) {
        return material.name().substring(Material.LEGACY_PREFIX.length());
    }

    public static String toString(Material material) {
        return CraftLegacy.name(material);
    }

    public static void init() {
    }

    // --- the tables ---------------------------------------------------------------------------

    private static @Nullable Material material(String key) {
        return Material.getMaterial(key.substring(key.indexOf(':') + 1).toUpperCase(Locale.ROOT));
    }

    /** The material of a state's block. */
    private static @Nullable Material blockMaterial(int state) {
        return CraftLegacy.material(HostData.get().blockOfState(state).name());
    }

    /** {@code Block.asItem()}: the item of a block, air if it has none. */
    private static Material blockItem(@Nullable Material block) {
        if (block == null) {
            return Material.AIR;
        }
        final HostData.BlockInfo info = HostData.get().block(block.getKey().toString());
        final Material item = info == null || info.item() == null ? null : CraftLegacy.material(info.item());
        return item == null ? Material.AIR : item;
    }

    private static boolean isBlock(Material material) {
        // From Material#isBlock before the rewrite to ItemType / BlockType
        // Git hash: 42f6cdf4c5dcdd52a27543403dcd17fb60311621
        return 0 <= material.getId() && material.getId() < 256;
    }

    private static JsonObject table() {
        try (InputStream in = CraftLegacy.class.getClassLoader().getResourceAsStream("paper-rs-data/legacy.json")) {
            if (in == null) {
                throw new IllegalStateException("missing bundled data file paper-rs-data/legacy.json");
            }
            return JsonParser.parseReader(new InputStreamReader(in, StandardCharsets.UTF_8)).getAsJsonObject();
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    private static @Nullable String lookup(JsonObject table, int id, int data) {
        // The data fixers treat a data value with no meaning as the id's first (data 0).
        JsonElement e = table.get(id + ":" + data);
        if (e == null) {
            e = table.get(id + ":0");
        }
        return e == null ? null : e.getAsString();
    }

    static {
        LOGGER.warning("Initializing Legacy Material Support. Unless you have legacy plugins and/or data this is a bug!");

        SPAWN_EGGS.put((byte) 0, Material.PIG_SPAWN_EGG);

        SPAWN_EGGS.put((byte) EntityType.BAT.getTypeId(), Material.BAT_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.BLAZE.getTypeId(), Material.BLAZE_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.CAVE_SPIDER.getTypeId(), Material.CAVE_SPIDER_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.CHICKEN.getTypeId(), Material.CHICKEN_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.COD.getTypeId(), Material.COD_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.COW.getTypeId(), Material.COW_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.CREEPER.getTypeId(), Material.CREEPER_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.DOLPHIN.getTypeId(), Material.DOLPHIN_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.DONKEY.getTypeId(), Material.DONKEY_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.ELDER_GUARDIAN.getTypeId(), Material.ELDER_GUARDIAN_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.ENDERMAN.getTypeId(), Material.ENDERMAN_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.ENDERMITE.getTypeId(), Material.ENDERMITE_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.EVOKER.getTypeId(), Material.EVOKER_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.GHAST.getTypeId(), Material.GHAST_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.GUARDIAN.getTypeId(), Material.GUARDIAN_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.HORSE.getTypeId(), Material.HORSE_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.HUSK.getTypeId(), Material.HUSK_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.LLAMA.getTypeId(), Material.LLAMA_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.MAGMA_CUBE.getTypeId(), Material.MAGMA_CUBE_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.MOOSHROOM.getTypeId(), Material.MOOSHROOM_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.MULE.getTypeId(), Material.MULE_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.OCELOT.getTypeId(), Material.OCELOT_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.PARROT.getTypeId(), Material.PARROT_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.PIG.getTypeId(), Material.PIG_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.PHANTOM.getTypeId(), Material.PHANTOM_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.POLAR_BEAR.getTypeId(), Material.POLAR_BEAR_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.PUFFERFISH.getTypeId(), Material.PUFFERFISH_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.RABBIT.getTypeId(), Material.RABBIT_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.SALMON.getTypeId(), Material.SALMON_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.SHEEP.getTypeId(), Material.SHEEP_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.SHULKER.getTypeId(), Material.SHULKER_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.SILVERFISH.getTypeId(), Material.SILVERFISH_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.SKELETON.getTypeId(), Material.SKELETON_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.SKELETON_HORSE.getTypeId(), Material.SKELETON_HORSE_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.SLIME.getTypeId(), Material.SLIME_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.SPIDER.getTypeId(), Material.SPIDER_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.SQUID.getTypeId(), Material.SQUID_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.STRAY.getTypeId(), Material.STRAY_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.TROPICAL_FISH.getTypeId(), Material.TROPICAL_FISH_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.TURTLE.getTypeId(), Material.TURTLE_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.VEX.getTypeId(), Material.VEX_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.VILLAGER.getTypeId(), Material.VILLAGER_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.VINDICATOR.getTypeId(), Material.VINDICATOR_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.WITCH.getTypeId(), Material.WITCH_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.WITHER_SKELETON.getTypeId(), Material.WITHER_SKELETON_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.WOLF.getTypeId(), Material.WOLF_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.ZOMBIE.getTypeId(), Material.ZOMBIE_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.ZOMBIE_HORSE.getTypeId(), Material.ZOMBIE_HORSE_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.ZOMBIFIED_PIGLIN.getTypeId(), Material.ZOMBIFIED_PIGLIN_SPAWN_EGG);
        SPAWN_EGGS.put((byte) EntityType.ZOMBIE_VILLAGER.getTypeId(), Material.ZOMBIE_VILLAGER_SPAWN_EGG);

        final JsonObject table = CraftLegacy.table();
        final JsonObject blocks = table.getAsJsonObject("blocks");
        final JsonObject items = table.getAsJsonObject("items");
        final HostData data = HostData.get();

        for (Material material : Material.values()) {
            if (!material.isLegacy()) {
                continue;
            }

            // Handle blocks
            if (CraftLegacy.isBlock(material)) { // Use custom method instead of Material#isBlock since it relies on this being already run
                for (byte value = 0; value < 16; value++) {
                    final String name = CraftLegacy.lookup(blocks, material.getId(), value);
                    if (name == null) {
                        continue;
                    }
                    final int state = data.parseState(name);
                    final Material block = state < 0 ? null : CraftLegacy.blockMaterial(state);
                    if (block == null || block == Material.AIR) {
                        continue;
                    }

                    MaterialData matData = new MaterialData(material, value);
                    materialToState.put(matData, state);
                    if (!stateToMaterial.containsKey(state)) {
                        stateToMaterial.put(state, matData);
                    }

                    materialToBlock.put(matData, block);
                    if (!blockToMaterial.containsKey(block)) {
                        blockToMaterial.put(block, matData);
                    }
                }
            }

            // Handle items (and second fallback for blocks)
            int maxData = switch (material) {
                case LEGACY_WOOD_AXE, LEGACY_WOOD_HOE, LEGACY_WOOD_PICKAXE, LEGACY_WOOD_SPADE,
                     LEGACY_WOOD_SWORD,
                     LEGACY_STONE_AXE, LEGACY_STONE_HOE, LEGACY_STONE_PICKAXE, LEGACY_STONE_SPADE,
                     LEGACY_STONE_SWORD,
                     LEGACY_IRON_AXE, LEGACY_IRON_HOE, LEGACY_IRON_PICKAXE, LEGACY_IRON_SPADE,
                     LEGACY_IRON_SWORD,
                     LEGACY_GOLD_AXE, LEGACY_GOLD_HOE, LEGACY_GOLD_PICKAXE, LEGACY_GOLD_SPADE,
                     LEGACY_GOLD_SWORD,
                     LEGACY_DIAMOND_AXE, LEGACY_DIAMOND_HOE, LEGACY_DIAMOND_PICKAXE, LEGACY_DIAMOND_SPADE,
                     LEGACY_DIAMOND_SWORD,
                     LEGACY_LEATHER_HELMET, LEGACY_LEATHER_CHESTPLATE,
                     LEGACY_LEATHER_LEGGINGS, LEGACY_LEATHER_BOOTS,
                     LEGACY_CHAINMAIL_HELMET, LEGACY_CHAINMAIL_CHESTPLATE,
                     LEGACY_CHAINMAIL_LEGGINGS, LEGACY_CHAINMAIL_BOOTS,
                     LEGACY_IRON_HELMET, LEGACY_IRON_CHESTPLATE,
                     LEGACY_IRON_LEGGINGS, LEGACY_IRON_BOOTS,
                     LEGACY_GOLD_HELMET, LEGACY_GOLD_CHESTPLATE,
                     LEGACY_GOLD_LEGGINGS, LEGACY_GOLD_BOOTS,
                     LEGACY_DIAMOND_HELMET, LEGACY_DIAMOND_CHESTPLATE,
                     LEGACY_DIAMOND_LEGGINGS, LEGACY_DIAMOND_BOOTS,
                     LEGACY_FISHING_ROD,
                     LEGACY_CARROT_STICK,
                     LEGACY_BOW,
                     LEGACY_SHEARS,
                     LEGACY_FLINT_AND_STEEL,
                     LEGACY_SHIELD,
                     LEGACY_ELYTRA -> 1;
                default -> 16;
            };
            // Old spawn eggs are added below
            if (material != Material.LEGACY_MONSTER_EGG) {
                for (byte value = 0; value < maxData; value++) {
                    final String name = CraftLegacy.lookup(items, material.getId(), value);
                    final Material newMaterial = name == null ? null : CraftLegacy.material(name);
                    if (newMaterial == null || newMaterial == Material.AIR) {
                        continue;
                    }

                    MaterialData matData = new MaterialData(material, value);
                    materialToItem.put(matData, newMaterial);
                    if (!itemToMaterial.containsKey(newMaterial)) {
                        itemToMaterial.put(newMaterial, matData);
                    }
                }
            }
        }

        for (Map.Entry<Byte, Material> entry : SPAWN_EGGS.entrySet()) {
            MaterialData matData = new MaterialData(Material.LEGACY_MONSTER_EGG, entry.getKey());
            materialToItem.put(matData, entry.getValue());
            itemToMaterial.put(entry.getValue(), matData);
        }
    }
}
