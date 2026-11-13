package io.papermc.paperrs.test.paper;

import io.papermc.paper.chat.ChatRenderer;
import io.papermc.paper.command.brigadier.BasicCommand;
import io.papermc.paper.command.brigadier.CommandSourceStack;
import io.papermc.paper.event.player.AsyncChatEvent;
import io.papermc.paper.plugin.lifecycle.event.types.LifecycleEvents;
import java.util.Collection;
import java.util.List;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.plugin.java.JavaPlugin;

/** A Paper plugin: a BasicCommand registered from onEnable and a chat renderer. */
public final class PaperTestPlugin extends JavaPlugin implements Listener {
    @Override
    public void onEnable() {
        this.getServer().getPluginManager().registerEvents(this, this);
        this.getLifecycleManager().registerEventHandler(LifecycleEvents.COMMANDS, event -> {
            event.registrar().register("where", "Tells you where you are", new BasicCommand() {
                @Override
                public void execute(final CommandSourceStack source, final String[] args) {
                    final var loc = source.getLocation();
                    source.getSender().sendMessage(Component.text("You are in " + loc.getWorld().getName() + " at y=" + loc.getBlockY()));
                }

                @Override
                public Collection<String> suggest(final CommandSourceStack source, final String[] args) {
                    return List.of("here", "there");
                }
            });
        });
    }

    @EventHandler
    public void onChat(final AsyncChatEvent event) {
        event.renderer((source, displayName, message, viewer) ->
            Component.text("[paper] ", NamedTextColor.GOLD).append(displayName).append(Component.text(": ")).append(message));
    }

    @SuppressWarnings("unused")
    private static ChatRenderer unused() {
        return ChatRenderer.defaultRenderer();
    }
}
