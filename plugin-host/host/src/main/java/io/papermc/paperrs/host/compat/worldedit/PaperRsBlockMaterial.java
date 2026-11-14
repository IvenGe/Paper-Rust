package io.papermc.paperrs.host.compat.worldedit;

import com.sk89q.worldedit.world.registry.BlockMaterial;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import org.bukkit.Material;
import org.bukkit.Tag;
import org.bukkit.block.BlockType;

/** What WorldEdit asks about a kind of block, from the server's block tables. */
record PaperRsBlockMaterial(boolean isAir, boolean isFullCube, boolean isOpaque, boolean isPowerSource, boolean isLiquid, boolean isSolid,
                            float getHardness, float getResistance, float getSlipperiness, int getLightValue, boolean isFragileWhenPushed,
                            boolean isUnpushable, boolean isTicksRandomly, boolean isMovementBlocker, boolean isBurnable, boolean isToolRequired,
                            boolean isReplacedDuringPlacement, boolean isTranslucent, boolean hasContainer) implements BlockMaterial {
    private static final Map<String, PaperRsBlockMaterial> CACHE = new ConcurrentHashMap<>();

    private static final Set<String> POWER_SOURCES = Set.of("minecraft:redstone_block", "minecraft:redstone_torch", "minecraft:redstone_wall_torch",
        "minecraft:lever", "minecraft:daylight_detector", "minecraft:observer", "minecraft:target", "minecraft:tripwire_hook", "minecraft:trapped_chest",
        "minecraft:lectern", "minecraft:sculk_sensor", "minecraft:calibrated_sculk_sensor", "minecraft:detector_rail", "minecraft:lightning_rod");

    private static final Set<String> CONTAINERS = Set.of("minecraft:chest", "minecraft:trapped_chest", "minecraft:barrel", "minecraft:furnace",
        "minecraft:blast_furnace", "minecraft:smoker", "minecraft:hopper", "minecraft:dropper", "minecraft:dispenser", "minecraft:brewing_stand",
        "minecraft:crafter", "minecraft:chiseled_bookshelf", "minecraft:decorated_pot", "minecraft:jukebox", "minecraft:lectern", "minecraft:campfire",
        "minecraft:soul_campfire");

    static PaperRsBlockMaterial of(final String id) {
        return CACHE.computeIfAbsent(id, PaperRsBlockMaterial::compute);
    }

    private static PaperRsBlockMaterial compute(final String id) {
        final Material material = Material.matchMaterial(id);
        final BlockType type = material == null ? null : material.asBlockType();
        if (type == null) {
            return new PaperRsBlockMaterial(true, false, false, false, false, false, 0, 0, 0.6f, 0, false, false, false, false, false, false, true, true, false);
        }
        final boolean air = type.isAir();
        final boolean liquid = material == Material.WATER || material == Material.LAVA || material == Material.BUBBLE_COLUMN;
        final boolean occluding = type.isOccluding();
        final boolean container = CONTAINERS.contains(id) || id.endsWith("shulker_box");
        return new PaperRsBlockMaterial(
            air,
            occluding,
            occluding,
            POWER_SOURCES.contains(id) || id.endsWith("_button") || id.endsWith("_pressure_plate"),
            liquid,
            type.isSolid(),
            type.getHardness(),
            type.getBlastResistance(),
            type.getSlipperiness(),
            0,
            !type.isSolid() && !air && !liquid,
            material == Material.OBSIDIAN || material == Material.CRYING_OBSIDIAN || material == Material.BEDROCK || material == Material.END_PORTAL_FRAME
                || material == Material.REINFORCED_DEEPSLATE || container,
            false,
            type.hasCollision(),
            type.isBurnable(),
            false,
            air || liquid || Tag.REPLACEABLE.isTagged(material),
            !occluding,
            container);
    }
}
