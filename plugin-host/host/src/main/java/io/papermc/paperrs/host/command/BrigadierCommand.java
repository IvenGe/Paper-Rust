package io.papermc.paperrs.host.command;

import com.mojang.brigadier.tree.LiteralCommandNode;
import io.papermc.paper.command.brigadier.CommandSourceStack;
import io.papermc.paper.plugin.configuration.PluginMeta;
import java.util.List;
import org.bukkit.Bukkit;
import org.bukkit.command.Command;
import org.bukkit.command.CommandSender;
import org.bukkit.command.PluginIdentifiableCommand;
import org.bukkit.plugin.Plugin;
import org.jetbrains.annotations.Nullable;

/**
 * A Brigadier command registered by a Paper plugin, entered into the Bukkit command map so
 * that lookups, permissions and the command list treat it like any other plugin command.
 * Execution and completion go straight to the dispatcher with the raw input.
 */
public final class BrigadierCommand extends Command implements PluginIdentifiableCommand {
    private final HostCommands registrar;
    private final LiteralCommandNode<CommandSourceStack> node;
    private final PluginMeta owner;

    BrigadierCommand(final HostCommands registrar, final String label, final LiteralCommandNode<CommandSourceStack> node, final PluginMeta owner, final @Nullable String description) {
        super(label, description == null ? "" : description, "/" + label, List.of());
        this.registrar = registrar;
        this.node = node;
        this.owner = owner;
    }

    public LiteralCommandNode<CommandSourceStack> node() {
        return this.node;
    }

    public PluginMeta owner() {
        return this.owner;
    }

    @Override
    public boolean execute(final CommandSender sender, final String commandLabel, final String[] args) {
        this.registrar.executeBrigadier(sender, args.length == 0 ? commandLabel : commandLabel + " " + String.join(" ", args));
        return true;
    }

    @Override
    public List<String> tabComplete(final CommandSender sender, final String alias, final String[] args) {
        return this.registrar.completeBrigadier(sender, alias + " " + String.join(" ", args));
    }

    @Override
    public boolean testPermissionSilent(final CommandSender target) {
        return this.node.canUse(this.registrar.source(target));
    }

    @Override
    public Plugin getPlugin() {
        return Bukkit.getPluginManager().getPlugin(this.owner.getName());
    }
}
