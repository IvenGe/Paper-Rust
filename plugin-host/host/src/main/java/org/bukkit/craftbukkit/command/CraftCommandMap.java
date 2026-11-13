package org.bukkit.craftbukkit.command;

import java.util.HashMap;
import java.util.Map;
import org.bukkit.Server;
import org.bukkit.command.Command;
import org.bukkit.command.SimpleCommandMap;

/**
 * The command map plugins see.
 *
 * <p>Paper-RS replacement for Paper's class of the same name, which forwards to the Brigadier
 * dispatcher. Here it is a plain map: Brigadier commands registered by Paper plugins are
 * entered into it as wrappers instead (see {@code io.papermc.paperrs.host.command}).
 */
public class CraftCommandMap extends SimpleCommandMap {

    public CraftCommandMap(final Server server) {
        super(server, new HashMap<>());
    }

    @Override
    public Map<String, Command> getKnownCommands() {
        return this.knownCommands;
    }
}
