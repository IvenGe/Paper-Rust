package io.papermc.paperrs.test.bukkit;

import net.kyori.adventure.text.Component;
import org.bukkit.Bukkit;
import org.bukkit.Location;
import org.bukkit.entity.EntityType;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.entity.CreatureSpawnEvent;
import org.bukkit.event.entity.EntityDamageEvent;
import org.bukkit.event.player.PlayerMoveEvent;
import org.bukkit.plugin.Plugin;

/**
 * Load for the server's plugin benchmark ({@code tests/load.rs} with {@code BENCH_PLUGINS=1}),
 * doing what common plugins do: a region check on every move, spawn and damage rules, a line
 * on every player's screen each tick, and a look at the entities around each player every
 * second. Only started with {@code -Dpaperrs.bench=true}.
 */
final class Bench implements Listener {
    private final Plugin plugin;
    private long blocksCrossed;
    private long spawns;
    private long damages;
    private long nearby;

    Bench(final Plugin plugin) {
        this.plugin = plugin;
    }

    void start() {
        Bukkit.getPluginManager().registerEvents(this, this.plugin);
        // A scoreboard or tab list plugin: something on every player's screen, every tick.
        Bukkit.getScheduler().runTaskTimer(this.plugin, () -> {
            for (final Player player : Bukkit.getOnlinePlayers()) {
                player.sendActionBar(Component.text("crossed " + this.blocksCrossed + ", spawns " + this.spawns + ", nearby " + this.nearby));
            }
        }, 1L, 1L);
        // A mob stacker or protection plugin: the entities around each player, every second.
        Bukkit.getScheduler().runTaskTimer(this.plugin, () -> {
            for (final Player player : Bukkit.getOnlinePlayers()) {
                this.nearby = player.getNearbyEntities(16, 8, 16).size();
            }
        }, 20L, 20L);
    }

    @EventHandler(ignoreCancelled = true)
    public void onMove(final PlayerMoveEvent event) {
        // A region plugin: has the player crossed into another block?
        final Location from = event.getFrom();
        final Location to = event.getTo();
        if (from.getBlockX() != to.getBlockX() || from.getBlockZ() != to.getBlockZ()) {
            this.blocksCrossed++;
        }
    }

    @EventHandler(ignoreCancelled = true)
    public void onSpawn(final CreatureSpawnEvent event) {
        this.spawns++;
        if (event.getEntityType() == EntityType.PHANTOM) {
            event.setCancelled(true);
        }
    }

    @EventHandler(ignoreCancelled = true)
    public void onDamage(final EntityDamageEvent event) {
        this.damages++;
    }
}
