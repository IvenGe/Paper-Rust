package io.papermc.paperrs.host.item;

import org.bukkit.Material;
import org.bukkit.inventory.meta.ArmorMeta;
import org.bukkit.inventory.meta.BannerMeta;
import org.bukkit.inventory.meta.BlockDataMeta;
import org.bukkit.inventory.meta.BlockStateMeta;
import org.bukkit.inventory.meta.BookMeta;
import org.bukkit.inventory.meta.BundleMeta;
import org.bukkit.inventory.meta.ColorableArmorMeta;
import org.bukkit.inventory.meta.CompassMeta;
import org.bukkit.inventory.meta.CrossbowMeta;
import org.bukkit.inventory.meta.Damageable;
import org.bukkit.inventory.meta.EnchantmentStorageMeta;
import org.bukkit.inventory.meta.FireworkEffectMeta;
import org.bukkit.inventory.meta.FireworkMeta;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.inventory.meta.MapMeta;
import org.bukkit.inventory.meta.OminousBottleMeta;
import org.bukkit.inventory.meta.PotionMeta;
import org.bukkit.inventory.meta.Repairable;
import org.bukkit.inventory.meta.SkullMeta;
import org.bukkit.inventory.meta.SpawnEggMeta;
import org.bukkit.inventory.meta.SuspiciousStewMeta;
import org.bukkit.inventory.meta.WritableBookMeta;

/**
 * The interfaces an item meta proxy implements, by item. Every meta is also
 * {@link Damageable}, {@link Repairable} and {@link BlockDataMeta}, as in Paper; each
 * combination redeclares {@code clone()} so the proxy has one most specific return type.
 * Methods of the special metas that Paper-RS does not implement throw a named
 * {@code UnsupportedOperationException}.
 */
public final class MetaKinds {
    private MetaKinds() {
    }

    public interface Base extends ItemMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Base clone();
    }

    public interface EnchantmentStorage extends EnchantmentStorageMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        EnchantmentStorage clone();
    }

    public interface LeatherArmor extends ColorableArmorMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        LeatherArmor clone();
    }

    public interface Armor extends ArmorMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Armor clone();
    }

    public interface Skull extends SkullMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Skull clone();
    }

    public interface Potion extends PotionMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Potion clone();
    }

    public interface Book extends BookMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Book clone();
    }

    public interface WritableBook extends WritableBookMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        WritableBook clone();
    }

    public interface Banner extends BannerMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Banner clone();
    }

    public interface SpawnEgg extends SpawnEggMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        SpawnEgg clone();
    }

    public interface Firework extends FireworkMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Firework clone();
    }

    public interface FireworkEffect extends FireworkEffectMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        FireworkEffect clone();
    }

    public interface Map extends MapMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Map clone();
    }

    public interface Crossbow extends CrossbowMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Crossbow clone();
    }

    public interface Compass extends CompassMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Compass clone();
    }

    public interface Bundle extends BundleMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        Bundle clone();
    }

    public interface SuspiciousStew extends SuspiciousStewMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        SuspiciousStew clone();
    }

    public interface BlockState extends BlockStateMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        BlockState clone();
    }

    public interface OminousBottle extends OminousBottleMeta, Damageable, Repairable, BlockDataMeta {
        @Override
        OminousBottle clone();
    }

    /** The meta interface for an item, as Paper's {@code CraftItemMetas} picks it. */
    public static Class<? extends ItemMeta> of(final Material material) {
        final String key = material.getKey().getKey();
        if (key.equals("enchanted_book")) {
            return EnchantmentStorage.class;
        }
        if (key.startsWith("leather_") && (key.endsWith("_helmet") || key.endsWith("_chestplate") || key.endsWith("_leggings") || key.endsWith("_boots"))
            || key.equals("leather_horse_armor") || key.equals("wolf_armor")) {
            return LeatherArmor.class;
        }
        if (key.endsWith("_helmet") || key.endsWith("_chestplate") || key.endsWith("_leggings") || key.endsWith("_boots")) {
            return Armor.class;
        }
        if (key.endsWith("_head") || key.endsWith("_skull")) {
            return Skull.class;
        }
        if (key.equals("ominous_bottle")) {
            return OminousBottle.class;
        }
        if (key.equals("potion") || key.equals("splash_potion") || key.equals("lingering_potion") || key.equals("tipped_arrow")) {
            return Potion.class;
        }
        if (key.equals("written_book")) {
            return Book.class;
        }
        if (key.equals("writable_book")) {
            return WritableBook.class;
        }
        if (key.endsWith("_banner")) {
            return Banner.class;
        }
        if (key.endsWith("_spawn_egg")) {
            return SpawnEgg.class;
        }
        if (key.equals("firework_rocket")) {
            return Firework.class;
        }
        if (key.equals("firework_star")) {
            return FireworkEffect.class;
        }
        if (key.equals("filled_map")) {
            return Map.class;
        }
        if (key.equals("crossbow")) {
            return Crossbow.class;
        }
        if (key.equals("compass")) {
            return Compass.class;
        }
        if (key.equals("bundle") || key.endsWith("_bundle")) {
            return Bundle.class;
        }
        if (key.equals("suspicious_stew")) {
            return SuspiciousStew.class;
        }
        if (key.endsWith("shulker_box") || switch (key) {
            case "chest", "trapped_chest", "barrel", "furnace", "smoker", "blast_furnace", "hopper", "dispenser", "dropper", "brewing_stand",
                 "beacon", "spawner", "beehive", "bee_nest", "lectern", "campfire", "soul_campfire", "jukebox", "decorated_pot", "crafter" -> true;
            default -> false;
        }) {
            return BlockState.class;
        }
        return Base.class;
    }
}
