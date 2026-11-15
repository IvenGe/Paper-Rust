package org.bukkit.craftbukkit;

import com.destroystokyo.paper.profile.PlayerProfile;
import io.papermc.paper.plugin.manager.PaperPluginManagerImpl;
import io.papermc.paper.threadedregions.scheduler.AsyncScheduler;
import io.papermc.paper.threadedregions.scheduler.FoliaAsyncScheduler;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.Host;
import io.papermc.paperrs.host.HostConsole;
import io.papermc.paperrs.host.HostPlayerProfile;
import io.papermc.paperrs.host.HostUnsafeValues;
import io.papermc.paperrs.host.HostVersions;
import io.papermc.paperrs.host.bridge.Bridge;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.data.HostData;
import io.papermc.paperrs.host.entity.HostPlayer;
import io.papermc.paperrs.host.registry.HostLegacyTag;
import io.papermc.paperrs.host.stub.AbstractHelpMap;
import io.papermc.paperrs.host.stub.AbstractServer;
import io.papermc.paperrs.host.world.HostBlockData;
import io.papermc.paperrs.host.world.HostWorld;
import java.io.File;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.function.Consumer;
import java.util.function.Function;
import java.util.logging.Level;
import java.util.logging.Logger;
import net.kyori.adventure.audience.Audience;
import net.kyori.adventure.key.Key;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.Bukkit;
import org.bukkit.Difficulty;
import org.bukkit.Fluid;
import org.bukkit.GameEvent;
import org.bukkit.GameMode;
import org.bukkit.Keyed;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.OfflinePlayer;
import org.bukkit.Registry;
import org.bukkit.Tag;
import org.bukkit.UnsafeValues;
import org.bukkit.Warning;
import org.bukkit.World;
import org.bukkit.block.data.BlockData;
import org.bukkit.command.Command;
import org.bukkit.command.CommandMap;
import org.bukkit.command.CommandSender;
import org.bukkit.command.ConsoleCommandSender;
import org.bukkit.command.PluginCommand;
import org.bukkit.craftbukkit.command.CraftCommandMap;
import org.bukkit.craftbukkit.metadata.PlayerMetadataStore;
import org.bukkit.craftbukkit.scheduler.CraftScheduler;
import org.bukkit.craftbukkit.util.Versioning;
import org.bukkit.entity.Entity;
import org.bukkit.entity.EntityType;
import org.bukkit.entity.Player;
import org.bukkit.help.HelpMap;
import org.bukkit.help.HelpTopic;
import org.bukkit.help.HelpTopicFactory;
import org.bukkit.permissions.Permission;
import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.PluginLoadOrder;
import org.bukkit.plugin.PluginManager;
import org.bukkit.plugin.ServicesManager;
import org.bukkit.plugin.SimplePluginManager;
import org.bukkit.plugin.SimpleServicesManager;
import org.bukkit.plugin.messaging.Messenger;
import org.bukkit.plugin.messaging.StandardMessenger;
import org.bukkit.scheduler.BukkitScheduler;
import org.jetbrains.annotations.Nullable;

/**
 * The {@code Server} plugins see.
 *
 * <p>Named and shaped like Paper's class because the lifted plugin manager casts to it and
 * plugins reflect on it ({@code paperPluginManager}, {@code getCommandMap()}). State the Rust
 * server owns is fetched over the bridge; players are cached and refreshed by the server.
 */
public final class CraftServer extends AbstractServer {
    private static final Component PERMISSION_MESSAGE = Component.text("I'm sorry, but you do not have permission to perform this command. "
        + "Please contact the server administrators if you believe that this is in error.", NamedTextColor.RED);

    public final PaperPluginManagerImpl paperPluginManager;
    private final SimplePluginManager pluginManager;
    private final CraftCommandMap commandMap;
    private final CraftScheduler scheduler = new CraftScheduler();
    private final AsyncScheduler asyncScheduler = new FoliaAsyncScheduler();
    private final io.papermc.paperrs.host.HostGlobalRegionScheduler globalRegionScheduler = new io.papermc.paperrs.host.HostGlobalRegionScheduler();
    private final io.papermc.paper.threadedregions.scheduler.RegionScheduler regionScheduler = new io.papermc.paper.threadedregions.scheduler.FallbackRegionScheduler();
    private final SimpleServicesManager servicesManager = new SimpleServicesManager();
    private final StandardMessenger messenger = new StandardMessenger();
    private final PlayerMetadataStore playerMetadata = new PlayerMetadataStore();
    private final org.bukkit.craftbukkit.metadata.WorldMetadataStore worldMetadata = new org.bukkit.craftbukkit.metadata.WorldMetadataStore();
    private final UnsafeValues unsafe = new HostUnsafeValues();
    private final HelpMap helpMap = new EmptyHelpMap();
    private final Logger logger = Logger.getLogger("Minecraft");
    private final Host host;
    private final Map<String, Object> welcome;
    private final Path root;
    private final Path pluginsDirectory;
    private final ConsoleCommandSender console;
    private final List<World> worlds = new ArrayList<>();
    private final Map<UUID, HostPlayer> players = Collections.synchronizedMap(new LinkedHashMap<>());
    private final org.bukkit.craftbukkit.metadata.EntityMetadataStore entityMetadata = new org.bukkit.craftbukkit.metadata.EntityMetadataStore();
    private volatile long epoch;
    private volatile Component motd;
    private volatile boolean stopping;
    private volatile int currentTick;

    public CraftServer(final Host host, final Map<String, Object> welcome, final Path root, final Path pluginsDirectory) {
        this.host = host;
        this.welcome = welcome;
        this.root = root;
        this.pluginsDirectory = pluginsDirectory;
        this.motd = Components.fromJson(welcome.get("motd"));
        Bukkit.setServer(this);
        this.commandMap = new CraftCommandMap(this);
        this.pluginManager = new SimplePluginManager(this, this.commandMap);
        this.paperPluginManager = new PaperPluginManagerImpl(this, this.commandMap, this.pluginManager);
        this.pluginManager.paperPluginManager = this.paperPluginManager;
        this.console = new HostConsole();
        for (Object world : Values.list(welcome.get("worlds"))) {
            this.pendingWorlds.add(new HostWorld(this, Values.map(world)));
        }
        // After /reload the worlds are there from the start, as on Paper; on a real start they
        // load after the STARTUP plugins are enabled (loadWorlds).
        if (Values.bool(welcome, "reload", false)) {
            this.worlds.addAll(this.pendingWorlds);
            this.pendingWorlds.clear();
        }
    }

    private final List<World> pendingWorlds = new java.util.ArrayList<>();

    /**
     * The server's worlds, loaded between the STARTUP and POSTWORLD plugins as on Paper: each
     * with {@code WorldInitEvent} and {@code WorldLoadEvent}.
     */
    public void loadWorlds() {
        for (final World world : this.pendingWorlds) {
            this.worlds.add(world);
            new org.bukkit.event.world.WorldInitEvent(world).callEvent();
            new org.bukkit.event.world.WorldLoadEvent(world).callEvent();
        }
        this.pendingWorlds.clear();
    }

    // --- bridge helpers ---------------------------------------------------------------------

    private Bridge bridge() {
        return this.host.bridge();
    }

    /**
     * Counts calls between the two processes. Cached state of entities is good until it
     * changes (the server cannot change anything while plugin code runs without a call).
     */
    public long epoch() {
        return this.epoch;
    }

    /** Something crossed the bridge (see {@link #epoch()}). */
    public void advanceEpoch() {
        this.epoch++;
    }

    /** Calls the Rust server and waits for the answer. */
    private final Map<org.bukkit.NamespacedKey, io.papermc.paperrs.host.bossbar.HostBossBar> bossBars = new java.util.concurrent.ConcurrentHashMap<>();

    @Override
    public org.bukkit.boss.BossBar createBossBar(final String title, final org.bukkit.boss.BarColor color, final org.bukkit.boss.BarStyle style, final org.bukkit.boss.BarFlag... flags) {
        return new io.papermc.paperrs.host.bossbar.HostBossBar(this, null, title, color, style, flags);
    }

    @Override
    public org.bukkit.boss.KeyedBossBar createBossBar(final org.bukkit.NamespacedKey key, final String title, final org.bukkit.boss.BarColor color, final org.bukkit.boss.BarStyle style, final org.bukkit.boss.BarFlag... flags) {
        final io.papermc.paperrs.host.bossbar.HostBossBar bar = new io.papermc.paperrs.host.bossbar.HostBossBar(this, key, title, color, style, flags);
        if (this.bossBars.putIfAbsent(key, bar) != null) {
            throw new IllegalArgumentException("A boss bar with the key " + key + " already exists");
        }
        return bar;
    }

    @Override
    public java.util.Iterator<org.bukkit.boss.KeyedBossBar> getBossBars() {
        return java.util.List.<org.bukkit.boss.KeyedBossBar>copyOf(this.bossBars.values()).iterator();
    }

    @Override
    public org.bukkit.boss.@Nullable KeyedBossBar getBossBar(final org.bukkit.NamespacedKey key) {
        return this.bossBars.get(key);
    }

    @Override
    public boolean removeBossBar(final org.bukkit.NamespacedKey key) {
        final io.papermc.paperrs.host.bossbar.HostBossBar bar = this.bossBars.remove(key);
        if (bar == null) {
            return false;
        }
        bar.removeAll();
        return true;
    }

    /**
     * Calls the Rust server for something it only reads (a section of blocks), without
     * counting it as a crossing: what is cached stays good, as the call changes nothing.
     */
    public Object read(final String method, final Map<String, Object> args) {
        return this.bridge().call(method, args);
    }

    public Object call(final String method, final Map<String, Object> args) {
        this.epoch++;
        try {
            return this.bridge().call(method, args);
        } finally {
            this.epoch++;
        }
    }

    /**
     * Sends a command to the Rust server. On the main thread it runs at once, in order with
     * everything else; elsewhere it is queued without waiting.
     */
    public void send(final String method, final Map<String, Object> args) {
        this.epoch++;
        if (this.bridge().isMainThread()) {
            this.bridge().call(method, args);
        } else {
            this.bridge().callAsync(method, args).exceptionally(e -> {
                this.logger.log(Level.WARNING, "Could not send " + method + " to the server", e);
                return null;
            });
        }
    }

    public void runOnMain(final Runnable task) {
        this.bridge().runOnMain(task);
    }

    /** Metadata of worlds ({@code CraftServer#getWorldMetadata}). */
    public org.bukkit.craftbukkit.metadata.WorldMetadataStore worldMetadata() {
        return this.worldMetadata;
    }

    public PlayerMetadataStore playerMetadata() {
        return this.playerMetadata;
    }

    public org.bukkit.craftbukkit.metadata.EntityMetadataStore entityMetadata() {
        return this.entityMetadata;
    }

    /** The server's difficulty, which all its worlds share. */
    public Difficulty difficulty() {
        return Difficulty.valueOf(String.valueOf(this.call("server.difficulty", Values.of())).toUpperCase(Locale.ROOT));
    }

    public void setDifficulty(final Difficulty difficulty) {
        this.call("server.set_difficulty", Values.of("difficulty", difficulty.name().toLowerCase(Locale.ROOT)));
    }

    public void setCurrentTick(final int tick) {
        this.currentTick = tick;
    }

    public void setStopping() {
        this.stopping = true;
    }

    // --- plugins ----------------------------------------------------------------------------

    /** Enables the plugins that load at {@code type}, as Paper's {@code CraftServer#enablePlugins}. */
    public void enablePlugins(final PluginLoadOrder type) {
        for (Plugin plugin : this.pluginManager.getPlugins()) {
            if (!plugin.isEnabled() && plugin.getDescription().getLoad() == type) {
                this.enablePlugin(plugin);
            }
        }
        if (type == PluginLoadOrder.POSTWORLD) {
            this.commandMap.registerServerAliases();
            org.bukkit.util.permissions.DefaultPermissions.registerCorePermissions();
            org.bukkit.craftbukkit.util.permissions.CraftDefaultPermissions.registerCorePermissions();
        }
    }

    private void enablePlugin(final Plugin plugin) {
        try {
            final List<Permission> perms = plugin.getDescription().getPermissions();
            for (Permission perm : perms) {
                if (this.pluginManager.getPermission(perm.getName()) == null) {
                    try {
                        this.pluginManager.addPermission(perm, false);
                    } catch (IllegalArgumentException ex) {
                        this.logger.log(Level.WARNING, "Plugin " + plugin.getDescription().getFullName() + " tried to register permission '" + perm.getName() + "' but it's already registered", ex);
                    }
                }
            }
            this.pluginManager.dirtyPermissibles();
            io.papermc.paperrs.host.compat.WorldEditCompat.beforeEnable(plugin);
            this.pluginManager.enablePlugin(plugin);
        } catch (Throwable ex) {
            this.logger.log(Level.SEVERE, ex.getMessage() + " loading " + plugin.getDescription().getFullName() + " (Is it up to date?)", ex);
        }
    }

    public void disablePlugins() {
        this.pluginManager.disablePlugins();
    }

    // --- players ----------------------------------------------------------------------------

    public HostPlayer addPlayer(final Map<String, Object> state) {
        final HostPlayer player = new HostPlayer(this, state);
        this.players.put(player.getUniqueId(), player);
        return player;
    }

    public @Nullable HostPlayer removePlayer(final UUID uuid) {
        final HostPlayer player = this.players.remove(uuid);
        if (player != null) {
            player.markOffline();
        }
        return player;
    }

    /** Applies the {@code players.state} notification. */
    public void updatePlayers(final List<Object> states) {
        for (Object s : states) {
            final Map<String, Object> state = Values.map(s);
            final HostPlayer player = this.players.get(Values.uuid(state, "uuid"));
            if (player != null) {
                player.updateState(state);
            }
        }
    }

    public @Nullable HostPlayer player(final UUID uuid) {
        return this.players.get(uuid);
    }

    /** {@code console} or a player UUID, as the server names command senders. */
    public CommandSender senderFor(final @Nullable String id) {
        if (id == null || id.equals("console")) {
            return this.console;
        }
        final HostPlayer player = this.players.get(UUID.fromString(id));
        if (player == null) {
            throw new IllegalArgumentException("player " + id + " is not online");
        }
        return player;
    }

    @Override
    public Collection<? extends Player> getOnlinePlayers() {
        synchronized (this.players) {
            return List.copyOf(this.players.values());
        }
    }

    @Override
    public @Nullable Player getPlayer(final UUID id) {
        return this.players.get(id);
    }

    @Override
    public @Nullable Player getPlayerExact(final String name) {
        for (Player player : this.getOnlinePlayers()) {
            if (player.getName().equalsIgnoreCase(name)) {
                return player;
            }
        }
        return null;
    }

    @Override
    public @Nullable Player getPlayer(final String name) {
        final Player exact = this.getPlayerExact(name);
        if (exact != null) {
            return exact;
        }
        final String lower = name.toLowerCase(Locale.ROOT);
        Player found = null;
        int delta = Integer.MAX_VALUE;
        for (Player player : this.getOnlinePlayers()) {
            if (player.getName().toLowerCase(Locale.ROOT).startsWith(lower)) {
                final int d = Math.abs(player.getName().length() - lower.length());
                if (d < delta) {
                    found = player;
                    delta = d;
                }
            }
        }
        return found;
    }

    @Override
    public @Nullable Entity getEntity(final UUID uuid) {
        final Player player = this.players.get(uuid);
        if (player != null) {
            return player;
        }
        for (World world : this.worlds) {
            final Entity entity = world.getEntity(uuid);
            if (entity != null) {
                return entity;
            }
        }
        return null;
    }

    // --- offline players --------------------------------------------------------------------

    @Override
    public OfflinePlayer getOfflinePlayer(final UUID id) {
        final Player online = this.getPlayer(id);
        if (online != null) {
            return online;
        }
        return new io.papermc.paperrs.host.entity.HostOfflinePlayer(this, id, null);
    }

    /**
     * By name: an online player, else the name cache ({@code usercache.json}), else the
     * offline-mode UUID of the name. Paper asks Mojang for unknown names in online mode;
     * Paper-RS does not block on web requests and uses the offline UUID instead.
     */
    @Override
    public OfflinePlayer getOfflinePlayer(final String name) {
        final Player online = this.getPlayerExact(name);
        if (online != null) {
            return online;
        }
        final Map<String, Object> info = Values.map(this.call("offline.get", Values.of("name", name)));
        return new io.papermc.paperrs.host.entity.HostOfflinePlayer(this, Values.uuid(info, "uuid"), Values.str(info, "name", name));
    }

    @Override
    public @Nullable OfflinePlayer getOfflinePlayerIfCached(final String name) {
        final Player online = this.getPlayerExact(name);
        if (online != null) {
            return online;
        }
        final Object info = this.call("offline.get", Values.of("name", name, "cached_only", true));
        if (!(info instanceof Map<?, ?>)) {
            return null;
        }
        final Map<String, Object> map = Values.map(info);
        return new io.papermc.paperrs.host.entity.HostOfflinePlayer(this, Values.uuid(map, "uuid"), Values.str(map, "name", name));
    }

    @Override
    public OfflinePlayer[] getOfflinePlayers() {
        final List<OfflinePlayer> out = new ArrayList<>();
        for (Object entry : Values.list(this.call("offline.list", Values.of()))) {
            final Map<String, Object> info = Values.map(entry);
            final UUID uuid = Values.uuid(info, "uuid");
            final Player online = this.getPlayer(uuid);
            out.add(online != null ? online : new io.papermc.paperrs.host.entity.HostOfflinePlayer(this, uuid, Values.str(info, "name", null)));
        }
        return out.toArray(new OfflinePlayer[0]);
    }

    private Set<OfflinePlayer> offlineSet(final String key) {
        final Set<OfflinePlayer> out = new LinkedHashSet<>();
        for (Object entry : Values.list(Values.map(this.call("server.whitelist", Values.of())).get(key))) {
            out.add(this.getOfflinePlayer(Values.uuid(Values.map(entry), "uuid")));
        }
        return out;
    }

    @Override
    public Set<OfflinePlayer> getOperators() {
        return this.offlineSet("ops");
    }

    @Override
    public Set<OfflinePlayer> getWhitelistedPlayers() {
        return this.offlineSet("players");
    }

    // --- inventories, items and scoreboards --------------------------------------------------

    @Override
    public org.bukkit.inventory.Merchant createMerchant() {
        return io.papermc.paperrs.host.entity.HostMerchant.create(this, null);
    }

    @Override
    @SuppressWarnings("deprecation")
    public org.bukkit.inventory.Merchant createMerchant(final @Nullable String title) {
        return io.papermc.paperrs.host.entity.HostMerchant.create(this, title == null ? org.bukkit.event.inventory.InventoryType.MERCHANT.defaultTitle() : Components.legacy(title));
    }

    @Override
    public org.bukkit.inventory.Merchant createMerchant(final Component title) {
        if (title == null) {
            throw new IllegalArgumentException("Title cannot be null");
        }
        return io.papermc.paperrs.host.entity.HostMerchant.create(this, title);
    }

    @Override
    public org.bukkit.inventory.Inventory createInventory(final @Nullable org.bukkit.inventory.InventoryHolder owner, final org.bukkit.event.inventory.InventoryType type) {
        return io.papermc.paperrs.host.inventory.Inventories.create(this, owner, type, type.getDefaultSize(), null);
    }

    @Override
    public org.bukkit.inventory.Inventory createInventory(final @Nullable org.bukkit.inventory.InventoryHolder owner, final org.bukkit.event.inventory.InventoryType type, final Component title) {
        return io.papermc.paperrs.host.inventory.Inventories.create(this, owner, type, type.getDefaultSize(), title);
    }

    @Override
    public org.bukkit.inventory.Inventory createInventory(final @Nullable org.bukkit.inventory.InventoryHolder owner, final org.bukkit.event.inventory.InventoryType type, final String title) {
        return io.papermc.paperrs.host.inventory.Inventories.create(this, owner, type, type.getDefaultSize(), Components.legacy(title));
    }

    @Override
    public org.bukkit.inventory.Inventory createInventory(final @Nullable org.bukkit.inventory.InventoryHolder owner, final int size) {
        return io.papermc.paperrs.host.inventory.Inventories.create(this, owner, org.bukkit.event.inventory.InventoryType.CHEST, size, null);
    }

    @Override
    public org.bukkit.inventory.Inventory createInventory(final @Nullable org.bukkit.inventory.InventoryHolder owner, final int size, final Component title) {
        return io.papermc.paperrs.host.inventory.Inventories.create(this, owner, org.bukkit.event.inventory.InventoryType.CHEST, size, title);
    }

    @Override
    public org.bukkit.inventory.Inventory createInventory(final @Nullable org.bukkit.inventory.InventoryHolder owner, final int size, final String title) {
        return io.papermc.paperrs.host.inventory.Inventories.create(this, owner, org.bukkit.event.inventory.InventoryType.CHEST, size, Components.legacy(title));
    }

    @Override
    public org.bukkit.scoreboard.ScoreboardManager getScoreboardManager() {
        return io.papermc.paperrs.host.scoreboard.HostScoreboards.manager(this);
    }

    // --- reloading ----------------------------------------------------------------------------

    /** {@code Bukkit.reload()}: as {@code /reload confirm}; the plugin host is restarted. */
    @Override
    public void reload() {
        this.dispatchCommand(this.console, "reload confirm");
    }

    @Override
    public PlayerProfile createProfile(final UUID uuid) {
        return new HostPlayerProfile(uuid, null);
    }

    @Override
    public PlayerProfile createProfile(final @Nullable String name) {
        return new HostPlayerProfile(null, name);
    }

    @Override
    public PlayerProfile createProfile(final @Nullable UUID uuid, final @Nullable String name) {
        return new HostPlayerProfile(uuid, name);
    }

    @Override
    public Iterable<? extends Audience> audiences() {
        final List<Audience> out = new ArrayList<>(this.getOnlinePlayers());
        out.add(this.console);
        return out;
    }

    @Override
    public int broadcast(final Component message, final String permission) {
        int count = 0;
        for (Player player : this.getOnlinePlayers()) {
            if (player.hasPermission(permission)) {
                player.sendMessage(message);
                count++;
            }
        }
        if (this.console.hasPermission(permission)) {
            this.console.sendMessage(message);
        }
        return count;
    }

    // --- worlds -----------------------------------------------------------------------------

    @Override
    public List<World> getWorlds() {
        return List.copyOf(this.worlds);
    }

    /**
     * The world of that name: loaded already, opened from its folder, or made by the server
     * (normal or flat; overworld, Nether or End). Plugins' own chunk generators and biome
     * providers are not supported.
     */
    @Override
    public @Nullable World createWorld(final org.bukkit.WorldCreator creator) {
        if (creator == null) {
            throw new IllegalArgumentException("WorldCreator cannot be null");
        }
        final World existing = this.getWorld(creator.name());
        if (existing != null) {
            return existing;
        }
        if (creator.generator() != null) {
            throw io.papermc.paperrs.host.Unsupported.method("WorldCreator#generator");
        }
        if (creator.biomeProvider() != null) {
            throw io.papermc.paperrs.host.Unsupported.method("WorldCreator#biomeProvider");
        }
        final Map<String, Object> info = Values.map(this.call("server.create_world", Values.of("name", creator.name(), "environment", creator.environment().name(),
            "seed", creator.seed(), "type", creator.type().name(), "structures", creator.generateStructures(), "key", creator.key().toString())));
        final World world = new HostWorld(this, info);
        this.worlds.add(world);
        new org.bukkit.event.world.WorldInitEvent(world).callEvent();
        new org.bukkit.event.world.WorldLoadEvent(world).callEvent();
        return world;
    }

    @Override
    public @Nullable World getWorld(final String name) {
        for (World world : this.worlds) {
            if (world.getName().equalsIgnoreCase(name)) {
                return world;
            }
        }
        return null;
    }

    @Override
    public @Nullable World getWorld(final UUID uid) {
        for (World world : this.worlds) {
            if (world.getUID().equals(uid)) {
                return world;
            }
        }
        return null;
    }

    @Override
    public @Nullable World getWorld(final Key key) {
        for (World world : this.worlds) {
            if (world.getKey().equals(key)) {
                return world;
            }
        }
        return null;
    }

    @Override
    public World getRespawnWorld() {
        return this.worlds.getFirst();
    }

    @Override
    public File getWorldContainer() {
        return this.root.toFile();
    }

    // --- commands ---------------------------------------------------------------------------

    @Override
    public CommandMap getCommandMap() {
        return this.commandMap;
    }

    @Override
    public @Nullable PluginCommand getPluginCommand(final String name) {
        final Command command = this.commandMap.getCommand(name);
        return command instanceof PluginCommand pluginCommand ? pluginCommand : null;
    }

    @Override
    public boolean dispatchCommand(final CommandSender sender, final String commandLine) {
        if (this.host.commands().execute(sender, commandLine)) {
            return true;
        }
        // Not a plugin command: let the Rust server run its own commands.
        final String senderId = sender instanceof Player player ? player.getUniqueId().toString() : "console";
        if (Boolean.TRUE.equals(this.call("server.dispatch_command", Values.of("sender", senderId, "command", commandLine)))) {
            return true;
        }
        sender.sendMessage(Component.translatable("command.unknown.command", NamedTextColor.RED));
        return false;
    }

    @Override
    public Map<String, String[]> getCommandAliases() {
        return Map.of();
    }

    @Override
    public Component permissionMessage() {
        return PERMISSION_MESSAGE;
    }

    @Override
    public String getPermissionMessage() {
        return Components.toLegacy(PERMISSION_MESSAGE);
    }

    @Override
    public HelpMap getHelpMap() {
        return this.helpMap;
    }

    // --- tags -------------------------------------------------------------------------------

    @Override
    @SuppressWarnings("unchecked")
    public <T extends Keyed> @Nullable Tag<T> getTag(final String registry, final NamespacedKey tag, final Class<T> clazz) {
        final Map<String, List<String>> tags = this.tagsOf(registry, clazz);
        final List<String> values = tags.get(tag.toString());
        return values == null ? null : (Tag<T>) this.legacyTag(tag, values, this.resolver(registry, clazz));
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T extends Keyed> Iterable<Tag<T>> getTags(final String registry, final Class<T> clazz) {
        final List<Tag<T>> out = new ArrayList<>();
        final Function<String, ? extends Keyed> resolver = this.resolver(registry, clazz);
        this.tagsOf(registry, clazz).forEach((name, values) -> out.add((Tag<T>) this.legacyTag(NamespacedKey.fromString(name), values, resolver)));
        return out;
    }

    private Map<String, List<String>> tagsOf(final String registry, final Class<?> clazz) {
        final String id = switch (registry) {
            case Tag.REGISTRY_BLOCKS -> "minecraft:block";
            case Tag.REGISTRY_ITEMS -> "minecraft:item";
            case Tag.REGISTRY_FLUIDS -> "minecraft:fluid";
            case Tag.REGISTRY_ENTITY_TYPES -> "minecraft:entity_type";
            case Tag.REGISTRY_GAME_EVENTS -> "minecraft:game_event";
            default -> throw new IllegalArgumentException("Not a tag registry: " + registry);
        };
        return HostData.get().tags(id);
    }

    private Function<String, ? extends Keyed> resolver(final String registry, final Class<?> clazz) {
        return switch (registry) {
            case Tag.REGISTRY_BLOCKS, Tag.REGISTRY_ITEMS -> Material::matchMaterial;
            case Tag.REGISTRY_FLUIDS -> name -> Registry.FLUID.get(NamespacedKey.fromString(name));
            case Tag.REGISTRY_ENTITY_TYPES -> name -> Registry.ENTITY_TYPE.get(NamespacedKey.fromString(name));
            case Tag.REGISTRY_GAME_EVENTS -> name -> Registry.GAME_EVENT.get(NamespacedKey.fromString(name));
            default -> throw new IllegalArgumentException("Not a tag registry: " + registry);
        };
    }

    private <T extends Keyed> HostLegacyTag<T> legacyTag(final NamespacedKey key, final List<String> values, final Function<String, ? extends Keyed> resolver) {
        final Set<T> resolved = new LinkedHashSet<>();
        for (String value : values) {
            @SuppressWarnings("unchecked") final T entry = (T) resolver.apply(value);
            if (entry != null) {
                resolved.add(entry);
            }
        }
        return new HostLegacyTag<>(key, resolved);
    }

    // --- the server itself ------------------------------------------------------------------

    @Override
    public String getName() {
        return Values.str(this.welcome, "server_name", "Paper");
    }

    @Override
    public String getVersion() {
        return HostVersions.MINECRAFT_VERSION + "-DEV-Paper-RS (MC: " + HostVersions.MINECRAFT_VERSION + ")";
    }

    @Override
    public String getBukkitVersion() {
        return Versioning.getBukkitVersion();
    }

    @Override
    public String getMinecraftVersion() {
        return HostVersions.MINECRAFT_VERSION;
    }

    @Override
    public Logger getLogger() {
        return this.logger;
    }

    @Override
    public File getPluginsFolder() {
        return this.pluginsDirectory.toFile();
    }

    @Override
    public String getUpdateFolder() {
        return "update";
    }

    @Override
    public File getUpdateFolderFile() {
        return this.pluginsDirectory.resolve("update").toFile();
    }

    @Override
    public org.bukkit.inventory.ItemFactory getItemFactory() {
        return io.papermc.paperrs.host.item.HostItemFactory.INSTANCE;
    }

    @Override
    public UnsafeValues getUnsafe() {
        return this.unsafe;
    }

    @Override
    public PluginManager getPluginManager() {
        return this.pluginManager;
    }

    @Override
    public BukkitScheduler getScheduler() {
        return this.scheduler;
    }

    public CraftScheduler craftScheduler() {
        return this.scheduler;
    }

    @Override
    public AsyncScheduler getAsyncScheduler() {
        return this.asyncScheduler;
    }

    @Override
    public io.papermc.paper.threadedregions.scheduler.GlobalRegionScheduler getGlobalRegionScheduler() {
        return this.globalRegionScheduler;
    }

    @Override
    public io.papermc.paper.threadedregions.scheduler.RegionScheduler getRegionScheduler() {
        return this.regionScheduler;
    }

    @Override
    public ServicesManager getServicesManager() {
        return this.servicesManager;
    }

    @Override
    public Messenger getMessenger() {
        return this.messenger;
    }

    @Override
    public ConsoleCommandSender getConsoleSender() {
        return this.console;
    }

    @Override
    public boolean isPrimaryThread() {
        return this.bridge().isMainThread();
    }

    @Override
    public boolean isStopping() {
        return this.stopping;
    }

    @Override
    public Warning.WarningState getWarningState() {
        return Warning.WarningState.DEFAULT;
    }

    @Override
    public int getMaxPlayers() {
        return Values.i32(this.welcome, "max_players", 20);
    }

    @Override
    public int getPort() {
        return Values.i32(this.welcome, "port", 25565);
    }

    @Override
    public String getIp() {
        return Values.str(this.welcome, "ip", "");
    }

    @Override
    public Component motd() {
        return this.motd == null ? Component.empty() : this.motd;
    }

    @Override
    public void motd(final Component motd) {
        this.motd = motd;
    }

    @Override
    public String getMotd() {
        return Components.toLegacy(this.motd());
    }

    @Override
    public boolean getOnlineMode() {
        return Values.bool(this.welcome, "online_mode", true);
    }

    @Override
    public int getViewDistance() {
        return Values.i32(this.welcome, "view_distance", 10);
    }

    @Override
    public int getSimulationDistance() {
        return Values.i32(this.welcome, "simulation_distance", 10);
    }

    @Override
    public GameMode getDefaultGameMode() {
        return GameMode.valueOf(Values.str(this.welcome, "default_game_mode", "survival").toUpperCase(Locale.ROOT));
    }

    @Override
    public boolean getAllowFlight() {
        return Values.bool(this.welcome, "allow_flight", false);
    }

    @Override
    public boolean isHardcore() {
        return Values.bool(this.welcome, "hardcore", false);
    }

    @Override
    public boolean getAllowEnd() {
        return false;
    }

    @Override
    public boolean getAllowNether() {
        return false;
    }

    @Override
    public boolean isLoggingIPs() {
        return true;
    }

    @Override
    public int getSpawnRadius() {
        return Values.i32(this.welcome, "spawn_protection", 16);
    }

    @Override
    public boolean hasWhitelist() {
        return Values.bool(Values.map(this.call("server.whitelist", Values.of())), "enabled", false);
    }

    @Override
    public boolean isWhitelistEnforced() {
        return this.hasWhitelist();
    }

    // --- more of Bukkit's Server (CraftServer's behaviour) --------------------------------

    @Override
    public List<Player> matchPlayer(final String partialName) {
        com.google.common.base.Preconditions.checkArgument(partialName != null, "partialName cannot be null");
        final List<Player> matched = new java.util.ArrayList<>();
        for (final Player player : this.getOnlinePlayers()) {
            final String name = player.getName();
            if (partialName.equalsIgnoreCase(name)) {
                // An exact match wins.
                matched.clear();
                matched.add(player);
                break;
            }
            if (name.toLowerCase(Locale.ROOT).contains(partialName.toLowerCase(Locale.ROOT))) {
                matched.add(player);
            }
        }
        return matched;
    }

    @Override
    public @Nullable UUID getPlayerUniqueId(final String playerName) {
        final Player online = this.getPlayerExact(playerName);
        if (online != null) {
            return online.getUniqueId();
        }
        final OfflinePlayer cached = this.getOfflinePlayerIfCached(playerName);
        return cached == null ? null : cached.getUniqueId();
    }

    // One main thread: it owns every region (Folia's checks, as Paper answers them).
    @Override
    public boolean isGlobalTickThread() {
        return this.isPrimaryThread();
    }

    @Override
    public boolean isOwnedByCurrentRegion(final World world, final io.papermc.paper.math.Position position) {
        return this.isPrimaryThread();
    }

    @Override
    public boolean isOwnedByCurrentRegion(final World world, final io.papermc.paper.math.Position position, final int squareRadiusChunks) {
        return this.isPrimaryThread();
    }

    @Override
    public boolean isOwnedByCurrentRegion(final org.bukkit.Location location) {
        return this.isPrimaryThread();
    }

    @Override
    public boolean isOwnedByCurrentRegion(final org.bukkit.Location location, final int squareRadiusChunks) {
        return this.isPrimaryThread();
    }

    @Override
    public boolean isOwnedByCurrentRegion(final World world, final int chunkX, final int chunkZ) {
        return this.isPrimaryThread();
    }

    @Override
    public boolean isOwnedByCurrentRegion(final World world, final int chunkX, final int chunkZ, final int squareRadiusChunks) {
        return this.isPrimaryThread();
    }

    @Override
    public boolean isOwnedByCurrentRegion(final World world, final int minChunkX, final int minChunkZ, final int maxChunkX, final int maxChunkZ) {
        return this.isPrimaryThread();
    }

    @Override
    public boolean isOwnedByCurrentRegion(final org.bukkit.entity.Entity entity) {
        return this.isPrimaryThread();
    }

    @Override
    public boolean isTickingWorlds() {
        return false;
    }

    @Override
    public boolean isPaused() {
        return false;
    }

    @Override
    public void allowPausing(final Plugin plugin, final boolean value) {
    }

    @Override
    public org.bukkit.profile.PlayerProfile createPlayerProfile(final @Nullable UUID uniqueId, final @Nullable String name) {
        return this.createProfile(uniqueId, name);
    }

    @Override
    public org.bukkit.profile.PlayerProfile createPlayerProfile(final UUID uniqueId) {
        return this.createProfile(uniqueId);
    }

    @Override
    public org.bukkit.profile.PlayerProfile createPlayerProfile(final String name) {
        return this.createProfile(name);
    }

    @Override
    public PlayerProfile createProfileExact(final @Nullable UUID uuid, final @Nullable String name) {
        return this.createProfile(uuid, name);
    }

    @Override
    public java.util.Set<String> getListeningPluginChannels() {
        return this.getMessenger().getIncomingChannels();
    }

    @Override
    public void sendPluginMessage(final Plugin source, final String channel, final byte[] message) {
        org.bukkit.plugin.messaging.StandardMessenger.validatePluginMessage(this.getMessenger(), source, channel, message);
        for (final Player player : this.getOnlinePlayers()) {
            player.sendPluginMessage(source, channel, message);
        }
    }

    private int idleTimeout = -1;

    @Override
    public int getIdleTimeout() {
        return this.idleTimeout >= 0 ? this.idleTimeout : Values.i32(this.welcome, "player_idle_timeout", 0);
    }

    @Override
    public void setIdleTimeout(final int threshold) {
        this.idleTimeout = threshold;
    }

    /** Bukkit's defaults (bukkit.yml {@code spawn-limits}), which Paper-RS's spawning uses. */
    @Override
    public int getSpawnLimit(final org.bukkit.entity.SpawnCategory spawnCategory) {
        com.google.common.base.Preconditions.checkArgument(spawnCategory != null, "SpawnCategory cannot be null");
        return switch (spawnCategory) {
            case MONSTER -> 70;
            case ANIMAL -> 10;
            case WATER_ANIMAL -> 5;
            case WATER_AMBIENT -> 20;
            case WATER_UNDERGROUND_CREATURE -> 5;
            case AMBIENT -> 15;
            case AXOLOTL -> 5;
            default -> -1;
        };
    }

    /** bukkit.yml {@code ticks-per}: animals every 400 ticks, the rest every tick. */
    @Override
    public int getTicksPerSpawns(final org.bukkit.entity.SpawnCategory spawnCategory) {
        com.google.common.base.Preconditions.checkArgument(spawnCategory != null, "SpawnCategory cannot be null");
        return spawnCategory == org.bukkit.entity.SpawnCategory.ANIMAL ? 400 : 1;
    }

    private String property(final String key, final String def) {
        final Object value = Values.map(this.welcome.get("properties")).get(key);
        return value == null ? def : value.toString();
    }

    @Override
    public String getWorldType() {
        return Values.str(this.welcome, "level_type", "minecraft:normal");
    }

    @Override
    public boolean getGenerateStructures() {
        return Values.bool(this.welcome, "generate_structures", true);
    }

    @Override
    public int getMaxWorldSize() {
        return Integer.parseInt(this.property("max-world-size", "29999984"));
    }

    @Override
    public boolean getHideOnlinePlayers() {
        return Values.bool(this.welcome, "hide_online_players", false);
    }

    @Override
    public boolean isEnforcingSecureProfiles() {
        return Boolean.parseBoolean(this.property("enforce-secure-profile", "true")) && this.getOnlineMode();
    }

    @Override
    public boolean isAcceptingTransfers() {
        return Values.bool(this.welcome, "accepts_transfers", false);
    }

    @Override
    public boolean forcesDefaultGameMode() {
        return Values.bool(this.welcome, "force_gamemode", false);
    }

    @Override
    public long getConnectionThrottle() {
        return 4000;
    }

    @Override
    public String getShutdownMessage() {
        return "Server closed";
    }

    @Override
    public net.kyori.adventure.text.Component shutdownMessage() {
        return net.kyori.adventure.text.Component.text(this.getShutdownMessage());
    }

    @Override
    public String getResourcePack() {
        return this.property("resource-pack", "");
    }

    @Override
    public String getResourcePackHash() {
        return this.property("resource-pack-sha1", "").toUpperCase(Locale.ROOT);
    }

    @Override
    public String getResourcePackPrompt() {
        return this.property("resource-pack-prompt", "");
    }

    @Override
    public boolean isResourcePackRequired() {
        return Boolean.parseBoolean(this.property("require-resource-pack", "false"));
    }

    @Override
    public int getMaxChainedNeighborUpdates() {
        return Integer.parseInt(this.property("max-chained-neighbor-updates", "1000000"));
    }

    @Override
    public int getPauseWhenEmptyTime() {
        return Integer.parseInt(this.property("pause-when-empty-seconds", "60"));
    }

    @Override
    public java.util.List<String> getInitialEnabledPacks() {
        return List.of("vanilla");
    }

    @Override
    public java.util.List<String> getInitialDisabledPacks() {
        return List.of();
    }

    @Override
    public boolean suggestPlayerNamesWhenNullTabCompletions() {
        return true;
    }

    @Override
    public java.nio.file.Path getLevelDirectory() {
        return this.getWorldContainer().toPath().resolve(this.getWorlds().getFirst().getName());
    }

    @Override
    public long[] getTickTimes() {
        // The average of the last 100 ticks, as a series (Paper keeps each tick's time).
        final long mspt = (long) (this.getAverageTickTime() * 1_000_000L);
        final long[] out = new long[100];
        java.util.Arrays.fill(out, mspt);
        return out;
    }

    @Override
    @SuppressWarnings({"unchecked", "deprecation"})
    public <T extends org.bukkit.Keyed> org.bukkit.@Nullable Registry<T> getRegistry(final Class<T> aClass) {
        return (org.bukkit.Registry<T>) io.papermc.paper.registry.RegistryAccess.registryAccess().getRegistry(aClass);
    }

    @Override
    public double[] getTPS() {
        final List<Object> tps = Values.list(Values.map(this.call("server.status", Values.of())).get("tps"));
        final double[] out = new double[tps.size()];
        for (int i = 0; i < out.length; i++) {
            out[i] = ((Number) tps.get(i)).doubleValue();
        }
        return out;
    }

    @Override
    public double getAverageTickTime() {
        return Values.f64(Values.map(this.call("server.status", Values.of())), "mspt", 0);
    }

    @Override
    public int getCurrentTick() {
        return this.currentTick;
    }

    @Override
    public void shutdown() {
        this.send("server.shutdown", Values.of());
    }

    @Override
    public void savePlayers() {
        // Player data is owned by the Rust server.
    }

    @Override
    public org.bukkit.scoreboard.Criteria getScoreboardCriteria(final String name) {
        return io.papermc.paperrs.host.scoreboard.HostScoreboards.criteria(name);
    }

    @Override
    public Spigot spigot() {
        return this.spigot;
    }

    /**
     * The settings plugins read from bukkit.yml, spigot.yml and Paper's global configuration,
     * with what Paper-RS does: it has no proxy (BungeeCord or Velocity) forwarding, so those
     * read as off.
     */
    private final Spigot spigot = new Spigot() {
        @Override
        public org.bukkit.configuration.file.YamlConfiguration getConfig() {
            return this.getSpigotConfig();
        }

        @Override
        public org.bukkit.configuration.file.YamlConfiguration getSpigotConfig() {
            final org.bukkit.configuration.file.YamlConfiguration config = new org.bukkit.configuration.file.YamlConfiguration();
            config.set("settings.bungeecord", false);
            config.set("settings.timeout-time", 60);
            config.set("settings.restart-on-crash", false);
            config.set("settings.restart-script", "");
            config.set("settings.sample-count", 12);
            config.set("settings.player-shuffle", 0);
            config.set("messages.whitelist", "You are not whitelisted on this server!");
            config.set("messages.unknown-command", "Unknown command. Type \"/help\" for help.");
            config.set("messages.server-full", "The server is full!");
            config.set("messages.restart", "Server is restarting");
            return config;
        }

        @Override
        public org.bukkit.configuration.file.YamlConfiguration getBukkitConfig() {
            final org.bukkit.configuration.file.YamlConfiguration config = new org.bukkit.configuration.file.YamlConfiguration();
            config.set("settings.allow-end", CraftServer.this.getAllowEnd());
            config.set("settings.warn-on-overload", true);
            config.set("settings.permissions-file", "permissions.yml");
            config.set("settings.update-folder", CraftServer.this.getUpdateFolder());
            config.set("settings.connection-throttle", CraftServer.this.getConnectionThrottle());
            config.set("settings.query-plugins", true);
            config.set("settings.shutdown-message", CraftServer.this.getShutdownMessage());
            config.set("settings.minimum-api", "none");
            final org.bukkit.entity.SpawnCategory[] categories = {
                org.bukkit.entity.SpawnCategory.MONSTER, org.bukkit.entity.SpawnCategory.ANIMAL, org.bukkit.entity.SpawnCategory.WATER_ANIMAL,
                org.bukkit.entity.SpawnCategory.WATER_AMBIENT, org.bukkit.entity.SpawnCategory.WATER_UNDERGROUND_CREATURE,
                org.bukkit.entity.SpawnCategory.AXOLOTL, org.bukkit.entity.SpawnCategory.AMBIENT
            };
            final String[] limits = {"monsters", "animals", "water-animals", "water-ambient", "water-underground-creature", "axolotls", "ambient"};
            final String[] ticks = {"monster-spawns", "animal-spawns", "water-spawns", "water-ambient-spawns", "water-underground-creature-spawns", "axolotl-spawns", "ambient-spawns"};
            for (int i = 0; i < categories.length; i++) {
                config.set("spawn-limits." + limits[i], CraftServer.this.getSpawnLimit(categories[i]));
                config.set("ticks-per." + ticks[i], CraftServer.this.getTicksPerSpawns(categories[i]));
            }
            config.set("ticks-per.autosave", 6000);
            config.set("chunk-gc.period-in-ticks", 600);
            config.set("aliases", "now-in-commands.yml");
            return config;
        }

        /** Paper's global configuration (with the keys of its older paper.yml as well). */
        @Override
        public org.bukkit.configuration.file.YamlConfiguration getPaperConfig() {
            final org.bukkit.configuration.file.YamlConfiguration config = new org.bukkit.configuration.file.YamlConfiguration();
            config.set("proxies.bungee-cord.online-mode", true);
            config.set("proxies.proxy-protocol", false);
            config.set("proxies.velocity.enabled", false);
            config.set("proxies.velocity.online-mode", false);
            config.set("proxies.velocity.secret", "");
            config.set("settings.velocity-support.enabled", false);
            config.set("settings.velocity-support.online-mode", false);
            config.set("settings.velocity-support.secret", "");
            config.set("settings.bungee-online-mode", true);
            return config;
        }

        @Override
        public void restart() {
            // No restart script: like Spigot without one, the server only stops.
            CraftServer.this.shutdown();
        }

        @Override
        public void broadcast(final net.md_5.bungee.api.chat.BaseComponent component) {
            this.broadcast(new net.md_5.bungee.api.chat.BaseComponent[] {component});
        }

        @Override
        public void broadcast(final net.md_5.bungee.api.chat.BaseComponent... components) {
            final net.kyori.adventure.text.Component message = net.kyori.adventure.text.serializer.gson.GsonComponentSerializer.gson()
                .deserialize(net.md_5.bungee.chat.ComponentSerializer.toString(components));
            CraftServer.this.broadcast(message);
        }
    };

    @Override
    public String toString() {
        return "CraftServer{serverName=" + this.getName() + ",serverVersion=" + this.getVersion() + ",minecraftVersion=" + this.getMinecraftVersion() + "}";
    }

    /** A help map without topics: Paper-RS has no {@code /help} index for plugin commands yet. */
    private static final class EmptyHelpMap extends AbstractHelpMap {
        @Override
        public @Nullable HelpTopic getHelpTopic(final String topicName) {
            return null;
        }

        @Override
        public Collection<HelpTopic> getHelpTopics() {
            return List.of();
        }

        @Override
        public void addTopic(final HelpTopic topic) {
        }

        @Override
        public void clear() {
        }

        @Override
        public void registerHelpTopicFactory(final Class<?> commandClass, final HelpTopicFactory<?> factory) {
        }

        @Override
        public List<String> getIgnoredPlugins() {
            return List.of();
        }
    }

    @SuppressWarnings("unused")
    private static void unusedImports(final Fluid f, final GameEvent g, final EntityType e) {
    }

    // --- block data ------------------------------------------------------------------------------

    @Override
    public BlockData createBlockData(final String data) {
        return HostBlockData.parse(null, data);
    }

    @Override
    public BlockData createBlockData(final Material material) {
        return HostBlockData.defaults(material);
    }

    @Override
    public BlockData createBlockData(final Material material, final String data) {
        return HostBlockData.parse(material, data);
    }

    @Override
    public BlockData createBlockData(final Material material, final Consumer<? super BlockData> consumer) {
        final BlockData data = HostBlockData.defaults(material);
        if (consumer != null) {
            consumer.accept(data);
        }
        return data;
    }

    // --- recipes -------------------------------------------------------------------------------

    /** Plugins' recipes, their own objects (getRecipe answers those, as on Paper). */
    private final Map<org.bukkit.NamespacedKey, org.bukkit.inventory.Recipe> pluginRecipes = new java.util.LinkedHashMap<>();
    /** Every recipe as a Bukkit recipe, read from the server when first asked; null after a change. */
    private @Nullable Map<org.bukkit.NamespacedKey, org.bukkit.inventory.Recipe> recipeCache;

    /** Every recipe by key: the server's, with plugins' own objects for theirs. */
    private Map<org.bukkit.NamespacedKey, org.bukkit.inventory.Recipe> recipes() {
        if (this.recipeCache == null) {
            final Map<org.bukkit.NamespacedKey, org.bukkit.inventory.Recipe> all = new java.util.LinkedHashMap<>();
            for (final Object value : Values.list(this.call("server.recipes", Values.of()))) {
                final Map<String, Object> map = Values.map(value);
                final org.bukkit.NamespacedKey key = org.bukkit.NamespacedKey.fromString(Values.str(map, "key"));
                if (key == null) {
                    continue;
                }
                final org.bukkit.inventory.Recipe own = this.pluginRecipes.get(key);
                final org.bukkit.inventory.Recipe recipe = own != null ? own : io.papermc.paperrs.host.item.Recipes.fromValue(map);
                if (recipe != null) {
                    all.put(key, recipe);
                }
            }
            this.recipeCache = all;
        }
        return this.recipeCache;
    }

    /** The recipe of a key, for events (a crafting result names its recipe). */
    public @Nullable org.bukkit.inventory.Recipe recipe(final org.bukkit.NamespacedKey key) {
        return this.recipes().get(key);
    }

    /**
     * Shaped, shapeless, transmute and cooking recipes go to the server, which crafts and cooks
     * them; others (stonecutting, smithing) and ingredients only a predicate can test are not
     * added. A recipe of a key there is already throws, as on Paper.
     */
    @Override
    public boolean addRecipe(final org.bukkit.inventory.Recipe recipe) {
        if (recipe == null) {
            throw new IllegalArgumentException("recipe cannot be null");
        }
        final Map<String, Object> value = io.papermc.paperrs.host.item.Recipes.toValue(recipe);
        if (value == null) {
            this.logger.warning("Paper-RS cannot craft a recipe like " + recipe.getClass().getSimpleName() + " " + (recipe instanceof org.bukkit.Keyed k ? k.getKey() : "") + " yet; it was not added");
            return false;
        }
        final org.bukkit.NamespacedKey key = ((org.bukkit.Keyed) recipe).getKey();
        if (!Boolean.TRUE.equals(this.call("server.add_recipe", value))) {
            throw new IllegalStateException("Duplicate recipe ignored with ID " + key);
        }
        this.pluginRecipes.put(key, recipe);
        this.recipeCache = null;
        return true;
    }

    @Override
    public boolean addRecipes(final Iterable<org.bukkit.inventory.Recipe> recipes) {
        if (recipes == null) {
            throw new IllegalArgumentException("recipes cannot be null");
        }
        boolean added = false;
        for (final org.bukkit.inventory.Recipe recipe : recipes) {
            added |= this.addRecipe(recipe);
        }
        return added;
    }

    @Override
    public boolean removeRecipe(final org.bukkit.NamespacedKey key) {
        if (key == null) {
            throw new IllegalArgumentException("recipeKey cannot be null");
        }
        final boolean removed = Boolean.TRUE.equals(this.call("server.remove_recipe", Values.of("key", key.asString())));
        this.pluginRecipes.remove(key);
        this.recipeCache = null;
        return removed;
    }

    @Override
    public boolean removeRecipes(final Iterable<org.bukkit.NamespacedKey> keys) {
        if (keys == null) {
            throw new IllegalArgumentException("recipeKeys cannot be null");
        }
        boolean removed = false;
        for (final org.bukkit.NamespacedKey key : keys) {
            removed |= this.removeRecipe(key);
        }
        return removed;
    }

    @Override
    public boolean hasRecipe(final org.bukkit.NamespacedKey key) {
        if (key == null) {
            throw new IllegalArgumentException("recipeKey cannot be null");
        }
        return Boolean.TRUE.equals(this.call("server.has_recipe", Values.of("key", key.asString())));
    }

    @Override
    public @Nullable org.bukkit.inventory.Recipe getRecipe(final org.bukkit.NamespacedKey key) {
        if (key == null) {
            throw new IllegalArgumentException("NamespacedKey recipeKey cannot be null");
        }
        return this.recipes().get(key);
    }

    /** Every recipe; {@code remove()} removes the current one from the server, as on Paper. */
    @Override
    public java.util.Iterator<org.bukkit.inventory.Recipe> recipeIterator() {
        final java.util.Iterator<org.bukkit.inventory.Recipe> all = new ArrayList<>(this.recipes().values()).iterator();
        return new java.util.Iterator<>() {
            private org.bukkit.inventory.@Nullable Recipe current;

            @Override
            public boolean hasNext() {
                return all.hasNext();
            }

            @Override
            public org.bukkit.inventory.Recipe next() {
                return this.current = all.next();
            }

            @Override
            public void remove() {
                if (this.current == null) {
                    throw new IllegalStateException();
                }
                CraftServer.this.removeRecipe(((org.bukkit.Keyed) this.current).getKey());
                this.current = null;
            }
        };
    }

    @Override
    @SuppressWarnings("deprecation")
    public List<org.bukkit.inventory.Recipe> getRecipesFor(final org.bukkit.inventory.ItemStack result) {
        if (result == null) {
            throw new IllegalArgumentException("ItemStack cannot be null");
        }
        final List<org.bukkit.inventory.Recipe> results = new ArrayList<>();
        for (final org.bukkit.inventory.Recipe recipe : this.recipes().values()) {
            final org.bukkit.inventory.ItemStack stack = recipe.getResult();
            if (stack.getType() == result.getType() && (result.getDurability() == -1 || result.getDurability() == stack.getDurability())) {
                results.add(recipe);
            }
        }
        return results;
    }

    @Override
    public org.bukkit.inventory.@Nullable Recipe getCraftingRecipe(final org.bukkit.inventory.ItemStack[] matrix, final World world) {
        if (matrix == null || matrix.length != 4 && matrix.length != 9) {
            throw new IllegalArgumentException("craftingMatrix must be an array of length 4 or 9");
        }
        final List<Object> grid = new ArrayList<>();
        for (final org.bukkit.inventory.ItemStack item : matrix) {
            grid.add(io.papermc.paperrs.host.item.HostItemStack.toValue(item));
        }
        final Object made = this.call("server.craft", Values.of("grid", grid));
        return made instanceof Map<?, ?> m ? this.recipe(org.bukkit.NamespacedKey.fromString(Values.str(Values.map(m), "key"))) : null;
    }

    /** No recipes at all, vanilla's included, as on Paper. */
    @Override
    public void clearRecipes() {
        this.call("server.clear_recipes", Values.of());
        this.pluginRecipes.clear();
        this.recipeCache = null;
    }

    /** Vanilla's recipes only, as at start-up. */
    @Override
    public void resetRecipes() {
        this.call("server.reset_recipes", Values.of());
        this.pluginRecipes.clear();
        this.recipeCache = null;
    }

    /** Players are not sent the recipe book yet: nothing to update. */
    @Override
    public void updateRecipes() {
    }
}
