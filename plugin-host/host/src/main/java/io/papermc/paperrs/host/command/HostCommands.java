package io.papermc.paperrs.host.command;

import com.google.common.base.Preconditions;
import com.mojang.brigadier.CommandDispatcher;
import com.mojang.brigadier.ParseResults;
import com.mojang.brigadier.arguments.StringArgumentType;
import com.mojang.brigadier.builder.LiteralArgumentBuilder;
import com.mojang.brigadier.builder.RequiredArgumentBuilder;
import com.mojang.brigadier.exceptions.CommandSyntaxException;
import com.mojang.brigadier.suggestion.Suggestion;
import com.mojang.brigadier.suggestion.Suggestions;
import com.mojang.brigadier.tree.CommandNode;
import com.mojang.brigadier.tree.LiteralCommandNode;
import io.papermc.paper.command.brigadier.BasicCommand;
import io.papermc.paper.command.brigadier.CommandRegistrationFlag;
import io.papermc.paper.command.brigadier.CommandSourceStack;
import io.papermc.paper.command.brigadier.Commands;
import io.papermc.paper.plugin.configuration.PluginMeta;
import io.papermc.paper.plugin.lifecycle.event.LifecycleEventOwner;
import io.papermc.paper.plugin.lifecycle.event.registrar.PaperRegistrar;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.bridge.Values;
import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.TimeUnit;
import java.util.logging.Level;
import java.util.logging.Logger;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.Bukkit;
import org.bukkit.Location;
import org.bukkit.command.Command;
import org.bukkit.command.CommandException;
import org.bukkit.command.CommandSender;
import org.bukkit.command.PluginIdentifiableCommand;
import org.bukkit.craftbukkit.command.CraftCommandMap;
import org.bukkit.entity.Entity;
import org.bukkit.entity.Player;
import org.bukkit.event.player.PlayerCommandSendEvent;
import org.jetbrains.annotations.Nullable;

/**
 * Plugin commands: the Brigadier registrar Paper plugins use ({@code LifecycleEvents.COMMANDS})
 * and the operations the Rust server asks for (run, complete, list visible commands).
 *
 * <p>Bukkit commands live in the command map. Brigadier commands live in this dispatcher and
 * are mirrored into the command map as {@link BrigadierCommand}s, so every plugin command is
 * found the same way.
 */
public final class HostCommands implements Commands, PaperRegistrar<LifecycleEventOwner> {
    private static final Logger LOGGER = Logger.getLogger("Commands");

    private final CommandDispatcher<CommandSourceStack> dispatcher = new CommandDispatcher<>();
    private final Map<String, BrigadierCommand> brigadier = new LinkedHashMap<>();
    private @Nullable LifecycleEventOwner currentContext;
    private @Nullable CraftCommandMap commandMap;

    // --- registrar --------------------------------------------------------------------------

    @Override
    public void setCurrentContext(final @Nullable LifecycleEventOwner owner) {
        this.currentContext = owner;
    }

    @Override
    public CommandDispatcher<CommandSourceStack> getDispatcher() {
        return this.dispatcher;
    }

    private PluginMeta contextMeta() {
        Preconditions.checkState(this.currentContext != null, "No lifecycle owner context is set in the current lifecycle event");
        return this.currentContext.getPluginMeta();
    }

    @Override
    public Set<String> register(final LiteralCommandNode<CommandSourceStack> node, final @Nullable String description, final Collection<String> aliases) {
        return this.register(this.contextMeta(), node, description, aliases);
    }

    @Override
    public Set<String> register(final PluginMeta pluginMeta, final LiteralCommandNode<CommandSourceStack> node, final @Nullable String description, final Collection<String> aliases) {
        return this.registerWithFlags(pluginMeta, node, description, aliases, Set.of());
    }

    @Override
    public Set<String> registerWithFlags(final PluginMeta pluginMeta, final LiteralCommandNode<CommandSourceStack> node, final @Nullable String description,
                                         final Collection<String> aliases, final Set<CommandRegistrationFlag> flags) {
        final String namespace = pluginMeta.getName().toLowerCase(Locale.ROOT);
        final Set<String> registered = new LinkedHashSet<>();
        final List<String> labels = new ArrayList<>();
        labels.add(node.getLiteral());
        labels.addAll(aliases);
        for (String label : labels) {
            final String plain = label.toLowerCase(Locale.ROOT);
            // The namespaced form always belongs to this plugin.
            this.install(namespace + ":" + plain, node, pluginMeta, description, true);
            registered.add(namespace + ":" + plain);
            // The plain label only if no other plugin has it.
            if (this.install(plain, node, pluginMeta, description, false)) {
                registered.add(plain);
            }
        }
        return Collections.unmodifiableSet(registered);
    }

    @Override
    public Set<String> register(final String label, final @Nullable String description, final Collection<String> aliases, final BasicCommand basicCommand) {
        return this.register(this.contextMeta(), label, description, aliases, basicCommand);
    }

    @Override
    public Set<String> register(final PluginMeta pluginMeta, final String label, final @Nullable String description, final Collection<String> aliases, final BasicCommand basicCommand) {
        final RequiredArgumentBuilder<CommandSourceStack, String> args = RequiredArgumentBuilder.<CommandSourceStack, String>argument("args", StringArgumentType.greedyString())
            // Argument splitting and (absence of) filtering as in Paper's BasicCommand adapter.
            .suggests((context, builder) -> {
                String[] split = org.apache.commons.lang3.StringUtils.split(builder.getRemaining());
                if (builder.getRemaining().endsWith(" ")) {
                    split = org.apache.commons.lang3.ArrayUtils.add(split, "");
                }
                final var offset = builder.createOffset(builder.getInput().lastIndexOf(' ') + 1);
                for (String suggestion : basicCommand.suggest(context.getSource(), split)) {
                    offset.suggest(suggestion);
                }
                return offset.buildFuture();
            })
            .executes(context -> {
                basicCommand.execute(context.getSource(), org.apache.commons.lang3.StringUtils.split(StringArgumentType.getString(context, "args"), ' '));
                return com.mojang.brigadier.Command.SINGLE_SUCCESS;
            });
        final LiteralCommandNode<CommandSourceStack> node = LiteralArgumentBuilder.<CommandSourceStack>literal(label)
            .requires(source -> basicCommand.canUse(source.getSender()))
            .then(args)
            .executes(context -> {
                basicCommand.execute(context.getSource(), new String[0]);
                return com.mojang.brigadier.Command.SINGLE_SUCCESS;
            })
            .build();
        return this.register(pluginMeta, node, description, aliases);
    }

    /** Puts a copy of {@code node} named {@code label} into the dispatcher and the command map. */
    private boolean install(final String label, final LiteralCommandNode<CommandSourceStack> node, final PluginMeta owner, final @Nullable String description, final boolean replace) {
        final BrigadierCommand existing = this.brigadier.get(label);
        if (existing != null && !replace && !existing.owner().getName().equals(owner.getName())) {
            return false;
        }
        if (this.commandMap != null && !replace && existing == null) {
            final Command other = this.commandMap.getKnownCommands().get(label);
            if (other instanceof PluginIdentifiableCommand) {
                return false;
            }
        }
        removeChild(this.dispatcher.getRoot(), label);
        final LiteralCommandNode<CommandSourceStack> copy = copy(label, node);
        this.dispatcher.getRoot().addChild(copy);
        final BrigadierCommand command = new BrigadierCommand(this, label, copy, owner, description);
        this.brigadier.put(label, command);
        if (this.commandMap != null) {
            this.commandMap.getKnownCommands().put(label, command);
        }
        return true;
    }

    private static LiteralCommandNode<CommandSourceStack> copy(final String label, final LiteralCommandNode<CommandSourceStack> node) {
        if (label.equals(node.getLiteral())) {
            return node;
        }
        final LiteralArgumentBuilder<CommandSourceStack> builder = LiteralArgumentBuilder.<CommandSourceStack>literal(label)
            .requires(node.getRequirement())
            .executes(node.getCommand());
        if (node.getRedirect() != null) {
            builder.forward(node.getRedirect(), node.getRedirectModifier(), node.isFork());
        } else {
            for (CommandNode<CommandSourceStack> child : node.getChildren()) {
                builder.then(child);
            }
        }
        return builder.build();
    }

    /** Stock Brigadier cannot remove nodes; Paper patches it to. Do it reflectively. */
    private static void removeChild(final CommandNode<?> parent, final String name) {
        for (String field : new String[] {"children", "literals", "arguments"}) {
            try {
                final Field f = CommandNode.class.getDeclaredField(field);
                f.setAccessible(true);
                ((Map<?, ?>) f.get(parent)).remove(name);
            } catch (ReflectiveOperationException e) {
                throw new IllegalStateException("cannot edit the command tree", e);
            }
        }
    }

    /** Called once the server exists; commands registered during bootstrap enter the map now. */
    public void attach(final CraftCommandMap commandMap) {
        this.commandMap = commandMap;
        for (Map.Entry<String, BrigadierCommand> e : this.brigadier.entrySet()) {
            commandMap.getKnownCommands().put(e.getKey(), e.getValue());
        }
    }

    /** Removes the Brigadier commands of a plugin (when it is disabled). */
    public void unregisterAll(final String pluginName) {
        this.brigadier.entrySet().removeIf(e -> {
            if (!e.getValue().owner().getName().equals(pluginName)) {
                return false;
            }
            removeChild(this.dispatcher.getRoot(), e.getKey());
            if (this.commandMap != null) {
                this.commandMap.getKnownCommands().remove(e.getKey(), e.getValue());
            }
            return true;
        });
    }

    // --- running commands -------------------------------------------------------------------

    public CommandSourceStack source(final CommandSender sender) {
        final Location location;
        final Entity executor;
        if (sender instanceof Entity entity) {
            location = entity.getLocation();
            executor = entity;
        } else {
            location = Bukkit.getWorlds().getFirst().getSpawnLocation();
            executor = null;
        }
        return new HostCommandSourceStack(sender, location, executor);
    }

    private static String label(final String line) {
        final int space = line.indexOf(' ');
        return (space < 0 ? line : line.substring(0, space)).toLowerCase(Locale.ROOT);
    }

    private static String strip(final String line) {
        return line.startsWith("/") ? line.substring(1) : line;
    }

    /**
     * Runs a plugin command. Returns {@code false} if no plugin command has that label (the
     * server then tries its own commands).
     */
    public boolean execute(final CommandSender sender, final String rawLine) {
        final String line = strip(rawLine.trim());
        if (this.commandMap == null || line.isEmpty()) {
            return false;
        }
        final Command command = this.commandMap.getCommand(label(line));
        if (!(command instanceof PluginIdentifiableCommand)) {
            return false;
        }
        if (command instanceof BrigadierCommand) {
            this.executeBrigadier(sender, line);
            return true;
        }
        try {
            return this.commandMap.dispatch(sender, line);
        } catch (CommandException e) {
            LOGGER.log(Level.SEVERE, "Unhandled exception executing command '" + line + "'", e.getCause() != null ? e.getCause() : e);
            sender.sendMessage(Component.text("An internal error occurred while attempting to perform this command", NamedTextColor.RED));
            return true;
        }
    }

    void executeBrigadier(final CommandSender sender, final String line) {
        final ParseResults<CommandSourceStack> parse = this.dispatcher.parse(line, this.source(sender));
        try {
            this.dispatcher.execute(parse);
        } catch (CommandSyntaxException e) {
            sender.sendMessage(Component.text(e.getRawMessage().getString(), NamedTextColor.RED));
            if (e.getInput() != null && e.getCursor() >= 0) {
                final int cursor = Math.min(e.getInput().length(), e.getCursor());
                final String before = e.getInput().substring(Math.max(0, cursor - 10), cursor);
                sender.sendMessage(Component.text((cursor > 10 ? "..." : "") + before, NamedTextColor.GRAY)
                    .append(Component.text(e.getInput().substring(cursor), NamedTextColor.RED).decorate(net.kyori.adventure.text.format.TextDecoration.UNDERLINED))
                    .append(Component.translatable("command.context.here", NamedTextColor.RED).decorate(net.kyori.adventure.text.format.TextDecoration.ITALIC)));
            }
        } catch (Throwable t) {
            LOGGER.log(Level.SEVERE, "Command exception: /" + line, t);
            sender.sendMessage(Component.text("An unexpected error occurred trying to execute that command", NamedTextColor.RED));
        }
    }

    List<String> completeBrigadier(final CommandSender sender, final String line) {
        final List<String> out = new ArrayList<>();
        for (Suggestion suggestion : this.suggestBrigadier(sender, line).getList()) {
            out.add(suggestion.getText());
        }
        return out;
    }

    private Suggestions suggestBrigadier(final CommandSender sender, final String line) {
        try {
            return this.dispatcher.getCompletionSuggestions(this.dispatcher.parse(line, this.source(sender))).get(5, TimeUnit.SECONDS);
        } catch (Exception e) {
            LOGGER.log(Level.WARNING, "Tab completion of /" + line + " failed", e);
            return Suggestions.empty().join();
        }
    }

    /** Completions for the input {@code text} (with its leading slash), in the server's format. */
    public Map<String, Object> suggest(final CommandSender sender, final String text) {
        final String line = strip(text);
        final int offset = text.length() - line.length();
        final List<Object> matches = new ArrayList<>();
        if (this.commandMap == null) {
            return Values.of("start", text.length(), "length", 0, "matches", matches);
        }
        final Command command = this.commandMap.getCommand(label(line));
        if (command instanceof BrigadierCommand) {
            final Suggestions suggestions = this.suggestBrigadier(sender, line);
            for (Suggestion s : suggestions.getList()) {
                matches.add(Values.of("text", s.getText(), "tooltip", s.getTooltip() == null ? null : Components.toJson(Component.text(s.getTooltip().getString()))));
            }
            return Values.of("start", suggestions.getRange().getStart() + offset, "length", suggestions.getRange().getLength(), "matches", matches);
        }
        List<String> completions;
        try {
            completions = this.commandMap.tabComplete(sender, line);
        } catch (CommandException e) {
            LOGGER.log(Level.SEVERE, "Exception when " + sender.getName() + " attempted to tab complete " + line, e);
            completions = null;
        }
        final int start = offset + line.lastIndexOf(' ') + 1;
        if (completions != null) {
            for (String completion : completions) {
                matches.add(Values.of("text", completion));
            }
        }
        return Values.of("start", start, "length", text.length() - start, "matches", matches);
    }

    /** Labels of plugin commands (all spellings), for the server to route to the host. */
    public Set<String> pluginLabels() {
        final Set<String> out = new LinkedHashSet<>();
        if (this.commandMap != null) {
            for (Map.Entry<String, Command> e : this.commandMap.getKnownCommands().entrySet()) {
                if (e.getValue() instanceof PluginIdentifiableCommand) {
                    out.add(e.getKey());
                }
            }
        }
        return out;
    }

    /** The plugin command labels a player may see, after {@code PlayerCommandSendEvent}. */
    public List<String> visibleLabels(final Player player) {
        final List<String> labels = new ArrayList<>();
        if (this.commandMap == null) {
            return labels;
        }
        for (Map.Entry<String, Command> e : this.commandMap.getKnownCommands().entrySet()) {
            if (e.getValue() instanceof PluginIdentifiableCommand && e.getValue().testPermissionSilent(player)) {
                labels.add(e.getKey());
            }
        }
        if (PlayerCommandSendEvent.getHandlerList().getRegisteredListeners().length > 0) {
            final PlayerCommandSendEvent event = new PlayerCommandSendEvent(player, new LinkedHashSet<>(labels));
            event.callEvent();
            return new ArrayList<>(event.getCommands());
        }
        return labels;
    }
}
