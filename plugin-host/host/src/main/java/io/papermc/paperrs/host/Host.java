package io.papermc.paperrs.host;

import io.papermc.paper.plugin.PluginInitializerManager;
import io.papermc.paper.plugin.bootstrap.BootstrapContext;
import io.papermc.paper.plugin.configuration.PluginMeta;
import io.papermc.paper.plugin.entrypoint.Entrypoint;
import io.papermc.paper.plugin.entrypoint.LaunchEntryPointHandler;
import io.papermc.paper.plugin.lifecycle.event.LifecycleEventRunner;
import io.papermc.paper.plugin.lifecycle.event.registrar.ReloadableRegistrarEvent;
import io.papermc.paper.plugin.lifecycle.event.types.LifecycleEvents;
import io.papermc.paper.plugin.provider.configuration.PaperPluginMeta;
import io.papermc.paperrs.host.bridge.Bridge;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.command.HostCommands;
import io.papermc.paperrs.host.entity.HostPlayer;
import io.papermc.paperrs.host.event.HostEvents;
import io.papermc.paperrs.host.registry.HostRegistryAccess;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.logging.Level;
import java.util.logging.Logger;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.craftbukkit.command.CraftCommandMap;
import org.bukkit.event.server.ServerLoadEvent;
import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.PluginLoadOrder;
import org.jetbrains.annotations.Nullable;

/**
 * Drives the plugin lifecycle and answers the Rust server's calls.
 *
 * <p>The order of {@link #load()} and {@link #postWorld()} follows Paper's start-up: scan the
 * plugins, run bootstrappers and the bootstrap {@code COMMANDS} event, create the server,
 * construct every plugin and call {@code onLoad}, enable {@code STARTUP} plugins; then, once
 * the worlds exist, enable {@code POSTWORLD} plugins, fire {@code COMMANDS} for plugins and
 * {@code ServerLoadEvent}.
 */
public final class Host implements Bridge.Handler {
    private static final Logger LOGGER = Logger.getLogger("PluginHost");

    private final Bridge bridge;
    private final Map<String, Object> welcome;
    private final Path root;
    private final Path pluginsDirectory;
    private final List<Path> extraJars;
    private final HostCommands commands = new HostCommands();
    private @Nullable CraftServer server;
    private @Nullable HostEvents events;
    private boolean shutDown;
    /** What the server last heard about, to send only changes. */
    private Object lastState = Map.of();

    public Host(final Bridge bridge, final Map<String, Object> welcome, final Path root, final Path pluginsDirectory, final List<Path> extraJars) {
        this.bridge = bridge;
        this.welcome = welcome;
        this.root = root;
        this.pluginsDirectory = pluginsDirectory;
        this.extraJars = extraJars;
    }

    public Bridge bridge() {
        return this.bridge;
    }

    public HostCommands commands() {
        return this.commands;
    }

    @Override
    public Object call(final String method, final Map<String, Object> args) throws Exception {
        final Object result = switch (method) {
            case "host.load" -> this.load();
            case "host.post_world" -> this.postWorld();
            case "host.tick" -> {
                this.tick(args);
                yield null;
            }
            case "host.shutdown" -> {
                this.shutdown();
                yield null;
            }
            case "command.execute" -> Values.of("handled", this.commands.execute(this.server().senderFor(Values.str(args, "sender", "console")), Values.str(args, "command")));
            case "command.suggest" -> this.commands.suggest(this.server().senderFor(Values.str(args, "sender", "console")), Values.str(args, "text"));
            case "command.visible" -> {
                final HostPlayer player = this.server().player(Values.uuid(args, "uuid"));
                yield Values.of("labels", player == null ? List.of() : new ArrayList<Object>(this.commands.visibleLabels(player)));
            }
            default -> {
                if (method.startsWith("event.")) {
                    yield Objects.requireNonNull(this.events, "plugins are not loaded").handle(method, args);
                }
                throw new IllegalArgumentException("unknown host method " + method);
            }
        };
        if (this.bridge.isMainThread() && this.server != null && !method.startsWith("host.load") && !method.startsWith("host.post_world")) {
            this.publishChanges();
        }
        return result;
    }

    @Override
    public void notify(final String method, final Map<String, Object> args) {
        if (method.equals("players.state") && this.server != null) {
            this.server.updatePlayers(Values.list(args.get("players")));
        }
    }

    private CraftServer server() {
        return Objects.requireNonNull(this.server, "plugins are not loaded");
    }

    private Map<String, Object> load() {
        PluginInitializerManager.load(this.pluginsDirectory, "update", this.extraJars);
        // Bootstrappers run before the server object exists, as on Paper.
        LaunchEntryPointHandler.enterBootstrappers();
        LifecycleEventRunner.INSTANCE.callReloadableRegistrarEvent(LifecycleEvents.COMMANDS, this.commands, BootstrapContext.class, ReloadableRegistrarEvent.Cause.INITIAL);

        this.server = new CraftServer(this, this.welcome, this.root, this.pluginsDirectory);
        this.commands.attach((CraftCommandMap) this.server.getCommandMap());
        this.events = new HostEvents(this.server);
        // Initialise the classes holding registry constants here, on the main thread, before
        // any plugin code can touch them from another thread.
        HostRegistryAccess.instance().lockReferenceHolders();

        LaunchEntryPointHandler.INSTANCE.enter(Entrypoint.PLUGIN);
        this.server.enablePlugins(PluginLoadOrder.STARTUP);
        return this.snapshot();
    }

    private Map<String, Object> postWorld() {
        final CraftServer server = this.server();
        server.enablePlugins(PluginLoadOrder.POSTWORLD);
        LifecycleEventRunner.INSTANCE.callReloadableRegistrarEvent(LifecycleEvents.COMMANDS, this.commands, Plugin.class, ReloadableRegistrarEvent.Cause.INITIAL);
        new ServerLoadEvent(ServerLoadEvent.LoadType.STARTUP).callEvent();
        return this.snapshot();
    }

    private void tick(final Map<String, Object> args) {
        final CraftServer server = this.server();
        server.setCurrentTick((int) Values.i64(args, "tick", 0));
        server.craftScheduler().mainThreadHeartbeat();
    }

    private void shutdown() {
        if (this.shutDown) {
            return;
        }
        this.shutDown = true;
        if (this.server != null) {
            this.server.setStopping();
            try {
                this.server.disablePlugins();
            } catch (Throwable t) {
                LOGGER.log(Level.SEVERE, "Error disabling plugins", t);
            }
        }
        // Close once the answer to host.shutdown has been sent.
        this.bridge.runOnMain(this.bridge::close);
    }

    /** Called when the connection is gone without a clean shutdown. */
    public void onDisconnected() {
        if (!this.shutDown && this.server != null) {
            LOGGER.warning("Lost the connection to the server; disabling plugins");
            this.shutDown = true;
            this.server.setStopping();
            try {
                this.server.disablePlugins();
            } catch (Throwable t) {
                LOGGER.log(Level.SEVERE, "Error disabling plugins", t);
            }
        }
    }

    /** Plugins, plugin command labels and listened events, as the server tracks them. */
    private Map<String, Object> snapshot() {
        final Map<String, Object> state = this.currentState();
        this.lastState = state;
        return state;
    }

    private Map<String, Object> currentState() {
        final List<Object> plugins = new ArrayList<>();
        for (Plugin plugin : this.server().getPluginManager().getPlugins()) {
            final PluginMeta meta = plugin.getPluginMeta();
            plugins.add(Values.of("name", meta.getName(), "version", meta.getVersion(), "enabled", plugin.isEnabled(),
                "kind", meta instanceof PaperPluginMeta ? "paper" : "bukkit"));
        }
        final Set<String> labels = this.commands.pluginLabels();
        final Set<String> events = Objects.requireNonNull(this.events).listened();
        return Values.of("plugins", plugins, "commands", new ArrayList<Object>(labels), "events", new ArrayList<Object>(events));
    }

    /** Tells the server about plugins, commands or listeners that changed since last time. */
    private void publishChanges() {
        final Map<String, Object> state = this.currentState();
        if (!state.equals(this.lastState)) {
            this.lastState = state;
            this.bridge.notify("host.update", state);
        }
    }
}
