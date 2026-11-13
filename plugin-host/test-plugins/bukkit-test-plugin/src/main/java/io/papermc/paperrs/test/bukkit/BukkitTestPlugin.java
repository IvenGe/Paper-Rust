package io.papermc.paperrs.test.bukkit;

import java.util.List;
import net.kyori.adventure.text.Component;
import org.bukkit.Material;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.block.BlockBreakEvent;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.event.player.PlayerQuitEvent;
import org.bukkit.plugin.java.JavaPlugin;

/** A classic plugin.yml plugin: commands, listeners, configuration and the scheduler. */
public final class BukkitTestPlugin extends JavaPlugin implements Listener {

    @Override
    public void onLoad() {
        this.getLogger().info("onLoad called");
    }

    @Override
    public void onEnable() {
        this.saveDefaultConfig();
        this.getServer().getPluginManager().registerEvents(this, this);
        this.getCommand("greet").setExecutor((sender, command, label, args) -> {
            final String who = args.length > 0 ? args[0] : sender.getName();
            sender.sendMessage(this.getConfig().getString("greeting", "Hi") + ", " + who + "!");
            return true;
        });
        this.getCommand("greet").setTabCompleter((sender, command, alias, args) -> List.of("world", "everyone"));
        this.getCommand("secret").setExecutor((sender, command, label, args) -> {
            sender.sendMessage("The secret is 42");
            return true;
        });
        this.getServer().getScheduler().runTaskLater(this, () -> this.getLogger().info("Scheduled task ran"), 5L);
        final ApiTests api = new ApiTests(this);
        this.getServer().getPluginManager().registerEvents(api, this);
        this.getCommand("apitest").setExecutor(api);
        // After /reload the players are already online when plugins are enabled.
        for (org.bukkit.entity.Player player : this.getServer().getOnlinePlayers()) {
            player.sendMessage("BukkitTestPlugin enabled with " + this.getServer().getOnlinePlayers().size() + " player(s) online");
        }
        this.getLogger().info("Enabled; protecting " + Material.GRASS_BLOCK.getKey());
    }

    @Override
    public void onDisable() {
        this.getLogger().info("onDisable called");
    }

    @EventHandler
    public void onJoin(final PlayerJoinEvent event) {
        event.getPlayer().sendMessage(Component.text("Welcome, " + event.getPlayer().getName() + ", from a Bukkit plugin"));
        event.joinMessage(Component.text(event.getPlayer().getName() + " joined (bukkit-test)"));
    }

    @EventHandler
    public void onQuit(final PlayerQuitEvent event) {
        event.quitMessage(Component.text(event.getPlayer().getName() + " left (bukkit-test)"));
    }

    @EventHandler
    public void onBreak(final BlockBreakEvent event) {
        if (event.getBlock().getType() == Material.GRASS_BLOCK) {
            event.setCancelled(true);
            event.getPlayer().sendMessage("This block is protected");
        }
    }
}
