package io.papermc.paperrs.host;

import org.bukkit.craftbukkit.command.CraftConsoleCommandSender;

/** The console: its output goes to the server log. */
public final class HostConsole extends CraftConsoleCommandSender {
    public HostConsole() {
        super();
    }
}
