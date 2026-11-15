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
    /** The players PlayerLoginEvent gave, to compare with the ones that join. */
    private final java.util.Map<java.util.UUID, org.bukkit.entity.Player> loggedIn = new java.util.concurrent.ConcurrentHashMap<>();

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
    public void onLogin(final org.bukkit.event.player.PlayerLoginEvent event) {
        final String name = event.getPlayer().getName();
        if (name.equals("Loki")) {
            event.disallow(org.bukkit.event.player.PlayerLoginEvent.Result.KICK_OTHER, Component.text("No Loki today"));
        } else if (name.equals("Wally") && event.getResult() == org.bukkit.event.player.PlayerLoginEvent.Result.KICK_BANNED) {
            // Let in past the server's own ban, as plugins with ban lists of their own decide.
            event.allow();
        }
        this.loggedIn.put(event.getPlayer().getUniqueId(), event.getPlayer());
    }

    @EventHandler
    public void onJoin(final PlayerJoinEvent event) {
        // The player that joins is the one PlayerLoginEvent gave, as in Paper.
        final boolean same = this.loggedIn.remove(event.getPlayer().getUniqueId()) == event.getPlayer();
        event.getPlayer().sendMessage(Component.text("Welcome, " + event.getPlayer().getName() + ", from a Bukkit plugin" + (same ? "" : " (not the player from PlayerLoginEvent)")));
        event.joinMessage(Component.text(event.getPlayer().getName() + " joined (bukkit-test)"));
    }

    @EventHandler
    public void onTabComplete(final org.bukkit.event.server.TabCompleteEvent event) {
        if (event.getBuffer().startsWith("/greet ")) {
            final List<String> completions = new java.util.ArrayList<>(event.getCompletions());
            completions.add("galaxy");
            event.setCompletions(completions);
        }
    }

    @EventHandler
    public void onAsyncTabComplete(final com.destroystokyo.paper.event.server.AsyncTabCompleteEvent event) {
        // Completions a plugin works out off the main thread.
        if (event.getBuffer().equals("/greet @")) {
            final String thread = org.bukkit.Bukkit.isPrimaryThread() ? "main thread" : "async";
            event.completions(List.of(com.destroystokyo.paper.event.server.AsyncTabCompleteEvent.Completion.completion("alpha", Component.text(thread)),
                com.destroystokyo.paper.event.server.AsyncTabCompleteEvent.Completion.completion("beta")));
            event.setHandled(true);
        }
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
