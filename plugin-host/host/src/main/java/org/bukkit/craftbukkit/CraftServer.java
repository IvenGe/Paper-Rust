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
            this.worlds.add(new HostWorld(this, Values.map(world)));
        }
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

    public PlayerMetadataStore playerMetadata() {
        return this.playerMetadata;
    }

    public org.bukkit.craftbukkit.metadata.EntityMetadataStore entityMetadata() {
        return this.entityMetadata;
    }

    public Difficulty difficulty() {
        return Difficulty.valueOf(Values.str(this.welcome, "difficulty", "normal").toUpperCase(Locale.ROOT));
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
        return new Spigot() {
        };
    }

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
}
