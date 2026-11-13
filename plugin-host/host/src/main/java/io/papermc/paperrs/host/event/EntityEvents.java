package io.papermc.paperrs.host.event;

import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.entity.Entities;
import io.papermc.paperrs.host.entity.HostDamageSource;
import io.papermc.paperrs.host.entity.HostPlayer;
import io.papermc.paperrs.host.item.HostItemStack;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import net.kyori.adventure.text.Component;
import org.bukkit.Location;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.World;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.damage.DamageSource;
import org.bukkit.damage.DamageType;
import org.bukkit.entity.Entity;
import org.bukkit.entity.LivingEntity;
import org.bukkit.event.entity.CreatureSpawnEvent;
import org.bukkit.event.entity.EntityDamageByBlockEvent;
import org.bukkit.event.entity.EntityDamageByEntityEvent;
import org.bukkit.event.entity.EntityDamageEvent;
import org.bukkit.event.entity.EntityDeathEvent;
import org.bukkit.event.entity.PlayerDeathEvent;
import org.bukkit.event.player.PlayerRespawnEvent;
import org.bukkit.inventory.ItemStack;
import org.jetbrains.annotations.Nullable;

/** Damage, death, respawn and spawn events the server fires (see {@code plugins::events}). */
final class EntityEvents {
    private final CraftServer server;

    EntityEvents(final CraftServer server) {
        this.server = server;
    }

    private @Nullable Entity entity(final Object ref) {
        return ref instanceof Map<?, ?> m ? Entities.of(this.server, Values.map(m)) : null;
    }

    private DamageSource source(final String type, final @Nullable Entity causing, final @Nullable Entity direct, final @Nullable Location at) {
        DamageType damageType = Registry.DAMAGE_TYPE.get(NamespacedKey.fromString(type));
        if (damageType == null) {
            damageType = Registry.DAMAGE_TYPE.get(NamespacedKey.minecraft("generic"));
        }
        return new HostDamageSource(damageType, causing, direct != null ? direct : causing, at);
    }

    private static List<ItemStack> stacks(final Object list) {
        final List<ItemStack> out = new ArrayList<>();
        for (Object item : Values.list(list)) {
            final HostItemStack stack = HostItemStack.fromValue(item);
            if (!stack.isEmpty()) {
                out.add(stack);
            }
        }
        return out;
    }

    private static List<Object> values(final List<ItemStack> stacks) {
        final List<Object> out = new ArrayList<>();
        for (ItemStack stack : stacks) {
            final Map<String, Object> value = HostItemStack.toValue(stack);
            if (value != null) {
                out.add(value);
            }
        }
        return out;
    }

    /** EntityDamageEvent, or its ByEntity / ByBlock form. */
    Object damage(final Map<String, Object> args) {
        final Entity target = this.entity(args.get("target"));
        if (target == null) {
            return Values.of();
        }
        final Entity attacker = this.entity(args.get("attacker"));
        final Entity direct = this.entity(args.get("direct"));
        final String type = Values.str(args, "cause", "minecraft:generic");
        final EntityDamageEvent.DamageCause cause = HostDamageSource.cause(type, attacker != null);
        final List<Object> position = Values.list(args.get("source_position"));
        final Location at = position.size() == 3
            ? new Location(target.getWorld(), ((Number) position.get(0)).doubleValue(), ((Number) position.get(1)).doubleValue(), ((Number) position.get(2)).doubleValue())
            : null;
        final DamageSource source = this.source(type, attacker, direct, at);
        final double amount = Values.f64(args, "amount", 0);
        final EntityDamageEvent event;
        if (attacker != null || direct != null) {
            event = new EntityDamageByEntityEvent(direct != null ? direct : attacker, target, cause, source, amount);
        } else if (HostDamageSource.byBlock(cause)) {
            // The server does not say which block yet.
            event = new EntityDamageByBlockEvent(null, null, target, cause, source, amount);
        } else {
            event = new EntityDamageEvent(target, cause, source, amount);
        }
        event.callEvent();
        return Values.of("cancelled", event.isCancelled(), "amount", event.getDamage());
    }

    /** EntityDeathEvent for a mob, with the drops and experience plugins may change. */
    Object entityDeath(final Map<String, Object> args) {
        final Entity entity = this.entity(Values.of("entity", args.get("entity"), "value", args.get("value")));
        if (!(entity instanceof LivingEntity living)) {
            return Values.of();
        }
        final Entity killer = this.entity(args.get("killer"));
        final DamageSource source = this.source(Values.str(args, "cause", "minecraft:generic"), killer, null, null);
        final List<ItemStack> drops = stacks(args.get("drops"));
        final EntityDeathEvent event = new EntityDeathEvent(living, source, drops, Values.i32(args, "xp", 0));
        event.callEvent();
        return Values.of("cancelled", event.isCancelled(), "drops", values(event.getDrops()), "xp", event.getDroppedExp());
    }

    /** PlayerDeathEvent: message, drops, experience and whether the inventory and level stay. */
    Object playerDeath(final Map<String, Object> args) {
        final HostPlayer player = this.server.player(Values.uuid(args, "uuid"));
        if (player == null) {
            return Values.of();
        }
        final Entity killer = this.entity(args.get("killer"));
        final DamageSource source = this.source(Values.str(args, "cause", "minecraft:generic"), killer, null, null);
        final List<ItemStack> drops = stacks(args.get("drops"));
        final Component message = Components.fromJson(args.get("message"));
        final PlayerDeathEvent event = new PlayerDeathEvent(player, source, drops, Values.i32(args, "xp", 0), message, true);
        event.setKeepInventory(Values.bool(args, "keep_inventory", false));
        event.setKeepLevel(Values.bool(args, "keep_level", false));
        event.callEvent();
        return Values.of("cancelled", event.isCancelled(), "message", Components.toJson(event.deathMessage()), "drops", values(event.getDrops()),
            "xp", event.getDroppedExp(), "keep_inventory", event.getKeepInventory(), "keep_level", event.getKeepLevel());
    }

    Object respawn(final Map<String, Object> args) {
        final HostPlayer player = this.server.player(Values.uuid(args, "uuid"));
        if (player == null) {
            return Values.of();
        }
        World world = this.server.getWorld(Values.str(args, "world", "world"));
        if (world == null) {
            world = this.server.getWorlds().getFirst();
        }
        final Location location = new Location(world, Values.f64(args, "x", 0), Values.f64(args, "y", 0), Values.f64(args, "z", 0),
            (float) Values.f64(args, "yaw", 0), (float) Values.f64(args, "pitch", 0));
        final PlayerRespawnEvent event = new PlayerRespawnEvent(player, location, false, false, false, PlayerRespawnEvent.RespawnReason.DEATH);
        event.callEvent();
        final Location to = event.getRespawnLocation();
        return Values.of("world", to.getWorld() == null ? world.getName() : to.getWorld().getName(), "x", to.getX(), "y", to.getY(), "z", to.getZ(),
            "yaw", (double) to.getYaw(), "pitch", (double) to.getPitch());
    }

    Object creatureSpawn(final Map<String, Object> args) {
        final Entity entity = this.entity(Values.of("entity", args.get("entity"), "value", args.get("value")));
        if (!(entity instanceof LivingEntity living)) {
            return Values.of("cancelled", false);
        }
        CreatureSpawnEvent.SpawnReason reason;
        try {
            reason = CreatureSpawnEvent.SpawnReason.valueOf(Values.str(args, "reason", "CUSTOM"));
        } catch (IllegalArgumentException e) {
            reason = CreatureSpawnEvent.SpawnReason.CUSTOM;
        }
        final CreatureSpawnEvent event = new CreatureSpawnEvent(living, reason);
        event.callEvent();
        return Values.of("cancelled", event.isCancelled());
    }
}
